//! One load test at a time: starts and stops the CPU and GPU loads, changes
//! their levels while running, and after every sensor sample (on the sensor
//! thread, so a busy window cannot delay it) checks the limits. When Vifte runs, its emergency
//! temperature (less a margin) also limits the test, and the test stops if
//! Vifte reports trouble.
//!
//! Each test also has a watchdog on its own thread: should the sensor thread
//! hang (a graphics driver in trouble can block a reading), the test still
//! ends on time, and it ends early when no reading has been checked for a while.

use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cpuload::CpuLoad;
use crate::gpuload::GpuLoad;
use crate::sensors::Sample;
use crate::vifte;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Levels {
    pub cpu_threads: usize,
    pub cpu_load: u32,
    /// Index into the GPU list; fixed for the whole test.
    pub gpu: Option<usize>,
    pub gpu_load: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub cpu: f64,
    pub gpu: f64,
    pub stop_on_battery: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StopReason {
    User,
    TimeUp,
    CpuLimit(f64),
    GpuLimit(f64),
    Battery,
    /// The GPU worker exited (driver error or crash).
    WorkerStopped,
    VifteEmergency,
    VifteSensorLost,
    ViftePaused,
    /// The readings stopped coming, or a temperature that was there is gone,
    /// so the limits can no longer be checked.
    SensorsLost,
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct Finished {
    pub reason: StopReason,
    pub ran: Duration,
}

#[derive(Clone, Copy, Debug)]
pub enum Status {
    Idle,
    Starting,
    Running { started: Instant, ends: Instant, levels: Levels, limits: Limits },
}

/// The watchdog ends a test this long after its time is up,
const OVERTIME: Duration = Duration::from_secs(5);
/// and when the sensor thread has not checked it for this long.
const STALLED: Duration = Duration::from_secs(15);
const WATCH_INTERVAL: Duration = Duration::from_secs(1);
/// Samples in a row a loaded part's temperature may be missing, once it has
/// been read.
const MAX_MISSING: u32 = 5;
/// The debug log gets the readings this often while a test runs.
const LOG_EVERY: Duration = Duration::from_secs(10);

struct Running {
    cpu: Option<CpuLoad>,
    gpu: Option<GpuLoad>,
    levels: Levels,
    limits: Limits,
    started: Instant,
    ends: Instant,
    /// When the sensor thread last checked the limits.
    checked: Instant,
    /// The loaded parts' temperatures have been read during this test.
    cpu_seen: bool,
    gpu_seen: bool,
    /// Samples in a row with such a temperature missing.
    missing: u32,
    /// When the readings were last written to the debug log.
    logged: Option<Instant>,
}

enum Phase {
    Idle,
    Starting { cancelled: bool },
    Running(Box<Running>),
}

struct State {
    phase: Phase,
    /// A finished test the window has not reported yet.
    unseen: Option<Finished>,
}

#[derive(Clone)]
pub struct Controller {
    state: Arc<Mutex<State>>,
    /// PCI address of every GPU, by index.
    gpus: Arc<Vec<String>>,
}

impl Controller {
    pub fn new(gpus: Vec<String>) -> Controller {
        Controller {
            state: Arc::new(Mutex::new(State { phase: Phase::Idle, unseen: None })),
            gpus: Arc::new(gpus),
        }
    }

    /// Starts a test. Opening a GPU can take a few seconds, so that happens
    /// on its own thread; the status is `Starting` meanwhile.
    pub fn start(&self, levels: Levels, duration: Duration, limits: Limits) {
        {
            let mut state = self.state.lock().unwrap();
            if !matches!(state.phase, Phase::Idle) {
                return;
            }
            state.phase = Phase::Starting { cancelled: false };
        }
        dlog!("Test starts: {levels:?}, {} s, {limits:?}", duration.as_secs());
        let this = self.clone();
        std::thread::spawn(move || {
            let gpu = match levels.gpu.and_then(|i| this.gpus.get(i)) {
                Some(pci) => match GpuLoad::start(pci, levels.gpu_load) {
                    Ok(load) => Some(load),
                    Err(e) => {
                        this.fail(e);
                        return;
                    }
                },
                None => None,
            };
            let cpu = (levels.cpu_load > 0 && levels.cpu_threads > 0)
                .then(|| CpuLoad::start(levels.cpu_threads, levels.cpu_load));
            let started = Instant::now();
            let running = Running {
                cpu,
                gpu,
                levels,
                limits,
                started,
                ends: started + duration,
                checked: started,
                cpu_seen: false,
                gpu_seen: false,
                missing: 0,
                logged: None,
            };
            if let Some(gpu) = &running.gpu {
                dlog!("GPU load running on “{}”", gpu.renderer);
            }
            let mut state = this.state.lock().unwrap();
            if matches!(state.phase, Phase::Starting { cancelled: true }) {
                state.phase = Phase::Idle;
                state.unseen = Some(Finished { reason: StopReason::User, ran: Duration::ZERO });
                drop(state);
                drop(running);
                return;
            }
            state.phase = Phase::Running(Box::new(running));
            drop(state);
            while this.watch(started) {
                std::thread::sleep(WATCH_INTERVAL);
            }
        });
    }

    /// The watchdog's check. Returns false once the test that began at
    /// `started` is over.
    fn watch(&self, started: Instant) -> bool {
        let state = self.state.lock().unwrap();
        let Phase::Running(run) = &state.phase else { return false };
        if run.started != started {
            return false;
        }
        let now = Instant::now();
        let reason = if now >= run.ends + OVERTIME {
            StopReason::TimeUp
        } else if now.duration_since(run.checked) >= STALLED {
            StopReason::SensorsLost
        } else {
            return true;
        };
        Self::end(state, reason);
        false
    }

    /// Changes the loads of a running test. The GPU itself cannot change.
    pub fn set_levels(&self, cpu_threads: usize, cpu_load: u32, gpu_load: u32) {
        let mut state = self.state.lock().unwrap();
        let Phase::Running(run) = &mut state.phase else { return };
        dlog!("Load changed: {cpu_threads} threads at {cpu_load} %, GPU {gpu_load} %");
        run.levels.cpu_threads = cpu_threads;
        run.levels.cpu_load = cpu_load;
        run.levels.gpu_load = gpu_load;
        match &run.cpu {
            Some(cpu) => cpu.set(cpu_threads, cpu_load),
            None if cpu_load > 0 && cpu_threads > 0 => run.cpu = Some(CpuLoad::start(cpu_threads, cpu_load)),
            None => {}
        }
        if let Some(gpu) = &mut run.gpu {
            gpu.set(gpu_load);
        }
    }

    pub fn stop(&self) {
        let mut state = self.state.lock().unwrap();
        match &mut state.phase {
            Phase::Starting { cancelled } => *cancelled = true,
            Phase::Running(_) => Self::end(state, StopReason::User),
            Phase::Idle => {}
        }
    }

    pub fn status(&self) -> Status {
        match &self.state.lock().unwrap().phase {
            Phase::Idle => Status::Idle,
            Phase::Starting { .. } => Status::Starting,
            Phase::Running(run) => {
                Status::Running { started: run.started, ends: run.ends, levels: run.levels, limits: run.limits }
            }
        }
    }

    /// The test that ended since the last call, if any.
    pub fn take_finished(&self) -> Option<Finished> {
        self.state.lock().unwrap().unseen.take()
    }

    /// Ends the running test. The phase changes under the lock the caller
    /// holds, so two enders cannot both act; the loads stop outside it, since
    /// stopping the GPU worker can take a moment.
    fn end(mut state: std::sync::MutexGuard<State>, reason: StopReason) {
        let ran = match &state.phase {
            Phase::Running(run) => run.started.elapsed(),
            _ => Duration::ZERO,
        };
        dlog!("Test ended after {} s: {reason:?}", ran.as_secs());
        state.unseen = Some(Finished { reason, ran });
        let running = std::mem::replace(&mut state.phase, Phase::Idle);
        drop(state);
        drop(running);
    }

    /// A test that could not start.
    fn fail(&self, message: String) {
        Self::end(self.state.lock().unwrap(), StopReason::Failed(message));
    }

    /// Called by the sensor thread after every sample. Returns the levels
    /// that were running, to be stored with the sample.
    pub fn after_sample(&self, sample: &Sample) -> Option<Levels> {
        // Asked outside the lock, and only while a test runs.
        // (Not in unit tests: they must not depend on a Vifte on this computer.)
        let vifte = (!cfg!(test) && matches!(self.status(), Status::Running { .. })).then(vifte::state).flatten();
        let mut state = self.state.lock().unwrap();
        let Phase::Running(run) = &mut state.phase else { return None };
        let levels = run.levels;
        run.checked = Instant::now();
        let gpu = levels.gpu.and_then(|i| sample.gpus.get(i));
        let gpu_temperature = gpu.and_then(|g| g.temperature);
        // A temperature that was there and is gone means the limit can no
        // longer be checked. Parts that never report one are let be.
        run.cpu_seen |= sample.cpu_temperature.is_some();
        run.gpu_seen |= gpu_temperature.is_some();
        let cpu_gone = run.cpu_seen && run.cpu.is_some() && sample.cpu_temperature.is_none();
        let gpu_gone = run.gpu_seen && gpu_temperature.is_none();
        run.missing = if cpu_gone || gpu_gone { run.missing + 1 } else { 0 };
        let vifte_limit = vifte.as_ref().and_then(|v| v.limit()).unwrap_or(f64::INFINITY);
        let (cpu_limit, gpu_limit) = (run.limits.cpu.min(vifte_limit), run.limits.gpu.min(vifte_limit));
        let reason = if Instant::now() >= run.ends {
            Some(StopReason::TimeUp)
        } else if let Some(t) = sample.cpu_temperature.filter(|t| *t >= cpu_limit) {
            Some(StopReason::CpuLimit(t))
        } else if let Some(t) = gpu_temperature.filter(|t| *t >= gpu_limit) {
            Some(StopReason::GpuLimit(t))
        } else if run.missing >= MAX_MISSING {
            Some(StopReason::SensorsLost)
        } else if vifte.as_ref().is_some_and(|v| v.emergency_active) {
            Some(StopReason::VifteEmergency)
        } else if vifte.as_ref().is_some_and(|v| v.sensor_lost) {
            Some(StopReason::VifteSensorLost)
        } else if vifte.as_ref().is_some_and(|v| v.paused) {
            Some(StopReason::ViftePaused)
        } else if run.limits.stop_on_battery && on_battery() {
            Some(StopReason::Battery)
        } else if run.gpu.as_mut().is_some_and(|worker| !worker.running()) {
            Some(StopReason::WorkerStopped)
        } else {
            None
        };
        if reason.is_some() || run.logged.is_none_or(|at| at.elapsed() >= LOG_EVERY) {
            run.logged = Some(Instant::now());
            dlog!(
                "{} · limits CPU {cpu_limit:.0} GPU {gpu_limit:.0}{}",
                sample.summary(),
                vifte.as_ref().map_or(String::new(), |v| format!(" · Vifte {v:?}"))
            );
        }
        if let Some(reason) = reason {
            Self::end(state, reason);
        }
        Some(levels)
    }
}

/// True when the computer has a battery and no mains power.
pub fn on_battery() -> bool {
    let Ok(entries) = fs::read_dir("/sys/class/power_supply") else { return false };
    let mut battery = false;
    for entry in entries.flatten() {
        let read = |name: &str| fs::read_to_string(entry.path().join(name)).unwrap_or_default();
        match read("type").trim() {
            "Mains" if read("online").trim() == "1" => return false,
            "Battery" if read("scope").trim() != "Device" => battery = true,
            _ => {}
        }
    }
    battery
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: Limits = Limits { cpu: 95.0, gpu: 90.0, stop_on_battery: false };

    fn start(controller: &Controller, duration: Duration) {
        // No load at all, so the test only exercises the bookkeeping.
        let levels = Levels { cpu_threads: 1, cpu_load: 0, gpu: None, gpu_load: 0 };
        controller.start(levels, duration, LIMITS);
        while matches!(controller.status(), Status::Starting) {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn running(controller: &Controller) -> bool {
        matches!(controller.status(), Status::Running { .. })
    }

    #[test]
    fn the_limits_stop_a_test() {
        let controller = Controller::new(Vec::new());
        start(&controller, Duration::from_secs(60));
        let cool = Sample { cpu_temperature: Some(60.0), ..Default::default() };
        assert!(controller.after_sample(&cool).is_some());
        assert!(running(&controller));
        let hot = Sample { cpu_temperature: Some(95.0), ..Default::default() };
        controller.after_sample(&hot);
        assert_eq!(controller.take_finished().unwrap().reason, StopReason::CpuLimit(95.0));
        assert!(controller.after_sample(&hot).is_none());
    }

    #[test]
    fn stopping_reports_once() {
        let controller = Controller::new(Vec::new());
        start(&controller, Duration::from_secs(60));
        controller.stop();
        controller.stop();
        assert_eq!(controller.take_finished().unwrap().reason, StopReason::User);
        assert!(controller.take_finished().is_none());
    }

    #[test]
    fn the_watchdog_ends_a_test_nobody_checks() {
        let controller = Controller::new(Vec::new());
        start(&controller, Duration::ZERO);
        let deadline = Instant::now() + OVERTIME + Duration::from_secs(5);
        while running(&controller) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(controller.take_finished().unwrap().reason, StopReason::TimeUp);
    }
}
