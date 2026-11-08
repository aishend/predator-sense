//! Service logic of AcerAgentService on top of hw.rs.

use crate::{gpu::Gpu, hw, power, wmi::Wmi};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Instant};

const STATE: &str = "/var/lib/predator-sense/state.json";
pub const EFFECTS: [&str; 8] = ["static", "breathing", "neon", "wave", "shifting", "zoom", "meteor", "twinkling"];

/// GPU clock offsets (core, memory MHz) per mode, from the PHN16-72 PredatorSense INI
/// (RTX 4060 Laptop: only Turbo is overclocked).
fn gpu_oc(mode: u8) -> (i32, i32) {
    if mode == hw::TURBO { (100, 200) } else { (0, 0) }
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Fan {
    pub mode: String,                // auto | max | custom | curve
    pub cpu: Option<u8>,             // custom: % or None = auto
    pub gpu: Option<u8>,
    pub cpu_curve: Vec<(u8, u8)>,    // curve: (°C, %) sorted by temperature
    pub gpu_curve: Vec<(u8, u8)>,
}
impl Default for Fan {
    fn default() -> Self {
        let curve = vec![(45, 25), (55, 40), (65, 50), (75, 60), (85, 90), (95, 100)];
        Fan { mode: "auto".into(), cpu: Some(50), gpu: Some(50), cpu_curve: curve.clone(), gpu_curve: curve }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Light {
    pub effect: String,
    pub zones: [String; 4], // static colours "#rrggbb", left to right
    pub color: String,      // colour of the animated effects
    pub speed: u8,          // 1..9
    pub brightness: u8,     // 0..100
    pub direction: u8,      // 1 right-to-left, 2 left-to-right
}
impl Default for Light {
    fn default() -> Self {
        let c = "#00aec7".to_string();
        Light { effect: "static".into(), zones: [c.clone(), c.clone(), c.clone(), c.clone()], color: c, speed: 5, brightness: 100, direction: 1 }
    }
}

/// What each operating mode remembers. None = never changed through us.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
struct Profile {
    fan: Option<Fan>,
    light: Option<Light>,
    rear: Option<Light>, // rear light (zones unused)
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    ac_mode: u8,
    dc_mode: u8,
    profiles: BTreeMap<u8, Profile>, // by operating mode
    gpu_oc: bool,
    refresh_ac: u32, // screen Hz, applied by the user agent (predatorctl agent)
    refresh_dc: u32,
}
impl Default for Saved {
    fn default() -> Self {
        Saved { ac_mode: hw::BALANCED, dc_mode: hw::BALANCED, profiles: BTreeMap::new(), gpu_oc: true, refresh_ac: 165, refresh_dc: 60 }
    }
}

pub struct Daemon {
    w: Wmi,
    gpu: Gpu,
    sim: bool,
    saved: Saved,
    modes: Vec<u8>,
    fans: usize, // CPU + GPU fans
    sensors: u16,
    lcd: bool,
    mode: Option<u8>,
    ac: Option<bool>,
    oc_applied: Option<(i32, i32)>,
    curve_pct: Vec<u8>,
    rapl: Option<(u64, Instant)>,
    profile: String,
    tick: u32,
    bus: Option<zbus::blocking::Connection>, // own system-bus connection for power-profiles-daemon
    slow: Option<(Instant, Value)>,           // EC-only sensors, refreshed at most every 10 s
    pub dirty: bool, // state changed: emit StateChanged
}

pub type R = Result<(), String>;

fn read(p: &str) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_owned())
}

fn on_ac() -> bool {
    // ACAD, or a USB-C PD charger
    std::fs::read_dir("/sys/class/power_supply").into_iter().flatten().flatten().any(|e| {
        let p = e.path().display().to_string();
        matches!(read(&format!("{p}/type")).as_deref(), Some("Mains" | "USB")) && read(&format!("{p}/online")).as_deref() == Some("1")
    })
}

/// CPU package temperature from coretemp. Unlike the EC sensors it costs no
/// firmware (SMM) call: every WMI call stalls all cores for 6-18 ms, and polling
/// them every 2 s made the sound stutter.
fn cpu_temp() -> Option<u32> {
    std::fs::read_dir("/sys/class/hwmon").ok()?.flatten().map(|e| e.path()).find_map(|p| {
        (read(&format!("{}/name", p.display()))? == "coretemp")
            .then(|| read(&format!("{}/temp1_input", p.display()))?.parse::<u32>().ok().map(|t| t / 1000))?
    })
}

fn rgb(hex: &str) -> [u8; 3] {
    let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0);
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

/// Linear interpolation over the curve points.
pub fn curve_at(curve: &[(u8, u8)], t: f64) -> u8 {
    let Some(first) = curve.first() else { return 100 };
    if t <= first.0 as f64 {
        return first.1;
    }
    for p in curve.windows(2) {
        let ((t0, s0), (t1, s1)) = (p[0], p[1]);
        if t <= t1 as f64 {
            return (s0 as f64 + (s1 as f64 - s0 as f64) * (t - t0 as f64) / (t1 as f64 - t0 as f64).max(1.0)).round() as u8;
        }
    }
    curve.last().unwrap().1
}

impl Daemon {
    pub fn new(sim: bool) -> std::io::Result<Self> {
        let w = if sim { Wmi::sim() } else { Wmi::open()? };
        let sensors = hw::sensor_mask(&w);
        // sensor bit 1 = CPU fan, bits 5 and 8 = GPU fans
        let fans = 1 + (sensors & 0x20 != 0) as usize + (sensors & 0x100 != 0) as usize;
        let saved = std::fs::read(STATE).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let mut d = Daemon {
            modes: hw::modes(&w),
            lcd: hw::lcd_overdrive(&w).is_some_and(|v| v != 0xFF),
            w, gpu: Gpu::new(), sim, sensors, fans,
            saved: Default::default(),
            mode: None, ac: None, oc_applied: None, curve_pct: vec![0; fans], rapl: None,
            profile: String::new(), tick: 0, dirty: false,
            bus: if sim { None } else { zbus::blocking::Connection::system().ok() },
            slow: None,
        };
        match saved {
            Some(s) => {
                d.saved = s;
                d.restore("startup");
            }
            None => {
                // first run: adopt what the firmware has, change nothing
                d.ac = Some(on_ac());
                d.mode = hw::mode(&d.w);
                if let Some(m) = d.mode {
                    if d.ac == Some(true) { d.saved.ac_mode = m } else if matches!(m, hw::BALANCED | hw::ECO) { d.saved.dc_mode = m }
                }
                d.save();
            }
        }
        Ok(d)
    }

    fn save(&self) {
        if self.sim {
            return;
        }
        let _ = std::fs::create_dir_all("/var/lib/predator-sense");
        let tmp = format!("{STATE}.tmp");
        if std::fs::write(&tmp, serde_json::to_vec_pretty(&self.saved).unwrap()).is_ok() {
            let _ = std::fs::rename(tmp, STATE);
        }
    }

    /// Mode chosen in the app for the charger or for battery.
    fn policy(&self, ac: bool) -> u8 {
        let m = if ac { self.saved.ac_mode } else { self.saved.dc_mode };
        if self.modes.contains(&m) { m } else { hw::BALANCED }
    }

    fn prof(&self) -> Profile {
        self.mode.and_then(|m| self.saved.profiles.get(&m).cloned()).unwrap_or_default()
    }
    fn prof_mut(&mut self) -> &mut Profile {
        self.saved.profiles.entry(self.mode.unwrap_or(hw::BALANCED)).or_default()
    }

    fn apply_mode(&mut self, m: u8) -> bool {
        if !hw::set_mode(&self.w, m) {
            return false;
        }
        let prev = self.mode.replace(m);
        self.enter_profile(prev);
        true
    }

    /// After a mode change: apply the fans and lighting saved for the new mode.
    /// Lighting carries over into a mode that has none yet; fans go back to Auto
    /// when the old mode had them on something else (that also keeps Quiet on Auto).
    fn enter_profile(&mut self, prev: Option<u8>) {
        self.apply_oc(true);
        if let (Some(bus), Some(m)) = (&self.bus, self.mode) {
            power::set_ppd(bus, power::ppd_profile(m));
        }
        self.dirty = true;
        if prev == self.mode {
            return;
        }
        let old = prev.and_then(|m| self.saved.profiles.get(&m).cloned()).unwrap_or_default();
        let p = self.prof_mut();
        p.light = p.light.take().or(old.light.clone());
        p.rear = p.rear.take().or(old.rear.clone());
        if self.prof().fan.is_some() || old.fan.is_some() {
            self.apply_fan();
        }
        // The firmware switches the lights off by itself in Eco; leave them so.
        // In every other mode send them again, also when unchanged: they may
        // still be off from Eco.
        if self.mode != Some(hw::ECO) {
            self.apply_light();
            self.apply_rear();
        }
    }

    fn apply_oc(&mut self, force: bool) {
        let Some(m) = self.mode else { return };
        let want = if self.saved.gpu_oc { gpu_oc(m) } else { (0, 0) };
        if self.sim || !self.gpu.active() || (!force && self.oc_applied == Some(want)) {
            return; // never wake the dGPU; offsets are applied when it powers up
        }
        if !self.gpu.set_offsets(want.0, want.1) {
            eprintln!("could not set GPU clock offsets {want:?}");
        }
        self.oc_applied = Some(want); // no retry loop on failure

    }

    /// Fans of the current mode; Quiet and Eco always stay on Auto.
    fn fan(&self) -> Fan {
        match self.mode {
            Some(hw::QUIET | hw::ECO) => Fan::default(),
            _ => self.prof().fan.unwrap_or_default(),
        }
    }

    fn apply_fan(&mut self) -> bool {
        let f = self.fan();
        let n = self.fans;
        let all = |beh: u8| vec![(beh, 0); n];
        match f.mode.as_str() {
            "max" => hw::set_fans(&self.w, &all(2)),
            "custom" => {
                let fan = |s: Option<u8>| s.map_or((1, 0), |p| (3, p));
                let list: Vec<_> = (0..n).map(|i| fan(if i == 0 { f.cpu } else { f.gpu })).collect();
                hw::set_fans(&self.w, &list)
            }
            "curve" => {
                self.curve_pct = vec![0; n];
                hw::set_fans(&self.w, &all(3)) && self.apply_curve()
            }
            _ => hw::set_fans(&self.w, &all(1)),
        }
    }

    /// Software fan curve, run every 2 s. Speeds go up at once and only go down
    /// once the temperature is 3 °C under the point that asked for the higher speed.
    fn apply_curve(&mut self) -> bool {
        let f = self.fan();
        if f.mode != "curve" {
            return true;
        }
        let temps = self.temps();
        let mut good = true;
        for i in 0..self.fans {
            let (curve, t) = if i == 0 { (&f.cpu_curve, temps[0]) } else { (&f.gpu_curve, temps[1]) };
            let Some(t) = t.map(|t| t as f64) else { continue };
            let up = curve_at(curve, t);
            let target = if up >= self.curve_pct[i] { up } else { curve_at(curve, t + 3.0).min(self.curve_pct[i]) };
            if target != self.curve_pct[i] && hw::set_fan_speed(&self.w, i, target) {
                self.curve_pct[i] = target;
            } else if target != self.curve_pct[i] {
                good = false;
            }
        }
        good
    }

    fn apply_light(&self) -> bool {
        let Some(l) = &self.prof().light else { return true };
        match EFFECTS.iter().position(|e| *e == l.effect) {
            Some(0) => hw::kb_zones(&self.w, l.brightness, &l.zones.clone().map(|z| rgb(&z))),
            Some(e) => hw::kb_effect(&self.w, hw::KEYBOARD, e as u8, l.speed, l.brightness, l.direction, rgb(&l.color)),
            None => false,
        }
    }

    /// Rear lid logo: colour and brightness only.
    fn apply_rear(&self) -> bool {
        let Some(l) = &self.prof().rear else { return true };
        hw::set_rear(&self.w, rgb(&l.color), l.brightness)
    }

    /// At startup, and after resume (the EC forgets fans and lighting). The app's
    /// charger/battery mode is applied at startup or when the charger changed
    /// during sleep; otherwise the mode you had stays.
    pub fn restore(&mut self, why: &str) {
        eprintln!("restoring settings: {why}");
        let ac = on_ac();
        let plug_changed = self.ac != Some(ac);
        self.ac = Some(ac);
        if !self.sim {
            power::set_audio_power_save(!ac);
        }
        let want = match self.mode {
            Some(m) if !plug_changed => m,
            _ => self.policy(ac),
        };
        self.mode = None; // so the fans and lighting of that mode are applied too
        self.apply_mode(want);
    }

    /// Mode changed outside the daemon (mode key through acer_wmi): follow it.
    /// Manual choices are never overridden; only plugging/unplugging applies the app's modes.
    fn check_mode(&mut self) {
        let Some(m) = hw::mode(&self.w) else { return };
        if Some(m) == self.mode {
            return;
        }
        eprintln!("mode changed externally to {m}");
        let prev = self.mode.replace(m);
        self.enter_profile(prev);
    }

    /// Tell power-profiles-daemon the profile of the current mode (when it starts).
    pub fn sync_ppd(&self) {
        if let (Some(bus), Some(m)) = (&self.bus, self.mode) {
            power::set_ppd(bus, power::ppd_profile(m));
        }
    }

    /// The power profile was changed through power-profiles-daemon (Noctalia):
    /// follow it with the matching mode.
    pub fn ppd_changed(&mut self, p: &str) {
        let Some(m) = self.mode else { return };
        if power::ppd_profile(m) == p {
            return;
        }
        let want = match p {
            "power-saver" => hw::ECO,
            "performance" => hw::TURBO,
            _ => hw::BALANCED,
        };
        eprintln!("power profile {p} -> mode {want}");
        self.apply_mode(want);
    }

    /// Called every second.
    pub fn poll(&mut self) {
        self.tick += 1;
        let ac = on_ac();
        if self.ac != Some(ac) {
            self.ac = Some(ac);
            let want = self.policy(ac);
            eprintln!("{} -> mode {want}", if ac { "AC connected" } else { "on battery" });
            if !self.sim {
                power::set_audio_power_save(!ac);
            }
            self.apply_mode(want);
        }
        // platform_profile is a cheap sysfs read; ask the EC itself every 5 s
        let profile = read("/sys/firmware/acpi/platform_profile").unwrap_or_default();
        if profile != self.profile || self.tick.is_multiple_of(5) {
            self.profile = profile;
            self.check_mode();
        }
        // dGPU power transitions: offsets are lost when it powers down
        if !self.gpu.active() {
            self.oc_applied = None;
            self.gpu.release();
        } else if self.oc_applied.is_none() {
            self.apply_oc(true);
        }
        if self.tick.is_multiple_of(2) {
            self.apply_curve();
        }
    }

    // ------------------------------------------------------------ D-Bus API

    pub fn info(&self) -> Value {
        json!({
            "product": read("/sys/class/dmi/id/product_name"),
            "bios": read("/sys/class/dmi/id/bios_version"),
            "modes": self.modes,
            "fans": self.fans,
            "effects": EFFECTS,
            "lcd_overdrive": self.lcd,
            "gpu": self.gpu.pci,
            "gpu_oc_turbo": gpu_oc(hw::TURBO),
            "simulated": self.sim,
        })
    }

    pub fn state(&self) -> Value {
        let w = &self.w;
        let usb = hw::usb_charging(w);
        let bat = hw::battery_health(w);
        let p = self.prof();
        let light_now = (
            p.fan.unwrap_or_default(),
            p.light.or_else(|| self.read_light()).unwrap_or_default(),
            p.rear.or_else(|| self.read_rear()).unwrap_or_default(),
        );
        json!({
            "mode": self.mode,
            "ac": self.ac,
            "ac_mode": self.saved.ac_mode,
            "dc_mode": self.saved.dc_mode,
            "fan": light_now.0,
            "light": light_now.1,
            "rear": light_now.2,
            // what each mode is set to, for the "Settings for" bar in the app
            "profiles": self.modes.iter().map(|m| {
                let p = self.saved.profiles.get(m).cloned().unwrap_or_default();
                (m.to_string(), json!({
                    "fan": p.fan.unwrap_or_default(),
                    "light": p.light.unwrap_or_else(|| light_now.1.clone()),
                    "rear": p.rear.unwrap_or_else(|| light_now.2.clone()),
                }))
            }).collect::<serde_json::Map<_, _>>(),
            "gpu_oc": self.saved.gpu_oc,
            "refresh_ac": self.saved.refresh_ac,
            "refresh_dc": self.saved.refresh_dc,
            "boot_sound": hw::boot_sound(w),
            "lcd_overdrive": if self.lcd { hw::lcd_overdrive(w).map(|v| v == 1) } else { None },
            "kb_timeout": hw::kb_timeout(w),
            "usb_charging": usb.map(|u| u.0),
            "usb_threshold": usb.map(|u| u.1),
            "battery_limit": bat.map(|b| b.0),
            "calibration": bat.map(|b| b.1),
        })
    }

    /// CPU and GPU temperatures without any firmware call (coretemp, NVML).
    pub fn temps(&mut self) -> [Option<u32>; 2] {
        [cpu_temp(), self.gpu.temp()]
    }

    /// For the GUI. Fans come from the EC (one call each); system and battery
    /// temperatures are EC-only too, so they are refreshed at most every 10 s.
    pub fn sensors(&mut self) -> Value {
        let s = |id: u8, bit: u16| if self.sensors & 1 << bit != 0 { hw::sensor(&self.w, id) } else { None };
        if self.slow.as_ref().is_none_or(|(t, _)| t.elapsed().as_secs() >= 10) {
            let v = json!({ "sys_temp": s(0x03, 2), "battery_temp": hw::battery_temp(&self.w) });
            self.slow = Some((Instant::now(), v));
        }
        let slow = self.slow.as_ref().unwrap().1.clone();
        // CPU package power from RAPL (root-only energy counter)
        let energy = read("/sys/class/powercap/intel-rapl:0/energy_uj").and_then(|v| v.parse::<u64>().ok());
        let cpu_w = match (energy, self.rapl) {
            (Some(e), Some((e0, t0))) if e >= e0 => Some((e - e0) as f64 / 1e6 / t0.elapsed().as_secs_f64().max(0.1)),
            _ => None,
        };
        self.rapl = energy.map(|e| (e, Instant::now()));
        json!({
            "cpu_temp": cpu_temp(),
            "cpu_fan": s(0x02, 1),
            "sys_temp": slow["sys_temp"],
            "gpu_fan": s(0x06, 5),
            "gpu2_fan": s(0x09, 8),
            "gpu_temp": self.gpu.temp(),
            "cpu_power": cpu_w,
            "battery_temp": slow["battery_temp"],
            "gpu": self.gpu.stats(),
        })
    }

    /// Mode right now (like the mode key); lasts until the charger is plugged or unplugged.
    pub fn set_mode(&mut self, m: u8) -> R {
        if !self.modes.contains(&m) {
            return Err("mode not supported".into());
        }
        if !self.apply_mode(m) {
            return Err("firmware rejected the mode".into());
        }
        Ok(())
    }

    /// Mode applied when the charger is plugged in (`charger`) or unplugged.
    pub fn set_policy(&mut self, charger: bool, m: u8) -> R {
        if !self.modes.contains(&m) {
            return Err("mode not supported".into());
        }
        if charger { self.saved.ac_mode = m } else { self.saved.dc_mode = m }
        self.save();
        self.dirty = true;
        Ok(())
    }

    /// Fans for `mode`; applied now if it is the current mode.
    pub fn set_fan(&mut self, mode: u8, f: Fan) -> R {
        if !["auto", "max", "custom", "curve"].contains(&f.mode.as_str()) {
            return Err("fan mode must be auto, max, custom or curve".into());
        }
        if !self.modes.contains(&mode) {
            return Err("mode not supported".into());
        }
        let mut f = f;
        f.cpu_curve.sort();
        f.gpu_curve.sort();
        let prev = self.saved.profiles.entry(mode).or_default().fan.replace(f);
        if self.mode == Some(mode) && !self.apply_fan() {
            self.prof_mut().fan = prev;
            return Err("firmware rejected the fan setting".into());
        }
        self.save();
        self.dirty = true;
        Ok(())
    }

    /// Keyboard (or rear logo) lighting for `mode`; applied now if it is the current mode.
    pub fn set_light(&mut self, mode: u8, l: Light, rear: bool) -> R {
        if !EFFECTS.contains(&l.effect.as_str()) {
            return Err("unknown effect".into());
        }
        if !self.modes.contains(&mode) {
            return Err("mode not supported".into());
        }
        fn slot(p: &mut Profile, rear: bool) -> &mut Option<Light> {
            if rear { &mut p.rear } else { &mut p.light }
        }
        let prev = slot(self.saved.profiles.entry(mode).or_default(), rear).replace(l);
        if self.mode == Some(mode) && !(if rear { self.apply_rear() } else { self.apply_light() }) {
            *slot(self.prof_mut(), rear) = prev;
            return Err("firmware rejected the lighting setting".into());
        }
        self.save();
        self.dirty = true;
        Ok(())
    }

    /// Current firmware lighting (GetGamingKBBacklight), used until the user sets one.
    fn read_light(&self) -> Option<Light> {
        let (effect, speed, brightness, direction, c) = hw::kb_read(&self.w, hw::KEYBOARD)?;
        let color = format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
        let effect = EFFECTS.get(effect as usize)?.to_string();
        Some(Light { effect, zones: std::array::from_fn(|_| color.clone()), color, speed: speed.max(1), brightness, direction: direction.max(1) })
    }

    fn read_rear(&self) -> Option<Light> {
        let (c, brightness) = hw::rear(&self.w)?;
        let color = format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
        Some(Light { color, brightness, ..Default::default() })
    }

    pub fn set_option(&mut self, key: &str, v: &Value) -> R {
        let (b, n) = (v.as_bool().unwrap_or(false), v.as_u64().unwrap_or(0) as u8);
        let w = &self.w;
        let ok = match key {
            "boot_sound" => hw::set_boot_sound(w, b),
            "lcd_overdrive" if self.lcd => hw::set_lcd_overdrive(w, b),
            "kb_timeout" => hw::set_kb_timeout(w, b),
            // for Noctalia's idle behaviour: the rear light has no firmware timeout
            "rear_off" if b => {
                if self.prof().rear.is_none() {
                    self.prof_mut().rear = self.read_rear(); // remember what to turn back on
                }
                hw::set_rear(&self.w, rgb(&self.prof().rear.unwrap_or_default().color), 0)
            }
            "rear_off" => self.apply_rear(),
            "usb_charging" | "usb_threshold" => {
                let (on, thr) = hw::usb_charging(w).ok_or("cannot read USB charging")?;
                if key == "usb_threshold" && ![10, 20, 30].contains(&n) {
                    return Err("threshold must be 10, 20 or 30".into());
                }
                if key == "usb_charging" { hw::set_usb_charging(w, b, thr) } else { hw::set_usb_charging(w, on, n) }
            }
            "battery_limit" => hw::set_battery(w, 1, b),
            "calibration" => {
                if b && !on_ac() {
                    return Err("connect the charger before calibrating".into());
                }
                hw::set_battery(w, 2, b)
            }
            "refresh_ac" | "refresh_dc" => {
                if ![60, 165].contains(&n) {
                    return Err("refresh rate must be 60 or 165".into());
                }
                if key == "refresh_ac" { self.saved.refresh_ac = n as u32 } else { self.saved.refresh_dc = n as u32 }
                self.save();
                true
            }
            "gpu_oc" => {
                self.saved.gpu_oc = b;
                self.apply_oc(true);
                self.save();
                true
            }
            _ => return Err(format!("unknown option {key}")),
        };
        if !ok {
            return Err("firmware rejected the setting".into());
        }
        self.dirty = true;
        Ok(())
    }
}
