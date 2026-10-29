//! Main window: the load test panel, then a section per device (processor,
//! each graphics card, the power supply) with reading cards that graph the
//! last 30 minutes, plus preferences, about and uninstalling.

pub mod claude;
mod graph;
mod helpers;
mod test;

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::config::{self, Appearance, Config};
use crate::devices::{self, Gpu};
use crate::history::History;
use crate::i18n::{self, n_, tr, trf};
use crate::loadtest::Controller;
use crate::sensors::{format, PsuReading, Sample, Sensors};
use crate::APP_ID;
use graph::{Card, Scale};
use test::TestControls;

pub use claude::{register as register_claude, unregister_from_claude_code};

const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const APPEARANCES: [Appearance; 3] = [Appearance::System, Appearance::Light, Appearance::Dark];
const APPEARANCE_LABELS: [&str; 3] = [n_("Follow system"), n_("Light"), n_("Dark")];

const CSS: &str = r#"
.reading { font-feature-settings: "tnum"; }
.reading-card { padding: 12px 14px; }
.reading-value { font-size: 1.8em; font-weight: 700; font-feature-settings: "tnum"; }
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
}

thread_local! {
    /// The open window, so a second launch brings it forward instead of
    /// opening another one.
    static CURRENT: RefCell<std::rc::Weak<App>> = const { RefCell::new(std::rc::Weak::new()) };
    static MACHINE: OnceCell<Rc<Machine>> = const { OnceCell::new() };
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
                Rc::new(Machine { cpu, gpus, history, controller, first })
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
    test: TestControls,
    /// How the last test ended, as shown under the panel.
    last_test: RefCell<Option<(String, crate::loadtest::Finished)>>,
    /// The running test was started by Claude.
    claude_started: Cell<bool>,
    /// The window is being replaced for a language change; a running test
    /// carries on.
    rebuilding: Cell<bool>,
    config: RefCell<Config>,
    machine: Rc<Machine>,
    readings: Vec<Reading>,
    psu: Option<PsuLabels>,
    refresh_source: RefCell<Option<glib::SourceId>>,
}

fn reading_label() -> gtk::Label {
    gtk::Label::builder()
        .valign(gtk::Align::Center)
        .css_classes(["reading", "dim-label"])
        .build()
}

/// A row with a value on the right, as used for the power supply details.
fn value_row(title: &str) -> (adw::ActionRow, gtk::Label) {
    let row = adw::ActionRow::builder().title(title).build();
    let label = reading_label();
    row.add_suffix(&label);
    (row, label)
}

/// A heading for one device, followed by a grid for its cards.
fn section(content: &gtk::Box, title: &str, subtitle: &str) -> gtk::FlowBox {
    let heading = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).margin_top(12).build();
    heading.append(&gtk::Label::builder().label(title).halign(gtk::Align::Start).wrap(true).css_classes(["title-4"]).build());
    if !subtitle.is_empty() {
        heading.append(
            &gtk::Label::builder()
                .label(subtitle)
                .halign(gtk::Align::Start)
                .wrap(true)
                .css_classes(["dim-label"])
                .build(),
        );
    }
    content.append(&heading);
    let grid = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(1)
        .max_children_per_line(4)
        .column_spacing(12)
        .row_spacing(12)
        .build();
    content.append(&grid);
    grid
}

impl App {
    fn new(application: &adw::Application, carry: Option<Carry>) -> Rc<Self> {
        let config = Config::load();
        let machine = machine();
        let (width, height) = carry.as_ref().map_or((1040, 820), |c| c.size);
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
        menu.append(Some(tr("About Stress")), Some("win.about"));
        let quit = gio::Menu::new();
        quit.append(Some(tr("Quit")), Some("app.quit"));
        menu.append_section(None, &quit);
        header.pack_end(
            &gtk::MenuButton::builder()
                .icon_name("open-menu-symbolic")
                .menu_model(&menu)
                .primary(true)
                .tooltip_text(tr("Main Menu"))
                .build(),
        );

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(24)
            .margin_start(12)
            .margin_end(12)
            .build();
        let mut readings = Vec::new();
        let mut add = |grid: &gtk::FlowBox, card: Card, value: ValueFn| {
            grid.append(&card.widget);
            if let Some(child) = card.widget.parent() {
                child.set_focusable(false);
            }
            readings.push(Reading { card, value });
        };
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
        content.append(&test.group);

        // Processor.
        let cpu = section(&content, &machine.cpu.model, &trf("{} threads", &[&machine.cpu.threads]));
        add(&cpu, Card::new(tr("Temperature"), "°C", 0, Scale::Temperature, ""), Box::new(|s| s.cpu_temperature));
        add(&cpu, Card::new(tr("Utilization"), "%", 0, Scale::Percent, ""), Box::new(|s| s.cpu_utilization));
        if first.cpu_power.is_some() {
            add(&cpu, Card::new(tr("Power"), "W", 0, Scale::Power, ""), Box::new(|s| s.cpu_power));
        }
        let rapl_offer = first.cpu_power.is_none().then_some(&content);

        // Graphics cards that can be used; the others are listed with the reason.
        let usable: Vec<usize> =
            (0..machine.gpus.len()).filter(|&i| machine.gpus[i].unavailable_reason().is_none()).collect();
        for &i in &usable {
            let gpu = &machine.gpus[i];
            let grid = section(&content, &gpu.name, &gpu_subtitle(gpu));
            let reading = &first.gpus[i];
            add(&grid, Card::new(tr("Temperature"), "°C", 0, Scale::Temperature, ""), Box::new(move |s| s.gpus[i].temperature));
            add(&grid, Card::new(tr("Utilization"), "%", 0, Scale::Percent, ""), Box::new(move |s| s.gpus[i].utilization));
            if reading.power.is_some() {
                add(&grid, Card::new(tr("Power"), "W", 0, Scale::Power, ""), Box::new(move |s| s.gpus[i].power));
            }
            if reading.fan_percent.is_some() {
                add(&grid, Card::new(tr("Fan"), "%", 0, Scale::Percent, ""), Box::new(move |s| s.gpus[i].fan_percent));
            }
        }
        let unusable: Vec<&Gpu> = machine.gpus.iter().filter(|g| g.unavailable_reason().is_some()).collect();
        if !unusable.is_empty() || machine.gpus.is_empty() {
            let group = adw::PreferencesGroup::builder()
                .title(if usable.is_empty() { tr("Graphics Cards") } else { tr("Other Graphics Cards") })
                .margin_top(12)
                .build();
            for gpu in &unusable {
                group.add(&adw::ActionRow::builder().title(&gpu.name).subtitle(gpu_subtitle(gpu)).css_classes(["dim-label"]).build());
            }
            if machine.gpus.is_empty() {
                group.add(&adw::ActionRow::builder().title(tr("No graphics card found")).build());
            }
            content.append(&group);
        }

        // Power supply, when it reports itself.
        let psu = first.psu.as_ref().map(|reading| {
            let grid = section(&content, &reading.name, tr("The power supply updates its readings every few seconds."));
            add(&grid, Card::new(tr("Output to the computer"), "W", 0, Scale::Power, ""), Box::new(|s| s.psu.as_ref().map(|p| p.output)));
            if reading.input_estimate.is_some() {
                let card = Card::new(tr("From the wall"), "W", 0, Scale::Power, "≈ ");
                card.widget.set_tooltip_text(Some(tr("Calculated from the efficiency class; the power supply does not measure it.")));
                add(&grid, card, Box::new(|s| s.psu.as_ref().and_then(|p| p.input_estimate)));
            }
            if first.cpu_power.is_some() && usable.iter().all(|&i| first.gpus[i].power.is_some()) {
                let card = Card::new(tr("Rest of the computer"), "W", 0, Scale::Power, "");
                card.widget.set_tooltip_text(Some(tr("Motherboard, memory, disks and fans: the power supply's output minus the processor and graphics cards.")));
                let usable = usable.clone();
                add(&grid, card, Box::new(move |s| rest_of_computer(s, &usable)));
            }
            let details = adw::ExpanderRow::builder().title(tr("Details")).build();
            let (volts_row, input_volts) = value_row(tr("Input voltage"));
            let (temp_row, temperature) = value_row(tr("Temperature"));
            let (fan_row, fan) = value_row(tr("Fan"));
            for row in [&volts_row, &temp_row, &fan_row] {
                details.add_row(row);
            }
            let rails = reading
                .rails
                .iter()
                .map(|rail| {
                    let (row, label) = value_row(&trf("{} rail", &[&rail.label]));
                    details.add_row(&row);
                    label
                })
                .collect();
            let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
            list.append(&details);
            content.append(&list);
            PsuLabels { input_volts, temperature, fan, rails }
        });

        let clamp = adw::Clamp::builder().maximum_size(1200).child(&content).build();
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&clamp)
            .vexpand(true)
            .build();
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&scroll));
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&toasts));
        window.set_content(Some(&view));

        let app = Rc::new(App {
            window,
            title,
            toasts,
            test,
            last_test: RefCell::new(None),
            claude_started: Cell::new(false),
            rebuilding: Cell::new(false),
            config: RefCell::new(config),
            machine,
            readings,
            psu,
            refresh_source: RefCell::new(None),
        });
        CURRENT.with(|current| *current.borrow_mut() = Rc::downgrade(&app));
        if let (Some(content), Some(offer)) = (rapl_offer, app.rapl_offer()) {
            // Right below the processor's cards.
            content.insert_child_after(&offer, content.observe_children().item(2).and_downcast_ref::<gtk::Widget>());
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
        app.window.connect_close_request(move |_| {
            let Some(app) = weak.upgrade() else { return glib::Propagation::Proceed };
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
    fn schedule_screenshot(self: &Rc<Self>, path: String, page: Option<String>) {
        let weak = Rc::downgrade(self);
        let delay = std::env::var("STRESS_SCREENSHOT_DELAY").ok().and_then(|d| d.parse().ok()).unwrap_or(3);
        glib::timeout_add_local_once(Duration::from_secs(delay), move || {
            let Some(app) = weak.upgrade() else { return };
            if page.as_deref() == Some("about") {
                app.show_about();
            } else if let Some(prefs_page) = page.as_deref().and_then(|p| p.strip_prefix("preferences")) {
                let dialog = app.show_preferences();
                if let Some(name) = prefs_page.strip_prefix('-') {
                    dialog.set_visible_page_name(name);
                }
            }
            let window = app.window.clone();
            glib::timeout_add_local_once(Duration::from_millis(1500), move || {
                let paintable = gtk::WidgetPaintable::new(Some(&window));
                let snapshot = gtk::Snapshot::new();
                paintable.snapshot(&snapshot, window.width() as f64, window.height() as f64);
                match (snapshot.to_node(), window.renderer()) {
                    (Some(node), Some(renderer)) => {
                        if let Err(err) = renderer.render_texture(node, None).save_to_png(&path) {
                            eprintln!("screenshot: {err}");
                        }
                    }
                    (node, renderer) => {
                        eprintln!("screenshot: node={} renderer={}", node.is_some(), renderer.is_some())
                    }
                }
                window.close();
            });
        });
    }

    fn tick(&self) {
        self.update_test_status();
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
    }

    fn toast(&self, message: &str) {
        self.toasts.add_toast(adw::Toast::new(message));
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
        self.window.add_action_entries([preferences, about]);
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
        page.add(&look);

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
        match crate::vifte::state() {
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

        let save = |app: &Rc<App>, change: &dyn Fn(&mut Config)| {
            change(&mut app.config.borrow_mut());
            app.config.borrow().save();
        };
        let weak = Rc::downgrade(self);
        cpu_limit.connect_value_notify(move |row| {
            if let Some(app) = weak.upgrade() {
                save(&app, &|c| c.cpu_limit = row.value());
            }
        });
        let weak = Rc::downgrade(self);
        gpu_limit.connect_value_notify(move |row| {
            if let Some(app) = weak.upgrade() {
                save(&app, &|c| c.gpu_limit = row.value());
            }
        });
        let weak = Rc::downgrade(self);
        max_minutes.connect_value_notify(move |row| {
            if let Some(app) = weak.upgrade() {
                save(&app, &|c| c.max_minutes = row.value());
            }
        });
        let weak = Rc::downgrade(self);
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
        let window = self.window.clone();
        done.connect_response(None, move |_, _| window.close());
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
        label.set_label(&[format(rail.volts, "V", 2), format(rail.amps, "A", 1), format(rail.watts, "W", 0)].join(" · "));
    }
}
