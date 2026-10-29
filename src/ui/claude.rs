//! Claude integration, app side: a D-Bus object that `stresstest mcp` calls.
//! Every request is checked here, in the app, so the rules hold however
//! Claude connects:
//!
//! - Nothing works until the user turns on "Allow Claude to run load tests".
//! - The user's temperature limits and longest test always apply; Claude can
//!   only make the limits stricter.
//! - Vifte's safety state applies to Claude's tests like to the user's.
//! - Every test Claude starts, changes or stops shows a notice, and the panel
//!   shows the levels Claude chose.

use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::{gio, glib};
use serde_json::{json, Map, Value};

use super::{App, CURRENT};
use crate::i18n::{tr, trf};
use crate::loadtest::{Finished, Levels, Limits, Status, StopReason};
use crate::vifte;

pub const OBJECT_PATH: &str = "/io/github/stresstest/Stress/Claude";
pub const INTERFACE: &str = "io.github.stresstest.Claude";
const INTROSPECTION: &str = r#"<node>
  <interface name="io.github.stresstest.Claude">
    <method name="Call">
      <arg type="s" name="method" direction="in"/>
      <arg type="s" name="arguments" direction="in"/>
      <arg type="s" name="result" direction="out"/>
    </method>
  </interface>
</node>"#;

const DEFAULT_MINUTES: f64 = 5.0;

/// One reply to a D-Bus `Call`, always a JSON object with `ok`.
pub struct Reply(gio::DBusMethodInvocation);

impl Reply {
    fn send(self, value: Value) {
        self.0.return_value(Some(&(value.to_string(),).to_variant()));
    }

    fn ok(self, text: impl Into<String>) {
        self.send(json!({ "ok": true, "text": text.into() }));
    }

    fn error(self, text: impl Into<String>) {
        self.send(json!({ "ok": false, "error": text.into() }));
    }
}

/// Registers the Claude D-Bus object on the application's own bus connection.
pub fn register(application: &adw::Application) {
    let Some(connection) = application.dbus_connection() else { return };
    let Some(info) = gio::DBusNodeInfo::for_xml(INTROSPECTION).ok().and_then(|n| n.lookup_interface(INTERFACE))
    else {
        return;
    };
    let registration = connection
        .register_object(OBJECT_PATH, &info)
        .method_call(|_, _, _, _, method, params, invocation| {
            if method != "Call" {
                invocation.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", method);
                return;
            }
            let (name, arguments) = params.get::<(String, String)>().unwrap_or_default();
            let arguments: Value = serde_json::from_str(&arguments).unwrap_or(Value::Null);
            let reply = Reply(invocation);
            match CURRENT.with(|current| current.borrow().upgrade()) {
                Some(app) => app.claude_request(&name, arguments, reply),
                None => reply.error("Stress has no window open yet. Try again in a moment."),
            }
        })
        .build();
    if let Err(err) = registration {
        eprintln!("Claude integration unavailable: {err}");
    }
}

/// The command that registers Stress with Claude Code. Claude Code inherits
/// PATH, so the bare name is used when it finds this program.
fn claude_code_command() -> String {
    format!("claude mcp add --scope user stress -- {} mcp", command_name())
}

fn command_name() -> String {
    let here = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok());
    let on_path = glib::find_program_in_path("stresstest").and_then(|p| p.canonicalize().ok());
    match (here, on_path) {
        (Some(here), Some(on_path)) if here == on_path => "stresstest".into(),
        _ => executable(),
    }
}

fn claude_desktop_config() -> String {
    format!(r#"{{ "mcpServers": {{ "stress": {{ "command": "{}", "args": ["mcp"] }} }} }}"#, executable())
}

/// Absolute path, so Claude Desktop finds it without ~/.local/bin on PATH.
fn executable() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_else(|| "stresstest".into())
}

/// One decimal, or null.
fn r1(value: Option<f64>) -> Value {
    json!(value.map(|v| (v * 10.0).round() / 10.0))
}

/// An optional whole number argument within a range.
fn whole(args: &Map<String, Value>, key: &str, min: u64, max: u64) -> Result<Option<u64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => match value.as_f64() {
            Some(v) if v.fract() == 0.0 && v >= min as f64 && v <= max as f64 => Ok(Some(v as u64)),
            _ => Err(format!("{key} must be a whole number from {min} to {max}")),
        },
    }
}

fn number(args: &Map<String, Value>, key: &str) -> Result<Option<f64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_f64().map(Some).ok_or_else(|| format!("{key} must be a number")),
    }
}

fn finished_json(finished: &Finished) -> Value {
    let (reason, temperature, message) = match &finished.reason {
        StopReason::User => ("stopped", None, None),
        StopReason::TimeUp => ("time_up", None, None),
        StopReason::CpuLimit(t) => ("processor_limit", Some(*t), None),
        StopReason::GpuLimit(t) => ("graphics_card_limit", Some(*t), None),
        StopReason::Battery => ("on_battery", None, None),
        StopReason::WorkerStopped => ("graphics_card_load_ended", None, None),
        StopReason::Failed(m) => ("could_not_start", None, Some(m.clone())),
        StopReason::VifteEmergency => ("vifte_emergency_mode", None, None),
        StopReason::VifteSensorLost => ("vifte_sensor_lost", None, None),
        StopReason::ViftePaused => ("vifte_paused", None, None),
    };
    json!({ "reason": reason, "ran_s": finished.ran.as_secs(), "temperature_c": temperature, "message": message })
}

impl App {
    pub(super) fn claude_request(self: &Rc<Self>, method: &str, args: Value, reply: Reply) {
        if !self.config.borrow().claude_control {
            return reply.error("Claude control is turned off. Ask the user to turn on “Allow Claude to run load tests” in Stress → Preferences → Claude.");
        }
        let args = match args {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        let result = match method {
            "get_status" => Ok(self.claude_status()),
            "get_history" => self.claude_history(&args),
            "start_test" => self.claude_start(&args),
            "set_load" => self.claude_set_load(&args),
            "stop_test" => self.claude_stop(),
            other => Err(format!("Unknown tool “{other}”.")),
        };
        match result {
            Ok(text) => reply.ok(text),
            Err(text) => reply.error(text),
        }
    }

    fn claude_status(&self) -> String {
        let machine = &self.machine;
        let sample = machine.history.latest().unwrap_or_else(|| machine.first.clone());
        let config = self.config.borrow();
        let gpus: Vec<Value> = machine
            .gpus
            .iter()
            .enumerate()
            .map(|(i, gpu)| {
                let reading = sample.gpus.get(i).cloned().unwrap_or_default();
                json!({
                    "pci": gpu.pci, "name": gpu.name, "drives_screen": gpu.boot_vga,
                    "can_load": gpu.unavailable_reason().is_none(), "unavailable_reason": gpu.unavailable_reason(),
                    "temperature_c": r1(reading.temperature), "utilization_percent": r1(reading.utilization),
                    "power_w": r1(reading.power), "fan_percent": r1(reading.fan_percent),
                })
            })
            .collect();
        let psu = sample.psu.as_ref().map(|p| {
            json!({ "name": p.name, "output_w": r1(Some(p.output)), "from_wall_estimate_w": r1(p.input_estimate) })
        });
        let vifte = match vifte::state() {
            Some(v) => json!({
                "running": true, "emergency_c": v.emergency_temp, "test_limit_c": v.limit(),
                "emergency_active": v.emergency_active, "sensor_lost": v.sensor_lost, "paused": v.paused,
            }),
            None => json!({ "running": false }),
        };
        let test = match machine.controller.status() {
            Status::Idle => json!({ "state": "idle" }),
            Status::Starting => json!({ "state": "starting" }),
            Status::Running { started, ends, levels } => json!({
                "state": "running",
                "started_by": if self.claude_started.get() { "claude" } else { "user" },
                "elapsed_s": Instant::now().saturating_duration_since(started).as_secs(),
                "duration_s": ends.duration_since(started).as_secs(),
                "cpu_threads": levels.cpu_threads, "cpu_load": levels.cpu_load,
                "gpu": levels.gpu.map(|i| machine.gpus[i].pci.clone()), "gpu_load": levels.gpu_load,
            }),
        };
        let status = json!({
            "processor": {
                "model": machine.cpu.model, "threads": machine.cpu.threads,
                "temperature_c": r1(sample.cpu_temperature), "utilization_percent": r1(sample.cpu_utilization),
                "power_w": r1(sample.cpu_power),
            },
            "graphics_cards": gpus,
            "power_supply": psu,
            "limits": {
                "cpu_c": config.cpu_limit, "gpu_c": config.gpu_limit,
                "longest_test_minutes": config.max_minutes, "stop_on_battery": config.stop_on_battery,
            },
            "vifte": vifte,
            "test": test,
            "last_test": self.last_test.borrow().as_ref().map(|(_, finished)| finished_json(finished)),
        });
        serde_json::to_string_pretty(&status).unwrap_or_default()
    }

    fn claude_history(&self, args: &Map<String, Value>) -> Result<String, String> {
        let minutes = number(args, "minutes")?.unwrap_or(10.0).clamp(1.0, 30.0);
        let max_points = whole(args, "max_points", 1, 600)?.unwrap_or(60) as usize;
        let mut entries = self.machine.history.recent(Duration::from_secs_f64(minutes * 60.0));
        if entries.len() > max_points {
            let step = entries.len() as f64 / max_points as f64;
            entries = (0..max_points).map(|i| entries[(i as f64 * step) as usize].clone()).collect();
        }
        let column = |value: &dyn Fn(&crate::sensors::Sample) -> Option<f64>| -> Vec<Value> {
            entries.iter().map(|(_, sample, _)| r1(value(sample))).collect()
        };
        let mut gpus = Map::new();
        for (i, gpu) in self.machine.gpus.iter().enumerate().filter(|(_, g)| g.unavailable_reason().is_none()) {
            gpus.insert(
                gpu.pci.clone(),
                json!({
                    "name": gpu.name,
                    "temperature_c": column(&|s| s.gpus.get(i).and_then(|g| g.temperature)),
                    "utilization_percent": column(&|s| s.gpus.get(i).and_then(|g| g.utilization)),
                    "power_w": column(&|s| s.gpus.get(i).and_then(|g| g.power)),
                    "fan_percent": column(&|s| s.gpus.get(i).and_then(|g| g.fan_percent)),
                }),
            );
        }
        let tests: Vec<Value> = entries
            .iter()
            .map(|(_, _, load)| {
                load.map_or(Value::Null, |l| {
                    json!({ "cpu_threads": l.cpu_threads, "cpu_load": l.cpu_load, "gpu_load": l.gpu_load })
                })
            })
            .collect();
        let history = json!({
            "seconds_ago": entries.iter().map(|(ago, _, _)| ago.round() as u64).collect::<Vec<_>>(),
            "processor": {
                "temperature_c": column(&|s| s.cpu_temperature),
                "utilization_percent": column(&|s| s.cpu_utilization),
                "power_w": column(&|s| s.cpu_power),
            },
            "graphics_cards": gpus,
            "power_supply_output_w": column(&|s| s.psu.as_ref().map(|p| p.output)),
            "test": tests,
            "note": "One reading per second, thinned evenly to max_points. test is null where no load test ran.",
        });
        Ok(history.to_string())
    }

    fn claude_start(&self, args: &Map<String, Value>) -> Result<String, String> {
        let machine = &self.machine;
        if !matches!(machine.controller.status(), Status::Idle) {
            return Err("A test is already running. Use set_load to change it, or stop_test first.".into());
        }
        let threads = machine.cpu.threads;
        let cpu_load = whole(args, "cpu_load", 0, 100)?.unwrap_or(0) as u32;
        let cpu_threads = whole(args, "cpu_threads", 1, threads as u64)?.map_or(threads, |t| t as usize);
        let gpu_load = whole(args, "gpu_load", 0, 100)?.unwrap_or(0) as u32;
        let loadable = |i: &usize| machine.gpus[*i].unavailable_reason().is_none();
        let gpu = match args.get("gpu").and_then(Value::as_str) {
            Some(pci) => {
                let index = machine.gpus.iter().position(|g| g.pci == pci).ok_or(format!("No graphics card at {pci}; see get_status."))?;
                if let Some(reason) = machine.gpus[index].unavailable_reason() {
                    return Err(format!("{} cannot be loaded: {reason}", machine.gpus[index].name));
                }
                Some(index)
            }
            None if gpu_load > 0 => Some((0..machine.gpus.len()).find(loadable).ok_or("No graphics card on this computer can be loaded.")?),
            None => None,
        };
        if cpu_load == 0 && gpu_load == 0 {
            return Err("Give cpu_load or gpu_load above 0.".into());
        }
        let config = self.config.borrow();
        let minutes = number(args, "minutes")?.unwrap_or(DEFAULT_MINUTES);
        if minutes <= 0.0 {
            return Err("minutes must be above 0".into());
        }
        let capped = minutes > config.max_minutes;
        let minutes = minutes.min(config.max_minutes);
        let lower = |key: &str, user: f64| number(args, key).map(|v| v.map_or(user, |v| v.min(user)));
        let limits = Limits {
            cpu: lower("cpu_limit", config.cpu_limit)?,
            gpu: lower("gpu_limit", config.gpu_limit)?,
            stop_on_battery: config.stop_on_battery,
        };
        drop(config);
        let levels = Levels { cpu_threads, cpu_load, gpu, gpu_load: if gpu.is_some() { gpu_load } else { 0 } };
        self.show_levels(&levels);
        self.claude_started.set(true);
        machine.controller.start(levels, Duration::from_secs_f64(minutes * 60.0), limits);
        self.update_test_status();
        self.toast(tr("Claude started a load test"));

        let mut text = format!("Starting a {minutes} minute test: processor {cpu_load} % on {cpu_threads} of {threads} threads");
        if let Some(i) = gpu {
            text += &format!(", {} at {} % utilization", machine.gpus[i].name, levels.gpu_load);
        }
        text += &format!(". Stops at processor {} °C or graphics card {} °C", limits.cpu, limits.gpu);
        if let Some(limit) = vifte::state().and_then(|v| v.limit()) {
            text += &format!(", and at {limit} °C for Vifte");
        }
        text += ".";
        if capped {
            text += " The duration was shortened to the user's longest test.";
        }
        if gpu.is_some() {
            text += " Opening the graphics card takes a few seconds; check get_status.";
        }
        Ok(text)
    }

    fn claude_set_load(&self, args: &Map<String, Value>) -> Result<String, String> {
        let Status::Running { mut levels, .. } = self.machine.controller.status() else {
            return Err("No test is running. Use start_test.".into());
        };
        if let Some(load) = whole(args, "cpu_load", 0, 100)? {
            levels.cpu_load = load as u32;
        }
        if let Some(threads) = whole(args, "cpu_threads", 1, self.machine.cpu.threads as u64)? {
            levels.cpu_threads = threads as usize;
        }
        if let Some(load) = whole(args, "gpu_load", 0, 100)? {
            if levels.gpu.is_none() {
                return Err("This test runs without a graphics card; start a new test to load one.".into());
            }
            levels.gpu_load = load as u32;
        }
        self.machine.controller.set_levels(levels.cpu_threads, levels.cpu_load, levels.gpu_load);
        self.show_levels(&levels);
        self.toast(tr("Claude changed the load"));
        Ok(format!(
            "Load is now processor {} % on {} threads, graphics card {} %.",
            levels.cpu_load, levels.cpu_threads, levels.gpu_load
        ))
    }

    fn claude_stop(&self) -> Result<String, String> {
        if matches!(self.machine.controller.status(), Status::Idle) {
            return Err("No test is running.".into());
        }
        self.machine.controller.stop();
        self.update_test_status();
        self.toast(tr("Claude stopped the test"));
        Ok("The test is stopped.".into())
    }

    pub(super) fn claude_preferences_page(self: &Rc<Self>, dialog: &adw::PreferencesDialog) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::builder()
            .name("claude")
            .title(tr("Claude"))
            .icon_name("user-available-symbolic")
            .build();

        let access = adw::PreferencesGroup::builder()
            .title(tr("Claude Integration (MCP)"))
            .description(tr("Lets Claude, in Claude Code or Claude Desktop, read the sensors and start, change and stop load tests, for example to tune fan curves together with Vifte. Everything is off until you turn it on here, and you can turn it off at any time."))
            .build();
        let control = adw::SwitchRow::builder()
            .title(tr("Allow Claude to run load tests"))
            .subtitle(tr("Your temperature limits and longest test always apply; Claude can only make the limits stricter."))
            .active(self.config.borrow().claude_control)
            .build();
        access.add(&control);
        page.add(&access);

        let connect = adw::PreferencesGroup::builder()
            .title(tr("Connect Claude"))
            .description(tr("Start a new Claude session after connecting, then ask for example “Test how warm my computer gets under full load”."))
            .build();
        let code = adw::ActionRow::builder()
            .title(tr("Claude Code"))
            .subtitle(claude_code_command())
            .subtitle_selectable(true)
            .build();
        let has_claude = glib::find_program_in_path("claude").is_some();
        let add = gtk::Button::builder().label(tr("Add")).valign(gtk::Align::Center).visible(has_claude).build();
        let copy_code = gtk::Button::builder()
            .icon_name("edit-copy-symbolic")
            .tooltip_text(tr("Copy"))
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        code.add_suffix(&add);
        code.add_suffix(&copy_code);
        let desktop = adw::ActionRow::builder()
            .title(tr("Claude Desktop"))
            .subtitle(tr("Add this to claude_desktop_config.json"))
            .build();
        let copy_desktop = gtk::Button::builder()
            .icon_name("edit-copy-symbolic")
            .tooltip_text(tr("Copy"))
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        desktop.add_suffix(&copy_desktop);
        connect.add(&code);
        connect.add(&desktop);
        page.add(&connect);

        let weak = Rc::downgrade(self);
        control.connect_active_notify(move |row| {
            let Some(app) = weak.upgrade() else { return };
            app.config.borrow_mut().claude_control = row.is_active();
            app.config.borrow().save();
        });
        let copy_to = |dialog: &adw::PreferencesDialog, text: String| {
            dialog.clipboard().set_text(&text);
            dialog.add_toast(adw::Toast::new(tr("Copied")));
        };
        let prefs = dialog.clone();
        copy_code.connect_clicked(move |_| copy_to(&prefs, claude_code_command()));
        let prefs = dialog.clone();
        copy_desktop.connect_clicked(move |_| copy_to(&prefs, claude_desktop_config()));
        let prefs = dialog.clone();
        add.connect_clicked(move |button| {
            button.set_sensitive(false);
            let program = command_name();
            let argv = ["claude", "mcp", "add", "--scope", "user", "stress", "--", &program, "mcp"];
            let argv: Vec<&std::ffi::OsStr> = argv.iter().map(|a| a.as_ref()).collect();
            let flags = gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_MERGE;
            let Ok(process) = gio::Subprocess::newv(&argv, flags) else {
                prefs.add_toast(adw::Toast::new(tr("Could not run the claude command")));
                return;
            };
            let prefs = prefs.clone();
            let button = button.clone();
            let child = process.clone();
            process.communicate_utf8_async(None, None::<&gio::Cancellable>, move |result| {
                button.set_sensitive(true);
                let message = match result {
                    Ok(_) if child.is_successful() => tr("Stress was added to Claude Code").to_owned(),
                    Ok((output, _)) => {
                        let output = output.map(|o| o.trim().to_owned()).unwrap_or_default();
                        if output.contains("already exists") {
                            tr("Stress is already added to Claude Code").to_owned()
                        } else {
                            trf("Could not add it: {}", &[&output])
                        }
                    }
                    Err(err) => trf("Could not add it: {}", &[&err]),
                };
                prefs.add_toast(adw::Toast::new(&message));
            });
        });
        page
    }
}

/// Removes Stress from Claude Code's list of MCP servers, when it is there.
pub fn unregister_from_claude_code() {
    if glib::find_program_in_path("claude").is_some() {
        let _ = std::process::Command::new("claude")
            .args(["mcp", "remove", "--scope", "user", "stress"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}
