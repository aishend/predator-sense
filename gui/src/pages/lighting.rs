use crate::{ctx::Ctx, widgets::*};
use adw::prelude::*;
use gtk::glib;
use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc};

const EFFECTS: [(&str, &str); 8] = [
    ("static", "Static"), ("breathing", "Breathing"), ("neon", "Neon"), ("wave", "Wave"),
    ("shifting", "Shifting"), ("zoom", "Zoom"), ("meteor", "Meteor"), ("twinkling", "Twinkling"),
];
/// Effects the firmware draws in rainbow colours (no colour choice).
const RAINBOW: [&str; 2] = ["neon", "wave"];
const SWATCHES: [&str; 10] = ["#ff0000", "#ff6a00", "#ffd000", "#3ddc5a", "#00aec7", "#1e7bff", "#8a2be2", "#ff2d95", "#ffffff", "#000000"];

/// Rows of key widths (in key units) of a 16" keyboard with numpad.
const ROWS: [&[f64]; 6] = [
    &[1.0; 19],
    &[1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 2.0, 1.0, 1.0, 1.0, 1.0],
    &[1.5, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.5, 1.0, 1.0, 1.0, 1.0],
    &[1.8, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 2.2, 1.0, 1.0, 1.0, 1.0],
    &[2.3, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.7, 1.0, 1.0, 1.0, 1.0, 1.0],
    &[1.0, 1.0, 1.0, 1.0, 5.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 2.0],
];

fn rgb(hex: &str) -> (f64, f64, f64) {
    let c = hex_to_rgba(hex);
    (c.red() as f64, c.green() as f64, c.blue() as f64)
}

fn hue(h: f64) -> (f64, f64, f64) {
    let f = |n: f64| {
        let k = (n + h * 6.0) % 6.0;
        1.0 - (k.min(4.0 - k).clamp(0.0, 1.0))
    };
    (f(5.0), f(3.0), f(1.0))
}

/// Colour of a key at horizontal position x (0..1), time t, roughly what the firmware shows.
fn key_color(l: &Value, x: f64, row: usize, t: f64) -> ((f64, f64, f64), f64) {
    let effect = l["effect"].as_str().unwrap_or("static");
    let speed = l["speed"].as_f64().unwrap_or(5.0) / 3.0;
    let dir = if l["direction"] == 2 { -1.0 } else { 1.0 };
    let base = rgb(l["color"].as_str().unwrap_or("#00aec7"));
    let zone = ((x * 4.0) as usize).min(3);
    match effect {
        "static" => (rgb(l["zones"][zone].as_str().unwrap_or("#000000")), 1.0),
        "breathing" => (base, 0.15 + 0.85 * (0.5 + 0.5 * (t * speed).sin())),
        "neon" => (hue((t * speed * 0.1) % 1.0), 1.0),
        "wave" => (hue((x * dir + t * speed * 0.15).rem_euclid(1.0)), 1.0),
        "shifting" => (base, if ((x * 4.0 + t * speed * dir).rem_euclid(4.0)) < 1.0 { 1.0 } else { 0.15 }),
        "zoom" => (base, 0.2 + 0.8 * (0.5 + 0.5 * ((x - 0.5).abs() * 8.0 - t * speed * 2.0).sin())),
        "meteor" => (base, (1.0 - ((x * dir - t * speed * 0.3).rem_euclid(1.0)) * 3.0).max(0.12)),
        _ => (base, if ((x * 97.0 + row as f64 * 31.0 + (t * speed).floor() * 13.0).sin() * 43758.5).fract().abs() > 0.8 { 1.0 } else { 0.12 }),
    }
}

fn keyboard(light: Rc<RefCell<Value>>, rear: Rc<RefCell<Value>>) -> gtk::DrawingArea {
    let a = gtk::DrawingArea::builder().content_height(270).hexpand(true).build();
    let start = std::time::Instant::now();
    a.add_tick_callback(|a, _| {
        a.queue_draw();
        glib::ControlFlow::Continue
    });
    a.set_draw_func(move |_, cr, w, h| {
        let l = light.borrow();
        let t = start.elapsed().as_secs_f64();
        let bright = l["brightness"].as_f64().unwrap_or(100.0) / 100.0;
        let units = 19.0;
        let gap = 4.0;
        let key = ((w as f64 - 20.0) / units).min((h as f64 - 50.0) / 6.0);
        let x0 = (w as f64 - key * units) / 2.0;
        for (r, row) in ROWS.iter().enumerate() {
            let mut x = x0;
            let y = 10.0 + r as f64 * key;
            for kw in row.iter() {
                let width = kw * key;
                let (c, a) = key_color(&l, (x - x0 + width / 2.0) / (key * units), r, t);
                let a = a * bright;
                // glow + key cap
                cr.rectangle(x + gap / 2.0, y + gap / 2.0, width - gap, key - gap);
                cr.set_source_rgba(0.07, 0.08, 0.09, 1.0);
                let _ = cr.fill_preserve();
                cr.set_source_rgba(c.0, c.1, c.2, 0.18 * a);
                let _ = cr.fill_preserve();
                cr.set_source_rgba(c.0, c.1, c.2, 0.9 * a);
                cr.set_line_width(1.5);
                let _ = cr.stroke();
                x += width;
            }
        }
        // zone separators
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.12);
        cr.set_dash(&[3.0, 4.0], 0.0);
        for z in 1..4 {
            let x = x0 + key * units * z as f64 / 4.0;
            cr.move_to(x, 4.0);
            cr.line_to(x, 10.0 + key * 6.0 + 4.0);
        }
        let _ = cr.stroke();
        cr.set_dash(&[], 0.0);

        // rear light, drawn as a glowing bar under the keyboard
        let mut r = rear.borrow().clone(); // one colour, no effects
        r["effect"] = json!("static");
        r["zones"] = json!([r["color"], r["color"], r["color"], r["color"]]);
        let rb = r["brightness"].as_f64().unwrap_or(100.0) / 100.0;
        let (bw, by) = (key * units * 0.7, 10.0 + key * 6.0 + 22.0);
        let n = 60;
        for i in 0..n {
            let fx = (i as f64 + 0.5) / n as f64;
            let (c, a) = key_color(&r, fx, 0, t);
            cr.set_source_rgba(c.0, c.1, c.2, a * rb);
            cr.rectangle(x0 + (key * units - bw) / 2.0 + fx * bw - bw / n as f64 / 2.0, by, bw / n as f64 + 0.5, 6.0);
            let _ = cr.fill();
        }
    });
    a
}

/// Sends a patched copy of `state` to `method`; skips no-op updates.
fn sender(ctx: &Rc<Ctx>, state: &Rc<RefCell<Value>>, method: &'static str) -> Rc<dyn Fn(Value)> {
    let (ctx, state) = (ctx.clone(), state.clone());
    Rc::new(move |patch: Value| {
        let mut l = state.borrow().clone();
        for (k, v) in patch.as_object().unwrap() {
            l[k] = v.clone();
        }
        if l == *state.borrow() {
            return; // nothing changed (e.g. widgets refreshed from the daemon)
        }
        state.replace(l.clone()); // preview follows at once
        ctx.set_json(method, &l);
    })
}

pub fn lighting(ctx: &Rc<Ctx>) -> gtk::Widget {
    let (sw, c) = page("Lighting");
    let light: Rc<RefCell<Value>> = Rc::new(RefCell::new(json!({})));
    let rear: Rc<RefCell<Value>> = Rc::new(RefCell::new(json!({})));
    let send = sender(ctx, &light, "SetLight");

    let for_mode = label("", "dim");
    c.append(&for_mode);
    let preview = card("");
    preview.add_css_class("keyboard");
    preview.append(&keyboard(light.clone(), rear.clone()));
    c.append(&preview);

    let body = hbox(14);
    body.set_homogeneous(true);
    c.append(&body);

    // effects
    let fx = card("Effect");
    let grid = gtk::Grid::builder().column_spacing(8).row_spacing(8).column_homogeneous(true).build();
    let mut fx_btns: Vec<gtk::ToggleButton> = vec![];
    for (i, (id, name)) in EFFECTS.iter().enumerate() {
        let b = gtk::ToggleButton::builder().label(*name).css_classes(["effect"]).build();
        b.set_group(fx_btns.first());
        let s = send.clone();
        b.connect_toggled(move |b| if b.is_active() { s(json!({ "effect": id })) });
        grid.attach(&b, (i % 4) as i32, (i / 4) as i32, 1, 1);
        fx_btns.push(b);
    }
    fx.append(&grid);

    let mk_scale = |min: f64, max: f64, key: &'static str| {
        let sc = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, 1.0);
        sc.set_hexpand(true);
        let s = send.clone();
        let deb = debounced(move |v: f64| s(json!({ key: v.round() as u8 })));
        sc.connect_value_changed(move |sc| deb(sc.value()));
        sc
    };
    let opt_row = |title: &str, w: &gtk::Widget| {
        let r = hbox(12);
        let l = label(title, "");
        l.set_width_chars(10);
        r.append(&l);
        r.append(w);
        r
    };
    let brightness = mk_scale(0.0, 100.0, "brightness");
    fx.append(&opt_row("Brightness", brightness.upcast_ref()));
    let speed = mk_scale(1.0, 9.0, "speed");
    let speed_row = opt_row("Speed", speed.upcast_ref());
    fx.append(&speed_row);
    let s = send.clone();
    let (dir, dir_btns) = segmented(&["Right to left", "Left to right"], move |i| s(json!({ "direction": i + 1 })));
    let dir_row = opt_row("Direction", dir.upcast_ref());
    fx.append(&dir_row);
    let timeout = adw::SwitchRow::builder().title("Turn off after 30 s without typing").build();
    let ctx2 = ctx.clone();
    timeout.connect_active_notify(move |r| ctx2.set_option("kb_timeout", json!(r.is_active())));
    let lb = gtk::ListBox::builder().css_classes(["boxed-list"]).build();
    lb.append(&timeout);
    fx.append(&lb);
    body.append(&fx);

    // colours
    let col = card("Colour");
    let zones_box = hbox(8);
    let mut zone_btns = vec![];
    for z in 0..4 {
        let v = vbox(4);
        v.append(&label(&format!("Zone {}", z + 1), "dim"));
        let b = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::builder().with_alpha(false).build()));
        let (s, lt) = (send.clone(), light.clone());
        let deb = debounced(move |hex: String| {
            let mut zones = lt.borrow()["zones"].clone();
            zones[z] = json!(hex);
            s(json!({ "zones": zones }))
        });
        b.connect_rgba_notify(move |b| deb(rgba_to_hex(&b.rgba())));
        v.append(&b);
        zones_box.append(&v);
        zone_btns.push(b);
    }
    let zones_row = vbox(6);
    zones_row.append(&zones_box);
    col.append(&zones_row);

    let effect_color = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::builder().with_alpha(false).build()));
    let s = send.clone();
    let deb = debounced(move |hex: String| s(json!({ "color": hex })));
    effect_color.connect_rgba_notify(move |b| deb(rgba_to_hex(&b.rgba())));
    let effect_color_row = opt_row("Colour", effect_color.upcast_ref());
    col.append(&effect_color_row);

    col.append(&label("Quick colours (all zones)", "dim"));
    let sw_box = gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).max_children_per_line(10).build();
    for hex in SWATCHES {
        let b = gtk::Button::builder().css_classes(["swatch"]).build();
        let css = gtk::CssProvider::new();
        css.load_from_string(&format!("button {{ background: {hex}; }}"));
        #[allow(deprecated)]
        b.style_context().add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        let s = send.clone();
        b.connect_clicked(move |_| s(json!({ "color": hex, "zones": [hex, hex, hex, hex] })));
        sw_box.insert(&b, -1);
    }
    col.append(&sw_box);
    body.append(&col);

    let rainbow_note = label("This effect uses the rainbow colours of the firmware.", "dim");
    col.append(&rainbow_note);

    // rear light
    let rear_card = card("Rear logo");
    let rs = sender(ctx, &rear, "SetRear");
    let rear_color = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::builder().with_alpha(false).build()));
    let r = rs.clone();
    let deb = debounced(move |hex: String| r(json!({ "color": hex })));
    rear_color.connect_rgba_notify(move |b| deb(rgba_to_hex(&b.rgba())));
    rear_card.append(&opt_row("Colour", rear_color.upcast_ref()));
    let rear_scale = |min: f64, max: f64, key: &'static str| {
        let sc = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, 1.0);
        sc.set_hexpand(true);
        let r = rs.clone();
        let deb = debounced(move |v: f64| r(json!({ key: v.round() as u8 })));
        sc.connect_value_changed(move |sc| deb(sc.value()));
        sc
    };
    let rear_bright = rear_scale(0.0, 100.0, "brightness");
    rear_card.append(&opt_row("Brightness", rear_bright.upcast_ref()));
    c.append(&rear_card);

    ctx.on_update(move |st, _| {
        for_mode.set_label(&format!("Saved for {} mode. Each mode keeps its own settings.", super::mode_name(&st["mode"])));
        let l = st["light"].clone();
        if l.is_null() {
            return;
        }
        light.replace(l.clone());
        let effect = l["effect"].as_str().unwrap_or("static");
        if let Some(i) = EFFECTS.iter().position(|e| e.0 == effect) {
            fx_btns[i].set_active(true);
        }
        brightness.set_value(l["brightness"].as_f64().unwrap_or(100.0));
        speed.set_value(l["speed"].as_f64().unwrap_or(5.0));
        dir_btns[(l["direction"].as_u64().unwrap_or(1) as usize).clamp(1, 2) - 1].set_active(true);
        for (z, b) in zone_btns.iter().enumerate() {
            b.set_rgba(&hex_to_rgba(l["zones"][z].as_str().unwrap_or("#000000")));
        }
        effect_color.set_rgba(&hex_to_rgba(l["color"].as_str().unwrap_or("#00aec7")));
        let is_static = effect == "static";
        let rainbow = RAINBOW.contains(&effect);
        zones_row.set_visible(is_static);
        effect_color_row.set_visible(!is_static && !rainbow);
        rainbow_note.set_visible(rainbow);
        speed_row.set_visible(!is_static);
        dir_row.set_visible(matches!(effect, "wave" | "shifting" | "meteor"));
        timeout.set_active(st["kb_timeout"] == true);

        let r = st["rear"].clone();
        rear.replace(r.clone());
        rear_color.set_rgba(&hex_to_rgba(r["color"].as_str().unwrap_or("#00aec7")));
        rear_bright.set_value(r["brightness"].as_f64().unwrap_or(100.0));
    });
    sw.upcast()
}
