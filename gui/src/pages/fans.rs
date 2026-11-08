use crate::{ctx::Ctx, widgets::*};
use adw::prelude::*;
use serde_json::{Value, json};
use std::{cell::{Cell, RefCell}, rc::Rc};

const MODES: [&str; 4] = ["auto", "curve", "custom", "max"];
const T_MIN: f64 = 30.0;
const T_MAX: f64 = 100.0;
const PRESETS: [(&str, [u8; 6]); 3] = [("Silent", [15, 25, 35, 50, 75, 100]), ("Balanced", [25, 40, 50, 60, 90, 100]), ("Aggressive", [40, 55, 70, 85, 100, 100])];

type Curve = Rc<RefCell<Vec<(f64, f64)>>>;

/// Curve editor: points at fixed temperatures, drag them up and down.
fn curve_editor(curve: Curve, now: Rc<Cell<Option<f64>>>, on_change: impl Fn() + 'static) -> gtk::DrawingArea {
    let a = gtk::DrawingArea::builder().content_height(260).hexpand(true).build();
    let (c2, now2) = (curve.clone(), now.clone());
    a.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (w as f64, h as f64);
        let (l, b) = (40.0, 24.0);
        let x = |t: f64| l + (t - T_MIN) / (T_MAX - T_MIN) * (w - l - 10.0);
        let y = |p: f64| (h - b) - p / 100.0 * (h - b - 10.0);
        cr.set_font_size(11.0);
        cr.set_line_width(1.0);
        for p in [0.0, 25.0, 50.0, 75.0, 100.0] {
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.07);
            cr.move_to(l, y(p));
            cr.line_to(w - 10.0, y(p));
            let _ = cr.stroke();
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.45);
            cr.move_to(4.0, y(p) + 4.0);
            let _ = cr.show_text(&format!("{p:.0}%"));
        }
        for t in (30..=100).step_by(10) {
            cr.move_to(x(t as f64) - 10.0, h - 6.0);
            let _ = cr.show_text(&format!("{t}°C"));
        }
        let pts = c2.borrow();
        // filled area under the curve
        cr.move_to(x(T_MIN), y(pts[0].1));
        for (t, p) in pts.iter() {
            cr.line_to(x(*t), y(*p));
        }
        cr.line_to(x(T_MAX), y(pts.last().unwrap().1));
        let (r, g, bl) = CYAN;
        cr.set_source_rgba(r, g, bl, 1.0);
        cr.set_line_width(2.5);
        let _ = cr.stroke_preserve();
        cr.line_to(x(T_MAX), y(0.0));
        cr.line_to(x(T_MIN), y(0.0));
        cr.set_source_rgba(r, g, bl, 0.12);
        let _ = cr.fill();
        for (t, p) in pts.iter() {
            cr.arc(x(*t), y(*p), 6.0, 0.0, 6.3);
            cr.set_source_rgba(r, g, bl, 1.0);
            let _ = cr.fill();
            cr.move_to(x(*t) - 12.0, y(*p) - 12.0);
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.8);
            let _ = cr.show_text(&format!("{p:.0}%"));
        }
        if let Some(t) = now2.get() {
            cr.set_source_rgba(1.0, 0.8, 0.2, 0.9);
            cr.set_line_width(1.5);
            cr.move_to(x(t), y(0.0));
            cr.line_to(x(t), y(100.0));
            let _ = cr.stroke();
            cr.move_to(x(t) + 4.0, y(100.0) + 12.0);
            let _ = cr.show_text(&format!("{t:.0}°C now"));
        }
    });

    let drag = gtk::GestureDrag::new();
    let picked = Rc::new(Cell::new(None::<usize>));
    let start = Rc::new(Cell::new((0.0, 0.0)));
    let to_pct = |a: &gtk::DrawingArea, py: f64| ((a.height() as f64 - 24.0 - py) / (a.height() as f64 - 34.0) * 100.0).clamp(0.0, 100.0).round();
    let (cv, pk, st) = (curve.clone(), picked.clone(), start.clone());
    drag.connect_drag_begin(move |g, sx, sy| {
        let a = g.widget().unwrap();
        let w = a.width() as f64;
        let x = |t: f64| 40.0 + (t - T_MIN) / (T_MAX - T_MIN) * (w - 50.0);
        let i = cv.borrow().iter().enumerate().min_by_key(|(_, (t, _))| (x(*t) - sx).abs() as i64).map(|(i, _)| i);
        pk.set(i);
        st.set((sx, sy));
    });
    let (cv, pk, st) = (curve.clone(), picked.clone(), start.clone());
    drag.connect_drag_update(move |g, _, dy| {
        let (Some(i), Ok(a)) = (pk.get(), g.widget().unwrap().downcast::<gtk::DrawingArea>()) else { return };
        let p = to_pct(&a, st.get().1 + dy);
        let mut c = cv.borrow_mut();
        c[i].1 = p;
        // keep the curve rising
        for j in 0..c.len() {
            if j < i { c[j].1 = c[j].1.min(p) } else if j > i { c[j].1 = c[j].1.max(p) }
        }
        drop(c);
        a.queue_draw();
    });
    drag.connect_drag_end(move |_, _, _| on_change());
    a.add_controller(drag);
    a
}

fn to_curve(v: &Value) -> Vec<(f64, f64)> {
    v.as_array().map(|a| a.iter().filter_map(|p| Some((p[0].as_f64()?, p[1].as_f64()?))).collect()).filter(|c: &Vec<_>| c.len() >= 2).unwrap_or_else(|| PRESETS[1].1.iter().enumerate().map(|(i, p)| (45.0 + 10.0 * i as f64, *p as f64)).collect())
}

pub fn fans(ctx: &Rc<Ctx>) -> gtk::Widget {
    let (sw, c) = page("Fan Control");
    let for_mode = label("", "dim");
    c.append(&for_mode);
    let quiet_note = label("In Quiet mode the fans stay on Automatic.", "warning");
    c.append(&quiet_note);

    // live fan cards
    let row = gtk::Grid::builder().column_spacing(14).column_homogeneous(true).build();
    let mut live = vec![];
    for (i, name) in ["CPU", "GPU"].into_iter().enumerate() {
        let card = hbox(10);
        card.add_css_class("panel");
        let v = vbox(4);
        v.set_hexpand(true);
        v.set_valign(gtk::Align::Center);
        v.append(&label(name, "panel-title"));
        let rpm_l = label("—", "big-number");
        let temp_l = label("", "dim");
        v.append(&rpm_l);
        v.append(&temp_l);
        card.append(&v);
        let rpm = Rc::new(Cell::new(0.0));
        card.append(&fan(rpm.clone()));
        row.attach(&card, i as i32, 0, 1, 1);
        live.push((rpm, rpm_l, temp_l));
    }
    c.append(&row);

    // what we last got from the daemon; edits are sent as a full Fan object
    let fan_cfg: Rc<RefCell<Value>> = Rc::new(RefCell::new(json!({})));
    let send = {
        let (ctx, cfg) = (ctx.clone(), fan_cfg.clone());
        Rc::new(move |patch: Value| {
            let mut f = cfg.borrow().clone();
            for (k, v) in patch.as_object().unwrap() {
                f[k] = v.clone();
            }
            if f == *cfg.borrow() {
                return; // nothing changed (e.g. widgets refreshed from the daemon)
            }
            ctx.set_json("SetFan", &f);
        })
    };

    let ctrl = card("");
    let stack = gtk::Stack::builder().vhomogeneous(false).build();
    let descr = label("", "dim");
    let s = send.clone();
    let (seg, seg_btns) = segmented(&["Automatic", "Curve", "Fixed", "Max"], move |i| s(json!({ "mode": MODES[i] })));
    let top = hbox(16);
    top.append(&seg);
    top.append(&descr);
    ctrl.append(&top);
    ctrl.append(&stack);
    c.append(&ctrl);

    stack.add_named(&label("The firmware controls the fans, like on Windows.", "dim"), Some("auto"));
    stack.add_named(&label("Both fans at full speed.", "dim"), Some("max"));

    // fixed speeds
    let fixed = vbox(8);
    let mut sliders = vec![];
    for (name, key) in [("CPU fan", "cpu"), ("GPU fan", "gpu")] {
        let r = hbox(12);
        r.append(&label(name, ""));
        let sc = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 5.0);
        sc.set_hexpand(true);
        sc.set_draw_value(true);
        sc.set_format_value_func(|_, v| format!("{v:.0}%"));
        let s = send.clone();
        let deb = debounced(move |v: f64| s(json!({ key: v.round() as u8 })));
        sc.connect_value_changed(move |sc| deb(sc.value()));
        r.append(&sc);
        fixed.append(&r);
        sliders.push((key, sc));
    }
    stack.add_named(&fixed, Some("custom"));

    // curves
    let curves = vbox(10);
    let which = Rc::new(Cell::new(0usize)); // 0 CPU, 1 GPU
    let curve: Curve = Rc::new(RefCell::new(vec![]));
    let now = Rc::new(Cell::new(None));
    let key = |w: usize| if w == 0 { "cpu_curve" } else { "gpu_curve" };
    let apply = {
        let (s, cv, wh) = (send.clone(), curve.clone(), which.clone());
        Rc::new(move || {
            let pts: Vec<(u8, u8)> = cv.borrow().iter().map(|(t, p)| (*t as u8, *p as u8)).collect();
            s(json!({ key(wh.get()): pts }))
        })
    };
    let ap = apply.clone();
    let editor = curve_editor(curve.clone(), now.clone(), move || ap());
    let (cv, wh, cfg, ed) = (curve.clone(), which.clone(), fan_cfg.clone(), editor.clone());
    let (pick, _) = segmented(&["CPU curve", "GPU curve"], move |i| {
        wh.set(i);
        cv.replace(to_curve(&cfg.borrow()[key(i)]));
        ed.queue_draw();
    });
    let bar = hbox(8);
    bar.append(&pick);
    let spacer = label("Presets", "dim");
    spacer.set_hexpand(true);
    spacer.set_xalign(1.0);
    bar.append(&spacer);
    for (name, p) in PRESETS {
        let b = gtk::Button::with_label(name);
        let (cv, ap, ed) = (curve.clone(), apply.clone(), editor.clone());
        b.connect_clicked(move |_| {
            for (pt, v) in cv.borrow_mut().iter_mut().zip(p) {
                pt.1 = v as f64;
            }
            ed.queue_draw();
            ap();
        });
        bar.append(&b);
    }
    curves.append(&bar);
    curves.append(&editor);
    curves.append(&label("Drag the points. Speeds rise at once and drop only after the temperature falls 3 °C, so the fans don't pulse.", "dim"));
    stack.add_named(&curves, Some("curve"));

    let ctx2 = ctx.clone();
    ctx.on_update(move |st, se| {
        for ((rpm, rpm_l, temp_l), (fan, temp)) in live.iter().zip([("cpu_fan", "cpu_temp"), ("gpu_fan", "gpu_temp")]) {
            rpm.set(se[fan].as_f64().unwrap_or(0.0));
            rpm_l.set_label(&format!("{} RPM", se[fan].as_u64().map_or("—".into(), |v| v.to_string())));
            temp_l.set_label(&format!("{}°C", se[temp].as_u64().map_or("—".into(), |v| v.to_string())));
        }
        let f = st["fan"].clone();
        let mode = f["mode"].as_str().unwrap_or("auto").to_owned();
        if *fan_cfg.borrow() != f {
            let first = fan_cfg.borrow().get("mode").is_none();
            fan_cfg.replace(f.clone());
            if first || curve.borrow().is_empty() {
                curve.replace(to_curve(&f[key(which.get())]));
            }
            for (k, sc) in &sliders {
                sc.set_value(f[*k].as_f64().unwrap_or(50.0));
            }
        }
        if let Some(i) = MODES.iter().position(|m| *m == mode) {
            seg_btns[i].set_active(true);
        }
        stack.set_visible_child_name(&mode);
        descr.set_label(match mode.as_str() {
            "curve" => "Your own speed for each temperature.",
            "custom" => "Constant speed you choose.",
            "max" => "Full speed (CoolBoost).",
            _ => "Managed by the firmware.",
        });
        for_mode.set_label(&format!("Saved for {} mode. Each mode keeps its own settings.", super::mode_name(&st["mode"])));
        let quiet = st["mode"] == 0;
        quiet_note.set_visible(quiet);
        ctrl.set_sensitive(!quiet && ctx2.online());
        now.set(se[if which.get() == 0 { "cpu_temp" } else { "gpu_temp" }].as_f64());
        editor.queue_draw();
    });
    sw.upcast()
}
