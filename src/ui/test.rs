//! The load test panel: profile, processor threads and load, graphics card and
//! its load, duration, Start/Stop and the test's status.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;

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
    pub group: adw::PreferencesGroup,
    profile: adw::ComboRow,
    threads: adw::SpinRow,
    cpu_load: gtk::Scale,
    gpu: Option<adw::ComboRow>,
    gpu_load: gtk::Scale,
    gpu_load_row: adw::ActionRow,
    duration: adw::ComboRow,
    start: gtk::Button,
    status: adw::ActionRow,
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
    scale.set_width_request(280);
    scale.set_hexpand(true);
    scale.set_valign(gtk::Align::Center);
    for mark in [0.0, 25.0, 50.0, 75.0, 100.0] {
        scale.add_mark(mark, gtk::PositionType::Bottom, None);
    }
    let row = adw::ActionRow::builder().title(title).build();
    row.add_suffix(&scale);
    (row, scale)
}

fn duration_label(minutes: u32, longest: f64) -> String {
    match minutes {
        0 => trf("Until stopped (at most {} min)", &[&longest]),
        1 => tr("1 minute").to_string(),
        m => trf("{} minutes", &[&m]),
    }
}

/// "2:05", or "1:02:05" from an hour.
fn clock(d: Duration) -> String {
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
    }
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

        let weak = Rc::downgrade(self);
        t.profile.connect_selected_notify(move |row| {
            let Some(app) = weak.upgrade() else { return };
            let t = &app.test;
            if t.updating.get() || row.selected() == 0 {
                return;
            }
            let Some(&(_, cpu, gpu)) = PROFILES.get(row.selected() as usize - 1) else { return };
            t.updating.set(true);
            t.threads.set_value(t.threads.adjustment().upper());
            t.cpu_load.set_value(cpu as f64);
            t.gpu_load.set_value(gpu as f64);
            t.updating.set(false);
            app.test_changed();
        });

        let weak = Rc::downgrade(self);
        t.start.connect_clicked(move |_| {
            let Some(app) = weak.upgrade() else { return };
            match app.machine.controller.status() {
                Status::Idle => app.start_test(),
                Status::Starting | Status::Running { .. } => app.machine.controller.stop(),
            }
            app.update_test_status();
        });
        self.update_test_status();
    }

    /// The setup as chosen in the panel.
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

    /// Something in the panel changed: remember it, mark the profile as
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

    /// Shows the profile that matches the panel, or "Custom".
    fn sync_profile(&self) {
        let t = &self.test;
        let all_threads = t.threads.value() as usize == t.threads.adjustment().upper() as usize;
        let (cpu_load, gpu_load) = (t.cpu_load.value() as u32, t.gpu_load.value() as u32);
        let profile = PROFILES
            .iter()
            .position(|&(_, cpu, gpu)| all_threads && cpu == cpu_load && gpu == gpu_load)
            .map_or(0, |i| i + 1);
        let updating = t.updating.replace(true);
        t.profile.set_selected(profile as u32);
        t.updating.set(updating);
    }

    /// Shows levels set by Claude in the panel, without saving them as the
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

    /// Shows the test's state in the panel and the window title, and reports
    /// a test that has just ended.
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
        let (label, css, title, subtitle, progress) = match status {
            Status::Idle => (
                tr("Start"),
                "suggested-action",
                tr("Ready"),
                self.last_test.borrow().as_ref().map(|(text, _)| text.clone()).unwrap_or_default(),
                None,
            ),
            Status::Starting => (tr("Stop"), "destructive-action", tr("Starting…"), String::new(), Some(0.0)),
            Status::Running { started, ends, .. } => {
                let total = ends.duration_since(started);
                let elapsed = Instant::now().saturating_duration_since(started).min(total);
                (
                    tr("Stop"),
                    "destructive-action",
                    if self.claude_started.get() { tr("Running, started by Claude") } else { tr("Running") },
                    trf("{} of {}", &[&clock(elapsed), &clock(total)]),
                    Some(elapsed.as_secs_f64() / total.as_secs_f64().max(1.0)),
                )
            }
        };
        t.start.set_label(label);
        t.start.set_css_classes(&["pill", css]);
        t.status.set_title(title);
        t.status.set_subtitle(&subtitle);
        t.progress.set_visible(progress.is_some());
        t.progress.set_fraction(progress.unwrap_or(0.0));
        self.title.set_subtitle(&if idle { String::new() } else { format!("{title} · {subtitle}") });
    }
}

impl TestControls {
    /// Builds the panel from the saved setup. `gpus` are (list index, name)
    /// of the graphics cards that can be loaded.
    pub fn new(app_config: &crate::config::Config, threads: usize, gpus: &[(usize, String, String)]) -> TestControls {
        let longest = app_config.max_minutes;
        let group = adw::PreferencesGroup::builder()
            .title(tr("Load Test"))
            .description(tr("Puts the chosen load on the processor and graphics card until the time is up or a limit is reached."))
            .build();
        let mut profile_labels = vec![tr("Custom")];
        profile_labels.extend(PROFILES.iter().map(|(name, _, _)| tr(name)));
        let profile = adw::ComboRow::builder()
            .title(tr("Profile"))
            .model(&gtk::StringList::new(&profile_labels))
            .build();
        group.add(&profile);

        let threads_row = adw::SpinRow::builder()
            .title(tr("Processor threads"))
            .adjustment(&gtk::Adjustment::new(
                app_config.cpu_threads.unwrap_or(threads).clamp(1, threads) as f64,
                1.0,
                threads as f64,
                1.0,
                4.0,
                0.0,
            ))
            .build();
        group.add(&threads_row);
        let (cpu_row, cpu_load) = scale_row(tr("Processor load"));
        cpu_load.set_value(app_config.cpu_load as f64);
        group.add(&cpu_row);

        let gpu = (!gpus.is_empty()).then(|| {
            let mut labels: Vec<&str> = gpus.iter().map(|(_, name, _)| name.as_str()).collect();
            labels.push(tr("None"));
            let selected = app_config
                .gpu
                .as_ref()
                .and_then(|pci| gpus.iter().position(|(_, _, p)| p == pci))
                .unwrap_or(if app_config.gpu.is_none() && app_config.gpu_load > 0 { 0 } else { gpus.len() });
            let row = adw::ComboRow::builder()
                .title(tr("Graphics card"))
                .model(&gtk::StringList::new(&labels))
                .selected(selected as u32)
                .build();
            group.add(&row);
            row
        });
        let (gpu_load_row, gpu_load) = scale_row(tr("Graphics card load"));
        gpu_load.set_value(app_config.gpu_load as f64);
        gpu_load_row.set_visible(gpu.is_some());
        group.add(&gpu_load_row);

        let duration_labels: Vec<String> = DURATIONS.iter().map(|m| duration_label(*m, longest)).collect();
        let duration_refs: Vec<&str> = duration_labels.iter().map(String::as_str).collect();
        let duration = adw::ComboRow::builder()
            .title(tr("Duration"))
            .model(&gtk::StringList::new(&duration_refs))
            .selected(DURATIONS.iter().position(|m| *m == app_config.duration_minutes).unwrap_or(2) as u32)
            .build();
        group.add(&duration);

        let progress = gtk::ProgressBar::builder().valign(gtk::Align::Center).width_request(160).build();
        let status = adw::ActionRow::builder().title(tr("Ready")).build();
        status.add_suffix(&progress);
        group.add(&status);
        let start = gtk::Button::builder().label(tr("Start")).valign(gtk::Align::Center).css_classes(["pill", "suggested-action"]).build();
        group.set_header_suffix(Some(&start));

        let controls = TestControls {
            group,
            profile,
            threads: threads_row,
            cpu_load,
            gpu,
            gpu_load,
            gpu_load_row,
            duration,
            start,
            status,
            progress,
            usable: gpus.iter().map(|(i, _, _)| *i).collect(),
            updating: Cell::new(true),
        };
        // Show the matching profile, if any, without saving anything yet.
        let all = controls.threads.value() as usize == threads;
        let gpu_value = controls.gpu_load.value() as u32;
        let gpu_on = controls.gpu.as_ref().is_some_and(|row| (row.selected() as usize) < gpus.len());
        controls.gpu_load_row.set_sensitive(gpu_on);
        let profile = PROFILES
            .iter()
            .position(|&(_, cpu, gpu)| all && cpu == app_config.cpu_load && gpu == gpu_value)
            .map_or(0, |i| i + 1);
        controls.profile.set_selected(profile as u32);
        controls.updating.set(false);
        controls
    }
}
