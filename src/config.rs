//! Persistent settings: appearance, language, the safety limits and the last
//! test setup, stored as JSON in ~/.config/stresstest/config.json.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Ranges offered in the preferences; values outside them are pulled back in
/// when loading, so a hand-edited file cannot switch the safety off.
pub const CPU_LIMIT_RANGE: (f64, f64) = (60.0, 105.0);
pub const GPU_LIMIT_RANGE: (f64, f64) = (60.0, 100.0);
pub const MAX_MINUTES_RANGE: (f64, f64) = (1.0, 240.0);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Language code, or `None` to follow the desktop.
    pub language: Option<String>,
    pub appearance: Appearance,
    /// Tint the window after the temperature, as Vifte does.
    pub window_tint: bool,
    /// A test stops when the CPU reaches this temperature (°C).
    pub cpu_limit: f64,
    /// A test stops when a loaded GPU reaches this temperature (°C).
    pub gpu_limit: f64,
    /// No test runs longer than this, also when started without a duration.
    pub max_minutes: f64,
    /// Stop a test when the computer switches to battery power.
    pub stop_on_battery: bool,
    /// Processor threads to load; `None` for all.
    pub cpu_threads: Option<usize>,
    pub cpu_load: u32,
    /// PCI address of the graphics card to load; `None` for none.
    pub gpu: Option<String>,
    pub gpu_load: u32,
    /// Test length; 0 runs until stopped (at most `max_minutes`).
    pub duration_minutes: u32,
    /// Let Claude (through `stresstest mcp`) read the sensors and run tests.
    /// Off until the user turns it on.
    pub claude_control: bool,
    /// Closing the window keeps Stress in the status icon instead of quitting,
    /// and a running test carries on.
    pub close_to_tray: bool,
    /// Look for a new release on GitHub once a month.
    pub auto_update_check: bool,
    /// Unix time of the last successful update check.
    pub last_update_check: i64,
    /// A release tag the user chose to skip.
    pub skipped_version: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            language: None,
            appearance: Appearance::System,
            window_tint: true,
            cpu_limit: 95.0,
            gpu_limit: 90.0,
            max_minutes: 60.0,
            stop_on_battery: true,
            cpu_threads: None,
            cpu_load: 100,
            gpu: None,
            gpu_load: 100,
            duration_minutes: 10,
            claude_control: false,
            close_to_tray: false,
            auto_update_check: true,
            last_update_check: 0,
            skipped_version: None,
        }
    }
}

fn config_path() -> PathBuf {
    gtk::glib::user_config_dir().join("stresstest").join("config.json")
}

impl Config {
    pub fn load() -> Self {
        let mut config: Config = fs::read_to_string(config_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        config.cpu_limit = config.cpu_limit.clamp(CPU_LIMIT_RANGE.0, CPU_LIMIT_RANGE.1);
        config.gpu_limit = config.gpu_limit.clamp(GPU_LIMIT_RANGE.0, GPU_LIMIT_RANGE.1);
        config.max_minutes = config.max_minutes.clamp(MAX_MINUTES_RANGE.0, MAX_MINUTES_RANGE.1);
        config.cpu_load = config.cpu_load.min(100);
        config.gpu_load = config.gpu_load.min(100);
        config
    }

    pub fn save(&self) {
        let path = config_path();
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, text);
        }
    }
}
