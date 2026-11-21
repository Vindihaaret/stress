//! Main window, in the same design as Vifte: a sidebar with the pages
//! (Overview, Load Test, and one per kind of hardware), a status card for
//! the test and Preferences; each page has a large title and the Start/Stop
//! button in its header. Also preferences, about and uninstalling.

pub mod claude;
mod devices;
mod graph;
mod helpers;
mod overview;
mod style;
mod test;
mod update;

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::config::{self, Appearance, Config};
use crate::devices::{self as hardware, Gpu};
use crate::history::History;
use crate::i18n::{self, n_, tr, trf};
use crate::loadtest::{Controller, StopReason};
use crate::sensors::{format, PsuReading, Sample, Sensors};
use crate::vifte::{self, VifteState};
use crate::{look, tray, APP_ID};
use devices::ReadingRow;
use graph::{HistoryGraph, Tone};
use overview::Overview;
use test::{TestControls, TestView};

pub use claude::{register as register_claude, unregister_from_claude_code};
pub use style::load_css;
pub use update::start_auto_checks as start_update_checks;

const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const VIFTE_INTERVAL: Duration = Duration::from_secs(5);
const APPEARANCES: [Appearance; 3] = [Appearance::System, Appearance::Light, Appearance::Dark];
const APPEARANCE_LABELS: [&str; 3] = [n_("Follow system"), n_("Light"), n_("Dark")];

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
    cpu: hardware::Cpu,
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
                (tray::Command::StartStop, Some(app)) => app.start_or_stop(),
                (tray::Command::StartStop, None) => {}
            }
        }
    });
}

fn machine() -> Rc<Machine> {
    MACHINE.with(|machine| {
        machine
            .get_or_init(|| {
                let cpu = hardware::cpu();
                let gpus = hardware::gpus();
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
    page: String,
}

/// Picks one reading out of a sample.
type ValueFn = Rc<dyn Fn(&Sample) -> Option<f64>>;

/// A row and the reading it shows.
struct Reading {
    row: ReadingRow,
    value: ValueFn,
}

/// Which limit a graph shows.
#[derive(Clone, Copy)]
enum LimitOf {
    Cpu,
    Gpu,
    Nearest,
}

/// A temperature graph and what it draws.
struct Graph {
    graph: HistoryGraph,
    series: Vec<(Tone, ValueFn)>,
    limit: LimitOf,
}

/// Power supply details that have no row of their own.
struct PsuLabels {
    input_volts: gtk::Label,
    temperature: gtk::Label,
    fan: gtk::Label,
    rails: Vec<gtk::Label>,
}

/// The status card at the bottom of the sidebar.
struct SidebarStatus {
    button: gtk::Button,
    icon: gtk::Image,
    title: gtk::Label,
    text: gtk::Label,
    progress: gtk::ProgressBar,
}

/// A page: (name, title, icon).
type PageInfo = (&'static str, &'static str, &'static [&'static str]);

pub struct App {
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    split: adw::NavigationSplitView,
    stack: adw::ViewStack,
    content_page: adw::NavigationPage,
    nav: gtk::ListBox,
    nav_rows: Vec<(&'static str, gtk::ListBoxRow)>,
    test_dot: gtk::Box,
    page_title: gtk::Label,
    page_subtitle: gtk::Label,
    header_start: gtk::Button,
    status: SidebarStatus,
    overview: Overview,
    /// The readings that drive the overview: graphics cards that can be used.
    usable: Vec<usize>,
    tint: Cell<Option<&'static str>>,
    test: TestControls,
    /// How the last test ended, as shown on the Load Test page.
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
    /// Controls on the overview are being set from code.
    syncing: Cell<bool>,
    /// The test's state as last shown, for the overview's subtitle.
    test_line: RefCell<Option<String>>,
    /// The nearest limit as last shown, for the status card.
    nearest: Cell<Option<f64>>,
    config: RefCell<Config>,
    machine: Rc<Machine>,
    readings: Vec<Reading>,
    graphs: Vec<Graph>,
    power_value: ValueFn,
    psu: Option<PsuLabels>,
    refresh_source: RefCell<Option<glib::SourceId>>,
}

/// A status card with a dot; returns it with its title, its text and the box
/// holding both, for anything more to go under them. Used in the preferences.
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

/// A row in the sidebar: icon and title.
fn nav_row(title: &str, icon: &[&str]) -> (gtk::ListBoxRow, gtk::Box) {
    let content = gtk::Box::builder().spacing(12).build();
    content.append(&graph::image(icon, 16));
    content.append(&gtk::Label::builder().label(title).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build());
    (gtk::ListBoxRow::builder().child(&content).build(), content)
}

/// A temperature with one decimal and the language's decimal separator.
fn degrees(value: Option<f64>) -> String {
    // Translators: the decimal separator.
    format(value, "°C", 1).replace('.', tr("."))
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
        let usable: Vec<usize> = loadable.iter().map(|(i, _, _)| *i).collect();
        let test = TestControls::new(&config, machine.cpu.threads, &loadable);
        let first_gpu = usable.first().copied();
        let has_rest = first.psu.is_some() && first.cpu_power.is_some() && usable.iter().all(|&i| first.gpus[i].power.is_some());
        let overview = Overview::new(first_gpu.map(|i| machine.gpus[i].name.as_str()), first.psu.is_some(), has_rest, config::MAX_MINUTES_RANGE);

        let mut readings = Vec::new();
        let mut graphs = Vec::new();
        let cpu_temperature: ValueFn = Rc::new(|s: &Sample| s.cpu_temperature);
        let gpu_temperature = |i: usize| -> ValueFn { Rc::new(move |s: &Sample| s.gpus[i].temperature) };

        // Overview: processor and the first graphics card.
        let mut series = vec![(Tone::Cpu, cpu_temperature.clone())];
        if let Some(i) = first_gpu {
            series.push((Tone::Gpu, gpu_temperature(i)));
        }
        graphs.push(Graph { graph: overview.history.clone(), series: series.clone(), limit: LimitOf::Nearest });
        // The Load Test page: the same, over two minutes.
        graphs.push(Graph { graph: test.graph.clone(), series, limit: LimitOf::Nearest });

        // Processor.
        let cpu_page = devices::page_box();
        let history = devices::history_card(&cpu_page, &[("band", tr("Test")), ("limit", tr("Limit"))]);
        graphs.push(Graph { graph: history, series: vec![(Tone::Cpu, cpu_temperature.clone())], limit: LimitOf::Cpu });
        let list = devices::group(&cpu_page, &machine.cpu.model, Some((graph::ICON_CPU, "cpu")), &trf("{} threads", &[&machine.cpu.threads]));
        let row = ReadingRow::new(&list, tr("Temperature"), graph::ICON_TEMPERATURE, Tone::Cpu, "°C", "");
        readings.push(Reading { row, value: cpu_temperature.clone() });
        let row = ReadingRow::new(&list, tr("Utilization"), graph::ICON_LOAD, Tone::Cpu, "%", "");
        readings.push(Reading { row, value: Rc::new(|s| s.cpu_utilization) });
        if first.cpu_power.is_some() {
            let row = ReadingRow::new(&list, tr("Power"), graph::ICON_POWER, Tone::Cpu, "W", "");
            readings.push(Reading { row, value: Rc::new(|s| s.cpu_power) });
        }

        // Graphics cards that can be used; the others get a note with the reason.
        let gpu_page = devices::page_box();
        if !usable.is_empty() {
            let mut legend = Vec::new();
            if usable.len() > 1 {
                legend.extend(usable.iter().take(1).map(|_| ("gpu", tr("Graphics card"))));
            }
            legend.push(("band", tr("Test")));
            legend.push(("limit", tr("Limit")));
            let history = devices::history_card(&gpu_page, &legend);
            let series = usable.iter().enumerate().map(|(n, &i)| (if n == 0 { Tone::Gpu } else { Tone::Neutral }, gpu_temperature(i))).collect();
            graphs.push(Graph { graph: history, series, limit: LimitOf::Gpu });
        }
        for &i in &usable {
            let gpu = &machine.gpus[i];
            let reading = &first.gpus[i];
            let tone = if Some(i) == first_gpu { Tone::Gpu } else { Tone::Neutral };
            let list = devices::group(&gpu_page, &gpu.name, Some((graph::ICON_GPU, if tone == Tone::Gpu { "gpu" } else { "neutral" })), &gpu_subtitle(gpu));
            let row = ReadingRow::new(&list, tr("Temperature"), graph::ICON_TEMPERATURE, tone, "°C", "");
            readings.push(Reading { row, value: gpu_temperature(i) });
            let row = ReadingRow::new(&list, tr("Utilization"), graph::ICON_LOAD, tone, "%", tr("the desktop counts too"));
            readings.push(Reading { row, value: Rc::new(move |s| s.gpus[i].utilization) });
            if reading.power.is_some() {
                let row = ReadingRow::new(&list, tr("Power"), graph::ICON_POWER, tone, "W", "");
                readings.push(Reading { row, value: Rc::new(move |s| s.gpus[i].power) });
            }
            if reading.fan_percent.is_some() {
                let row = ReadingRow::new(&list, tr("Fan"), graph::ICON_FAN, tone, "%", "");
                readings.push(Reading { row, value: Rc::new(move |s| s.gpus[i].fan_percent) });
            }
        }
        for gpu in machine.gpus.iter().filter(|g| g.unavailable_reason().is_some()) {
            let list = devices::group(&gpu_page, &gpu.name, Some((graph::ICON_GPU, "neutral")), &gpu.pci);
            list.append(&devices::note_row(&gpu.unavailable_reason().unwrap_or_default()));
        }
        if machine.gpus.is_empty() {
            let list = devices::group(&gpu_page, tr("Graphics card"), Some((graph::ICON_GPU, "neutral")), "");
            list.append(&devices::note_row(tr("No graphics card found")));
        }

        // Power supply, when it reports itself.
        let psu_page = devices::page_box();
        let psu = first.psu.as_ref().map(|reading| {
            let list = devices::group(&psu_page, &reading.name, Some((graph::ICON_PSU, "neutral")), "");
            let row = ReadingRow::new(&list, tr("Output to the computer"), graph::ICON_POWER, Tone::Neutral, "W", "");
            readings.push(Reading { row, value: Rc::new(|s| s.psu.as_ref().map(|p| p.output)) });
            if reading.input_estimate.is_some() {
                let row = ReadingRow::new(&list, tr("From the wall"), graph::ICON_PSU, Tone::Neutral, "W", tr("estimated"));
                row.row.set_tooltip_text(Some(tr("Calculated from the efficiency class; the power supply does not measure it.")));
                readings.push(Reading { row, value: Rc::new(|s| s.psu.as_ref().and_then(|p| p.input_estimate)) });
            }
            if has_rest {
                let row = ReadingRow::new(&list, tr("Rest of the computer"), graph::ICON_REST, Tone::Neutral, "W", "");
                row.row.set_tooltip_text(Some(tr("Motherboard, memory, disks and fans: the power supply's output minus the processor and graphics cards.")));
                let usable = usable.clone();
                readings.push(Reading { row, value: Rc::new(move |s| rest_of_computer(s, &usable)) });
            }
            let details = devices::group(&psu_page, tr("Details"), None, "");
            let input_volts = devices::detail_row(&details, tr("Input voltage"));
            let temperature = devices::detail_row(&details, tr("Temperature"));
            let fan = devices::detail_row(&details, tr("Fan"));
            let rails = reading.rails.iter().map(|rail| devices::detail_row(&details, &trf("{} rail", &[&rail.label]))).collect();
            PsuLabels { input_volts, temperature, fan, rails }
        });

        // The power card on the overview: from the wall, else the power
        // supply's output, else the processor and graphics cards together.
        let power_value: ValueFn = {
            let usable = usable.clone();
            Rc::new(move |s: &Sample| match &s.psu {
                Some(psu) => psu.input_estimate.or(Some(psu.output)),
                None => std::iter::once(s.cpu_power).chain(usable.iter().map(|&i| s.gpus[i].power)).sum(),
            })
        };

        // The pages.
        let mut pages: Vec<PageInfo> = vec![
            ("overview", tr("Overview"), graph::ICON_HOME),
            ("test", tr("Load Test"), graph::ICON_TEST),
            ("cpu", tr("Processor"), graph::ICON_CPU),
            ("gpu", if usable.len() > 1 { tr("Graphics cards") } else { tr("Graphics card") }, graph::ICON_GPU),
        ];
        if psu.is_some() {
            pages.push(("psu", tr("Power supply"), graph::ICON_PSU));
        }
        let stack = adw::ViewStack::new();
        stack.add_named(&overview.page, Some("overview"));
        stack.add_named(&test.page, Some("test"));
        stack.add_named(&devices::page(&cpu_page), Some("cpu"));
        stack.add_named(&devices::page(&gpu_page), Some("gpu"));
        if psu.is_some() {
            stack.add_named(&devices::page(&psu_page), Some("psu"));
        }
        for (name, title, _) in &pages {
            if let Some(child) = stack.child_by_name(name) {
                stack.page(&child).set_title(Some(title));
            }
        }

        // The page header: title on the left, Start/Stop and the menu on the right.
        let header = adw::HeaderBar::builder().css_classes(["stress-page-header"]).title_widget(&gtk::Box::new(gtk::Orientation::Horizontal, 0)).build();
        let titles = gtk::Box::builder().orientation(gtk::Orientation::Vertical).valign(gtk::Align::Center).margin_start(8).build();
        let page_title = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["stress-page-title"]).build();
        let page_subtitle = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["stress-page-subtitle", "dim-label"]).build();
        titles.append(&page_title);
        titles.append(&page_subtitle);
        header.pack_start(&titles);
        let menu = gio::Menu::new();
        let section = gio::Menu::new();
        section.append(Some(tr("Preferences")), Some("win.preferences"));
        section.append(Some(tr("Check for Updates…")), Some("win.check-update"));
        section.append(Some(tr("About Stress")), Some("win.about"));
        menu.append_section(None, &section);
        let quit = gio::Menu::new();
        quit.append(Some(tr("Quit")), Some("app.quit"));
        menu.append_section(None, &quit);
        let menu_button = gtk::MenuButton::builder().icon_name("open-menu-symbolic").menu_model(&menu).primary(true).valign(gtk::Align::Center).tooltip_text(tr("Main Menu")).build();
        let header_start = gtk::Button::builder().label(tr("Start")).valign(gtk::Align::Center).css_classes(["suggested-action"]).build();
        header.pack_end(&header_start);
        header.pack_end(&menu_button);
        let content = adw::ToolbarView::new();
        content.add_top_bar(&header);
        content.set_content(Some(&stack));
        let content_page = adw::NavigationPage::builder().title(tr("Overview")).tag("content").child(&content).build();

        // The sidebar.
        let sidebar = adw::ToolbarView::builder().css_classes(["stress-sidebar"]).build();
        sidebar.add_top_bar(&adw::HeaderBar::builder().css_classes(["stress-page-header"]).title_widget(&gtk::Box::new(gtk::Orientation::Horizontal, 0)).build());
        let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).margin_start(10).margin_end(10).margin_bottom(12).build();
        // The app's name under the window buttons, not beside them.
        let brand = gtk::Box::builder().spacing(10).margin_start(6).margin_bottom(16).build();
        brand.append(&gtk::Image::builder().icon_name(APP_ID).pixel_size(36).build());
        let names = gtk::Box::builder().orientation(gtk::Orientation::Vertical).valign(gtk::Align::Center).build();
        names.append(&gtk::Label::builder().label("Stress").xalign(0.0).css_classes(["heading"]).build());
        names.append(&gtk::Label::builder().label(tr("CPU and GPU stress test")).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["caption", "dim-label"]).build());
        brand.append(&names);
        column.append(&brand);
        let nav = gtk::ListBox::builder().css_classes(["navigation-sidebar", "stress-nav"]).build();
        let hardware_nav = gtk::ListBox::builder().css_classes(["navigation-sidebar", "stress-nav"]).build();
        let mut nav_rows = Vec::new();
        let mut test_dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        for (name, title, icon) in &pages {
            let (row, content) = nav_row(title, icon);
            if *name == "test" {
                test_dot = gtk::Box::builder().valign(gtk::Align::Center).visible(false).css_classes(["stress-nav-dot"]).build();
                content.append(&test_dot);
            }
            if matches!(*name, "overview" | "test") {
                nav.append(&row);
            } else {
                hardware_nav.append(&row);
            }
            nav_rows.push((*name, row));
        }
        column.append(&nav);
        column.append(&gtk::Label::builder().label(tr("Hardware")).xalign(0.0).margin_start(6).margin_top(14).margin_bottom(4).css_classes(["caption-heading", "dim-label"]).build());
        column.append(&hardware_nav);
        column.append(&gtk::Box::builder().vexpand(true).build());
        let status_button = gtk::Button::builder().css_classes(["stress-status"]).margin_bottom(6).build();
        let status_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).build();
        let status_top = gtk::Box::builder().spacing(10).build();
        let status_icon = gtk::Image::builder().icon_name("emblem-ok-symbolic").valign(gtk::Align::Center).css_classes(["stress-status-icon"]).build();
        status_top.append(&status_icon);
        let status_texts = gtk::Box::builder().orientation(gtk::Orientation::Vertical).valign(gtk::Align::Center).build();
        let status_title = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["stress-status-title"]).build();
        let status_text = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["caption", "dim-label", "stress-sub"]).build();
        status_texts.append(&status_title);
        status_texts.append(&status_text);
        status_top.append(&status_texts);
        status_box.append(&status_top);
        let status_progress = gtk::ProgressBar::builder().css_classes(["stress-progress", "thin"]).visible(false).build();
        status_box.append(&status_progress);
        status_button.set_child(Some(&status_box));
        column.append(&status_button);
        let settings = gtk::ListBox::builder().css_classes(["navigation-sidebar", "stress-nav"]).selection_mode(gtk::SelectionMode::None).build();
        settings.append(&nav_row(tr("Preferences"), &["emblem-system-symbolic"]).0);
        column.append(&settings);
        sidebar.set_content(Some(&column));
        let status = SidebarStatus { button: status_button, icon: status_icon, title: status_title, text: status_text, progress: status_progress };

        // Sidebar on the left, the pages on the right; one at a time in narrow windows.
        let split = adw::NavigationSplitView::builder().min_sidebar_width(232.0).max_sidebar_width(232.0).css_classes(["stress-main"]).build();
        split.set_sidebar(Some(&adw::NavigationPage::builder().title("Stress").tag("sidebar").child(&sidebar).build()));
        split.set_content(Some(&content_page));
        let toasts = adw::ToastOverlay::new();
        toasts.add_css_class("stress-root");
        toasts.set_child(Some(&split));
        window.set_content(Some(&toasts));
        style::follow_style(&toasts);

        // Medium wide: the overview's cards two and two, the bottom cards and
        // the test page's columns stacked; narrow: also sidebar and page one
        // at a time. Only the last matching breakpoint applies, so the narrow
        // one has all the setters.
        for (width, collapse) in [(1100, false), (700, true)] {
            let breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::parse(&format!("max-width: {width}sp")).unwrap());
            breakpoint.add_setter(&overview.cards, "orientation", Some(&gtk::Orientation::Vertical.to_value()));
            breakpoint.add_setter(&overview.bottom, "orientation", Some(&gtk::Orientation::Vertical.to_value()));
            breakpoint.add_setter(&overview.bottom, "homogeneous", Some(&false.to_value()));
            breakpoint.add_setter(&test.columns, "orientation", Some(&gtk::Orientation::Vertical.to_value()));
            breakpoint.add_setter(&test.columns, "homogeneous", Some(&false.to_value()));
            if collapse {
                breakpoint.add_setter(&split, "collapsed", Some(&true.to_value()));
            }
            window.add_breakpoint(breakpoint);
        }

        let app = Rc::new(App {
            window,
            toasts,
            split,
            stack,
            content_page,
            nav,
            nav_rows,
            test_dot,
            page_title,
            page_subtitle,
            header_start,
            status,
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
            syncing: Cell::new(false),
            test_line: RefCell::new(None),
            nearest: Cell::new(None),
            config: RefCell::new(config),
            machine,
            readings,
            graphs,
            power_value,
            psu,
            refresh_source: RefCell::new(None),
        });
        CURRENT.with(|current| *current.borrow_mut() = Rc::downgrade(&app));
        if let Some(offer) = app.rapl_offer() {
            // Under the processor's readings.
            offer.add_css_class("stress-list");
            cpu_page.append(&offer);
        }
        app.connect_navigation(&hardware_nav, &settings);
        app.connect_overview();
        app.install_actions();
        app.build_test_panel();
        app.show_page(carry.as_ref().map_or("overview", |c| c.page.as_str()));
        app.tick();
        // The window is not shown yet, so the first readings are put in by hand.
        app.show_readings();

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

    /// The sidebar's rows, its status card, and the header's Start/Stop.
    fn connect_navigation(self: &Rc<Self>, hardware_nav: &gtk::ListBox, settings: &gtk::ListBox) {
        // The two lists act as one: picking in one clears the other.
        for (list, other) in [(&self.nav, hardware_nav), (hardware_nav, &self.nav)] {
            let weak = Rc::downgrade(self);
            let other = other.clone();
            list.connect_row_activated(move |_, row| {
                let Some(app) = weak.upgrade() else { return };
                other.unselect_all();
                if let Some((name, _)) = app.nav_rows.iter().find(|(_, r)| r == row) {
                    app.show_page(name);
                }
            });
        }
        let weak = Rc::downgrade(self);
        settings.connect_row_activated(move |_, _| {
            if let Some(app) = weak.upgrade() {
                app.show_preferences();
            }
        });
        let weak = Rc::downgrade(self);
        self.status.button.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.show_page("test");
            }
        });
        let weak = Rc::downgrade(self);
        self.header_start.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.start_or_stop();
            }
        });
    }

    /// The overview's links and controls.
    fn connect_overview(self: &Rc<Self>) {
        let o = &self.overview;
        let weak = Rc::downgrade(self);
        o.power_link.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.show_page("psu");
            }
        });
        let weak = Rc::downgrade(self);
        o.safety_link.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.show_preferences().set_visible_page_name("safety");
            }
        });
        let weak = Rc::downgrade(self);
        o.test_click.connect_released(move |_, _, _, _| {
            if let Some(app) = weak.upgrade() {
                app.show_page("test");
            }
        });
        o.longest.set_value(self.config.borrow().max_minutes);
        o.battery.set_active(self.config.borrow().stop_on_battery);
        let weak = Rc::downgrade(self);
        o.longest.connect_value_changed(move |button| {
            let Some(app) = weak.upgrade().filter(|app| !app.syncing.get()) else { return };
            app.config.borrow_mut().max_minutes = button.value();
            app.config.borrow().save();
            app.update_test_status();
        });
        let weak = Rc::downgrade(self);
        o.battery.connect_active_notify(move |switch| {
            let Some(app) = weak.upgrade().filter(|app| !app.syncing.get()) else { return };
            app.config.borrow_mut().stop_on_battery = switch.is_active();
            app.config.borrow().save();
        });
    }

    /// Shows a page, marks it in the sidebar and fills in the header.
    fn show_page(&self, name: &str) {
        let Some(child) = self.stack.child_by_name(name) else { return };
        self.stack.set_visible_child(&child);
        let title = self.stack.page(&child).title().map(|t| t.to_string()).unwrap_or_default();
        self.page_title.set_label(&title);
        self.content_page.set_title(&title);
        for (row_name, row) in &self.nav_rows {
            if let Some(list) = row.parent().and_downcast::<gtk::ListBox>() {
                if *row_name == name {
                    list.select_row(Some(row));
                } else if list.selected_row().as_ref() == Some(row) {
                    list.unselect_all();
                }
            }
        }
        // The Load Test page has its own large button.
        self.header_start.set_visible(name != "test");
        self.split.set_show_content(true);
        self.update_subtitle();
    }

    fn visible_page(&self) -> String {
        self.stack.visible_child_name().map(|n| n.to_string()).unwrap_or_default()
    }

    fn update_subtitle(&self) {
        let machine = &self.machine;
        let text = match self.visible_page().as_str() {
            "overview" => self.test_line.borrow().clone().unwrap_or_else(|| tr("Temperatures and power right now").to_owned()),
            "test" => tr("Choose a load; the test stops by itself at a limit").to_owned(),
            "cpu" => tr("Temperature, utilization and power").to_owned(),
            "gpu" if machine.gpus.is_empty() => tr("No graphics card found").to_owned(),
            "gpu" => tr("Temperature, utilization, power and fan").to_owned(),
            "psu" => tr("The power supply updates its readings every few seconds.").to_owned(),
            _ => String::new(),
        };
        self.page_subtitle.set_label(&text);
    }

    /// Development aid: render the window to a PNG and quit, since GNOME on
    /// Wayland does not let other programs capture it.
    ///
    /// `tour` idles for 30 seconds, starts a test with the saved setup and
    /// captures it after the delay; a page name (`test`, `cpu`, `gpu`, `psu`)
    /// shows that page.
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
            let mut dialog: Option<adw::Dialog> = None;
            match page.as_deref() {
                Some("about") => dialog = Some(app.show_about().upcast()),
                Some("panel") => app.show_page("test"),
                Some(name @ ("overview" | "test" | "cpu" | "gpu" | "psu")) => app.show_page(name),
                Some(other) => {
                    if let Some(prefs_page) = other.strip_prefix("preferences") {
                        let preferences = app.show_preferences();
                        if let Some(name) = prefs_page.strip_prefix('-') {
                            preferences.set_visible_page_name(name);
                        }
                        dialog = Some(preferences.upcast());
                    }
                }
                None => {}
            }
            let window = app.window.clone();
            let weak = Rc::downgrade(&app);
            glib::timeout_add_local_once(Duration::from_millis(1500), move || {
                capture(&window, &path);
                // A window with a dialog open closes the dialog instead of itself.
                if let Some(dialog) = dialog {
                    dialog.force_close();
                }
                if let Some(app) = weak.upgrade() {
                    app.force_close();
                }
            });
        });
    }

    fn tick(&self) {
        self.update_test_status();
        self.sync_tray();
        // In the status icon there is nothing to draw.
        if self.window.is_visible() {
            self.show_readings();
        }
    }

    /// The limits that apply now, as (processor, graphics card): a running
    /// test may have stricter ones (set by Claude), and Vifte's emergency
    /// temperature lowers both while it runs.
    fn limits(&self, vifte: Option<&VifteState>) -> (f64, f64) {
        let vifte_limit = vifte.and_then(VifteState::limit).unwrap_or(f64::INFINITY);
        let config = self.config.borrow();
        let (cpu, gpu) = match self.machine.controller.status() {
            crate::loadtest::Status::Running { limits, .. } => (limits.cpu, limits.gpu),
            _ => (config.cpu_limit, config.gpu_limit),
        };
        (cpu.min(vifte_limit), gpu.min(vifte_limit))
    }

    fn show_readings(&self) {
        let history = &self.machine.history;
        let tests = history.test_periods();
        for reading in &self.readings {
            reading.row.set(&history.series(&*reading.value));
        }
        let vifte = self.vifte_state();
        let (cpu_limit, gpu_limit) = self.limits(vifte.as_ref());
        let latest = history.latest();
        let mut temperatures = vec![(latest.as_ref().and_then(|s| s.cpu_temperature), cpu_limit)];
        temperatures.extend(self.usable.iter().map(|&i| (latest.as_ref().and_then(|s| s.gpus[i].temperature), gpu_limit)));
        let nearest = look::nearest_limit(&temperatures);
        for graph in &self.graphs {
            let series = graph.series.iter().map(|(tone, value)| (*tone, history.series(&**value))).collect();
            let limit = match graph.limit {
                LimitOf::Cpu => cpu_limit,
                LimitOf::Gpu => gpu_limit,
                LimitOf::Nearest => nearest.map_or(cpu_limit.min(gpu_limit), |(limit, _)| limit),
            };
            graph.graph.set(series, tests.clone(), limit.is_finite().then_some(limit));
        }
        if let (Some(labels), Some(reading)) = (&self.psu, latest.as_ref().and_then(|s| s.psu.as_ref())) {
            show_psu(labels, reading);
        }
        if let Some(sample) = latest {
            self.show_overview(&sample, &temperatures, vifte.as_ref());
        }
    }

    /// The overview's cards, the window's tint and the live numbers on the
    /// Load Test page.
    fn show_overview(&self, sample: &Sample, temperatures: &[(Option<f64>, f64)], vifte: Option<&VifteState>) {
        let o = &self.overview;
        let history = &self.machine.history;
        o.cpu.set(&degrees(sample.cpu_temperature), sample.cpu_temperature, &self.machine.cpu.model);
        o.cpu.spark.set(&history.series(|s| s.cpu_temperature));
        let first_gpu = self.usable.first().copied();
        if let (Some(tile), Some(i)) = (&o.gpu, first_gpu) {
            let t = sample.gpus[i].temperature;
            tile.set(&degrees(t), t, &self.machine.gpus[i].name);
            tile.spark.set(&history.series(|s| s.gpus[i].temperature));
        }

        // Power.
        match &sample.psu {
            Some(psu) => match psu.input_estimate {
                Some(wall) => {
                    o.power.title.set_label(tr("From the wall"));
                    o.power.set(&format!("≈ {}", format(Some(wall), "W", 0)), None, &trf("Output {}", &[&format(Some(psu.output), "W", 0)]));
                    o.power.widget.set_tooltip_text(Some(tr("Calculated from the efficiency class; the power supply does not measure it.")));
                }
                None => {
                    o.power.title.set_label(tr("Output to the computer"));
                    o.power.set(&format(Some(psu.output), "W", 0), None, &psu.name);
                }
            },
            None => {
                o.power.title.set_label(tr("Power"));
                o.power.set(&format((self.power_value)(sample), "W", 0), None, tr("Processor and graphics cards"));
            }
        }
        o.power.spark.set(&history.series(&*self.power_value));
        let gpu_power = first_gpu.and_then(|i| sample.gpus[i].power);
        let rest = rest_of_computer(sample, &self.usable);
        let total = match &sample.psu {
            Some(psu) => Some(psu.output),
            None => std::iter::once(sample.cpu_power).chain(self.usable.iter().map(|&i| sample.gpus[i].power)).sum(),
        };
        o.power_total.set(&format(total, "W", 0));
        o.power_total_sub.set_label(&match &sample.psu {
            Some(psu) => match psu.input_estimate {
                Some(wall) => trf("to the computer · ≈ {} from the wall", &[&format(Some(wall), "W", 0)]),
                None => tr("to the computer").to_owned(),
            },
            None => tr("Processor and graphics cards").to_owned(),
        });
        let share = |part: Option<f64>| match (part, total) {
            (Some(p), Some(t)) if t > 0.0 => (p / t).clamp(0.0, 1.0),
            _ => 0.0,
        };
        let row = &o.power_cpu;
        row.value.set_label(&format(sample.cpu_power, "W", 0));
        row.bar.set_value(share(sample.cpu_power));
        row.sub.set_label(&trf("{} in use", &[&format(sample.cpu_utilization, "%", 0)]));
        if let (Some(row), Some(i)) = (&o.power_gpu, first_gpu) {
            row.value.set_label(&format(gpu_power, "W", 0));
            row.bar.set_value(share(gpu_power));
            row.sub.set_label(&trf("{} in use", &[&format(sample.gpus[i].utilization, "%", 0)]));
        }
        if let Some(row) = &o.power_rest {
            row.value.set_label(&format(rest, "W", 0));
            row.bar.set_value(share(rest));
        }

        // Protection.
        let idle = matches!(self.machine.controller.status(), crate::loadtest::Status::Idle);
        let limit_reached = idle
            && self.last_test.borrow().as_ref().is_some_and(|(_, f)| matches!(f.reason, StopReason::CpuLimit(_) | StopReason::GpuLimit(_)));
        let vifte_problem = vifte.is_some_and(|v| v.emergency_active || v.sensor_lost || v.paused);
        let (title, level) = if limit_reached {
            (tr("Limit reached"), "emergency")
        } else if vifte_problem {
            (tr("Check Vifte"), "warm")
        } else {
            (tr("All good"), "calm")
        };
        o.safety_title.set_label(title);
        o.safety_icon.set_css_classes(&["stress-status-icon", level]);
        let nearest = look::nearest_limit(temperatures);
        self.nearest.set(nearest.map(|(limit, _)| limit));
        o.safety_sub.set_label(&match nearest {
            Some((limit, left)) => [trf("Stops at {}", &[&format(Some(limit), "°C", 0)]), trf("{} left", &[&format(Some(left), "°C", 0)])].join(" · "),
            None => String::new(),
        });
        match vifte {
            Some(state) => {
                o.vifte_sub.set_label(&match state.limit() {
                    Some(limit) => trf("Running. Tests also stop at {} °C, 5 °C below Vifte's emergency temperature, and whenever Vifte reports a problem.", &[&limit]),
                    None => tr("Running. Tests stop whenever Vifte reports a problem.").to_string(),
                });
                o.vifte_icon.set_visible(true);
            }
            None => {
                o.vifte_sub.set_label(tr("Not running. When Vifte controls the fans, tests also respect its emergency temperature."));
                o.vifte_icon.set_visible(false);
            }
        }
        {
            let config = self.config.borrow();
            self.syncing.set(true);
            if o.longest.value() != config.max_minutes && !o.longest.has_focus() {
                o.longest.set_value(config.max_minutes);
            }
            o.battery.set_active(config.stop_on_battery);
            self.syncing.set(false);
        }

        // The Load Test page's live numbers.
        let t = &self.test;
        t.cpu_temperature.set_label(&format!("{} {}", tr("Processor"), degrees(sample.cpu_temperature)));
        t.cpu_now.set_label(&trf("{} now", &[&format(sample.cpu_utilization, "%", 0)]));
        if let Some(i) = first_gpu {
            t.gpu_temperature.set_label(&format!("{} {}", tr("Graphics card"), degrees(sample.gpus[i].temperature)));
            t.gpu_now.set_label(&trf("{} now", &[&format(sample.gpus[i].utilization, "%", 0)]));
        }

        let hottest = temperatures.iter().filter_map(|(t, _)| *t).reduce(f64::max);
        let tint = self.config.borrow().temperature_tint;
        self.set_tint(tint.then_some(hottest).flatten());
    }

    /// Shows the test's state in the sidebar, the header, the overview and
    /// the status icon's subtitle.
    fn show_test_view(&self, view: &TestView) {
        let s = &self.status;
        let (icon, title, text) = match view.level {
            "running" | "claude" => ("media-playback-start-symbolic", view.title.clone(), format!("{} · {}", view.text, self.test.profile_label())),
            "stopped" => ("dialog-warning-symbolic", view.title.clone(), view.text.clone()),
            _ => (
                "emblem-ok-symbolic",
                view.title.clone(),
                self.nearest.get().map_or_else(String::new, |limit| trf("Stops at {}", &[&format(Some(limit), "°C", 0)])),
            ),
        };
        s.icon.set_icon_name(Some(icon));
        s.icon.set_css_classes(&["stress-status-icon", if view.level == "idle" { "calm" } else { view.level }]);
        s.title.set_label(&title);
        s.text.set_label(&text);
        s.text.set_visible(!text.is_empty());
        s.progress.set_visible(view.running);
        s.progress.set_fraction(view.elapsed.as_secs_f64() / view.total.as_secs_f64().max(1.0));
        self.test_dot.set_visible(view.running);

        self.header_start.set_label(if view.running { tr("Stop") } else { tr("Start") });
        self.header_start.set_css_classes(if view.running { &["destructive-action", "stress-stop"] } else { &["suggested-action"] });

        let tile = &self.overview.test;
        tile.chip.widget.set_visible(view.level != "idle");
        tile.chip.set(view.level, if view.running { tr("Running") } else { tr("Stopped") });
        tile.progress.set_visible(view.running);
        tile.progress.set_fraction(view.elapsed.as_secs_f64() / view.total.as_secs_f64().max(1.0));
        if view.running {
            tile.value.set_parts(&test::clock(view.elapsed), &trf("of {}", &[&test::clock(view.total)]));
            tile.sub.set_label(&view.setup);
        } else if view.level == "stopped" {
            tile.value.set_parts(tr("Stopped"), "");
            tile.sub.set_label(&view.text);
        } else {
            tile.value.set_parts(tr("Ready"), "");
            tile.sub.set_label(&view.setup);
        }
        tile.sub.set_tooltip_text(Some(&tile.sub.label()));

        let line = view.running.then(|| format!("{} · {}", view.title, view.text));
        if *self.test_line.borrow() != line {
            *self.test_line.borrow_mut() = line;
            self.update_subtitle();
        }
    }

    /// Vifte's state as last read in the background (at most a few seconds old).
    pub(crate) fn vifte_state(&self) -> Option<VifteState> {
        self.machine.vifte.lock().ok().and_then(|state| state.clone())
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

    fn show_about(&self) -> adw::AboutDialog {
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
        about
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
            .active(self.config.borrow().temperature_tint)
            .build();
        look.add(&tint);
        page.add(&look);
        let weak = Rc::downgrade(self);
        tint.connect_active_notify(move |row| {
            let Some(app) = weak.upgrade() else { return };
            app.config.borrow_mut().temperature_tint = row.is_active();
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
        let vifte_limit = self.vifte_state().as_ref().and_then(VifteState::limit);
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
        match self.vifte_state() {
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
            page: self.visible_page(),
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
