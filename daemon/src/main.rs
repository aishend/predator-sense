//! predator-sensed: root service for the Acer Predator PHN16-72.
//! System-bus API org.predatorsense.Daemon1; reads are open, changes need the
//! polkit action org.predatorsense.control. `--simulate` fakes the firmware on the session bus.

mod daemon;
mod gpu;
mod hw;
mod power;
mod wmi;

use daemon::{Daemon, Fan, Light};
use std::{collections::HashMap, sync::{Arc, Mutex}, time::Duration};
use zbus::{Connection, fdo, interface, message::Header, object_server::SignalEmitter, zvariant::Value};

const NAME: &str = "org.predatorsense.Daemon";
const PATH: &str = "/org/predatorsense/Daemon";

struct Bus {
    d: Arc<Mutex<Daemon>>,
    auth: bool,
}

fn parse<T: serde::de::DeserializeOwned>(j: &str) -> fdo::Result<T> {
    serde_json::from_str(j).map_err(|e| fdo::Error::InvalidArgs(e.to_string()))
}

impl Bus {
    async fn check(&self, c: &Connection, h: &Header<'_>) -> fdo::Result<()> {
        if !self.auth {
            return Ok(());
        }
        let sender = h.sender().ok_or_else(|| fdo::Error::AccessDenied("no sender".into()))?;
        if fdo::DBusProxy::new(c).await?.get_connection_unix_user(sender.clone().into()).await? == 0 {
            return Ok(()); // root, e.g. sudo predatorctl
        }
        let subject = ("system-bus-name", HashMap::from([("name", Value::from(sender.as_str()))]));
        let reply = c
            .call_method(
                Some("org.freedesktop.PolicyKit1"),
                "/org/freedesktop/PolicyKit1/Authority",
                Some("org.freedesktop.PolicyKit1.Authority"),
                "CheckAuthorization",
                &(subject, "org.predatorsense.control", HashMap::<&str, &str>::new(), 1u32, ""),
            )
            .await?;
        let (ok, _, _): (bool, bool, HashMap<String, String>) = reply.body().deserialize()?;
        if ok { Ok(()) } else { Err(fdo::Error::AccessDenied("not authorized".into())) }
    }

    fn run(&self, f: impl FnOnce(&mut Daemon) -> daemon::R) -> fdo::Result<()> {
        f(&mut self.d.lock().unwrap()).map_err(fdo::Error::Failed)
    }
}

#[interface(name = "org.predatorsense.Daemon1")]
impl Bus {
    fn get_info(&self) -> String {
        self.d.lock().unwrap().info().to_string()
    }
    fn get_state(&self) -> String {
        self.d.lock().unwrap().state().to_string()
    }
    fn get_sensors(&self) -> String {
        self.d.lock().unwrap().sensors().to_string()
    }
    async fn set_mode(&self, #[zbus(connection)] c: &Connection, #[zbus(header)] h: Header<'_>, mode: u8) -> fdo::Result<()> {
        self.check(c, &h).await?;
        self.run(|d| d.set_mode(mode))
    }
    async fn set_charger_mode(&self, #[zbus(connection)] c: &Connection, #[zbus(header)] h: Header<'_>, mode: u8) -> fdo::Result<()> {
        self.check(c, &h).await?;
        self.run(|d| d.set_policy(true, mode))
    }
    async fn set_battery_mode(&self, #[zbus(connection)] c: &Connection, #[zbus(header)] h: Header<'_>, mode: u8) -> fdo::Result<()> {
        self.check(c, &h).await?;
        self.run(|d| d.set_policy(false, mode))
    }
    async fn set_fan(&self, #[zbus(connection)] c: &Connection, #[zbus(header)] h: Header<'_>, json: &str) -> fdo::Result<()> {
        let f: Fan = parse(json)?;
        self.check(c, &h).await?;
        self.run(|d| d.set_fan(f))
    }
    async fn set_light(&self, #[zbus(connection)] c: &Connection, #[zbus(header)] h: Header<'_>, json: &str) -> fdo::Result<()> {
        let l: Light = parse(json)?;
        self.check(c, &h).await?;
        self.run(|d| d.set_light(l, false))
    }
    async fn set_rear(&self, #[zbus(connection)] c: &Connection, #[zbus(header)] h: Header<'_>, json: &str) -> fdo::Result<()> {
        let l: Light = parse(json)?;
        self.check(c, &h).await?;
        self.run(|d| d.set_light(l, true))
    }
    async fn set_option(&self, #[zbus(connection)] c: &Connection, #[zbus(header)] h: Header<'_>, key: &str, json: &str) -> fdo::Result<()> {
        let v: serde_json::Value = parse(json)?;
        self.check(c, &h).await?;
        self.run(|d| d.set_option(key, &v))
    }
    #[zbus(signal)]
    async fn state_changed(e: &SignalEmitter<'_>, state: &str) -> zbus::Result<()>;
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sim = std::env::args().any(|a| a == "--simulate");
    let d = Daemon::new(sim).map_err(|e| format!("/dev/predator-wmi: {e} (is the predator_wmi module loaded?)"))?;
    let d = Arc::new(Mutex::new(d));
    let builder = if sim { zbus::blocking::connection::Builder::session()? } else { zbus::blocking::connection::Builder::system()? };
    let conn = builder.name(NAME)?.serve_at(PATH, Bus { d: d.clone(), auth: !sim })?.build()?;

    // Restore everything after resume, like PSSdk.ServiceResume (the EC forgets fan/lighting state).
    let (d2, c2) = (d.clone(), conn.clone());
    std::thread::spawn(move || {
        let Ok(p) = zbus::blocking::Proxy::new(&c2, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager") else { return };
        let Ok(signals) = p.receive_signal("PrepareForSleep") else { return };
        for m in signals {
            if matches!(m.body().deserialize::<bool>(), Ok(false)) {
                std::thread::sleep(Duration::from_secs(2));
                d2.lock().unwrap().restore("resume");
            }
        }
    });

    // Power profile picked in Noctalia (power-profiles-daemon) -> matching mode.
    if !sim {
        let d3 = d.clone();
        std::thread::spawn(move || {
            if let Ok(bus) = zbus::blocking::Connection::system() {
                power::watch_ppd(&bus, |p| d3.lock().unwrap().ppd_changed(p));
            }
        });
    }

    let iface = conn.object_server().interface::<_, Bus>(PATH)?;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let changed = {
            let mut d = d.lock().unwrap();
            d.poll();
            std::mem::take(&mut d.dirty).then(|| d.state().to_string())
        };
        if let Some(s) = changed {
            let _ = zbus::block_on(Bus::state_changed(iface.signal_emitter(), &s));
        }
    }
}
