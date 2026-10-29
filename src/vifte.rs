//! Vifte (fan control), when installed and running: its safety state, read
//! from its service, so a load test never goes past what the fans are set up
//! for. Without Vifte this has no effect.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use serde_json::Value;

const SOCKET: &str = "/run/vifte/vifte.sock";
const TIMEOUT: Duration = Duration::from_millis(500);
/// Tests stop this far below Vifte's emergency temperature, so Stress ends
/// the load before Vifte has to step in.
pub const MARGIN: f64 = 5.0;

#[derive(Clone, Debug, Default)]
pub struct VifteState {
    /// Above this every fan Vifte controls runs at full speed.
    pub emergency_temp: Option<f64>,
    pub emergency_active: bool,
    /// A fan curve's temperature sensor stopped responding.
    pub sensor_lost: bool,
    /// The user handed the fans back to the BIOS.
    pub paused: bool,
}

impl VifteState {
    /// The highest temperature a test may reach while Vifte runs.
    pub fn limit(&self) -> Option<f64> {
        self.emergency_temp.map(|t| t - MARGIN)
    }
}

/// Vifte's state, or `None` when it is not installed, not running or not
/// reachable (the user is not in the `vifte` group).
pub fn state() -> Option<VifteState> {
    let mut stream = UnixStream::connect(SOCKET).ok()?;
    stream.set_read_timeout(Some(TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(TIMEOUT)).ok()?;
    stream.write_all(b"{\"cmd\":\"state\"}\n").ok()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    parse(&line)
}

fn parse(line: &str) -> Option<VifteState> {
    let reply: Value = serde_json::from_str(line).ok()?;
    if reply["ok"] != true {
        return None;
    }
    let failsafe = &reply["failsafe"];
    Some(VifteState {
        emergency_temp: (failsafe["enabled"] == true).then(|| failsafe["temp"].as_f64()).flatten(),
        emergency_active: failsafe["active"] == true,
        sensor_lost: reply["sensor_lost"]["controls"].as_object().is_some_and(|c| !c.is_empty()),
        paused: reply["paused"] == true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_safety_state() {
        let line = r#"{"ok": true, "failsafe": {"enabled": true, "temp": 95.0, "active": false},
            "sensor_lost": {"controls": {"/lpc/nct6798d/control/1": true}}, "paused": false}"#;
        let state = parse(line).unwrap();
        assert_eq!(state.limit(), Some(90.0));
        assert!(state.sensor_lost && !state.emergency_active && !state.paused);
        assert!(parse(r#"{"ok": false, "error": "not in group"}"#).is_none());
        let off = parse(r#"{"ok": true, "failsafe": {"enabled": false, "temp": 95.0}, "paused": true}"#).unwrap();
        assert_eq!(off.limit(), None);
        assert!(off.paused && !off.sensor_lost);
    }
}
