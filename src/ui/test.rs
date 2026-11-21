//! The Load Test page: the test's status with a large clock, Start/Stop and
//! the temperature of the last two minutes, the profiles, processor threads
//! and load, graphics card and its load, and duration.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;

use super::devices;
use super::graph::{self, BigValue, Chip, HistoryGraph};
use super::App;
use crate::i18n::{n_, tr, trf};
use crate::loadtest::{Finished, Levels, Limits, Status, StopReason};

/// Profiles as (name, processor load, graphics card load); all threads.
const PROFILES: [(&str, u32, u32); 5] = [
    (n_("Full load"), 100, 100),
    (n_("Processor only"), 100, 0),
    (n_("Graphics card only"), 0, 100),
    (n_("Gaming"), 40, 95),
    (n_("Light load"), 25, 25),
];

/// Offered test lengths in minutes; 0 runs until stopped.
const DURATIONS: [u32; 7] = [1, 5, 10, 15, 30, 60, 0];

pub struct TestControls {
    pub page: gtk::ScrolledWindow,
    /// The two columns; a breakpoint stacks them.
    pub columns: gtk::Box,
    clock: BigValue,
    chip: Chip,
    hero_subtitle: gtk::Label,
    pub graph: HistoryGraph,
    pub cpu_now: gtk::Label,
    pub gpu_now: gtk::Label,
    pub cpu_temperature: gtk::Label,
    pub gpu_temperature: gtk::Label,
    profiles: Vec<gtk::ToggleButton>,
    /// "Custom" when no profile matches.
    profile_name: gtk::Label,
    threads: adw::SpinRow,
    cpu_load: gtk::Scale,
    gpu: Option<adw::ComboRow>,
    gpu_load: gtk::Scale,
    gpu_load_row: adw::ActionRow,
    duration: adw::ComboRow,
    start: gtk::Button,
    progress: gtk::ProgressBar,
    /// GPU list indices offered in the picker, in its order.
    usable: Vec<usize>,
    /// Set while controls are changed from code, so it is not taken as the
    /// user picking a custom setup.
    updating: Cell<bool>,
}

fn scale_row(title: &str) -> (adw::ActionRow, gtk::Scale) {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    scale.set_draw_value(true);
    scale.set_value_pos(gtk::PositionType::Right);
    scale.set_format_value_func(|_, v| format!("{v:.0} %"));
    scale.set_width_request(180);
    scale.set_hexpand(true);
    scale.set_valign(gtk::Align::Center);
    let row = adw::ActionRow::builder().title(title).build();
    row.add_suffix(&scale);
    (row, scale)
}

/// The profile that matches a setup, if any.
fn matching_profile(all_threads: bool, cpu_load: u32, gpu_load: u32) -> Option<usize> {
    PROFILES.iter().position(|&(_, cpu, gpu)| all_threads && cpu == cpu_load && gpu == gpu_load)
}

fn duration_label(minutes: u32, longest: f64) -> String {
    match minutes {
        0 => trf("Until stopped (at most {} min)", &[&longest]),
        1 => tr("1 minute").to_string(),
        m => trf("{} minutes", &[&m]),
    }
}

/// "2:05", or "1:02:05" from an hour.
pub fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

pub fn reason_text(finished: &Finished) -> String {
    let ran = clock(finished.ran);
    match &finished.reason {
        StopReason::User => trf("Stopped after {}", &[&ran]),
        StopReason::TimeUp => trf("Finished after {}", &[&ran]),
        StopReason::CpuLimit(t) => trf("Stopped after {}: the processor reached {} °C", &[&ran, &t.round()]),
        StopReason::GpuLimit(t) => trf("Stopped after {}: the graphics card reached {} °C", &[&ran, &t.round()]),
        StopReason::Battery => trf("Stopped after {}: the computer is running on battery", &[&ran]),
        StopReason::WorkerStopped => trf("Stopped after {}: the graphics card load ended unexpectedly", &[&ran]),
        StopReason::Failed(message) => trf("Could not start: {}", &[message]),
        StopReason::VifteEmergency => trf("Stopped after {}: Vifte switched to emergency mode", &[&ran]),
        StopReason::VifteSensorLost => trf("Stopped after {}: Vifte lost a fan curve's temperature sensor", &[&ran]),
        StopReason::ViftePaused => trf("Stopped after {}: Vifte's fan control is paused", &[&ran]),
        StopReason::SensorsLost => trf("Stopped after {}: the temperature readings stopped coming", &[&ran]),
    }
}

/// How the test stands, for every place that shows it.
pub struct TestView {
    /// idle, stopped, running or claude.
    pub level: &'static str,
    pub title: String,
    pub text: String,
    pub running: bool,
    pub elapsed: Duration,
    pub total: Duration,
    /// The chosen profile, or "Custom", with its loads.
    pub setup: String,
}

impl App {
    pub(super) fn build_test_panel(self: &Rc<Self>) {
        let t = &self.test;
        let weak = Rc::downgrade(self);
        let changed = move || {
            if let Some(app) = weak.upgrade().filter(|app| !app.test.updating.get()) {
                app.test_changed();
            }
        };
        let c = changed.clone();
        t.threads.connect_value_notify(move |_| c());
        let c = changed.clone();
        t.cpu_load.connect_value_changed(move |_| c());
        let c = changed.clone();
        t.gpu_load.connect_value_changed(move |_| c());
        let c = changed.clone();
        if let Some(gpu) = &t.gpu {
            gpu.connect_selected_notify(move |_| c());
        }
        let c = changed;
        t.duration.connect_selected_notify(move |_| c());

        for (button, &(_, cpu, gpu)) in t.profiles.iter().zip(&PROFILES) {
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                let Some(app) = weak.upgrade() else { return };
                let t = &app.test;
                t.updating.set(true);
                t.threads.set_value(t.threads.adjustment().upper());
                t.cpu_load.set_value(cpu as f64);
                t.gpu_load.set_value(gpu as f64);
                t.updating.set(false);
                app.test_changed();
            });
        }

        let weak = Rc::downgrade(self);
        t.start.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.start_or_stop();
            }
        });
        self.update_test_status();
    }

    /// Starts a test with the chosen setup, or stops the one running.
    pub(super) fn start_or_stop(&self) {
        match self.machine.controller.status() {
            Status::Idle => self.start_test(),
            Status::Starting | Status::Running { .. } => self.machine.controller.stop(),
        }
        self.update_test_status();
    }

    /// The setup as chosen on the page.
    fn chosen_levels(&self) -> Levels {
        let t = &self.test;
        let gpu = t.gpu.as_ref().and_then(|row| t.usable.get(row.selected() as usize).copied());
        Levels {
            cpu_threads: t.threads.value() as usize,
            cpu_load: t.cpu_load.value() as u32,
            gpu,
            gpu_load: if gpu.is_some() { t.gpu_load.value() as u32 } else { 0 },
        }
    }

    /// Something on the page changed: remember it, mark the profile as
    /// custom unless it still matches one, and apply it to a running test.
    fn test_changed(&self) {
        let t = &self.test;
        let levels = self.chosen_levels();
        let all_threads = levels.cpu_threads == t.threads.adjustment().upper() as usize;
        t.gpu_load_row.set_sensitive(levels.gpu.is_some());
        {
            let mut config = self.config.borrow_mut();
            config.cpu_threads = (!all_threads).then_some(levels.cpu_threads);
            config.cpu_load = levels.cpu_load;
            config.gpu_load = t.gpu_load.value() as u32;
            config.gpu = levels.gpu.map(|i| self.machine.gpus[i].pci.clone());
            config.duration_minutes = DURATIONS[(t.duration.selected() as usize).min(DURATIONS.len() - 1)];
            config.save();
        }
        self.sync_profile();
        self.machine.controller.set_levels(levels.cpu_threads, levels.cpu_load, levels.gpu_load);
        self.update_test_status();
    }

    /// Marks the profile that matches the page, or says "Custom".
    fn sync_profile(&self) {
        let t = &self.test;
        let all_threads = t.threads.value() as usize == t.threads.adjustment().upper() as usize;
        let (cpu_load, gpu_load) = (t.cpu_load.value() as u32, t.gpu_load.value() as u32);
        self.test.show_profile(matching_profile(all_threads, cpu_load, gpu_load));
    }

    /// Shows levels set by Claude on the page, without saving them as the
    /// user's own setup.
    pub(super) fn show_levels(&self, levels: &Levels) {
        let t = &self.test;
        t.updating.set(true);
        t.threads.set_value(levels.cpu_threads as f64);
        t.cpu_load.set_value(levels.cpu_load as f64);
        if let Some(row) = &t.gpu {
            let position = levels.gpu.and_then(|g| t.usable.iter().position(|&i| i == g)).unwrap_or(t.usable.len());
            row.set_selected(position as u32);
        }
        if levels.gpu.is_some() {
            t.gpu_load.set_value(levels.gpu_load as f64);
        }
        t.gpu_load_row.set_sensitive(levels.gpu.is_some());
        self.sync_profile();
        t.updating.set(false);
    }

    pub(super) fn start_test(&self) {
        let levels = self.chosen_levels();
        if levels.cpu_load == 0 && levels.gpu_load == 0 {
            self.toast(tr("Choose a load for the processor or the graphics card first."));
            return;
        }
        let config = self.config.borrow();
        let longest = Duration::from_secs_f64(config.max_minutes * 60.0);
        let duration = match config.duration_minutes {
            0 => longest,
            m => Duration::from_secs(m as u64 * 60).min(longest),
        };
        let limits = Limits { cpu: config.cpu_limit, gpu: config.gpu_limit, stop_on_battery: config.stop_on_battery };
        self.machine.controller.start(levels, duration, limits);
    }

    /// The length the next test gets.
    fn planned(&self) -> Duration {
        let config = self.config.borrow();
        let longest = Duration::from_secs_f64(config.max_minutes * 60.0);
        match config.duration_minutes {
            0 => longest,
            m => Duration::from_secs(m as u64 * 60).min(longest),
        }
    }

    /// Shows the test's state everywhere, and reports a test that has just
    /// ended.
    pub(super) fn update_test_status(&self) {
        let t = &self.test;
        let controller = &self.machine.controller;
        if let Some(finished) = controller.take_finished() {
            let text = reason_text(&finished);
            if finished.reason != StopReason::User {
                self.toast(&text);
            }
            *self.last_test.borrow_mut() = Some((text, finished));
            self.claude_started.set(false);
        }
        let status = controller.status();
        let idle = matches!(status, Status::Idle);
        for row in [t.gpu.as_ref().map(|r| r.upcast_ref::<gtk::Widget>()), Some(t.duration.upcast_ref())].into_iter().flatten() {
            row.set_sensitive(idle);
        }
        let last = self.last_test.borrow();
        let unusual_end = last.as_ref().is_some_and(|(_, f)| !matches!(f.reason, StopReason::User | StopReason::TimeUp));
        let (cpu_load, gpu_load) = (t.cpu_load.value() as u32, if t.gpu_load_row.is_sensitive() { t.gpu_load.value() as u32 } else { 0 });
        let setup = format!("{} · {cpu_load} % · {gpu_load} %", t.profile_label());
        let view = match status {
            Status::Idle => TestView {
                level: if unusual_end { "stopped" } else { "idle" },
                title: if unusual_end { tr("Test stopped") } else { tr("Ready") }.to_owned(),
                text: last.as_ref().map_or_else(|| tr("Choose a load and press Start").to_owned(), |(text, _)| text.clone()),
                running: false,
                elapsed: last.as_ref().map_or(Duration::ZERO, |(_, f)| f.ran),
                total: self.planned(),
                setup,
            },
            Status::Starting => TestView {
                level: "running",
                title: tr("Starting…").to_owned(),
                text: String::new(),
                running: true,
                elapsed: Duration::ZERO,
                total: self.planned(),
                setup,
            },
            Status::Running { started, ends, .. } => {
                let total = ends.duration_since(started);
                let elapsed = Instant::now().saturating_duration_since(started).min(total);
                let by_claude = self.claude_started.get();
                TestView {
                    level: if by_claude { "claude" } else { "running" },
                    title: if by_claude { tr("Running, started by Claude") } else { tr("Running") }.to_owned(),
                    text: trf("{} of {}", &[&clock(elapsed), &clock(total)]),
                    running: true,
                    elapsed,
                    total,
                    setup,
                }
            }
        };
        drop(last);

        // The page itself.
        if view.running {
            t.clock.set_parts(&clock(view.elapsed), &trf("of {}", &[&clock(view.total)]));
            t.hero_subtitle.set_label(&trf("{} left", &[&clock(view.total.saturating_sub(view.elapsed))]));
        } else {
            let shown = if view.level == "stopped" { view.elapsed } else { view.total };
            t.clock.set_parts(&clock(shown), if view.level == "stopped" { "" } else { tr("planned") });
            t.hero_subtitle.set_label(&view.text);
        }
        t.chip.set(view.level, &view.title);
        t.progress.set_fraction(if view.running { view.elapsed.as_secs_f64() / view.total.as_secs_f64().max(1.0) } else { 0.0 });
        t.start.set_label(if view.running { tr("Stop") } else { tr("Start") });
        t.start.set_css_classes(if view.running { &["stress-go", "destructive-action", "stress-stop"] } else { &["stress-go", "suggested-action"] });
        self.show_test_view(&view);
    }
}

/// A group heading with an icon surface and a small note on the right, and
/// an empty list under it.
fn group(column: &gtk::Box, title: &str, icon: &[&str], kind: &str) -> (gtk::Box, gtk::ListBox, gtk::Label) {
    let section = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(9).build();
    let head = gtk::Box::builder().spacing(9).margin_start(4).margin_end(4).build();
    head.append(&graph::icon_surface(icon, kind, true));
    head.append(&gtk::Label::builder().label(title).xalign(0.0).hexpand(true).css_classes(["stress-group-label"]).build());
    let note = gtk::Label::builder().css_classes(["caption", "dim-label", "stress-sub"]).build();
    head.append(&note);
    section.append(&head);
    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list", "stress-list"]).build();
    section.append(&list);
    column.append(&section);
    (section, list, note)
}

impl TestControls {
    /// Builds the page from the saved setup. `gpus` are (list index, name)
    /// of the graphics cards that can be loaded.
    pub fn new(app_config: &crate::config::Config, threads: usize, gpus: &[(usize, String, String)]) -> TestControls {
        let longest = app_config.max_minutes;
        let left = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(16).hexpand(true).build();
        let right = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(16).hexpand(true).build();

        // The status: a large clock, Start/Stop and the last two minutes.
        let hero = graph::card(14, "stress-hero-card");
        let top = gtk::Box::builder().spacing(12).build();
        let texts = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).hexpand(true).build();
        let clock = BigValue::new("stress-huge");
        clock.widget.set_halign(gtk::Align::Start);
        texts.append(&clock.widget);
        let hero_subtitle = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["dim-label", "stress-sub"]).build();
        texts.append(&hero_subtitle);
        top.append(&texts);
        let chip = Chip::new();
        chip.widget.set_valign(gtk::Align::Start);
        chip.widget.set_margin_top(8);
        top.append(&chip.widget);
        hero.append(&top);
        let progress = gtk::ProgressBar::builder().css_classes(["stress-progress"]).build();
        hero.append(&progress);
        let start = gtk::Button::builder().label(tr("Start")).css_classes(["stress-go", "suggested-action"]).build();
        hero.append(&start);
        let legend = gtk::Box::builder().spacing(16).margin_top(4).build();
        let temperature_label = |class: &str| {
            let item = gtk::Box::builder().spacing(6).build();
            item.append(&gtk::Box::builder().valign(gtk::Align::Center).css_classes(["stress-legend", class]).build());
            let label = gtk::Label::builder().css_classes(["caption", "stress-sub"]).build();
            item.append(&label);
            legend.append(&item);
            (item, label)
        };
        let (_, cpu_temperature) = temperature_label("cpu");
        let (gpu_item, gpu_temperature) = temperature_label("gpu");
        gpu_item.set_visible(!gpus.is_empty());
        hero.append(&legend);
        let graph = HistoryGraph::new(170, 120.0, graph::minute_labels(&[2, 1]));
        hero.append(&graph.area);
        left.append(&hero);

        // Profiles.
        let profile_section = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(9).build();
        let head = gtk::Box::builder().margin_start(4).margin_end(4).build();
        head.append(&gtk::Label::builder().label(tr("Profile")).xalign(0.0).hexpand(true).css_classes(["stress-group-label"]).build());
        let profile_name = gtk::Label::builder().css_classes(["caption", "dim-label"]).build();
        head.append(&profile_name);
        profile_section.append(&head);
        let profile_grid = gtk::Grid::builder().column_homogeneous(true).column_spacing(8).row_spacing(8).build();
        let profiles: Vec<gtk::ToggleButton> = PROFILES
            .iter()
            .enumerate()
            .map(|(i, (name, cpu, gpu))| {
                let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).build();
                text.append(&gtk::Label::builder().label(tr(name)).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).tooltip_text(tr(name)).css_classes(["stress-profile-name"]).build());
                text.append(&gtk::Label::builder().label(format!("{cpu} % · {gpu} %")).xalign(0.0).css_classes(["stress-profile-load"]).build());
                let button = gtk::ToggleButton::builder().child(&text).css_classes(["stress-profile"]).build();
                profile_grid.attach(&button, i as i32 % 3, i as i32 / 3, 1, 1);
                button
            })
            .collect();
        profile_section.append(&profile_grid);
        left.append(&profile_section);

        // Processor.
        let (_, processor, cpu_now) = group(&right, tr("Processor"), graph::ICON_CPU, "cpu");
        let threads_row = adw::SpinRow::builder()
            .title(tr("Processor threads"))
            .subtitle(trf("of {}", &[&threads]))
            .adjustment(&gtk::Adjustment::new(
                app_config.cpu_threads.unwrap_or(threads).clamp(1, threads) as f64,
                1.0,
                threads as f64,
                1.0,
                4.0,
                0.0,
            ))
            .build();
        processor.append(&threads_row);
        let (cpu_row, cpu_load) = scale_row(tr("Load"));
        cpu_load.set_value(app_config.cpu_load as f64);
        processor.append(&cpu_row);

        // Graphics card.
        let (graphics_section, graphics, gpu_now) = group(&right, tr("Graphics card"), graph::ICON_GPU, "gpu");
        let gpu = (!gpus.is_empty()).then(|| {
            let mut labels: Vec<&str> = gpus.iter().map(|(_, name, _)| name.as_str()).collect();
            labels.push(tr("None"));
            let selected = app_config
                .gpu
                .as_ref()
                .and_then(|pci| gpus.iter().position(|(_, _, p)| p == pci))
                .unwrap_or(if app_config.gpu.is_none() && app_config.gpu_load > 0 { 0 } else { gpus.len() });
            let row = adw::ComboRow::builder()
                .title(tr("Card"))
                .model(&gtk::StringList::new(&labels))
                .selected(selected as u32)
                .build();
            graphics.append(&row);
            row
        });
        let (gpu_load_row, gpu_load) = scale_row(tr("Load"));
        gpu_load.set_value(app_config.gpu_load as f64);
        graphics.append(&gpu_load_row);
        graphics_section.set_visible(gpu.is_some());

        // Time.
        let (_, time, _) = group(&right, tr("Time"), graph::ICON_TIME, "neutral");
        let duration_labels: Vec<String> = DURATIONS.iter().map(|m| duration_label(*m, longest)).collect();
        let duration_refs: Vec<&str> = duration_labels.iter().map(String::as_str).collect();
        let duration = adw::ComboRow::builder()
            .title(tr("Duration"))
            .model(&gtk::StringList::new(&duration_refs))
            .selected(DURATIONS.iter().position(|m| *m == app_config.duration_minutes).unwrap_or(2) as u32)
            .build();
        time.append(&duration);
        right.append(&devices::foot(tr("Puts the chosen load on the processor and graphics card until the time is up or a limit is reached.")));

        let columns = gtk::Box::builder().spacing(16).homogeneous(true).build();
        columns.append(&left);
        columns.append(&right);
        let content = devices::page_box();
        content.append(&columns);
        let page = devices::page(&content);

        let controls = TestControls {
            page,
            columns,
            clock,
            chip,
            hero_subtitle,
            graph,
            cpu_now,
            gpu_now,
            cpu_temperature,
            gpu_temperature,
            profiles,
            profile_name,
            threads: threads_row,
            cpu_load,
            gpu,
            gpu_load,
            gpu_load_row,
            duration,
            start,
            progress,
            usable: gpus.iter().map(|(i, _, _)| *i).collect(),
            updating: Cell::new(true),
        };
        // Show the matching profile, if any, without saving anything yet.
        let all = controls.threads.value() as usize == threads;
        let gpu_value = controls.gpu_load.value() as u32;
        let gpu_on = controls.gpu.as_ref().is_some_and(|row| (row.selected() as usize) < gpus.len());
        controls.gpu_load_row.set_sensitive(gpu_on);
        controls.show_profile(matching_profile(all, app_config.cpu_load, gpu_value));
        controls.updating.set(false);
        controls
    }

    fn show_profile(&self, profile: Option<usize>) {
        for (i, button) in self.profiles.iter().enumerate() {
            button.set_active(profile == Some(i));
        }
        self.profile_name.set_label(if profile.is_some() { "" } else { tr("Custom") });
    }

    /// The chosen profile's name, or "Custom".
    pub fn profile_label(&self) -> &'static str {
        match self.profiles.iter().zip(&PROFILES).find(|(button, _)| button.is_active()) {
            Some((_, (name, _, _))) => tr(name),
            None => tr("Custom"),
        }
    }
}
