//! Predator Sense GUI for the Acer Predator PHN16-72. Talks to predator-sensed over D-Bus.

mod ctx;
mod pages;
mod widgets;

use adw::prelude::*;
use gtk::glib;
use std::{cell::Cell, time::Duration};

const CSS: &str = include_str!("style.css");

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id("org.predatorsense.Gui").build();
    app.connect_startup(|_| {
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
        let css = gtk::CssProvider::new();
        css.load_from_string(CSS);
        let display = gtk::gdk::Display::default().unwrap();
        gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        // our icons when running from the source tree (installed ones live in hicolor)
        gtk::IconTheme::for_display(&display).add_search_path(concat!(env!("CARGO_MANIFEST_DIR"), "/../data/icons"));
    });
    app.connect_activate(build);
    app.run_with_args(&std::env::args().take(1).collect::<Vec<_>>()) // our own --page flag
}

fn build(app: &adw::Application) {
    if let Some(w) = app.active_window() {
        return w.present();
    }
    let ctx = ctx::Ctx::new();
    let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).hexpand(true).build();

    let sidebar = widgets::vbox(4);
    sidebar.add_css_class("sidebar");
    let brand = widgets::hbox(10);
    brand.add_css_class("brand");
    brand.append(&gtk::Image::builder().icon_name("predator-sense").pixel_size(32).build());
    brand.append(&widgets::label("PREDATOR", "brand-text"));
    sidebar.append(&brand);

    let pages: [(&str, &str, gtk::Widget); 6] = [
        ("Dashboard", "computer-symbolic", pages::home(&ctx)),
        ("Monitor", "predator-monitor-symbolic", pages::monitor(&ctx)),
        ("Modes", "power-profile-performance-symbolic", pages::modes(&ctx)),
        ("Fan Control", "weather-windy-symbolic", pages::fans(&ctx)),
        ("Lighting", "keyboard-brightness-symbolic", pages::lighting(&ctx)),
        ("Battery & System", "battery-symbolic", pages::battery(&ctx)),
    ];
    // `predator-sense --page fans` opens on that page
    let want = std::env::args().skip_while(|a| a != "--page").nth(1).unwrap_or_default().to_lowercase();
    let mut first: Option<gtk::ToggleButton> = None;
    let mut start: Option<gtk::ToggleButton> = None;
    for (name, icon, w) in pages {
        stack.add_named(&w, Some(name));
        let row = widgets::hbox(12);
        row.append(&gtk::Image::from_icon_name(icon));
        row.append(&gtk::Label::new(Some(name)));
        let b = gtk::ToggleButton::builder().child(&row).css_classes(["nav"]).build();
        b.set_group(first.as_ref());
        let s = stack.clone();
        b.connect_toggled(move |b| if b.is_active() { s.set_visible_child_name(name) });
        sidebar.append(&b);
        if !want.is_empty() && name.to_lowercase().starts_with(&want) {
            start = Some(b.clone());
        }
        first.get_or_insert(b);
    }
    start.or(first).unwrap().set_active(true);

    let footer = widgets::vbox(2);
    footer.set_vexpand(true);
    footer.set_valign(gtk::Align::End);
    footer.add_css_class("footer");
    let product = ctx.info["product"].as_str().unwrap_or("Predator PHN16-72").to_owned();
    footer.append(&widgets::label(&product, "dim"));
    let status = if !ctx.online() {
        widgets::label("● Service not running", "error")
    } else if ctx.info["simulated"] == true {
        widgets::label("● Simulated hardware", "warning")
    } else {
        widgets::label("● Service active", "success")
    };
    footer.append(&status);
    sidebar.append(&footer);

    let body = widgets::hbox(0);
    body.append(&sidebar);
    body.append(&stack);
    ctx.toast.set_child(Some(&body));
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::builder().title_widget(&widgets::label("PredatorSense", "app-title")).build());
    view.set_content(Some(&ctx.toast));

    let win = adw::ApplicationWindow::builder().application(app).title("Predator Sense").default_width(1280).default_height(820).content(&view).build();
    ctx.tick(0);
    let n = Cell::new(1u32);
    glib::timeout_add_local(Duration::from_secs(2), glib::clone!(#[weak] win, #[upgrade_or] glib::ControlFlow::Break, move || {
        if win.is_visible() {
            ctx.tick(n.get());
            n.set(n.get() + 1);
        }
        glib::ControlFlow::Continue
    }));
    win.present();
}
