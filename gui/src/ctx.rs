//! Connection to predator-sensed plus the shared state every page reads.

use adw::prelude::*;
use gtk::{gio, glib};
use serde_json::Value;
use std::{cell::{Cell, RefCell}, rc::Rc};

type Updater = Box<dyn Fn(&Value, &Value)>; // (state, sensors)

pub struct Ctx {
    proxy: Option<gio::DBusProxy>,
    pub toast: adw::ToastOverlay,
    pub info: Value,
    pub state: RefCell<Value>,
    pub sensors: RefCell<Value>,
    updating: Cell<bool>,
    updaters: RefCell<Vec<Updater>>,
}

fn get(proxy: &Option<gio::DBusProxy>, method: &str) -> Value {
    let Some(p) = proxy else { return Value::Null };
    let Ok(v) = p.call_sync(method, None, gio::DBusCallFlags::NONE, 3000, gio::Cancellable::NONE) else { return Value::Null };
    v.try_child_value(0).and_then(|c| c.str().and_then(|s| serde_json::from_str(s).ok())).unwrap_or(Value::Null)
}

impl Ctx {
    pub fn new() -> Rc<Self> {
        let bus = if std::env::var_os("PREDATOR_SIM").is_some() { gio::BusType::Session } else { gio::BusType::System };
        let proxy = gio::DBusProxy::for_bus_sync(
            bus,
            gio::DBusProxyFlags::DO_NOT_LOAD_PROPERTIES,
            None,
            "org.predatorsense.Daemon",
            "/org/predatorsense/Daemon",
            "org.predatorsense.Daemon1",
            gio::Cancellable::NONE,
        )
        .ok();
        Rc::new(Ctx {
            info: get(&proxy, "GetInfo"),
            state: RefCell::new(get(&proxy, "GetState")),
            proxy,
            toast: adw::ToastOverlay::new(),
            sensors: RefCell::new(Value::Null),
            updating: Cell::new(false),
            updaters: RefCell::new(vec![]),
        })
    }

    pub fn online(&self) -> bool {
        !self.info.is_null()
    }

    pub fn get(&self, method: &str) -> Value {
        get(&self.proxy, method)
    }

    /// Call a setter; ignored while widgets are being refreshed from the daemon.
    pub fn set(self: &Rc<Self>, method: &str, args: glib::Variant) {
        if self.updating.get() {
            return;
        }
        let Some(p) = &self.proxy else { return };
        let ctx = self.clone();
        p.call(method, Some(&args), gio::DBusCallFlags::NONE, 30000, gio::Cancellable::NONE, move |r| {
            if let Err(e) = r {
                // "GDBus.Error:org.freedesktop.DBus.Error.Failed: <message>"
                let m = e.message();
                ctx.toast.add_toast(adw::Toast::new(m.split_once(": ").map_or(m, |x| x.1)));
            }
            ctx.refresh_state();
        });
    }

    /// Settings of one mode: SetFan / SetLight / SetRear (mode, json).
    pub fn set_for(self: &Rc<Self>, method: &str, mode: u8, v: &Value) {
        self.set(method, (mode, v.to_string()).to_variant());
    }

    pub fn is_updating(&self) -> bool {
        self.updating.get()
    }

    pub fn set_option(self: &Rc<Self>, key: &str, v: Value) {
        self.set("SetOption", (key, v.to_string()).to_variant());
    }

    pub fn on_update(&self, f: impl Fn(&Value, &Value) + 'static) {
        self.updaters.borrow_mut().push(Box::new(f));
    }

    pub fn update_all(&self) {
        self.updating.set(true);
        let (st, se) = (self.state.borrow().clone(), self.sensors.borrow().clone());
        for f in self.updaters.borrow().iter() {
            f(&st, &se);
        }
        self.updating.set(false);
    }

    pub fn refresh_state(&self) {
        self.state.replace(self.get("GetState"));
        self.update_all();
    }

    pub fn tick(&self, n: u32) {
        self.sensors.replace(self.get("GetSensors"));
        if n.is_multiple_of(3) {
            self.state.replace(self.get("GetState"));
        }
        self.update_all();
    }
}
