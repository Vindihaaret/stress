//! System changes Stress can make with the administrator's permission, and
//! undo again. A helper only ever writes files with "stresstest" in their
//! name, so whether it is installed is read from the system itself, and the
//! uninstaller always knows what to remove.

use std::path::Path;

use gtk::prelude::*;
use gtk::{gio, glib};

pub struct Helper {
    /// Files the helper puts on the system; any of them means installed.
    files: &'static [&'static str],
    setup: &'static str,
    remove: &'static str,
}

/// Makes the processor's energy counters readable, for its power use.
pub const RAPL: Helper = Helper {
    files: &["/etc/udev/rules.d/60-stresstest-rapl.rules"],
    setup: include_str!("../data/setup-rapl.sh"),
    remove: include_str!("../data/remove-rapl.sh"),
};

const ALL: &[&Helper] = &[&RAPL];

impl Helper {
    pub fn is_installed(&self) -> bool {
        self.files.iter().any(|f| Path::new(f).exists())
    }
}

/// The helpers that are on this system now.
pub fn installed() -> Vec<&'static Helper> {
    ALL.iter().copied().filter(|h| h.is_installed()).collect()
}

pub enum Error {
    /// The administrator password dialog was closed.
    Cancelled,
    Failed(String),
}

pub fn install(helper: &Helper, done: impl FnOnce(Result<(), Error>) + 'static) {
    run_privileged(helper.setup.to_owned(), done);
}

/// Removes several helpers with a single password prompt.
pub fn remove(helpers: &[&Helper], done: impl FnOnce(Result<(), Error>) + 'static) {
    let script = helpers.iter().map(|h| format!("(\n{}\n)", h.remove)).collect::<Vec<_>>().join("\n");
    run_privileged(script, done);
}

fn run_privileged(script: String, done: impl FnOnce(Result<(), Error>) + 'static) {
    const PKEXEC_DISMISSED: i32 = 126;
    let flags = gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_MERGE;
    let argv: [&std::ffi::OsStr; 4] = ["pkexec".as_ref(), "sh".as_ref(), "-c".as_ref(), script.as_ref()];
    let process = match gio::Subprocess::newv(&argv, flags) {
        Ok(process) => process,
        Err(err) => return done(Err(Error::Failed(err.to_string()))),
    };
    let Some(stdout) = process.stdout_pipe() else {
        return done(Err(Error::Failed("no output pipe".into())));
    };
    let reader = gio::DataInputStream::new(&stdout);
    glib::spawn_future_local(async move {
        let mut output = String::new();
        while let Ok(Some(line)) = reader.read_line_utf8_future(glib::Priority::DEFAULT).await {
            output.push_str(&line);
            output.push('\n');
        }
        let outcome = match process.wait_future().await {
            Err(err) => Err(Error::Failed(err.to_string())),
            Ok(()) if process.is_successful() => Ok(()),
            Ok(()) if process.exit_status() == PKEXEC_DISMISSED => Err(Error::Cancelled),
            Ok(()) => Err(Error::Failed(output.trim().to_owned())),
        };
        done(outcome);
    });
}
