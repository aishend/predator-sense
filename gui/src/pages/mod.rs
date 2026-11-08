mod fans;
mod lighting;

pub use fans::fans;
pub use lighting::lighting;

use crate::{ctx::Ctx, widgets::*};
use adw::prelude::*;
use serde_json::{Value, json};
use std::{cell::{Cell, RefCell}, collections::VecDeque, rc::Rc};

pub const MODES: [(u8, &str, &str); 5] = [
    (0, "Quiet", "Lowest noise. Fans stay on Automatic."),
    (1, "Balanced", "Everyday use."),
    (4, "Performance", "More power, louder fans."),
    (5, "Turbo", "Maximum power and GPU overclock."),
    (6, "Eco", "Longest battery life."),
];

/// "Settings for" bar: pick the mode whose fans or lighting you are editing.
/// Follows the active mode until you pick one. Returns the bar and a function
/// giving the mode being edited for a state (also refreshes the buttons).
pub fn mode_picker(ctx: &Rc<Ctx>) -> (gtk::Box, impl Fn(&Value) -> u8 + use<>) {
    let modes: Vec<u8> = ctx.info["modes"].as_array().map(|a| a.iter().filter_map(|v| v.as_u64().map(|v| v as u8)).collect()).unwrap_or(vec![0, 1, 4, 5, 6]);
    let picked: Rc<Cell<Option<u8>>> = Rc::default();
    let (p, c, ms) = (picked.clone(), ctx.clone(), modes.clone());
    let names: Vec<&str> = modes.iter().map(|m| MODES.iter().find(|x| x.0 == *m).map_or("?", |x| x.1)).collect();
    let (bar, btns) = segmented(&names, move |i| {
        if !c.is_updating() {
            p.set(Some(ms[i]));
            c.update_all();
        }
    });
    let row = hbox(12);
    row.append(&label("Settings for", "dim"));
    row.append(&bar);
    let which = move |st: &Value| {
        let active = st["mode"].as_u64().map(|m| m as u8);
        let m = picked.get().or(active).unwrap_or(1);
        for (i, b) in btns.iter().enumerate() {
            let name = MODES.iter().find(|x| x.0 == modes[i]).map_or("?", |x| x.1);
            b.set_label(&if active == Some(modes[i]) { format!("{name} ●") } else { name.to_string() });
            if modes[i] == m {
                b.set_active(true);
            }
        }
        m
    };
    (row, which)
}

pub fn mode_name(v: &Value) -> &'static str {
    MODES.iter().find(|m| Some(m.0 as u64) == v.as_u64()).map_or("—", |m| m.1)
}

fn read(p: &str) -> String {
    std::fs::read_to_string(p).unwrap_or_default().trim().to_owned()
}

fn field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find(|l| l.starts_with(key)).and_then(|l| l.split_once(':')).map(|(_, v)| v.trim())
}

fn fmt(v: &Value, unit: &str) -> String {
    v.as_f64().map_or("—".into(), |n| format!("{n:.0}{unit}"))
}

// ---------------------------------------------------------------- Dashboard

fn specs() -> Vec<(&'static str, &'static str, String)> {
    let cpu = read("/proc/cpuinfo");
    let threads = cpu.lines().filter(|l| l.starts_with("processor")).count();
    let ghz = read("/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq").parse::<f64>().unwrap_or(0.0) / 1e6;
    let gpu = std::fs::read_dir("/proc/driver/nvidia/gpus").ok().and_then(|mut d| d.next()).and_then(|e| e.ok())
        .map(|e| read(&format!("{}/information", e.path().display())));
    let gpu_model = gpu.as_deref().and_then(|g| field(g, "Model")).unwrap_or("NVIDIA GeForce RTX 4060 Laptop GPU");
    let driver = read("/proc/driver/nvidia/version").split_whitespace().skip_while(|w| *w != "Module").nth(1).unwrap_or("").to_owned();
    let mem_kb: f64 = field(&read("/proc/meminfo"), "MemTotal").and_then(|v| v.split(' ').next()?.parse().ok()).unwrap_or(0.0);
    let disks: Vec<String> = std::fs::read_dir("/sys/block").into_iter().flatten().flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("nvme"))
        .map(|e| {
            let p = e.path().display().to_string();
            let gb = read(&format!("{p}/size")).parse::<f64>().unwrap_or(0.0) * 512.0 / 1e9;
            format!("{} · {gb:.0} GB", read(&format!("{p}/device/model")))
        })
        .collect();
    let os_release = read("/etc/os-release");
    let os = os_release.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")).unwrap_or("Linux").trim_matches('"');
    vec![
        ("CPU", "cpu-symbolic", format!("{}\n{threads} threads · up to {ghz:.1} GHz", field(&cpu, "model name").unwrap_or("?"))),
        ("GPU", "video-display-symbolic", format!("{gpu_model}\nDriver {driver}")),
        ("Memory", "memory-symbolic", format!("{:.0} GB", mem_kb / 1024.0 / 1024.0)),
        ("Storage", "drive-harddisk-symbolic", disks.join("\n")),
        ("System", "computer-symbolic", format!("{os}\nKernel {}", read("/proc/sys/kernel/osrelease"))),
        ("BIOS", "application-x-firmware-symbolic", read("/sys/class/dmi/id/bios_version")),
    ]
}

pub fn home(ctx: &Rc<Ctx>) -> gtk::Widget {
    let (sw, c) = page("Dashboard");
    let hero = hbox(28);
    hero.add_css_class("panel");
    hero.add_css_class("hero");
    hero.append(&gtk::Image::builder().icon_name("predator-sense").pixel_size(140).build());
    let t = vbox(4);
    t.set_valign(gtk::Align::Center);
    t.append(&label("Acer", "accent"));
    t.append(&label(ctx.info["product"].as_str().unwrap_or("Predator PHN16-72"), "hero-title"));
    let quick = label("", "dim");
    t.append(&quick);
    hero.append(&t);
    c.append(&hero);

    c.append(&label("Specifications", "section"));
    let grid = gtk::Grid::builder().column_spacing(14).row_spacing(14).column_homogeneous(true).build();
    for (i, (title, icon, text)) in specs().into_iter().enumerate() {
        let card = hbox(14);
        card.add_css_class("panel");
        card.append(&gtk::Image::builder().icon_name(icon).pixel_size(28).valign(gtk::Align::Start).build());
        let v = vbox(4);
        v.append(&label(title, "accent"));
        v.append(&label(&text, ""));
        card.append(&v);
        grid.attach(&card, (i % 2) as i32, (i / 2) as i32, 1, 1);
    }
    c.append(&grid);

    ctx.on_update(move |st, se| {
        quick.set_label(&format!(
            "Mode {} · CPU {} · GPU {} · Fans {} · {}",
            mode_name(&st["mode"]),
            fmt(&se["cpu_temp"], "°C"),
            fmt(&se["gpu_temp"], "°C"),
            st["fan"]["mode"].as_str().unwrap_or("—"),
            if st["ac"] == true { "Plugged in" } else { "On battery" },
        ));
    });
    sw.upcast()
}

// ---------------------------------------------------------------- Monitor

const HISTORY: usize = 60; // samples every 2 s = 2 minutes

pub fn monitor(ctx: &Rc<Ctx>) -> gtk::Widget {
    let (sw, c) = page("Monitor");
    let grid = gtk::Grid::builder().column_spacing(14).row_spacing(14).column_homogeneous(true).build();
    let mut gauges = vec![];
    let temps = [("CPU", "cpu_temp"), ("GPU", "gpu_temp"), ("System", "sys_temp"), ("Battery", "battery_temp"), ("SSD", "nvme"), ("Wi-Fi", "iwlwifi_1"), ("RAM", "spd5118")];
    for (i, (name, key)) in temps.into_iter().enumerate() {
        let card = hbox(10);
        card.add_css_class("panel");
        let v = vbox(2);
        v.set_hexpand(true);
        v.set_valign(gtk::Align::Center);
        v.append(&label(name, ""));
        let value = label("—", "big-number");
        v.append(&value);
        card.append(&v);
        let level = Rc::new(Cell::new(0.0));
        card.append(&gauge(level.clone(), if i == 1 { ORANGE } else { CYAN }));
        grid.attach(&card, (i % 4) as i32, (i / 4) as i32, 1, 1);
        gauges.push((key, value, level, card));
    }
    c.append(&grid);

    let hist: Rc<RefCell<Vec<VecDeque<f64>>>> = Rc::new(RefCell::new(vec![VecDeque::new(), VecDeque::new()]));
    let chart_card = card("Temperature, last 2 minutes");
    let legend = hbox(16);
    legend.append(&label("● CPU", "accent"));
    legend.append(&label("● GPU", "orange"));
    chart_card.append(&legend);
    let ch = chart(hist.clone(), vec![CYAN, ORANGE], 100.0, HISTORY);
    chart_card.append(&ch);
    c.append(&chart_card);

    let stats = gtk::Grid::builder().column_spacing(14).row_spacing(14).column_homogeneous(true).build();
    let stat = |i: i32, title: &str| {
        let card = card(title);
        let l = label("—", "stat");
        card.append(&l);
        stats.attach(&card, i % 3, i / 3, 1, 1);
        l
    };
    let (cpu_l, gpu_l, ram_l) = (stat(0, "Processor"), stat(1, "Graphics"), stat(2, "Memory"));
    c.append(&stats);

    let prev_cpu = Cell::new((0u64, 0u64));
    let ch2 = ch.clone();
    ctx.on_update(move |_, se| {
        // SSD, Wi-Fi and RAM come from the kernel's hwmon sensors (hottest of each kind)
        let mut se = se.clone();
        for e in std::fs::read_dir("/sys/class/hwmon").into_iter().flatten().flatten() {
            let p = e.path().display().to_string();
            let t = read(&format!("{p}/temp1_input")).parse::<f64>().ok().map(|t| t / 1000.0);
            let name = read(&format!("{p}/name"));
            if let Some(t) = t.filter(|_| ["nvme", "iwlwifi_1", "spd5118"].contains(&name.as_str())) {
                se[&name] = json!(se[&name].as_f64().map_or(t, |v| v.max(t)));
            }
        }
        let se = &se;
        for (key, value, level, card) in &gauges {
            let t = se[*key].as_f64();
            card.set_visible(t.is_some() || ["cpu_temp", "gpu_temp", "sys_temp"].contains(key));
            value.set_label(&fmt(&se[*key], "°"));
            level.set(t.unwrap_or(0.0) / 100.0);
            card.last_child().unwrap().queue_draw();
        }
        {
            let mut h = hist.borrow_mut();
            for (s, k) in h.iter_mut().zip(["cpu_temp", "gpu_temp"]) {
                if let Some(v) = se[k].as_f64() {
                    s.push_back(v);
                    if s.len() > HISTORY {
                        s.pop_front();
                    }
                }
            }
        }
        ch2.queue_draw();

        // CPU usage from /proc/stat
        let cpu: Vec<u64> = read("/proc/stat").lines().next().unwrap_or("").split_whitespace().skip(1).filter_map(|v| v.parse().ok()).collect();
        let (idle, total) = (cpu.get(3).unwrap_or(&0) + cpu.get(4).unwrap_or(&0), cpu.iter().sum::<u64>());
        let (pi, pt) = prev_cpu.replace((idle, total));
        let usage = if total > pt { 100.0 * (1.0 - (idle - pi) as f64 / (total - pt) as f64) } else { 0.0 };
        let mhz = read("/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq").parse::<f64>().unwrap_or(0.0) / 1000.0;
        cpu_l.set_label(&format!("{usage:.0}% load\n{mhz:.0} MHz\n{} package power", fmt(&se["cpu_power"], " W")));

        let g = &se["gpu"];
        gpu_l.set_label(&if g.is_null() {
            "Sleeping (saving power)".into()
        } else {
            format!("{} load\n{} core · {} memory\n{} power", fmt(&g["usage"], "%"), fmt(&g["core_mhz"], " MHz"), fmt(&g["mem_mhz"], " MHz"), fmt(&g["power_w"], " W"))
        });

        let mi = read("/proc/meminfo");
        let kb = |k| field(&mi, k).and_then(|v| v.split(' ').next()?.parse::<f64>().ok()).unwrap_or(0.0) / 1048576.0;
        let (total, avail) = (kb("MemTotal"), kb("MemAvailable"));
        ram_l.set_label(&format!("{:.1} / {total:.0} GB used\n{:.0}%", total - avail, 100.0 * (total - avail) / total.max(1.0)));
    });
    sw.upcast()
}

// ---------------------------------------------------------------- Modes

/// "Screen refresh rate" row for the charger or battery card.
fn refresh_row(ctx: &Rc<Ctx>, key: &'static str) -> adw::ComboRow {
    let row = adw::ComboRow::builder().title("Screen refresh rate").model(&gtk::StringList::new(&["165 Hz", "60 Hz"])).build();
    let ctx = ctx.clone();
    row.connect_selected_notify(move |r| ctx.set_option(key, json!(if r.selected() == 0 { 165 } else { 60 })));
    row
}

fn mode_buttons(ctx: &Rc<Ctx>, modes: &[u8], method: &'static str) -> (gtk::FlowBox, Vec<(u8, gtk::ToggleButton)>) {
    let flow = gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).min_children_per_line(2).max_children_per_line(4).homogeneous(true).column_spacing(12).row_spacing(12).build();
    let mut btns: Vec<(u8, gtk::ToggleButton)> = vec![];
    for (v, name, desc) in MODES.iter().filter(|m| modes.contains(&m.0)) {
        let b = vbox(6);
        b.append(&label(name, &format!("mode-name mode-{}", name.to_lowercase())));
        b.append(&label(desc, "dim"));
        let t = gtk::ToggleButton::builder().child(&b).css_classes(["mode-card"]).build();
        t.set_group(btns.first().map(|b| &b.1));
        let (ctx, v) = (ctx.clone(), *v);
        t.connect_toggled(move |t| if t.is_active() { ctx.set(method, (v,).to_variant()) });
        flow.insert(&t, -1);
        btns.push((v, t));
    }
    (flow, btns)
}

pub fn modes(ctx: &Rc<Ctx>) -> gtk::Widget {
    let (sw, c) = page("Modes");
    let all: Vec<u8> = ctx.info["modes"].as_array().map(|a| a.iter().filter_map(|v| v.as_u64().map(|v| v as u8)).collect()).unwrap_or(vec![0, 1, 4, 5, 6]);

    let now_card = card("Mode now");
    let (now_flow, now_btns) = mode_buttons(ctx, &all, "SetMode");
    now_card.append(&now_flow);
    let source = label("", "dim");
    now_card.append(&source);
    c.append(&now_card);

    // what plugging / unplugging switches to
    let policy = |title: &str, method: &'static str, hz_key: &'static str| {
        let card = card(title);
        let (flow, btns) = mode_buttons(ctx, &all, method);
        card.append(&flow);
        let hz = refresh_row(ctx, hz_key);
        let lb = gtk::ListBox::builder().css_classes(["boxed-list"]).build();
        lb.append(&hz);
        card.append(&lb);
        c.append(&card);
        (btns, hz)
    };
    let (ac_btns, ac_hz) = policy("When I plug in the charger", "SetChargerMode", "refresh_ac");
    let (dc_btns, dc_hz) = policy("When I unplug the charger", "SetBatteryMode", "refresh_dc");
    c.append(&label("These apply only when the charger is plugged in or unplugged, and at startup. Anything you change by hand, here, with the mode key or in Noctalia, stays until then.", "dim"));

    let oc = adw::SwitchRow::builder().title("GPU overclock in Turbo").subtitle("+100 MHz core and +200 MHz memory on the RTX 4060, like PredatorSense on Windows").build();
    let ctx2 = ctx.clone();
    oc.connect_active_notify(move |s| ctx2.set_option("gpu_oc", json!(s.is_active())));
    let group = adw::PreferencesGroup::new();
    group.add(&oc);
    c.append(&group);

    ctx.on_update(move |st, _| {
        let pick = |btns: &[(u8, gtk::ToggleButton)], v: &Value| {
            for (m, b) in btns {
                b.set_active(v.as_u64() == Some(*m as u64));
            }
        };
        pick(&now_btns, &st["mode"]);
        pick(&ac_btns, &st["ac_mode"]);
        pick(&dc_btns, &st["dc_mode"]);
        source.set_label(if st["ac"] == true { "Charger plugged in" } else { "On battery" });
        oc.set_active(st["gpu_oc"] == true);
        ac_hz.set_selected(if st["refresh_ac"] == 60 { 1 } else { 0 });
        dc_hz.set_selected(if st["refresh_dc"] == 165 { 0 } else { 1 });
    });
    sw.upcast()
}

// ---------------------------------------------------------------- Battery & System

fn switch(ctx: &Rc<Ctx>, group: &adw::PreferencesGroup, key: &'static str, title: &str, subtitle: &str) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder().title(title).subtitle(subtitle).build();
    let ctx = ctx.clone();
    row.connect_active_notify(move |r| ctx.set_option(key, json!(r.is_active())));
    group.add(&row);
    row
}

pub fn battery(ctx: &Rc<Ctx>) -> gtk::Widget {
    let (sw, c) = page("Battery & System");
    let bat = hbox(28);
    bat.add_css_class("panel");
    let pct = label("—", "hero-title");
    bat.append(&pct);
    let details = label("", "");
    bat.append(&details);
    c.append(&bat);

    let g = adw::PreferencesGroup::builder().title("Battery").build();
    let limit = switch(ctx, &g, "battery_limit", "Limit charge to 80%", "Keeps the battery healthy when the laptop stays plugged in");
    let calib = switch(ctx, &g, "calibration", "Battery calibration", "Fully discharges and recharges once to re-measure the capacity. Keep the charger connected.");
    c.append(&g);

    let g = adw::PreferencesGroup::builder().title("USB").build();
    let usb = switch(ctx, &g, "usb_charging", "Charge USB devices when the laptop is off", "");
    let thr = adw::ComboRow::builder().title("Stop when the battery reaches").model(&gtk::StringList::new(&["10%", "20%", "30%"])).build();
    let ctx2 = ctx.clone();
    thr.connect_selected_notify(move |r| ctx2.set_option("usb_threshold", json!(10 + 10 * r.selected())));
    g.add(&thr);
    c.append(&g);

    let g = adw::PreferencesGroup::builder().title("System").build();
    let boot = switch(ctx, &g, "boot_sound", "Boot sound", "Play the Predator sound when the laptop turns on");
    let lcd = switch(ctx, &g, "lcd_overdrive", "Panel overdrive", "Faster pixel response, can add slight ghosting");
    lcd.set_visible(ctx.info["lcd_overdrive"] == true);
    c.append(&g);

    ctx.on_update(move |st, se| {
        let b = "/sys/class/power_supply/BAT1";
        let num = |f: &str| read(&format!("{b}/{f}")).parse::<f64>().ok();
        pct.set_label(&format!("{}%", read(&format!("{b}/capacity"))));
        let power = num("power_now").or_else(|| Some(num("current_now")? * num("voltage_now")? / 1e6)).map_or(0.0, |p| p / 1e6);
        let health = num("energy_full").zip(num("energy_full_design")).or(num("charge_full").zip(num("charge_full_design"))).map_or(0.0, |(f, d)| 100.0 * f / d);
        details.set_label(&format!(
            "{} · {power:.1} W\nHealth {health:.0}% · {} cycles\nTemperature {}",
            read(&format!("{b}/status")),
            read(&format!("{b}/cycle_count")),
            fmt(&se["battery_temp"], " °C"),
        ));
        limit.set_active(st["battery_limit"] == true);
        calib.set_active(st["calibration"] == true);
        usb.set_active(st["usb_charging"] == true);
        thr.set_selected((st["usb_threshold"].as_u64().unwrap_or(10) / 10).clamp(1, 3) as u32 - 1);
        boot.set_active(st["boot_sound"] == true);
        lcd.set_active(st["lcd_overdrive"] == true);
    });
    sw.upcast()
}
