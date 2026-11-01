mod cli;
mod config;
mod cpuload;
mod devices;
mod dl;
mod gpuload;
mod helpers;
mod history;
mod i18n;
mod loadtest;
mod look;
mod mcp;
mod nvml;
mod sensors;
mod tray;
mod ui;
mod uninstall;
mod vifte;

use adw::prelude::*;
use gtk::{gio, glib};

pub const APP_ID: &str = "io.github.stresstest.Stress";

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("mcp") {
        return mcp::run();
    }
    if let Some(code) = cli::run_command(&args[1..]) {
        return code;
    }
    let config = config::Config::load();
    if let Some(lang) = &config.language {
        // Also covers strings from GTK and libadwaita themselves, which read
        // LANGUAGE through gettext. Set before any other thread exists.
        std::env::set_var("LANGUAGE", lang);
        i18n::set_language(Some(lang));
    }

    let mut flags = gio::ApplicationFlags::empty();
    // Screenshots run as a separate instance, next to a Stress already open.
    if std::env::var_os("STRESS_SCREENSHOT").is_some() {
        flags |= gio::ApplicationFlags::NON_UNIQUE;
    }
    let app = adw::Application::builder().application_id(APP_ID).flags(flags).build();

    app.connect_startup(move |app| {
        gtk::Window::set_default_icon_name(APP_ID);
        ui::load_css();
        ui::apply_appearance(config.appearance);
        ui::start_tray(app);
        ui::register_claude(app);
        ui::start_update_checks(app);

        let quit = gio::ActionEntry::builder("quit").activate(|app: &adw::Application, _, _| app.quit()).build();
        app.add_action_entries([quit]);
        app.set_accels_for_action("app.quit", &["<Control>q"]);
        app.set_accels_for_action("window.close", &["<Control>w"]);
        app.set_accels_for_action("win.preferences", &["<Control>comma"]);
    });

    app.connect_activate(ui::activate);

    if app.run_with_args(&args) == glib::ExitCode::SUCCESS {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
