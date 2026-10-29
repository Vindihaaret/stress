//! `stresstest mcp`: a Model Context Protocol server on stdin/stdout for
//! Claude Code and Claude Desktop. It only forwards requests to the running
//! Stress window over D-Bus; the window checks what the user allowed and
//! enforces the limits.

use std::io::{self, BufRead, Write};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use gtk::glib::prelude::*;
use gtk::{gio, glib};
use serde_json::{json, Value};

use crate::ui::claude::{INTERFACE, OBJECT_PATH};
use crate::APP_ID;

const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const CALL_TIMEOUT_MS: i32 = 20_000;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);

const INSTRUCTIONS: &str = "Stress puts a controlled load on this computer's processor and graphics card and records \
temperatures, utilization, fan speed and power once a second. Typical flow: get_status, then start_test with the load \
you need, follow it with get_status or get_history, and stop_test when done. Tests stop by themselves when the time is \
up or a temperature limit is reached; the user's limits always apply and you can only make them stricter. When Vifte \
(fan control) runs, tests also stop 5 °C below Vifte's emergency temperature and whenever Vifte reports a problem. \
To tune fan curves together with Vifte: start a steady load, wait until temperatures level off (often 3–10 minutes), \
read get_history, change the curves in Vifte, and run the same load again to compare. Loads heat the computer and \
make the fans loud, so tell the user before starting a long or heavy test. The user decides in Stress → Preferences → \
Claude whether you may use it; if a tool says it is not allowed, ask them to turn it on instead of retrying.";

fn tools() -> Value {
    let none = json!({ "type": "object", "properties": {} });
    json!([
        { "name": "get_status", "inputSchema": none,
          "description": "Processor, graphics cards (with the PCI address used to pick one and whether it can be loaded), power supply, current readings, the user's limits, Vifte's safety state, and the running or last test. Call this first." },
        { "name": "get_history",
          "description": "Readings for the last minutes, sampled every second and thinned to at most max_points: processor and graphics card temperature, utilization, power and fan, power supply output, and which load test was running at each point.",
          "inputSchema": { "type": "object", "properties": {
              "minutes": { "type": "number", "description": "1–30, default 10" },
              "max_points": { "type": "integer", "description": "default 60, at most 600" } } } },
        { "name": "start_test",
          "description": "Start a load test. Give at least one of cpu_load and gpu_load. The test stops by itself after `minutes`, at the user's temperature limits, or earlier limits you set here (they can only be lower than the user's).",
          "inputSchema": { "type": "object", "properties": {
              "cpu_load": { "type": "integer", "description": "processor load in percent, 0–100" },
              "cpu_threads": { "type": "integer", "description": "threads to load; default all" },
              "gpu": { "type": "string", "description": "PCI address of the graphics card from get_status; default the first one that can be loaded" },
              "gpu_load": { "type": "integer", "description": "graphics card utilization in percent, 0–100" },
              "minutes": { "type": "number", "description": "default 5; at most the user's longest test" },
              "cpu_limit": { "type": "number", "description": "stop when the processor reaches this °C (only lower than the user's)" },
              "gpu_limit": { "type": "number", "description": "stop when the graphics card reaches this °C (only lower than the user's)" } } } },
        { "name": "set_load",
          "description": "Change the loads of the running test, for example to step it up. The graphics card and the duration stay as started.",
          "inputSchema": { "type": "object", "properties": {
              "cpu_load": { "type": "integer" }, "cpu_threads": { "type": "integer" }, "gpu_load": { "type": "integer" } } } },
        { "name": "stop_test", "inputSchema": none, "description": "Stop the running test." }
    ])
}

fn has_owner(connection: &gio::DBusConnection) -> bool {
    connection
        .call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            Some(&(APP_ID,).to_variant()),
            Some(glib::VariantTy::new("(b)").unwrap()),
            gio::DBusCallFlags::NONE,
            2000,
            None::<&gio::Cancellable>,
        )
        .ok()
        .and_then(|reply| reply.get::<(bool,)>())
        .is_some_and(|(owned,)| owned)
}

/// Starts Stress if it is not running, so the user sees what Claude does.
fn ensure_running(connection: &gio::DBusConnection) -> Result<(), String> {
    if has_owner(connection) {
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Command::new(exe)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not start Stress: {e}"))?;
    let start = Instant::now();
    while start.elapsed() < STARTUP_TIMEOUT {
        if has_owner(connection) {
            // Give the window a moment to take its first readings.
            std::thread::sleep(Duration::from_secs(2));
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err("Stress did not start. Ask the user to open it.".into())
}

fn call_app(method: &str, arguments: &Value) -> Result<Value, String> {
    let connection = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>).map_err(|e| e.to_string())?;
    ensure_running(&connection)?;
    let call = || {
        connection.call_sync(
            Some(APP_ID),
            OBJECT_PATH,
            INTERFACE,
            "Call",
            Some(&(method, arguments.to_string()).to_variant()),
            Some(glib::VariantTy::new("(s)").unwrap()),
            gio::DBusCallFlags::NONE,
            CALL_TIMEOUT_MS,
            None::<&gio::Cancellable>,
        )
    };
    // Right after start-up the object may not be registered yet.
    let mut reply = call();
    for _ in 0..10 {
        match &reply {
            Err(err) if err.message().contains("No such") || err.message().contains("UnknownObject") => {
                std::thread::sleep(Duration::from_millis(300));
                reply = call();
            }
            _ => break,
        }
    }
    let reply = reply.map_err(|e| format!("Could not reach Stress: {}", e.message()))?;
    let (json,) = reply.get::<(String,)>().ok_or("Unexpected reply from Stress")?;
    serde_json::from_str(&json).map_err(|e| e.to_string())
}

fn call_tool(params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
    let arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let known = tools().as_array().is_some_and(|t| t.iter().any(|tool| tool["name"] == name));
    if !known {
        return json!({ "isError": true, "content": [{ "type": "text", "text": format!("Unknown tool “{name}”.") }] });
    }
    match call_app(name, &arguments) {
        Ok(result) if result["ok"] == true => {
            let text = result.get("text").and_then(Value::as_str).unwrap_or_default();
            json!({ "content": [{ "type": "text", "text": text }] })
        }
        Ok(result) => {
            let error = result.get("error").and_then(Value::as_str).unwrap_or("Stress refused the request.");
            json!({ "isError": true, "content": [{ "type": "text", "text": error }] })
        }
        Err(error) => json!({ "isError": true, "content": [{ "type": "text", "text": error }] }),
    }
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL_VERSIONS[0]);
    let version = PROTOCOL_VERSIONS.iter().find(|v| **v == asked).copied().unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "stress", "title": "Stress", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

pub fn run() -> ExitCode {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(_) => {
                let error = json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "Parse error" } });
                let _ = writeln!(stdout, "{error}");
                let _ = stdout.flush();
                continue;
            }
        };
        // Notifications (no id) need no answer.
        let Some(id) = message.get("id").cloned() else { continue };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let response = match message.get("method").and_then(Value::as_str).unwrap_or_default() {
            "initialize" => json!({ "jsonrpc": "2.0", "id": id, "result": initialize(&params) }),
            "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
            "tools/list" => json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": tools() } }),
            "tools/call" => json!({ "jsonrpc": "2.0", "id": id, "result": call_tool(&params) }),
            other => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("Method not found: {other}") } }),
        };
        let _ = writeln!(stdout, "{response}");
        let _ = stdout.flush();
    }
    ExitCode::SUCCESS
}
