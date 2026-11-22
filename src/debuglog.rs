//! Debug log, as in Waypoint. With "Debug log" turned on (Preferences →
//! General, `debug` in the config) every Stress process – the window, the
//! MCP server for Claude, the GPU worker and the command line – writes what
//! it does to ~/.local/state/stresstest/logs/stress-<date>.log: start-up and
//! the hardware found, tests with their setup, readings every 10 seconds and
//! why they ended, Claude's requests, update checks, helper installs and
//! unexpected errors (panics).
//!
//! The folder is readable only by the user, a day's file stops at
//! `MAX_BYTES`, and files are deleted after `KEEP_DAYS` days. Stress has no
//! passwords; the logs hold paths and hardware names, nothing else private.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use gtk::glib;

pub const KEEP_DAYS: u64 = 14;
const MAX_BYTES: u64 = 20_000_000;
/// The setting is read again at most this often, so turning it on or off in
/// the window also reaches the other processes (the MCP server runs for days).
const RECHECK: Duration = Duration::from_secs(5);

struct State {
    on: bool,
    checked: Option<Instant>,
}

static STATE: Mutex<State> = Mutex::new(State { on: false, checked: None });
static ROLE: OnceLock<String> = OnceLock::new();
static WRITING: Mutex<()> = Mutex::new(());
static FIRST: AtomicBool = AtomicBool::new(true);

pub fn log_dir() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| glib::home_dir().join(".local/state"));
    state.join("stresstest").join("logs")
}

/// True when the debug log is turned on.
pub fn enabled() -> bool {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if state.checked.is_none_or(|at| at.elapsed() >= RECHECK) {
        state.on = !cfg!(test) && crate::config::Config::load().debug;
        state.checked = Some(Instant::now());
    }
    state.on
}

/// Called right after the setting changed in this process.
pub fn set_enabled(on: bool) {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.on = on;
    state.checked = Some(Instant::now());
}

/// Writes one line (or several) when the log is on. Use [`dlog!`].
pub fn write(message: &str) {
    if !enabled() {
        return;
    }
    let _guard = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let dir = log_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    if FIRST.swap(false, Ordering::Relaxed) {
        cleanup();
    }
    let now = glib::DateTime::now_local().ok();
    let day = now.as_ref().and_then(|t| t.format("%Y-%m-%d").ok()).map_or_else(|| "today".into(), |s| s.to_string());
    let time = now.as_ref().and_then(|t| t.format("%H:%M:%S.%f").ok()).map_or_else(String::new, |s| s[..12.min(s.len())].to_string());
    let path = dir.join(format!("stress-{day}.log"));
    if fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        return;
    }
    let role = ROLE.get().map_or("stress", String::as_str);
    let pid = std::process::id();
    let mut text = String::new();
    for (i, line) in message.trim_end().lines().enumerate() {
        if i == 0 {
            text += &format!("{time} [{pid} {role}] {line}\n");
        } else {
            text += &format!("    {line}\n");
        }
    }
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).mode(0o600).open(&path) {
        let _ = file.write_all(text.as_bytes());
    }
}

/// Writes to the debug log when it is on; the text is only formatted then.
#[macro_export]
macro_rules! dlog {
    ($($arg:tt)*) => {
        if $crate::debuglog::enabled() {
            $crate::debuglog::write(&format!($($arg)*));
        }
    };
}

/// Called once when a process starts: names it in the log, logs panics, and
/// writes the versions it runs with.
pub fn setup(role: &str) {
    let _ = ROLE.set(role.to_owned());
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // No backtrace: the released program carries no symbols, so it would be empty.
        let thread = std::thread::current();
        write(&format!("PANIC in thread {}: {info}", thread.name().unwrap_or("?")));
        previous(info);
    }));
    if role == "gpu-worker" || !enabled() {
        return;
    }
    write(&format!("Stress {} starts, args {:?}\n{}", env!("CARGO_PKG_VERSION"), std::env::args().skip(1).collect::<Vec<_>>(), environment()));
}

/// The system, desktop and library versions, for the log.
pub fn environment() -> String {
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    let os = fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_owned())))
        .unwrap_or_default();
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    // The ffi calls, since the safe ones insist that GTK is started first.
    let (gtk, adw) = unsafe {
        (
            (gtk::ffi::gtk_get_major_version(), gtk::ffi::gtk_get_minor_version(), gtk::ffi::gtk_get_micro_version()),
            (adw::ffi::adw_get_major_version(), adw::ffi::adw_get_minor_version(), adw::ffi::adw_get_micro_version()),
        )
    };
    format!(
        "{os}, kernel {}, {desktop} ({session}), GTK {}.{}.{}, libadwaita {}.{}.{}",
        kernel.trim(),
        gtk.0,
        gtk.1,
        gtk.2,
        adw.0,
        adw.1,
        adw.2,
    )
}

/// Deletes logs older than `KEEP_DAYS` days.
pub fn cleanup() {
    let Ok(entries) = fs::read_dir(log_dir()) else { return };
    let limit = Duration::from_secs(KEEP_DAYS * 86_400);
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age > limit);
        if old && entry.path().extension().is_some_and(|e| e == "log") {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// The newest log file, if any.
pub fn latest() -> Option<PathBuf> {
    fs::read_dir(log_dir())
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

/// The end of a command's output, for the log.
pub fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.trim_end().lines().collect();
    let skip = all.len().saturating_sub(lines);
    let mut out = String::new();
    if skip > 0 {
        out += &format!("… ({skip} lines left out)\n");
    }
    for line in &all[skip..] {
        let line: String = line.chars().take(500).collect();
        out += &line;
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_the_end() {
        let text = (1..=50).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
        let t = tail(&text, 3);
        assert_eq!(t, "… (47 lines left out)\n48\n49\n50\n");
        assert_eq!(tail("a\nb", 5), "a\nb\n");
    }
}
