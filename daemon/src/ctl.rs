//! predatorctl: command line client (handy for Hyprland keybinds).

use serde_json::{Value, json};

const USAGE: &str = "usage: predatorctl status
       predatorctl mode quiet|balanced|performance|turbo|eco        (now)
       predatorctl charger-mode|battery-mode <mode>               (applied on plug/unplug)
       predatorctl fan auto|max|custom|curve
       predatorctl light <effect> [#rrggbb] [brightness]
       predatorctl rear #rrggbb [brightness]
       predatorctl set <option> <json value>    e.g. set battery_limit true
       predatorctl agent     user service: screen Hz for charger/battery, 90 °C alerts";

const MODES: [(&str, u8); 5] = [("quiet", 0), ("balanced", 1), ("performance", 4), ("turbo", 5), ("eco", 6)];

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let a: Vec<&str> = a.iter().map(String::as_str).collect();
    let bus = if std::env::var_os("PREDATOR_SIM").is_some() { zbus::blocking::Connection::session() } else { zbus::blocking::Connection::system() };
    let conn = bus.unwrap_or_else(|e| fail(&e.to_string()));
    let call = |method: &str, body: &dyn erased::Body| -> String { body.call(&conn, method).unwrap_or_else(|e| fail(&e.to_string())) };
    let get = |m: &str| -> Value { serde_json::from_str(&call(m, &())).unwrap() };
    let mode = |name: &str| MODES.iter().find(|m| m.0 == name).map(|m| m.1).unwrap_or_else(|| fail(USAGE));

    match a.as_slice() {
        ["status"] | [] => {
            let (st, se) = (get("GetState"), get("GetSensors"));
            let name = |v: &Value| MODES.iter().find(|m| Some(m.1 as u64) == v.as_u64()).map_or("?", |m| m.0);
            println!("mode      {}  (AC: {}, battery: {})", name(&st["mode"]), name(&st["ac_mode"]), name(&st["dc_mode"]));
            println!("CPU       {}°C  {} RPM", se["cpu_temp"], se["cpu_fan"]);
            println!("GPU       {}°C  {} RPM", se["gpu_temp"], se["gpu_fan"]);
            println!("fans      {}", st["fan"]["mode"]);
            println!("lighting  {} {}%", st["light"]["effect"], st["light"]["brightness"]);
            println!("battery   80% limit {}", st["battery_limit"]);
        }
        ["mode", m] => { call("SetMode", &mode(m)); }
        ["charger-mode", m] => { call("SetChargerMode", &mode(m)); }
        ["battery-mode", m] => { call("SetBatteryMode", &mode(m)); }
        ["fan", m] => {
            let mut f = get("GetState")["fan"].clone();
            f["mode"] = json!(m);
            call("SetFan", &(now_mode(&get("GetState")), f.to_string()));
        }
        ["rear", color, rest @ ..] => {
            let mut l = get("GetState")["rear"].clone();
            l["color"] = json!(color);
            if let Some(b) = rest.first() {
                l["brightness"] = json!(b.parse::<u8>().unwrap_or_else(|_| fail(USAGE)));
            }
            call("SetRear", &(now_mode(&get("GetState")), l.to_string()));
        }
        [which @ "light", effect, rest @ ..] => {
            let mut l = get("GetState")[*which].clone();
            l["effect"] = json!(effect);
            if let Some(c) = rest.first() {
                l["color"] = json!(c);
                l["zones"] = json!([c, c, c, c]);
            }
            if let Some(b) = rest.get(1) {
                l["brightness"] = json!(b.parse::<u8>().unwrap_or_else(|_| fail(USAGE)));
            }
            call("SetLight", &(now_mode(&get("GetState")), l.to_string()));
        }
        ["set", key, value] => { call("SetOption", &(*key, *value)); }
        ["agent"] => agent(&conn),
        _ => fail(USAGE),
    }
}

/// Screen refresh rate of the built-in panel (eDP-1) through hyprctl.
fn refresh_rate() -> Option<u32> {
    let out = std::process::Command::new("hyprctl").args(["monitors", "-j"]).output().ok()?;
    let mons: Value = serde_json::from_slice(&out.stdout).ok()?;
    let m = mons.as_array()?.iter().find(|m| m["name"] == "eDP-1")?;
    Some(m["refreshRate"].as_f64()?.round() as u32)
}

/// The user-session half of the service, run by predator-sense-agent.service:
/// - turns the screen back on after suspend/hibernate (NVIDIA can leave it black);
/// - sets the panel refresh rate chosen for charger / battery when the charger is
///   plugged or unplugged, at login and after a Hyprland config reload; a rate you
///   set by hand in between is left alone;
/// - desktop notification at 90 °C, again only after the part cooled below 85 °C.
fn agent(sys: &zbus::blocking::Connection) -> ! {
    let session = zbus::blocking::Connection::session().unwrap_or_else(|e| fail(&e.to_string()));
    // started before Hyprland exported its socket to systemd: find the instance ourselves
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        let dir = format!("{}/hypr", std::env::var("XDG_RUNTIME_DIR").unwrap_or_default());
        let newest = std::fs::read_dir(dir).into_iter().flatten().flatten().max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
        if let Some(e) = newest {
            unsafe { std::env::set_var("HYPRLAND_INSTANCE_SIGNATURE", e.file_name()) };
        }
    }
    // Hyprland reloads its config (monitors.lua sets the mode again): event on socket2
    let reloaded = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = reloaded.clone();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let sock = format!(
            "{}/hypr/{}/.socket2.sock",
            std::env::var("XDG_RUNTIME_DIR").unwrap_or_default(),
            std::env::var("HYPRLAND_INSTANCE_SIGNATURE").unwrap_or_default()
        );
        let Ok(stream) = std::os::unix::net::UnixStream::connect(sock) else { return };
        for line in std::io::BufReader::new(stream).lines().map_while(Result::ok) {
            if line.starts_with("configreloaded>>") {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    });
    // After suspend/hibernate the NVIDIA panel can stay black (DPMS stays off):
    // turn it back on when logind says we are awake again.
    let sys2 = sys.clone();
    std::thread::spawn(move || {
        let Ok(p) = zbus::blocking::Proxy::new(&sys2, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager") else { return };
        let Ok(signals) = p.receive_signal("PrepareForSleep") else { return };
        for m in signals {
            if matches!(m.body().deserialize::<bool>(), Ok(false)) {
                for wait in [2, 4] {
                    std::thread::sleep(std::time::Duration::from_secs(wait));
                    let _ = std::process::Command::new("hyprctl").args(["dispatch", r#"hl.dsp.dpms({ action = "on" })"#]).output();
                }
                eprintln!("resumed: screen turned on");
            }
        }
    });
    let mut last_ac: Option<bool> = None;
    let mut armed = [true; 2];
    loop {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let get = |m: &str| erased::Body::call(&(), sys, m).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok());
        if let Some(st) = get("GetState") {
            let ac = st["ac"] == true;
            let event = last_ac != Some(ac) || reloaded.swap(false, std::sync::atomic::Ordering::Relaxed);
            last_ac = Some(ac);
            let want = st[if ac { "refresh_ac" } else { "refresh_dc" }].as_u64().unwrap_or(165) as u32;
            if event && refresh_rate().is_some_and(|hz| hz != want) {
                let lua = format!(r#"hl.monitor({{ output = "eDP-1", mode = "2560x1600@{want}", position = "auto", scale = "auto" }})"#);
                let _ = std::process::Command::new("hyprctl").args(["eval", &lua]).output();
                eprintln!("screen set to {want} Hz ({})", if st["ac"] == true { "charger" } else { "battery" });
            }
        }
        let Some(se) = get("GetSensors") else { continue };
        for (i, (key, part)) in [("cpu_temp", "CPU"), ("gpu_temp", "GPU")].into_iter().enumerate() {
            match se[key].as_f64() {
                Some(t) if t >= 90.0 && armed[i] => {
                    armed[i] = false;
                    let hints = std::collections::HashMap::from([("urgency", zbus::zvariant::Value::U8(2))]);
                    let body = format!("{part} is at {t:.0} °C. Check the fans or switch to a quieter mode.");
                    let _ = session.call_method(
                        Some("org.freedesktop.Notifications"),
                        "/org/freedesktop/Notifications",
                        Some("org.freedesktop.Notifications"),
                        "Notify",
                        &("Predator Sense", 0u32, "predator-sense", format!("{part} temperature high"), body, Vec::<&str>::new(), hints, 10000i32),
                    );
                }
                Some(t) if t < 85.0 => armed[i] = true,
                _ => {}
            }
        }
    }
}

/// predatorctl changes the settings of the mode that is active now.
fn now_mode(st: &Value) -> u8 {
    st["mode"].as_u64().unwrap_or(1) as u8
}

fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1)
}

/// Lets one closure send bodies of different types.
mod erased {
    pub trait Body {
        fn call(&self, c: &zbus::blocking::Connection, method: &str) -> zbus::Result<String>;
    }
    impl<T: serde::Serialize + zbus::zvariant::DynamicType> Body for T {
        fn call(&self, c: &zbus::blocking::Connection, method: &str) -> zbus::Result<String> {
            let r = c.call_method(Some("org.predatorsense.Daemon"), "/org/predatorsense/Daemon", Some("org.predatorsense.Daemon1"), method, self)?;
            Ok(r.body().deserialize::<String>().unwrap_or_default())
        }
    }
}
