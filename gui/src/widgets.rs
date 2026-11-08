//! Small drawing helpers shared by the pages.

use gtk::{cairo, glib, prelude::*};
use std::{cell::{Cell, RefCell}, collections::VecDeque, f64::consts::PI, rc::Rc, time::Duration};

pub const CYAN: (f64, f64, f64) = (0.0, 0.78, 0.88);
pub const ORANGE: (f64, f64, f64) = (1.0, 0.55, 0.15);

pub fn label(text: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::builder().label(text).xalign(0.0).wrap(true).build();
    if !class.is_empty() {
        l.add_css_class(class);
    }
    l
}

pub fn vbox(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Vertical, spacing)
}
pub fn hbox(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Horizontal, spacing)
}

/// A panel with an optional title.
pub fn card(title: &str) -> gtk::Box {
    let b = vbox(10);
    b.add_css_class("panel");
    if !title.is_empty() {
        b.append(&label(title, "panel-title"));
    }
    b
}

/// Page container: big title, scrollable content.
pub fn page(title: &str) -> (gtk::ScrolledWindow, gtk::Box) {
    let content = vbox(18);
    content.add_css_class("page");
    content.append(&label(title, "page-title"));
    let clamp = adw::Clamp::builder().maximum_size(1100).child(&content).build();
    (gtk::ScrolledWindow::builder().child(&clamp).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build(), content)
}

/// Group of toggle buttons that behave like radio buttons. Returns the buttons.
pub fn segmented(items: &[&str], on: impl Fn(usize) + 'static) -> (gtk::Box, Vec<gtk::ToggleButton>) {
    let b = hbox(0);
    b.add_css_class("linked");
    b.add_css_class("segmented");
    let on = Rc::new(on);
    let mut btns: Vec<gtk::ToggleButton> = vec![];
    for (i, t) in items.iter().enumerate() {
        let tb = gtk::ToggleButton::with_label(t);
        tb.set_hexpand(true);
        if let Some(first) = btns.first() {
            tb.set_group(Some(first));
        }
        let on = on.clone();
        tb.connect_toggled(move |t| if t.is_active() { on(i) });
        b.append(&tb);
        btns.push(tb);
    }
    (b, btns)
}

/// Run `f` once the value stops changing for 300 ms (sliders, colour pickers).
pub fn debounced<T: 'static>(f: impl Fn(T) + 'static) -> impl Fn(T) {
    let pending: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    let f = Rc::new(f);
    move |v: T| {
        if let Some(id) = pending.take() {
            id.remove();
        }
        let (f, p, v) = (f.clone(), pending.clone(), Cell::new(Some(v)));
        pending.replace(Some(glib::timeout_add_local_once(Duration::from_millis(300), move || {
            p.take();
            if let Some(v) = v.take() {
                f(v)
            }
        })));
    }
}

fn set(cr: &cairo::Context, c: (f64, f64, f64), a: f64) {
    cr.set_source_rgba(c.0, c.1, c.2, a);
}

/// Segmented circular gauge, `value` in 0..=1.
pub fn gauge(value: Rc<Cell<f64>>, color: (f64, f64, f64)) -> gtk::DrawingArea {
    let a = gtk::DrawingArea::builder().content_width(110).content_height(110).build();
    a.set_draw_func(move |_, cr, w, h| {
        let (cx, cy, r) = (w as f64 / 2.0, h as f64 / 2.0, w.min(h) as f64 / 2.0 - 8.0);
        let n = 48;
        cr.set_line_width(7.0);
        for i in 0..n {
            let a0 = -PI / 2.0 + i as f64 / n as f64 * 2.0 * PI;
            let lit = (i as f64) < value.get().clamp(0.0, 1.0) * n as f64;
            set(cr, if lit { color } else { (1.0, 1.0, 1.0) }, if lit { 1.0 } else { 0.08 });
            cr.arc(cx, cy, r, a0, a0 + 2.0 * PI / n as f64 * 0.6);
            let _ = cr.stroke();
        }
    });
    a
}

/// Line chart of the last N samples per series, fixed 0..max scale.
pub fn chart(series: Rc<RefCell<Vec<VecDeque<f64>>>>, colors: Vec<(f64, f64, f64)>, max: f64, len: usize) -> gtk::DrawingArea {
    let a = gtk::DrawingArea::builder().content_height(180).hexpand(true).build();
    a.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (w as f64, h as f64);
        cr.set_line_width(1.0);
        set(cr, (1.0, 1.0, 1.0), 0.06);
        for i in 0..=4 {
            let y = (h - 1.0) * i as f64 / 4.0 + 0.5;
            cr.move_to(0.0, y);
            cr.line_to(w, y);
        }
        let _ = cr.stroke();
        cr.set_line_width(2.0);
        for (s, c) in series.borrow().iter().zip(&colors) {
            for (i, v) in s.iter().enumerate() {
                let x = w * (len - s.len() + i) as f64 / (len - 1) as f64;
                let y = h - (v / max).clamp(0.0, 1.0) * (h - 4.0) - 2.0;
                if i == 0 { cr.move_to(x, y) } else { cr.line_to(x, y) }
            }
            set(cr, *c, 1.0);
            let _ = cr.stroke_preserve();
            cr.line_to(w, h);
            cr.line_to(w * (len - s.len()) as f64 / (len - 1) as f64, h);
            set(cr, *c, 0.08);
            let _ = cr.fill();
        }
    });
    a
}

/// Fan drawing that spins at a speed proportional to `rpm`.
pub fn fan(rpm: Rc<Cell<f64>>) -> gtk::DrawingArea {
    let a = gtk::DrawingArea::builder().content_width(150).content_height(150).build();
    let angle = Rc::new(Cell::new(0.0));
    let last = Cell::new(0i64);
    let ang = angle.clone();
    a.add_tick_callback(move |a, clock| {
        let t = clock.frame_time();
        let dt = if last.get() == 0 { 0.0 } else { (t - last.get()) as f64 / 1e6 };
        last.set(t);
        // drawn at 1/20 of the real speed so the blades stay visible
        ang.set((ang.get() + rpm.get() / 60.0 / 20.0 * 2.0 * PI * dt) % (2.0 * PI));
        a.queue_draw();
        glib::ControlFlow::Continue
    });
    a.set_draw_func(move |_, cr, w, h| {
        let (cx, cy, r) = (w as f64 / 2.0, h as f64 / 2.0, w.min(h) as f64 / 2.0 - 4.0);
        cr.translate(cx, cy);
        cr.rotate(angle.get());
        for i in 0..11 {
            cr.save().ok();
            cr.rotate(i as f64 * 2.0 * PI / 11.0);
            cr.move_to(r * 0.22, 0.0);
            cr.curve_to(r * 0.5, -r * 0.05, r * 0.8, r * 0.1, r, r * 0.38);
            cr.curve_to(r * 0.7, r * 0.3, r * 0.45, r * 0.2, r * 0.22, r * 0.12);
            cr.close_path();
            set(cr, CYAN, 0.25 + 0.05 * (i % 2) as f64);
            let _ = cr.fill_preserve();
            set(cr, CYAN, 0.6);
            cr.set_line_width(1.0);
            let _ = cr.stroke();
            cr.restore().ok();
        }
        cr.arc(0.0, 0.0, r * 0.22, 0.0, 2.0 * PI);
        set(cr, (0.04, 0.09, 0.11), 1.0);
        let _ = cr.fill_preserve();
        set(cr, CYAN, 0.5);
        let _ = cr.stroke();
    });
    a
}

pub fn hex_to_rgba(hex: &str) -> gtk::gdk::RGBA {
    gtk::gdk::RGBA::parse(hex).unwrap_or(gtk::gdk::RGBA::BLACK)
}
pub fn rgba_to_hex(c: &gtk::gdk::RGBA) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c.red()), b(c.green()), b(c.blue()))
}
