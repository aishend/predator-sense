//! /dev/predator-wmi (kernel/predator_wmi.c): one ioctl = one ACPI-WMI method call.

use std::{collections::HashMap, fs::File, io, os::fd::AsRawFd, sync::Mutex};

#[derive(Clone, Copy, PartialEq)]
pub enum Iface {
    Gaming = 0,  // AcerGamingFunction
    Apge = 1,    // APGeAction
    Battery = 2, // BatteryControl
}

#[repr(C)]
struct Call {
    iface: u32,
    method: u32,
    in_len: u32,
    out_len: u32,
    out_type: u32,
    reserved: u32,
    input: [u8; 32],
    out: [u8; 64],
}

#[repr(C)]
struct Info {
    api_version: u32,
    iface_mask: u32,
}

const fn ioc(dir: u64, nr: u64, size: usize) -> u64 {
    (dir << 30) | ((size as u64) << 16) | ((b'P' as u64) << 8) | nr
}
const IOC_INFO: u64 = ioc(2, 1, size_of::<Info>()); // _IOR
const IOC_CALL: u64 = ioc(3, 2, size_of::<Call>()); // _IOWR

pub fn le64(b: &[u8]) -> u64 {
    let mut a = [0u8; 8];
    let n = b.len().min(8);
    a[..n].copy_from_slice(&b[..n]);
    u64::from_le_bytes(a)
}

pub enum Wmi {
    Dev(File),
    /// Fake firmware for developing the GUI without the module.
    Sim(Mutex<HashMap<&'static str, u64>>),
}

impl Wmi {
    pub fn open() -> io::Result<Self> {
        let f = File::options().read(true).write(true).open("/dev/predator-wmi")?;
        let mut info = Info { api_version: 0, iface_mask: 0 };
        if unsafe { libc::ioctl(f.as_raw_fd(), IOC_INFO as _, &mut info) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if info.api_version != 1 || info.iface_mask & 1 == 0 {
            return Err(io::Error::other("predator_wmi: API mismatch or no gaming interface"));
        }
        Ok(Wmi::Dev(f))
    }

    pub fn sim() -> Self {
        let s = [("mode", 1), ("boot", 1), ("usb", 10 << 16 | 0x1F << 8), ("kbto", 0), ("health", 0), ("calib", 0), ("fan", 0)];
        Wmi::Sim(Mutex::new(s.into_iter().collect()))
    }

    pub fn call(&self, iface: Iface, method: u32, input: &[u8]) -> Option<Vec<u8>> {
        let f = match self {
            Wmi::Dev(f) => f,
            Wmi::Sim(st) => return simulate(&mut st.lock().unwrap(), iface, method, input),
        };
        let mut c = Call { iface: iface as u32, method, in_len: input.len() as u32, out_len: 0, out_type: 0, reserved: 0, input: [0; 32], out: [0; 64] };
        c.input[..input.len()].copy_from_slice(input);
        if unsafe { libc::ioctl(f.as_raw_fd(), IOC_CALL as _, &mut c) } < 0 {
            eprintln!("WMI iface {} method {method} failed: {}", iface as u32, io::Error::last_os_error());
            return None;
        }
        Some(c.out[..c.out_len as usize].to_vec())
    }

    /// Integer arguments use the width the firmware MOF declares (uint32 or uint64).
    pub fn u32(&self, iface: Iface, method: u32, v: u32) -> Option<u64> {
        self.call(iface, method, &v.to_le_bytes()).map(|o| le64(&o))
    }
    pub fn u64(&self, iface: Iface, method: u32, v: u64) -> Option<u64> {
        self.call(iface, method, &v.to_le_bytes()).map(|o| le64(&o))
    }
}

fn simulate(st: &mut HashMap<&'static str, u64>, iface: Iface, m: u32, input: &[u8]) -> Option<Vec<u8>> {
    let v = le64(input);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    let int = |x: u64| Some(x.to_le_bytes().to_vec());
    match (iface, m) {
        (Iface::Gaming, 5) if v == 0 => int(0x0227 << 24),
        (Iface::Gaming, 5) => {
            let load = [0.35, 0.55, 0.0, 0.0, 0.8, 1.0, 0.3][st["mode"] as usize];
            let max = st["fan"] & 3 == 2;
            let val = match v >> 8 {
                1 => 52.0 + 25.0 * load + 6.0 * (t / 7.0).sin(),
                3 => 41.0 + 4.0 * (t / 13.0).sin(),
                10 => 48.0 + 20.0 * load + 5.0 * (t / 9.0).sin(),
                2 | 6 if max => 6800.0,
                2 | 6 => 2400.0 + 3400.0 * load + 150.0 * (t / 5.0).sin(),
                _ => return int(1),
            };
            int((val as u64) << 8)
        }
        (Iface::Gaming, 23) => match v {
            0x0B => int(st["mode"] << 8),
            0x0A => int(0x73 << 8),
            0x06 => int(st["boot"] << 8),
            _ => int(1),
        },
        (Iface::Gaming, 22) => {
            match v & 0xFF {
                0x0B => st.insert("mode", v >> 8 & 0xFF),
                0x06 => st.insert("boot", v >> 8 & 0xFF),
                _ => None,
            };
            int(0)
        }
        (Iface::Gaming, 3) => int(0xFF << 48),
        (Iface::Gaming, 12) => {
            st.insert("rear", le64(&input[1..6]));
            int(0)
        }
        (Iface::Gaming, 13) => Some([&[0u8][..], &st.get("rear").copied().unwrap_or(0).to_le_bytes()[..5]].concat()),
        (Iface::Gaming, 14) => {
            st.insert("fan", v >> 16);
            int(0)
        }
        (Iface::Gaming, 21) => Some(vec![0; 16]),
        (Iface::Gaming, _) => int(0),
        (Iface::Apge, 2) if v == 4 => int(st["usb"]),
        (Iface::Apge, 2) => int(if st["kbto"] != 0 { 0x1E << 40 } else { 0 } | 100 << 32),
        (Iface::Apge, _) => {
            if v & 0xFF == 4 {
                st.insert("usb", v & 0xFFFF00);
            } else {
                st.insert("kbto", (v >> 40 & 0xFF == 0x1E) as u64);
            }
            int(0)
        }
        (Iface::Battery, 19) => Some(3031u32.to_le_bytes().to_vec()),
        (Iface::Battery, 20) => Some(vec![3, 0, 0, st["health"] as u8, st["calib"] as u8, 0, 0, 0]),
        (Iface::Battery, _) => {
            st.insert(if input[1] == 1 { "health" } else { "calib" }, input[2] as u64);
            Some(vec![0; 4])
        }
    }
}
