//! Power settings outside the firmware that follow the mode and the power source.

use std::collections::HashMap;
use zbus::{blocking::Connection, zvariant::OwnedValue};

const PPD: &str = "org.freedesktop.UPower.PowerProfiles";
const PPD_PATH: &str = "/org/freedesktop/UPower/PowerProfiles";

/// power-profiles-daemon profile for each operating mode. PPD only drives the CPU
/// (EPP): the package starts it with --block-driver platform_profile, the
/// firmware mode is ours.
pub fn ppd_profile(mode: u8) -> &'static str {
    match mode {
        0 | 6 => "power-saver", // Quiet, Eco
        5 => "performance",     // Turbo
        _ => "balanced",
    }
}

/// Never start PPD through D-Bus activation: it is ordered after multi-user.target,
/// which waits for us, so activating it at boot deadlocks until our start times out.
fn ppd_running(bus: &Connection) -> bool {
    zbus::blocking::fdo::DBusProxy::new(bus)
        .and_then(|p| Ok(p.name_has_owner(PPD.try_into()?)?))
        .unwrap_or(false)
}

pub fn set_ppd(bus: &Connection, profile: &str) {
    if !ppd_running(bus) {
        return; // set when it starts, see watch_ppd
    }
    let args = (PPD, "ActiveProfile", zbus::zvariant::Value::from(profile));
    if let Err(e) = bus.call_method(Some(PPD), PPD_PATH, Some("org.freedesktop.DBus.Properties"), "Set", &args) {
        eprintln!("power-profiles-daemon: {e}");
    }
}

/// Waits for PPD to start, calls `started` (to give it our profile), then calls
/// `f` whenever PPD's profile changes (e.g. picked in Noctalia).
pub fn watch_ppd(bus: &Connection, started: impl Fn(), f: impl Fn(&str)) {
    while !ppd_running(bus) {
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    started();
    let Ok(p) = zbus::blocking::Proxy::new(bus, PPD, PPD_PATH, "org.freedesktop.DBus.Properties") else { return };
    let Ok(signals) = p.receive_signal("PropertiesChanged") else { return };
    for m in signals {
        let Ok((_, changed, _)) = m.body().deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>() else { continue };
        if let Some(Ok(profile)) = changed.get("ActiveProfile").map(|v| String::try_from(v.clone())) {
            f(&profile);
        }
    }
}

/// Sound card sleeps after 1 s of silence on battery; never on the charger (no pops).
pub fn set_audio_power_save(battery: bool) {
    let _ = std::fs::write("/sys/module/snd_hda_intel/parameters/power_save", if battery { "1" } else { "0" });
}
