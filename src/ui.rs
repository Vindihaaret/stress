//! Main window: an overview, a section per device (processor, each graphics
//! card, the power supply) with reading cards that graph the last 30 minutes,
//! and the load test panel at the side; plus preferences, about and
//! uninstalling.

pub mod claude;
mod graph;
mod helpers;
mod overview;
mod test;
mod update;

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::config::{self, Appearance, Config};
use crate::devices::{self, Gpu};
use crate::history::History;
use crate::i18n::{self, n_, tr, trf};
use crate::loadtest::{Controller, StopReason};
use crate::sensors::{format, PsuReading, Sample, Sensors};
use crate::vifte::{self, VifteState};
use crate::{look, tray, APP_ID};
use graph::{tile, Card, Scale};
use overview::{Overview, Safety};
use test::TestControls;

pub use claude::{register as register_claude, unregister_from_claude_code};
pub use update::start_auto_checks as start_update_checks;

const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const VIFTE_INTERVAL: Duration = Duration::from_secs(5);
const APPEARANCES: [Appearance; 3] = [Appearance::System, Appearance::Light, Appearance::Dark];
const APPEARANCE_LABELS: [&str; 3] = [n_("Follow system"), n_("Light"), n_("Dark")];

const CSS: &str = r#"
.reading { font-feature-settings: "tnum"; }
.stress-tile { padding: 13px 15px; }
.stress-tile-title { font-size: 13px; font-weight: 600; opacity: 0.75; }
.stress-value { font-size: 26px; font-weight: 700; letter-spacing: -0.4px; font-feature-settings: "tnum"; }
.stress-large .stress-value { font-size: 32px; }
.stress-text-value { font-size: 20px; font-weight: 700; margin-top: 8px; }
.stress-sub { font-size: 12.5px; opacity: 0.62; font-feature-settings: "tnum"; }
.stress-section { font-size: 14.5px; font-weight: 700; }
.stress-chip { font-size: 11.5px; font-weight: 700; padding: 1px 8px; border-radius: 99px; background-color: alpha(currentColor, 0.1); }
.stress-dot { min-width: 7px; min-height: 7px; border-radius: 99px; background-color: @success_color; }
.stress-dot.warm { background-color: @warning_color; }
.stress-dot.hot { background-color: @error_color; }
.stress-slim { padding: 5px 12px; border-radius: 99px; font-size: 13px; box-shadow: inset 0 0 0 1px alpha(currentColor, 0.1); }
.stress-slim-dot { min-width: 7px; min-height: 7px; border-radius: 99px; box-shadow: inset 0 0 0 1.5px alpha(currentColor, 0.5); }
.stress-details { font-size: 13px; }
.status-hero {
  padding: 12px 14px;
  border-radius: 14px;
  box-shadow: inset 0 0 0 1px alpha(currentColor, 0.08);
  background-image: linear-gradient(115deg, alpha(@success_color, 0.18), alpha(currentColor, 0.04));
}
.status-hero.idle { background-image: linear-gradient(115deg, alpha(currentColor, 0.1), alpha(currentColor, 0.04)); }
.status-hero.running { background-image: linear-gradient(115deg, alpha(@accent_color, 0.18), alpha(currentColor, 0.04)); }
.status-hero.stopped { background-image: linear-gradient(115deg, alpha(@error_color, 0.18), alpha(currentColor, 0.04)); }
.status-hero.claude { background-image: linear-gradient(115deg, alpha(#d97757, 0.24), alpha(currentColor, 0.04)); }
.status-hero-dot { min-width: 10px; min-height: 10px; border-radius: 99px; background-color: @success_color; }
.status-hero.idle .status-hero-dot { background-color: alpha(currentColor, 0.5); }
.status-hero.running .status-hero-dot { background-color: @accent_color; }
.status-hero.stopped .status-hero-dot { background-color: @error_color; }
.status-hero.claude .status-hero-dot { background-color: #d97757; }
.status-hero-title { font-size: 15px; font-weight: 700; }
.status-hero-text { font-size: 13px; opacity: 0.75; font-feature-settings: "tnum"; }
.stress-go { padding: 10px; border-radius: 12px; font-size: 15px; font-weight: 700; }
.stress-profile {
  padding: 7px 4px;
  border-radius: 10px;
  font-size: 13px;
  font-weight: 600;
  background-color: alpha(currentColor, 0.06);
  box-shadow: inset 0 0 0 1px alpha(currentColor, 0.08);
}
.stress-profile:hover { background-color: alpha(currentColor, 0.11); }
.stress-profile:checked { background-color: @accent_bg_color; color: @accent_fg_color; box-shadow: none; }
.stress-profile-load { font-size: 11.5px; font-weight: 400; opacity: 0.7; font-feature-settings: "tnum"; }

/* The main window: layered cards on a soft tint. Dialogs stay outside and keep the plain look. */
.stress-main { transition: background-image 1200ms ease; }
.stress-main headerbar { background: none; box-shadow: none; }
.stress-main flowboxchild { padding: 0; background: none; }
.stress-main .stress-tile, .stress-main list.boxed-list {
  background-color: alpha(white, 0.065);
  border-radius: 16px;
  box-shadow: inset 0 1px alpha(white, 0.09), inset 0 0 0 1px alpha(white, 0.07), 0 6px 20px alpha(black, 0.16);
}
.stress-light .stress-tile, .stress-light list.boxed-list {
  background-color: alpha(white, 0.74);
  box-shadow: inset 0 0 0 1px alpha(black, 0.06), 0 6px 18px alpha(black, 0.07);
}
.stress-main list.boxed-list > row:first-child { border-top-left-radius: 16px; border-top-right-radius: 16px; }
.stress-main list.boxed-list > row:last-child { border-bottom-left-radius: 16px; border-bottom-right-radius: 16px; }
.stress-docked > .sidebar-pane { background-color: alpha(black, 0.2); box-shadow: inset 1px 0 alpha(white, 0.07); }
.stress-docked.stress-light > .sidebar-pane { background-color: alpha(white, 0.38); box-shadow: inset 1px 0 alpha(black, 0.07); }
.stress-tint-cool { background-image: radial-gradient(farthest-side at 10% 0, alpha(#4a8fd0, 0.30), transparent); }
.stress-tint-mild { background-image: radial-gradient(farthest-side at 10% 0, alpha(#48a89a, 0.28), transparent); }
.stress-tint-warm { background-image: radial-gradient(farthest-side at 10% 0, alpha(#d9a03a, 0.30), transparent); }
.stress-tint-hot { background-image: radial-gradient(farthest-side at 10% 0, alpha(#de6a3a, 0.34), transparent); }
"#;

pub fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

pub fn apply_appearance(appearance: Appearance) {
    let scheme = match appearance {
        Appearance::System => adw::ColorScheme::Default,
        Appearance::Light => adw::ColorScheme::ForceLight,
        Appearance::Dark => adw::ColorScheme::ForceDark,
    };
    adw::StyleManager::default().set_color_scheme(scheme);
}

/// The hardware and its sensor history. Kept for the whole run, so the
/// history survives the window being rebuilt for a language change.
struct Machine {
    cpu: devices::Cpu,
    gpus: Vec<Gpu>,
    history: History,
    controller: Controller,
    /// Taken before the history starts, to know which readings exist.
    first: Sample,
    /// Vifte's latest state, read off the window's thread since it can take
    /// a moment.
    vifte: Arc<Mutex<Option<VifteState>>>,
}

thread_local! {
    /// The open window, so a second launch brings it forward instead of
    /// opening another one.
    static CURRENT: RefCell<std::rc::Weak<App>> = const { RefCell::new(std::rc::Weak::new()) };
    static MACHINE: OnceCell<Rc<Machine>> = const { OnceCell::new() };
    /// The status icon belongs to the application, not to a window, so it
    /// survives the window being rebuilt for a language change.
    static TRAY: RefCell<Option<ksni::blocking::Handle<tray::StressTray>>> = const { RefCell::new(None) };
}

fn has_tray() -> bool {
    TRAY.with(|tray| tray.borrow().is_some())
}

/// Starts the status icon and routes its menu commands to the current window.
pub fn start_tray(application: &adw::Application) {
    let (tx, rx) = async_channel::unbounded();
    let Some(handle) = tray::spawn(tx) else { return };
    TRAY.with(|tray| *tray.borrow_mut() = Some(handle));
    let application = application.downgrade();
    glib::spawn_future_local(async move {
        while let Ok(command) = rx.recv().await {
            let Some(application) = application.upgrade() else { break };
            let app = CURRENT.with(|current| current.borrow().upgrade());
            match (command, app) {
                (tray::Command::Quit, Some(app)) => {
                    app.force_close();
                    application.quit();
                }
                (tray::Command::Quit, None) => application.quit(),
                (tray::Command::Show, Some(app)) => app.window.present(),
                (tray::Command::Show, None) => application.activate(),
                (tray::Command::StartStop, Some(app)) => {
                    match app.machine.controller.status() {
                        crate::loadtest::Status::Idle => app.start_test(),
                        _ => app.machine.controller.stop(),
                    }
                    app.update_test_status();
                }
                (tray::Command::StartStop, None) => {}
            }
        }
    });
}

fn machine() -> Rc<Machine> {
    MACHINE.with(|machine| {
        machine
            .get_or_init(|| {
                let cpu = devices::cpu();
                let gpus = devices::gpus();
                let mut sensors = Sensors::new(&gpus);
                let first = sensors.sample();
                let controller = Controller::new(gpus.iter().map(|g| g.pci.clone()).collect());
                let history = History::start(sensors, controller.clone());
                let vifte = Arc::new(Mutex::new(None));
                let shared = vifte.clone();
                std::thread::spawn(move || loop {
                    let state = vifte::state();
                    if let Ok(mut latest) = shared.lock() {
                        *latest = state;
                    }
                    std::thread::sleep(VIFTE_INTERVAL);
                });
                Rc::new(Machine { cpu, gpus, history, controller, first, vifte })
            })
            .clone()
    })
}

pub fn activate(application: &adw::Application) {
    if let Some(app) = CURRENT.with(|current| current.borrow().upgrade()) {
        app.window.present();
        return;
    }
    App::new(application, None).window.present();
}

/// Window state handed to the new window when it is rebuilt for a language change.
struct Carry {
    size: (i32, i32),
    maximized: bool,
}

/// Picks one reading out of a sample.
type ValueFn = Box<dyn Fn(&Sample) -> Option<f64>>;

/// A card and the reading it shows.
struct Reading {
    card: Card,
    value: ValueFn,
}

/// Power supply details that have no card of their own.
struct PsuLabels {
    input_volts: gtk::Label,
    temperature: gtk::Label,
    fan: gtk::Label,
    rails: Vec<gtk::Label>,
}

pub struct App {
    window: adw::ApplicationWindow,
    title: adw::WindowTitle,
    toasts: adw::ToastOverlay,
    split: adw::OverlaySplitView,
    overview: Rc<Overview>,
    /// The readings that drive the overview: graphics cards that can be used.
    usable: Vec<usize>,
    tint: Cell<Option<&'static str>>,
    test: TestControls,
    /// How the last test ended, as shown under the panel.
    last_test: RefCell<Option<(String, crate::loadtest::Finished)>>,
    /// The running test was started by Claude.
    claude_started: Cell<bool>,
    /// The window is being replaced for a language change; a running test
    /// carries on.
    rebuilding: Cell<bool>,
    /// The window closes for real, also when it would otherwise go to the
    /// status icon.
    closing: Cell<bool>,
    /// "Still running" was already shown once for closing to the status icon.
    hidden_notice_shown: Cell<bool>,
    /// Last state pushed to the status icon, to avoid needless D-Bus updates.
    tray_state: RefCell<Option<tray::State>>,
    config: RefCell<Config>,
    machine: Rc<Machine>,
    readings: Vec<Reading>,
    psu: Option<PsuLabels>,
    refresh_source: RefCell<Option<glib::SourceId>>,
}

/// Adds a line to the power supply's details and returns its value label.
fn detail(grid: &gtk::Grid, title: &str) -> gtk::Label {
    let row = grid.observe_children().n_items() as i32 / 2;
    grid.attach(&gtk::Label::builder().label(title).halign(gtk::Align::Start).css_classes(["stress-sub"]).build(), 0, row, 1, 1);
    // A small natural width, so these do not push the card onto a line of its own.
    let label = gtk::Label::builder().xalign(1.0).hexpand(true).max_width_chars(8).ellipsize(gtk::pango::EllipsizeMode::Start).css_classes(["reading"]).build();
    grid.attach(&label, 1, row, 1, 1);
    label
}

/// A status card with a dot; returns it with its title, its text and the box
/// holding both, for anything more to go under them.
pub(crate) fn status_hero() -> (gtk::Box, gtk::Label, gtk::Label, gtk::Box) {
    let hero = gtk::Box::builder().spacing(12).css_classes(["status-hero"]).build();
    hero.append(&gtk::Box::builder().valign(gtk::Align::Center).css_classes(["status-hero-dot"]).build());
    let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).build();
    let title = gtk::Label::builder().halign(gtk::Align::Start).wrap(true).xalign(0.0).css_classes(["status-hero-title"]).build();
    let subtitle = gtk::Label::builder().halign(gtk::Align::Start).wrap(true).xalign(0.0).css_classes(["status-hero-text"]).build();
    text.append(&title);
    text.append(&subtitle);
    hero.append(&text);
    (hero, title, subtitle, text)
}

/// A heading for one device, followed by a grid for its cards.
fn section(content: &gtk::Box, title: &str, subtitle: &str) -> gtk::FlowBox {
    let heading = gtk::Box::builder().spacing(12).margin_top(4).build();
    heading.append(
        &gtk::Label::builder()
            .label(title)
            .halign(gtk::Align::Start)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .tooltip_text(title)
            .css_classes(["stress-section"])
            .build(),
    );
    if !subtitle.is_empty() {
        heading.append(
            &gtk::Label::builder()
                .label(subtitle)
                .halign(gtk::Align::End)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .tooltip_text(subtitle)
                .css_classes(["stress-sub"])
                .build(),
        );
    }
    content.append(&heading);
    let grid = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(1)
        .column_spacing(12)
        .row_spacing(12)
        .build();
    content.append(&grid);
    grid
}

/// Puts all of a section's cards on one line when they fit.
fn fill_row(grid: &gtk::FlowBox) {
    grid.set_max_children_per_line(grid.observe_children().n_items().max(1));
}

/// A small label for something that takes no room of its own.
fn slim(text: &str, tooltip: &str) -> gtk::Box {
    let label = gtk::Box::builder().spacing(7).tooltip_text(tooltip).css_classes(["stress-slim"]).build();
    label.append(&gtk::Box::builder().valign(gtk::Align::Center).css_classes(["stress-slim-dot"]).build());
    label.append(&gtk::Label::builder().label(text).ellipsize(gtk::pango::EllipsizeMode::End).build());
    label
}

impl App {
    fn new(application: &adw::Application, carry: Option<Carry>) -> Rc<Self> {
        let config = Config::load();
        let machine = machine();
        // STRESS_WINDOW_SIZE ("900x700") is a development aid for trying narrow windows.
        let size = std::env::var("STRESS_WINDOW_SIZE").ok().and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        });
        let (width, height) = carry.as_ref().map(|c| c.size).or(size).unwrap_or((1280, 860));
        let window = adw::ApplicationWindow::builder()
            .application(application)
            .title("Stress")
            .default_width(width)
            .default_height(height)
            .maximized(carry.as_ref().is_some_and(|c| c.maximized))
            .width_request(360)
            .height_request(400)
            .build();

        let title = adw::WindowTitle::new("Stress", "");
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&title));
        let menu = gio::Menu::new();
        menu.append(Some(tr("Preferences")), Some("win.preferences"));
        menu.append(Some(tr("Check for Updates…")), Some("win.check-update"));
        menu.append(Some(tr("About Stress")), Some("win.about"));
        let quit = gio::Menu::new();
        quit.append(Some(tr("Quit")), Some("app.quit"));
        menu.append_section(None, &quit);
        let menu_button = |primary: bool| {
            gtk::MenuButton::builder()
                .icon_name("open-menu-symbolic")
                .menu_model(&menu)
                .primary(primary)
                .tooltip_text(tr("Main Menu"))
                .build()
        };
        // The panel holds the menu; when it is hidden, the window's own header does.
        let header_menu = menu_button(false);
        header_menu.set_visible(false);
        header.pack_end(&header_menu);
        let panel_toggle = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-right-symbolic")
            .tooltip_text(tr("Load Test"))
            .visible(false)
            .build();
        header.pack_end(&panel_toggle);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .margin_top(4)
            .margin_bottom(18)
            .margin_start(20)
            .margin_end(20)
            .build();
        let mut readings = Vec::new();
        // The newest sample, so a rebuilt window shows readings that became
        // available since the start (such as the processor's power).
        let latest = machine.history.latest();
        let first = latest.as_ref().unwrap_or(&machine.first);

        let loadable: Vec<(usize, String, String)> = machine
            .gpus
            .iter()
            .enumerate()
            .filter(|(_, g)| g.unavailable_reason().is_none())
            .map(|(i, g)| (i, g.name.clone(), g.pci.clone()))
            .collect();
        let test = TestControls::new(&config, machine.cpu.threads, &loadable);
        let usable: Vec<usize> = loadable.iter().map(|(i, _, _)| *i).collect();

        // The overview: the first graphics card that can be used shares the
        // top row with the processor.
        let cpu_temperature = Card::temperature(tr("Processor"));
        let gpu_temperature = usable.first().map(|_| Card::temperature(tr("Graphics card")));
        let overview = Rc::new(Overview::new(
            cpu_temperature.widget.clone().upcast(),
            gpu_temperature.as_ref().map(|card| card.widget.clone().upcast()),
        ));
        overview.grid.set_margin_bottom(8);
        content.append(&overview.grid);
        readings.push(Reading { card: cpu_temperature, value: Box::new(|s| s.cpu_temperature) });
        if let (Some(card), Some(&i)) = (gpu_temperature, usable.first()) {
            readings.push(Reading { card, value: Box::new(move |s| s.gpus[i].temperature) });
        }
        let mut add = |grid: &gtk::FlowBox, card: Card, value: ValueFn| {
            grid.append(&card.widget);
            if let Some(child) = card.widget.parent() {
                child.set_focusable(false);
            }
            readings.push(Reading { card, value });
        };

        // Processor.
        let cpu = section(&content, &machine.cpu.model, &trf("{} threads", &[&machine.cpu.threads]));
        add(&cpu, Card::new(tr("Utilization"), "%", Scale::Percent), Box::new(|s| s.cpu_utilization));
        if first.cpu_power.is_some() {
            add(&cpu, Card::new(tr("Power"), "W", Scale::Power), Box::new(|s| s.cpu_power));
        }
        fill_row(&cpu);

        // Graphics cards that can be used; the others get a small label with the reason.
        for &i in &usable {
            let gpu = &machine.gpus[i];
            let grid = section(&content, &gpu.name, &gpu_subtitle(gpu));
            let reading = &first.gpus[i];
            if usable.first() != Some(&i) {
                add(&grid, Card::new(tr("Temperature"), "°C", Scale::Temperature), Box::new(move |s| s.gpus[i].temperature));
            }
            add(&grid, Card::new(tr("Utilization"), "%", Scale::Percent), Box::new(move |s| s.gpus[i].utilization));
            if reading.power.is_some() {
                add(&grid, Card::new(tr("Power"), "W", Scale::Power), Box::new(move |s| s.gpus[i].power));
            }
            if reading.fan_percent.is_some() {
                add(&grid, Card::new(tr("Fan"), "%", Scale::Percent), Box::new(move |s| s.gpus[i].fan_percent));
            }
            fill_row(&grid);
        }
        let unusable: Vec<&Gpu> = machine.gpus.iter().filter(|g| g.unavailable_reason().is_some()).collect();
        if !unusable.is_empty() || machine.gpus.is_empty() {
            let labels = gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).column_spacing(8).row_spacing(8).max_children_per_line(8).build();
            for gpu in &unusable {
                let reason = gpu.unavailable_reason().unwrap_or_default();
                labels.append(&slim(&format!("{} · {reason}", gpu.name), &gpu_subtitle(gpu)));
            }
            if machine.gpus.is_empty() {
                labels.append(&slim(tr("No graphics card found"), ""));
            }
            for child in std::iter::successors(labels.first_child(), |child| child.next_sibling()) {
                child.set_focusable(false);
                child.set_halign(gtk::Align::Start);
            }
            content.append(&labels);
        }

        // Power supply, when it reports itself.
        let psu = first.psu.as_ref().map(|reading| {
            let grid = section(&content, &reading.name, tr("The power supply updates its readings every few seconds."));
            add(&grid, Card::new(tr("Output to the computer"), "W", Scale::Power), Box::new(|s| s.psu.as_ref().map(|p| p.output)));
            if first.cpu_power.is_some() && usable.iter().all(|&i| first.gpus[i].power.is_some()) {
                let card = Card::new(tr("Rest of the computer"), "W", Scale::Power);
                card.widget.set_tooltip_text(Some(tr("Motherboard, memory, disks and fans: the power supply's output minus the processor and graphics cards.")));
                let usable = usable.clone();
                add(&grid, card, Box::new(move |s| rest_of_computer(s, &usable)));
            }
            let details = tile();
            details.append(&gtk::Label::builder().label(tr("Details")).halign(gtk::Align::Start).css_classes(["stress-tile-title"]).build());
            let rows = gtk::Grid::builder().column_spacing(12).row_spacing(2).margin_top(4).css_classes(["stress-details"]).build();
            details.append(&rows);
            let input_volts = detail(&rows, tr("Input voltage"));
            let temperature = detail(&rows, tr("Temperature"));
            let fan = detail(&rows, tr("Fan"));
            let rails = reading.rails.iter().map(|rail| detail(&rows, &trf("{} rail", &[&rail.label]))).collect();
            grid.append(&details);
            if let Some(child) = details.parent() {
                child.set_focusable(false);
            }
            fill_row(&grid);
            PsuLabels { input_volts, temperature, fan, rails }
        });

        let rapl_offer = first.cpu_power.is_none().then_some(cpu);
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&content)
            .vexpand(true)
            .build();
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&scroll));

        // The load test panel.
        let panel_header = adw::HeaderBar::builder().show_title(false).build();
        panel_header.pack_start(&gtk::Label::builder().label(tr("Load Test")).margin_start(8).css_classes(["heading"]).build());
        panel_header.pack_end(&menu_button(true));
        let panel = adw::ToolbarView::new();
        panel.add_top_bar(&panel_header);
        panel.set_content(Some(&test.panel));
        let split = adw::OverlaySplitView::builder()
            .css_classes(["stress-main", "stress-docked"])
            .sidebar_position(gtk::PackType::End)
            .min_sidebar_width(380.0)
            .max_sidebar_width(400.0)
            .sidebar_width_fraction(0.31)
            .content(&view)
            .sidebar(&panel)
            .build();
        split.bind_property("show-sidebar", &panel_toggle, "active").bidirectional().sync_create().build();
        split.connect_collapsed_notify(|split| {
            // Over the readings the panel needs its normal, solid background.
            if split.is_collapsed() {
                split.remove_css_class("stress-docked");
            } else {
                split.add_css_class("stress-docked");
            }
        });
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&split));
        window.set_content(Some(&toasts));

        // The panel goes over the readings when it no longer fits beside them,
        // and the overview takes two rows when it no longer fits on one.
        let hide_panel = |breakpoint: &adw::Breakpoint| {
            breakpoint.add_setter(&split, "collapsed", Some(&true.to_value()));
            breakpoint.add_setter(&split, "show-sidebar", Some(&false.to_value()));
            breakpoint.add_setter(&panel_toggle, "visible", Some(&true.to_value()));
            breakpoint.add_setter(&header_menu, "visible", Some(&true.to_value()));
        };
        let medium = adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 1180sp").unwrap());
        hide_panel(&medium);
        window.add_breakpoint(medium);
        let narrow = adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 800sp").unwrap());
        hide_panel(&narrow);
        let tiles = overview.clone();
        narrow.connect_apply(move |_| tiles.arrange(true));
        let tiles = overview.clone();
        narrow.connect_unapply(move |_| tiles.arrange(false));
        window.add_breakpoint(narrow);

        // The layered cards look different on a light background.
        let style = adw::StyleManager::default();
        let sync_style = {
            let split = split.clone();
            move |style: &adw::StyleManager| {
                if style.is_dark() {
                    split.remove_css_class("stress-light");
                } else {
                    split.add_css_class("stress-light");
                }
            }
        };
        sync_style(&style);
        style.connect_dark_notify(sync_style);

        let app = Rc::new(App {
            window,
            title,
            toasts,
            split,
            overview,
            usable,
            tint: Cell::new(None),
            test,
            last_test: RefCell::new(None),
            claude_started: Cell::new(false),
            rebuilding: Cell::new(false),
            closing: Cell::new(false),
            hidden_notice_shown: Cell::new(false),
            tray_state: RefCell::new(None),
            config: RefCell::new(config),
            machine,
            readings,
            psu,
            refresh_source: RefCell::new(None),
        });
        CURRENT.with(|current| *current.borrow_mut() = Rc::downgrade(&app));
        if let (Some(cards), Some(offer)) = (rapl_offer, app.rapl_offer()) {
            // Right below the processor's cards.
            if let Some(content) = cards.parent().and_downcast::<gtk::Box>() {
                content.insert_child_after(&offer, Some(&cards));
            }
        }
        app.install_actions();
        app.build_test_panel();
        app.tick();

        // Holds the app alive; removed when the window closes.
        let strong = app.clone();
        let source = glib::timeout_add_local(REFRESH_INTERVAL, move || {
            strong.tick();
            glib::ControlFlow::Continue
        });
        *app.refresh_source.borrow_mut() = Some(source);
        let weak = Rc::downgrade(&app);
        app.window.connect_close_request(move |window| {
            let Some(app) = weak.upgrade() else { return glib::Propagation::Proceed };
            if !app.closing.get() && !app.rebuilding.get() && app.config.borrow().close_to_tray && has_tray() {
                app.hide_to_tray(window);
                return glib::Propagation::Stop;
            }
            if let Some(source) = app.refresh_source.take() {
                source.remove();
            }
            if !app.rebuilding.get() {
                app.machine.controller.stop();
            }
            glib::Propagation::Proceed
        });
        if let Ok(path) = std::env::var("STRESS_SCREENSHOT") {
            let page = std::env::var("STRESS_PAGE").ok();
            // Starts a test right away with the saved setup, to capture it running.
            if page.as_deref() == Some("start") {
                app.start_test();
            }
            app.schedule_screenshot(path, page);
        }
        app
    }

    /// Development aid: render the window to a PNG and quit, since GNOME on
    /// Wayland does not let other programs capture it.
    ///
    /// `tour` idles for 30 seconds, starts a test with the saved setup and
    /// captures it after the delay; `panel` shows the load test panel, for
    /// trying it over the readings in a narrow window.
    fn schedule_screenshot(self: &Rc<Self>, path: String, page: Option<String>) {
        let weak = Rc::downgrade(self);
        let delay = std::env::var("STRESS_SCREENSHOT_DELAY").ok().and_then(|d| d.parse().ok()).unwrap_or(3);
        if page.as_deref() == Some("tour") {
            let start = weak.clone();
            glib::timeout_add_local_once(Duration::from_secs(30), move || {
                if let Some(app) = start.upgrade() {
                    app.start_test();
                }
            });
            glib::timeout_add_local_once(Duration::from_secs(delay), move || {
                let Some(app) = weak.upgrade() else { return };
                capture(&app.window, &path);
                app.force_close();
            });
            return;
        }
        glib::timeout_add_local_once(Duration::from_secs(delay), move || {
            let Some(app) = weak.upgrade() else { return };
            if page.as_deref() == Some("about") {
                app.show_about();
            } else if page.as_deref() == Some("panel") {
                app.split.set_show_sidebar(true);
            } else if let Some(prefs_page) = page.as_deref().and_then(|p| p.strip_prefix("preferences")) {
                let dialog = app.show_preferences();
                if let Some(name) = prefs_page.strip_prefix('-') {
                    dialog.set_visible_page_name(name);
                }
            }
            let window = app.window.clone();
            let weak = Rc::downgrade(&app);
            glib::timeout_add_local_once(Duration::from_millis(1500), move || {
                capture(&window, &path);
                if let Some(app) = weak.upgrade() {
                    app.force_close();
                }
            });
        });
    }

    fn tick(&self) {
        self.update_test_status();
        self.sync_tray();
        let history = &self.machine.history;
        let tests = history.test_periods();
        for reading in &self.readings {
            let points = history.series(&reading.value);
            let current = points.last().and_then(|(_, v)| *v);
            reading.card.set(current, points, tests.clone());
        }
        if let (Some(labels), Some(reading)) = (&self.psu, history.latest().and_then(|s| s.psu)) {
            show_psu(labels, &reading);
        }
        if let Some(sample) = history.latest() {
            self.show_overview(&sample);
        }
    }

    /// Power use, safety and the window's tint.
    fn show_overview(&self, sample: &Sample) {
        match &sample.psu {
            Some(psu) => match psu.input_estimate {
                Some(wall) => {
                    self.overview.show_power(
                        tr("From the wall"),
                        &format!("≈ {}", format(Some(wall), "W", 0)),
                        &trf("Output {}", &[&format(Some(psu.output), "W", 0)]),
                    );
                    self.overview.set_power_tooltip(Some(tr("Calculated from the efficiency class; the power supply does not measure it.")));
                }
                None => self.overview.show_power(tr("Output to the computer"), &format(Some(psu.output), "W", 0), ""),
            },
            None => {
                let parts = std::iter::once(sample.cpu_power).chain(self.usable.iter().map(|&i| sample.gpus[i].power));
                let total: Option<f64> = parts.sum();
                self.overview.show_power(tr("Power"), &format(total, "W", 0), tr("Processor and graphics cards"));
            }
        }

        let vifte = self.machine.vifte.lock().ok().and_then(|state| state.clone());
        let vifte_limit = vifte.as_ref().and_then(VifteState::limit).unwrap_or(f64::INFINITY);
        let (cpu_limit, gpu_limit, tint) = {
            let config = self.config.borrow();
            (config.cpu_limit.min(vifte_limit), config.gpu_limit.min(vifte_limit), config.window_tint)
        };
        let mut readings = vec![(sample.cpu_temperature, cpu_limit)];
        readings.extend(self.usable.iter().map(|&i| (sample.gpus[i].temperature, gpu_limit)));
        let idle = matches!(self.machine.controller.status(), crate::loadtest::Status::Idle);
        let limit_reached = idle
            && self.last_test.borrow().as_ref().is_some_and(|(_, f)| matches!(f.reason, StopReason::CpuLimit(_) | StopReason::GpuLimit(_)));
        self.overview.show_safety(&Safety { readings: &readings, vifte: vifte.as_ref(), limit_reached });

        let hottest = readings.iter().filter_map(|(t, _)| *t).reduce(f64::max);
        self.set_tint(tint.then_some(hottest).flatten());
    }

    fn set_tint(&self, temperature: Option<f64>) {
        let old = self.tint.get();
        let new = look::heat_step(temperature, old);
        if new == old {
            return;
        }
        if let Some(step) = old {
            self.split.remove_css_class(&format!("stress-tint-{step}"));
        }
        if let Some(step) = new {
            self.split.add_css_class(&format!("stress-tint-{step}"));
        }
        self.tint.set(new);
    }

    /// Shows a message: a toast in the window, or a desktop notification
    /// while the window is hidden in the status icon.
    fn toast(&self, message: &str) {
        if self.window.is_visible() {
            self.toasts.add_toast(adw::Toast::new(message));
        } else if let Some(application) = self.window.application() {
            let notification = gio::Notification::new("Stress");
            notification.set_body(Some(message));
            application.send_notification(Some("stress"), &notification);
        }
    }

    fn hide_to_tray(&self, window: &adw::ApplicationWindow) {
        window.set_visible(false);
        if self.hidden_notice_shown.replace(true) {
            return;
        }
        if let Some(application) = window.application() {
            let notification = gio::Notification::new(tr("Stress is still running"));
            notification.set_body(Some(tr("Stress is in the status icon, and a running test carries on. Quit from the icon.")));
            application.send_notification(Some("background"), &notification);
        }
    }

    /// Closes the window for real, also when it would go to the status icon.
    fn force_close(&self) {
        self.closing.set(true);
        self.window.close();
    }

    /// Pushes the test state and readings to the status icon.
    fn sync_tray(&self) {
        if !has_tray() {
            return;
        }
        let machine = &self.machine;
        let Some(sample) = machine.history.latest() else { return };
        let levels = match machine.controller.status() {
            crate::loadtest::Status::Running { levels, .. } => Some(levels),
            _ => None,
        };
        let gpu_temperature = levels.and_then(|l| l.gpu).and_then(|i| sample.gpus.get(i)).and_then(|g| g.temperature);
        let cpu_temperature = levels.filter(|l| l.cpu_load > 0).and(sample.cpu_temperature);
        let hottest = [cpu_temperature, gpu_temperature].into_iter().flatten().reduce(f64::max);
        let mut lines = vec![trf("Processor: {}", &[&[format(sample.cpu_temperature, "°C", 0), format(sample.cpu_power, "W", 0)].join(" · ")])];
        for (gpu, reading) in machine.gpus.iter().zip(&sample.gpus) {
            if gpu.unavailable_reason().is_none() {
                lines.push(format!("{}: {}", gpu.name, [format(reading.temperature, "°C", 0), format(reading.power, "W", 0)].join(" · ")));
            }
        }
        if let Some(psu) = &sample.psu {
            lines.push(trf("Power supply: {}", &[&format(Some(psu.output), "W", 0)]));
        }
        let state = tray::State { running: levels.is_some(), hottest, summary: lines.join("\n") };
        if self.tray_state.borrow().as_ref() == Some(&state) {
            return;
        }
        *self.tray_state.borrow_mut() = Some(state.clone());
        TRAY.with(|tray| {
            if let Some(handle) = tray.borrow().as_ref() {
                handle.update(|tray| tray.state = state);
            }
        });
    }

    fn install_actions(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let about = gio::ActionEntry::builder("about")
            .activate(move |_: &adw::ApplicationWindow, _, _| {
                if let Some(app) = weak.upgrade() {
                    app.show_about();
                }
            })
            .build();
        let weak = Rc::downgrade(self);
        let preferences = gio::ActionEntry::builder("preferences")
            .activate(move |_: &adw::ApplicationWindow, _, _| {
                if let Some(app) = weak.upgrade() {
                    app.show_preferences();
                }
            })
            .build();
        let weak = Rc::downgrade(self);
        let check_update = gio::ActionEntry::builder("check-update")
            .activate(move |_: &adw::ApplicationWindow, _, _| {
                if let Some(app) = weak.upgrade() {
                    app.check_for_updates(true);
                }
            })
            .build();
        self.window.add_action_entries([preferences, about, check_update]);
    }

    fn show_about(&self) {
        let about = adw::AboutDialog::builder()
            .application_name("Stress")
            .application_icon(APP_ID)
            .version(env!("CARGO_PKG_VERSION"))
            .comments(tr("Put the processor and graphics card under controlled load and watch temperatures, fans and power use."))
            .developer_name("From Norway with Love ❤️")
            .developers(["Jan Ivar Sørensen"])
            .copyright("© 2026 Jan Ivar Sørensen")
            .website("https://github.com/Vindihaaret/stress")
            .issue_url("https://github.com/Vindihaaret/stress/issues")
            .license_type(gtk::License::Gpl30)
            .build();
        // Translators: put your name(s) here to be credited in the About dialog.
        let credits = tr("translator-credits");
        if credits != "translator-credits" {
            about.set_translator_credits(credits);
        }
        about.present(Some(&self.window));
    }

    fn show_preferences(self: &Rc<Self>) -> adw::PreferencesDialog {
        let dialog = adw::PreferencesDialog::new();
        let page = adw::PreferencesPage::builder()
            .title(tr("General"))
            .name("general")
            .icon_name("preferences-system-symbolic")
            .build();

        let look = adw::PreferencesGroup::builder().title(tr("Appearance")).build();
        let current = self.config.borrow().appearance;
        let style = adw::ComboRow::builder()
            .title(tr("Style"))
            .model(&gtk::StringList::new(&APPEARANCE_LABELS.map(tr)))
            .selected(APPEARANCES.iter().position(|a| *a == current).unwrap_or(0) as u32)
            .build();
        look.add(&style);
        let tint = adw::SwitchRow::builder()
            .title(tr("Colour follows temperature"))
            .subtitle(tr("Soft window tint: cool when idle, warm under load"))
            .active(self.config.borrow().window_tint)
            .build();
        look.add(&tint);
        page.add(&look);
        let weak = Rc::downgrade(self);
        tint.connect_active_notify(move |row| {
            let Some(app) = weak.upgrade() else { return };
            app.config.borrow_mut().window_tint = row.is_active();
            app.config.borrow().save();
            app.tick();
        });

        let behaviour = adw::PreferencesGroup::builder().title(tr("Window")).build();
        let tray_available = has_tray();
        let on_close = adw::SwitchRow::builder()
            .title(tr("Keep in the status icon when the window is closed"))
            .active(self.config.borrow().close_to_tray && tray_available)
            .sensitive(tray_available)
            .build();
        on_close.set_subtitle(if tray_available {
            tr("Instead of quitting. A running test carries on, and the icon shows its temperature.")
        } else {
            tr("Needs a status icon area. On GNOME, install the AppIndicator extension.")
        });
        behaviour.add(&on_close);
        page.add(&behaviour);
        let weak = Rc::downgrade(self);
        on_close.connect_active_notify(move |row| {
            let Some(app) = weak.upgrade() else { return };
            app.config.borrow_mut().close_to_tray = row.is_active();
            app.config.borrow().save();
        });

        let languages = i18n::available_languages();
        let mut labels = vec![tr("System default")];
        labels.extend(languages.iter().map(|(_, name)| *name));
        let current = self.config.borrow().language.clone();
        let selected = current
            .as_deref()
            .and_then(|code| languages.iter().position(|(c, _)| *c == code))
            .map_or(0, |i| i + 1);
        let language_group = adw::PreferencesGroup::builder()
            .title(tr("Language"))
            .description(tr("“System default” uses the language of your desktop."))
            .build();
        let language = adw::ComboRow::builder()
            .title(tr("Language"))
            .model(&gtk::StringList::new(&labels))
            .selected(selected as u32)
            .build();
        language_group.add(&language);
        page.add(&language_group);
        page.add(&self.update_preferences_group());
        page.add(&self.helpers_group(&dialog));

        // Uninstalling lives at the bottom here, not in the main menu, so it is a deliberate choice.
        let removal = adw::PreferencesGroup::new();
        let uninstall_row = adw::ActionRow::builder()
            .title(tr("Uninstall Stress"))
            .subtitle(tr("Removes the program, its menu entry and the helpers it installed, and optionally the settings."))
            .build();
        let uninstall = gtk::Button::builder()
            .label(tr("Uninstall"))
            .valign(gtk::Align::Center)
            .css_classes(["destructive-action"])
            .build();
        uninstall_row.add_suffix(&uninstall);
        removal.add(&uninstall_row);
        page.add(&removal);
        dialog.add(&page);
        dialog.add(&self.safety_page());
        dialog.add(&self.claude_preferences_page(&dialog));

        let weak = Rc::downgrade(self);
        let prefs = dialog.clone();
        uninstall.connect_clicked(move |_| {
            prefs.close();
            if let Some(app) = weak.upgrade() {
                app.ask_uninstall();
            }
        });
        let weak = Rc::downgrade(self);
        style.connect_selected_notify(move |row| {
            let Some(app) = weak.upgrade() else { return };
            let appearance = APPEARANCES.get(row.selected() as usize).copied().unwrap_or_default();
            app.config.borrow_mut().appearance = appearance;
            app.config.borrow().save();
            apply_appearance(appearance);
        });
        let weak = Rc::downgrade(self);
        let prefs = dialog.clone();
        language.connect_selected_notify(move |row| {
            let Some(app) = weak.upgrade() else { return };
            let code = match row.selected() as usize {
                0 => None,
                i => languages.get(i - 1).map(|(code, _)| *code),
            };
            app.config.borrow_mut().language = code.map(str::to_owned);
            app.config.borrow().save();
            i18n::set_language(code);
            prefs.close();
            app.rebuild_window();
        });
        dialog.present(Some(&self.window));
        dialog
    }

    fn safety_page(self: &Rc<Self>) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::builder()
            .title(tr("Safety"))
            .name("safety")
            .icon_name("security-high-symbolic")
            .build();
        let (hero, hero_title, hero_text, _) = status_hero();
        hero_title.set_label(tr("Protection is active"));
        let hero_group = adw::PreferencesGroup::new();
        hero_group.add(&hero);
        page.add(&hero_group);
        let vifte_limit = self.machine.vifte.lock().ok().and_then(|state| state.as_ref().and_then(VifteState::limit));
        let show_protection = move |config: &Config| {
            let limit = config.cpu_limit.min(config.gpu_limit).min(vifte_limit.unwrap_or(f64::INFINITY));
            hero_text.set_label(&trf("Tests stop at {} · longest test {} minutes", &[&format(Some(limit), "°C", 0), &config.max_minutes]));
        };
        show_protection(&self.config.borrow());
        let limits = adw::PreferencesGroup::builder()
            .title(tr("Temperature Limits"))
            .description(tr("A test stops by itself when a limit is reached."))
            .build();
        let config = self.config.borrow().clone();
        let spin = |title: &str, subtitle: &str, (min, max): (f64, f64), value: f64| {
            adw::SpinRow::builder()
                .title(title)
                .subtitle(subtitle)
                .adjustment(&gtk::Adjustment::new(value, min, max, 1.0, 5.0, 0.0))
                .build()
        };
        let cpu_limit = spin(tr("Processor"), tr("°C"), config::CPU_LIMIT_RANGE, config.cpu_limit);
        let gpu_limit = spin(tr("Graphics card"), tr("°C"), config::GPU_LIMIT_RANGE, config.gpu_limit);
        limits.add(&cpu_limit);
        limits.add(&gpu_limit);
        page.add(&limits);

        let other = adw::PreferencesGroup::builder().title(tr("Tests")).build();
        let max_minutes = spin(
            tr("Longest test"),
            tr("Minutes. Also applies to tests started without a duration."),
            config::MAX_MINUTES_RANGE,
            config.max_minutes,
        );
        let battery = adw::SwitchRow::builder()
            .title(tr("Stop when running on battery"))
            .active(config.stop_on_battery)
            .build();
        other.add(&max_minutes);
        other.add(&battery);
        page.add(&other);

        // Vifte's emergency temperature also limits tests while it runs.
        let fans = adw::PreferencesGroup::builder().title(tr("Fan Control")).build();
        let vifte_row = adw::ActionRow::builder().title("Vifte").build();
        match self.machine.vifte.lock().ok().and_then(|state| state.clone()) {
            Some(state) => {
                vifte_row.set_subtitle(&match state.limit() {
                    Some(limit) => trf("Running. Tests also stop at {} °C, 5 °C below Vifte's emergency temperature, and whenever Vifte reports a problem.", &[&limit]),
                    None => tr("Running. Tests stop whenever Vifte reports a problem.").to_string(),
                });
                vifte_row.add_prefix(&gtk::Image::from_icon_name("emblem-ok-symbolic"));
            }
            None => vifte_row.set_subtitle(tr("Not running. When Vifte controls the fans, tests also respect its emergency temperature.")),
        }
        fans.add(&vifte_row);
        page.add(&fans);

        let save = move |app: &Rc<App>, change: &dyn Fn(&mut Config)| {
            change(&mut app.config.borrow_mut());
            app.config.borrow().save();
            show_protection(&app.config.borrow());
        };
        let weak = Rc::downgrade(self);
        let save_change = Rc::new(save);
        let save = save_change.clone();
        cpu_limit.connect_value_notify(move |row| {
            if let Some(app) = weak.upgrade() {
                save(&app, &|c| c.cpu_limit = row.value());
            }
        });
        let weak = Rc::downgrade(self);
        let save = save_change.clone();
        gpu_limit.connect_value_notify(move |row| {
            if let Some(app) = weak.upgrade() {
                save(&app, &|c| c.gpu_limit = row.value());
            }
        });
        let weak = Rc::downgrade(self);
        let save = save_change.clone();
        max_minutes.connect_value_notify(move |row| {
            if let Some(app) = weak.upgrade() {
                save(&app, &|c| c.max_minutes = row.value());
            }
        });
        let weak = Rc::downgrade(self);
        let save = save_change;
        battery.connect_active_notify(move |row| {
            if let Some(app) = weak.upgrade() {
                save(&app, &|c| c.stop_on_battery = row.is_active());
            }
        });
        page
    }

    /// Recreates the window so every label picks up a new language.
    fn rebuild_window(self: &Rc<Self>) {
        let Some(application) = self.window.application().and_downcast::<adw::Application>() else {
            return;
        };
        let carry = Carry {
            size: (self.window.width(), self.window.height()),
            maximized: self.window.is_maximized(),
        };
        // Keep the application running between closing this window and opening the new one.
        let _hold = application.hold();
        self.rebuilding.set(true);
        self.window.close();
        App::new(&application, Some(carry)).window.present();
    }

    fn ask_uninstall(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::new(
            Some(tr("Uninstall Stress?")),
            Some(tr("The program, its entry in the app menu and the helpers it installed will be removed.")),
        );
        let purge = gtk::CheckButton::builder().label(tr("Also delete the settings")).build();
        let helpers = gtk::CheckButton::builder()
            .label(tr("Also remove the system helpers (needs the administrator password)"))
            .active(true)
            .visible(!crate::helpers::installed().is_empty())
            .build();
        let options = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).build();
        options.append(&purge);
        options.append(&helpers);
        dialog.set_extra_child(Some(&options));
        dialog.add_responses(&[("cancel", tr("Cancel")), ("uninstall", tr("Uninstall"))]);
        dialog.set_response_appearance("uninstall", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |_, response| {
            if response != "uninstall" {
                return;
            }
            let Some(app) = weak.upgrade() else { return };
            let purge = purge.is_active();
            if helpers.is_visible() && helpers.is_active() {
                app.remove_helpers_then(move |app| app.finish_uninstall(purge));
            } else {
                app.finish_uninstall(purge);
            }
        });
        dialog.present(Some(&self.window));
    }

    fn finish_uninstall(self: &Rc<Self>, purge: bool) {
        unregister_from_claude_code();
        if let Err(err) = crate::uninstall::uninstall(purge) {
            self.toast(&trf("Uninstalling failed: {}", &[&err]));
            return;
        }
        let body = if purge {
            tr("Stress and all its settings have been removed. The window will now close.")
        } else {
            tr("Stress has been removed. The window will now close.")
        };
        let done = adw::AlertDialog::new(Some(tr("Uninstalled")), Some(body));
        done.add_response("close", tr("Close"));
        let weak = Rc::downgrade(self);
        done.connect_response(None, move |_, _| {
            if let Some(app) = weak.upgrade() {
                app.force_close();
            }
        });
        done.present(Some(&self.window));
    }
}

fn gpu_subtitle(gpu: &Gpu) -> String {
    let mut parts = vec![gpu.pci.clone()];
    if gpu.boot_vga {
        parts.push(tr("drives the screen").to_string());
    }
    if let Some(reason) = gpu.unavailable_reason() {
        parts.push(reason);
    }
    parts.join(" · ")
}

/// Renders a window to a PNG (development aid for screenshots).
fn capture(window: &adw::ApplicationWindow, path: &str) {
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, window.width() as f64, window.height() as f64);
    match (snapshot.to_node(), window.renderer()) {
        (Some(node), Some(renderer)) => {
            if let Err(err) = renderer.render_texture(node, None).save_to_png(path) {
                eprintln!("screenshot: {err}");
            }
        }
        (node, renderer) => eprintln!("screenshot: node={} renderer={}", node.is_some(), renderer.is_some()),
    }
}

/// Power the power supply delivers beyond the processor and graphics cards.
fn rest_of_computer(sample: &Sample, gpus: &[usize]) -> Option<f64> {
    let mut rest = sample.psu.as_ref()?.output - sample.cpu_power?;
    for &i in gpus {
        rest -= sample.gpus[i].power?;
    }
    // The power supply reports with a delay, so right after a load change the
    // difference can briefly go below zero.
    Some(rest.max(0.0))
}

fn show_psu(labels: &PsuLabels, reading: &PsuReading) {
    labels.input_volts.set_label(&format(reading.input_volts, "V", 0));
    labels.temperature.set_label(&format(reading.temperature, "°C", 0));
    labels.fan.set_label(&format(reading.fan_rpm, "rpm", 0));
    for (label, rail) in labels.rails.iter().zip(&reading.rails) {
        // The card is narrow: amps and watts, as asked for most; the voltage in the tooltip.
        label.set_label(&[format(rail.amps, "A", 1), format(rail.watts, "W", 0)].join(" · ").replace('.', tr(".")));
        label.set_tooltip_text(Some(&format(rail.volts, "V", 2).replace('.', tr("."))));
    }
}
