//! What PredatorSense sends to the PHN16-72 firmware (see docs/REVERSE_ENGINEERING.md).
//! AcerGamingFunction integer results: byte 0 is a status (0 = ok), payload from bit 8.

use crate::wmi::{Iface::*, Wmi, le64};

pub const QUIET: u8 = 0;
pub const BALANCED: u8 = 1;
pub const TURBO: u8 = 5;
pub const ECO: u8 = 6;

/// Keyboard-backlight hotkey number from SMBIOS type 170 (PHN16-72 BIOS V1.20).
const BK: u64 = 0x84;

// AcerGamingFunction method IDs (from the firmware MOF)
const SET_PROFILE: u32 = 1;
const SET_LED: u32 = 2;
const GET_PROFILE: u32 = 3;
const GET_SYSINFO: u32 = 5;
const SET_RGB_KB: u32 = 6;
const SET_BACK_LOGO: u32 = 12;
const GET_BACK_LOGO: u32 = 13;
const SET_FAN_BEHAVIOR: u32 = 14;
const SET_FAN_SPEED: u32 = 16;
const SET_KB_BACKLIGHT: u32 = 20;
const GET_KB_BACKLIGHT: u32 = 21;
const SET_MISC: u32 = 22;
const GET_MISC: u32 = 23;

fn ok(v: u64) -> bool {
    v & 0xFF == 0
}
fn get(w: &Wmi, method: u32, arg: u32) -> Option<u64> {
    w.u32(Gaming, method, arg).filter(|v| ok(*v))
}
fn set(w: &Wmi, method: u32, arg: u64) -> bool {
    w.u64(Gaming, method, arg).is_some_and(ok)
}
fn set_buf(w: &Wmi, iface: crate::wmi::Iface, method: u32, buf: &[u8]) -> bool {
    w.call(iface, method, buf).is_some_and(|o| ok(le64(&o)))
}

// ---- operating mode: misc setting 0x0B, capability bitmap 0x0A

pub fn mode(w: &Wmi) -> Option<u8> {
    get(w, GET_MISC, 0x0B).map(|v| (v >> 8) as u8)
}
pub fn set_mode(w: &Wmi, m: u8) -> bool {
    set(w, SET_MISC, (m as u64) << 8 | 0x0B)
}
pub fn modes(w: &Wmi) -> Vec<u8> {
    match get(w, GET_MISC, 0x0A) {
        Some(v) => (0..7).filter(|i| v >> 8 & (1 << i) != 0).collect(),
        None => vec![QUIET, BALANCED, 4, TURBO, ECO],
    }
}

// ---- sensors: GetGamingSysInfo((id << 8) | 1); bitmap of supported ids from GetGamingSysInfo(0)

pub fn sensor_mask(w: &Wmi) -> u16 {
    get(w, GET_SYSINFO, 0).map_or(0, |v| (v >> 24) as u16)
}
pub fn sensor(w: &Wmi, id: u8) -> Option<u32> {
    get(w, GET_SYSINFO, (id as u32) << 8 | 1).map(|v| (v >> 8 & 0xFFFF) as u32)
}

// ---- fans. Index 0 = CPU, 1.. = GPU fans. Behaviour: 1 auto, 2 max, 3 custom.

fn fan_slot(i: usize) -> (u64, u64, u64) {
    // (bitmap bit, 2-bit behaviour slot, fan id for SetGamingFanSpeed)
    if i == 0 { (1, 0, 1) } else { (1 << (2 + i), 4 + 2 * i as u64, 3 + i as u64) }
}

/// One SetGamingFanBehavior call per behaviour group, then the speed of each custom fan.
pub fn set_fans(w: &Wmi, fans: &[(u8, u8)]) -> bool {
    let mut good = true;
    for beh in 1..=3 {
        let (mut map, mut bits) = (0, 0);
        for (i, _) in fans.iter().enumerate().filter(|(_, f)| f.0 == beh) {
            let (m, s, _) = fan_slot(i);
            map |= m;
            bits |= (beh as u64) << s;
        }
        if map != 0 {
            good &= set(w, SET_FAN_BEHAVIOR, bits << 16 | map);
        }
    }
    for (i, f) in fans.iter().enumerate().filter(|(_, f)| f.0 == 3) {
        good &= set_fan_speed(w, i, f.1);
    }
    good
}
pub fn set_fan_speed(w: &Wmi, i: usize, pct: u8) -> bool {
    set(w, SET_FAN_SPEED, (pct.min(100) as u64) << 8 | fan_slot(i).2)
}

// ---- rear lid logo: one colour and a brightness, no effects.
// Protocol from nekro-sense / linuwu_sense (PHN16-72 only).

/// SetBackLogo [1, R, G, B, brightness, enabled], then the light-bar enable
/// gate through SetGamingKBBacklight (byte 0 = enabled, byte 9 = 2).
pub fn set_rear(w: &Wmi, rgb: [u8; 3], brightness: u8) -> bool {
    let on = (brightness > 0) as u8;
    let mut gate = [0u8; 16];
    gate[0] = on;
    gate[9] = 2;
    set_buf(w, Gaming, SET_BACK_LOGO, &[1, rgb[0], rgb[1], rgb[2], brightness.min(100), on])
        && w.call(Gaming, SET_KB_BACKLIGHT, &gate).is_some()
}
/// GetBackLogo [1] -> [status, R, G, B, brightness, enabled]
pub fn rear(w: &Wmi) -> Option<([u8; 3], u8)> {
    let o = w.call(Gaming, GET_BACK_LOGO, &[1]).filter(|o| o.len() >= 6 && o[0] == 0)?;
    Some(([o[1], o[2], o[3]], if o[5] != 0 { o[4] } else { 0 }))
}

// ---- keyboard (EC, 4 RGB zones)

pub const KEYBOARD: u8 = 1;

/// SetGamingKBBacklight; `device` is byte 9 (1 = keyboard).
pub fn kb_effect(w: &Wmi, device: u8, effect: u8, speed: u8, brightness: u8, dir: u8, rgb: [u8; 3]) -> bool {
    let mut b = [0u8; 16];
    b[0] = effect;
    b[1] = if effect == 0 { 0 } else { speed.clamp(1, 9) };
    b[2] = brightness.min(100);
    b[4] = if effect == 0 { 0 } else { dir };
    b[5..8].copy_from_slice(&rgb);
    b[8] = 3;
    b[9] = device;
    set_buf(w, Gaming, SET_KB_BACKLIGHT, &b)
}

/// GetGamingKBBacklight(device): (effect, speed, brightness, direction, rgb).
/// Result = status byte, then the setter's layout.
pub fn kb_read(w: &Wmi, device: u8) -> Option<(u8, u8, u8, u8, [u8; 3])> {
    let o = w.call(Gaming, GET_KB_BACKLIGHT, &[device, 0, 0, 0]).filter(|o| o.len() >= 9 && o[0] == 0)?;
    Some((o[1], o[2], o[3], o[5], [o[6], o[7], o[8]]))
}

/// Static per-zone colours: zone mask (SetGamingLED), static effect, then one SetGamingRgbKb per zone.
pub fn kb_zones(w: &Wmi, brightness: u8, zones: &[[u8; 3]; 4]) -> bool {
    let mut led = [0u8; 16];
    led[0] = 8;
    led[5] = (0..4).filter(|&i| zones[i] != [0; 3]).fold(0, |m, i| m | 1 << i);
    if !set_buf(w, Gaming, SET_LED, &led) || !kb_effect(w, KEYBOARD, 0, 0, brightness, 0, [0; 3]) {
        return false;
    }
    zones.iter().enumerate().fold(true, |g, (i, c)| {
        set(w, SET_RGB_KB, (c[2] as u64) << 24 | (c[1] as u64) << 16 | (c[0] as u64) << 8 | 1 << i) && g
    })
}

/// Backlight turns off after 30 s idle (APGeAction, BK hotkey).
pub fn kb_timeout(w: &Wmi) -> Option<bool> {
    w.u32(Apge, 2, (BK << 8 | 0x80001) as u32).filter(|v| ok(*v)).map(|v| v >> 40 & 0xFF == 0x1E)
}
pub fn set_kb_timeout(w: &Wmi, on: bool) -> bool {
    // keep the current brightness (byte 4 of the get result)
    let br = w.u32(Apge, 2, (BK << 8 | 0x80001) as u32).filter(|v| ok(*v)).map_or(100, |v| v >> 32 & 0xFF);
    let t = if on { 0x1E00 } else { 0 };
    w.u64(Apge, 1, BK << 8 | 0x80002 | (t | br) << 32).is_some_and(ok)
}

// ---- misc

pub fn boot_sound(w: &Wmi) -> Option<bool> {
    get(w, GET_MISC, 6).and_then(|v| match v & 0xFF00 {
        0x100 => Some(true),
        0 => Some(false),
        _ => None,
    })
}
pub fn set_boot_sound(w: &Wmi, on: bool) -> bool {
    set(w, SET_MISC, if on { 0x106 } else { 6 })
}
/// 0 off, 1 on, 0xFF unsupported
pub fn lcd_overdrive(w: &Wmi) -> Option<u8> {
    get(w, GET_PROFILE, 0).map(|v| (v >> 48) as u8)
}
pub fn set_lcd_overdrive(w: &Wmi, on: bool) -> bool {
    set(w, SET_PROFILE, if on { 0x1_0000_0000_0010 } else { 0x10 })
}

/// Power-off USB charging (APGeAction function 4): (enabled, battery threshold %).
pub fn usb_charging(w: &Wmi) -> Option<(bool, u8)> {
    w.u32(Apge, 2, 4).filter(|v| ok(*v)).map(|v| (v >> 8 & 0xFF == 0x0F, (v >> 16) as u8))
}
pub fn set_usb_charging(w: &Wmi, on: bool, threshold: u8) -> bool {
    let state = if on { 0x0F } else { 0x1F };
    let thr = if matches!(threshold, 20 | 30) { threshold } else { 10 } as u64;
    w.u64(Apge, 1, thr << 16 | state << 8 | 4).is_some_and(ok)
}

// ---- battery (BatteryControl)

/// (80% limit, calibration running)
pub fn battery_health(w: &Wmi) -> Option<(bool, bool)> {
    let o = w.call(Battery, 20, &[1, 1, 0, 0]).filter(|o| o.len() >= 5)?;
    Some((o[3] != 0, o[4] != 0))
}
/// function 1 = 80% limit, 2 = calibration
pub fn set_battery(w: &Wmi, function: u8, on: bool) -> bool {
    w.call(Battery, 21, &[1, function, on as u8, 0, 0, 0, 0, 0]).is_some_and(|o| o.len() >= 2 && o[0] == 0 && o[1] == 0)
}
/// GetBatteryInformation(index 8 = temperature, battery 1), deci-Kelvin.
pub fn battery_temp(w: &Wmi) -> Option<f64> {
    let o = w.call(Battery, 19, &[8, 0, 0, 0, 1, 0, 0, 0]).filter(|o| o.len() == 4)?;
    let v = le64(&o);
    (2000..0xFFFF).contains(&v).then(|| (v as f64 - 2731.0) / 10.0)
}
