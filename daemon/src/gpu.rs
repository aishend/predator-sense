//! NVIDIA dGPU through NVML (loaded at runtime). Like the Windows service, a
//! runtime-suspended GPU is never woken up just to read numbers.

use libloading::{Library, Symbol};
use std::ffi::{CString, c_char, c_int, c_uint, c_void};

type Dev = *mut c_void;

pub struct Gpu {
    pub pci: Option<String>, // "0000:01:00.0"
    nvml: Option<(Library, Dev)>,
}
// The NVML handle is only used from behind the daemon's mutex.
unsafe impl Send for Gpu {}

#[derive(Default, serde::Serialize)]
pub struct Stats {
    pub usage: Option<u32>,
    pub core_mhz: Option<u32>,
    pub mem_mhz: Option<u32>,
    pub power_w: Option<f64>,
    pub temp: Option<u32>,
}

fn read(p: &str) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_owned())
}

impl Gpu {
    pub fn new() -> Self {
        let pci = std::fs::read_dir("/sys/bus/pci/devices").ok().and_then(|d| {
            d.flatten().map(|e| e.path()).find(|p| {
                read(&format!("{}/vendor", p.display())).as_deref() == Some("0x10de")
                    && read(&format!("{}/class", p.display())).is_some_and(|c| c.starts_with("0x03"))
            })
        });
        Gpu { pci: pci.and_then(|p| Some(p.file_name()?.to_str()?.to_owned())), nvml: None }
    }

    pub fn active(&self) -> bool {
        self.pci.as_ref().is_some_and(|p| read(&format!("/sys/bus/pci/devices/{p}/power/runtime_status")).is_none_or(|s| s == "active"))
    }

    fn dev(&mut self) -> Option<(&Library, Dev)> {
        if self.nvml.is_none() {
            let pci = CString::new(self.pci.clone()?).ok()?;
            let lib = unsafe { Library::new("libnvidia-ml.so.1") }.ok()?;
            let mut dev: Dev = std::ptr::null_mut();
            unsafe {
                let init: Symbol<unsafe extern "C" fn() -> c_int> = lib.get(b"nvmlInit_v2").ok()?;
                let by_pci: Symbol<unsafe extern "C" fn(*const c_char, *mut Dev) -> c_int> = lib.get(b"nvmlDeviceGetHandleByPciBusId_v2").ok()?;
                if init() != 0 || by_pci(pci.as_ptr(), &mut dev) != 0 {
                    return None;
                }
            }
            self.nvml = Some((lib, dev));
        }
        self.nvml.as_ref().map(|(l, d)| (l, *d))
    }

    /// Release NVML so the GPU can go back to D3cold.
    pub fn release(&mut self) {
        if let Some((lib, _)) = self.nvml.take() {
            unsafe {
                if let Ok(shutdown) = lib.get::<unsafe extern "C" fn() -> c_int>(b"nvmlShutdown") {
                    shutdown();
                }
            }
        }
    }

    /// GPU temperature only (NVML), None while the GPU sleeps.
    pub fn temp(&mut self) -> Option<u32> {
        if !self.active() {
            return None;
        }
        let (lib, d) = self.dev()?;
        let mut v: c_uint = 0;
        unsafe {
            let f = lib.get::<unsafe extern "C" fn(Dev, c_int, *mut c_uint) -> c_int>(b"nvmlDeviceGetTemperature").ok()?;
            (f(d, 0, &mut v) == 0).then_some(v)
        }
    }

    pub fn stats(&mut self) -> Option<Stats> {
        if !self.active() {
            self.release();
            return None;
        }
        let (lib, d) = self.dev()?;
        let mut s = Stats::default();
        unsafe {
            let u32_of = |name: &[u8], arg: Option<c_int>| -> Option<u32> {
                let mut v: c_uint = 0;
                let r = match arg {
                    Some(a) => lib.get::<unsafe extern "C" fn(Dev, c_int, *mut c_uint) -> c_int>(name).ok()?(d, a, &mut v),
                    None => lib.get::<unsafe extern "C" fn(Dev, *mut c_uint) -> c_int>(name).ok()?(d, &mut v),
                };
                (r == 0).then_some(v)
            };
            let mut util = [0 as c_uint; 2]; // nvmlUtilization_t { gpu, memory }
            if let Ok(f) = lib.get::<unsafe extern "C" fn(Dev, *mut [c_uint; 2]) -> c_int>(b"nvmlDeviceGetUtilizationRates") {
                s.usage = (f(d, &mut util) == 0).then_some(util[0]);
            }
            s.core_mhz = u32_of(b"nvmlDeviceGetClockInfo", Some(0));
            s.mem_mhz = u32_of(b"nvmlDeviceGetClockInfo", Some(2));
            s.temp = u32_of(b"nvmlDeviceGetTemperature", Some(0));
            s.power_w = u32_of(b"nvmlDeviceGetPowerUsage", None).map(|mw| mw as f64 / 1000.0);
        }
        Some(s)
    }

    /// Core/memory clock offsets in MHz, like NVAPI on Windows.
    pub fn set_offsets(&mut self, core: i32, mem: i32) -> bool {
        let Some((lib, d)) = self.dev() else { return false };
        #[repr(C)]
        struct Offset { version: c_uint, kind: c_int, pstate: c_int, mhz: c_int, min: c_int, max: c_int }
        unsafe {
            // driver >= 555
            if let Ok(f) = lib.get::<unsafe extern "C" fn(Dev, *mut Offset) -> c_int>(b"nvmlDeviceSetClockOffsets") {
                let mut o = Offset { version: size_of::<Offset>() as c_uint | 1 << 24, kind: 0, pstate: 0, mhz: core, min: 0, max: 0 };
                let a = f(d, &mut o);
                o.kind = 2;
                o.mhz = mem;
                if a == 0 && f(d, &mut o) == 0 {
                    return true;
                }
            }
            let gpc = lib.get::<unsafe extern "C" fn(Dev, c_int) -> c_int>(b"nvmlDeviceSetGpcClkVfOffset");
            let memf = lib.get::<unsafe extern "C" fn(Dev, c_int) -> c_int>(b"nvmlDeviceSetMemClkVfOffset");
            matches!((gpc, memf), (Ok(g), Ok(m)) if g(d, core) == 0 && m(d, mem) == 0)
        }
    }
}
