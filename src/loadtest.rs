//! One load test at a time: starts and stops the CPU and GPU loads, changes
//! their levels while running, and after every sensor sample (on the sensor
//! thread, so a busy window cannot delay it) checks the limits and steers the
//! GPU toward the utilization asked for. When Vifte runs, its emergency
//! temperature (less a margin) also limits the test, and the test stops if
//! Vifte reports trouble.

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
    Running { started: Instant, ends: Instant, levels: Levels },
}

struct Running {
    cpu: Option<CpuLoad>,
    /// The worker and the load currently asked of it, which differs from the
    /// target while the utilization is being steered.
    gpu: Option<(GpuLoad, f64)>,
    levels: Levels,
    limits: Limits,
    started: Instant,
    ends: Instant,
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
        let this = self.clone();
        std::thread::spawn(move || {
            let gpu = match levels.gpu.and_then(|i| this.gpus.get(i)) {
                Some(pci) => match GpuLoad::start(pci, levels.gpu_load) {
                    Ok(load) => Some((load, levels.gpu_load as f64)),
                    Err(e) => {
                        this.finish(StopReason::Failed(e), Duration::ZERO);
                        return;
                    }
                },
                None => None,
            };
            let cpu = (levels.cpu_load > 0 && levels.cpu_threads > 0)
                .then(|| CpuLoad::start(levels.cpu_threads, levels.cpu_load));
            let started = Instant::now();
            let running = Running { cpu, gpu, levels, limits, started, ends: started + duration };
            let mut state = this.state.lock().unwrap();
            if matches!(state.phase, Phase::Starting { cancelled: true }) {
                state.phase = Phase::Idle;
                state.unseen = Some(Finished { reason: StopReason::User, ran: Duration::ZERO });
                drop(state);
                drop(running);
            } else {
                state.phase = Phase::Running(Box::new(running));
            }
        });
    }

    /// Changes the loads of a running test. The GPU itself cannot change.
    pub fn set_levels(&self, cpu_threads: usize, cpu_load: u32, gpu_load: u32) {
        let mut state = self.state.lock().unwrap();
        let Phase::Running(run) = &mut state.phase else { return };
        run.levels.cpu_threads = cpu_threads;
        run.levels.cpu_load = cpu_load;
        run.levels.gpu_load = gpu_load;
        match &run.cpu {
            Some(cpu) => cpu.set(cpu_threads, cpu_load),
            None if cpu_load > 0 && cpu_threads > 0 => run.cpu = Some(CpuLoad::start(cpu_threads, cpu_load)),
            None => {}
        }
        if let Some((gpu, command)) = &mut run.gpu {
            *command = gpu_load as f64;
            gpu.set(gpu_load);
        }
    }

    pub fn stop(&self) {
        let mut state = self.state.lock().unwrap();
        match &mut state.phase {
            Phase::Starting { cancelled } => *cancelled = true,
            Phase::Running(run) => {
                let ran = run.started.elapsed();
                drop(state);
                self.finish(StopReason::User, ran);
            }
            Phase::Idle => {}
        }
    }

    pub fn status(&self) -> Status {
        match &self.state.lock().unwrap().phase {
            Phase::Idle => Status::Idle,
            Phase::Starting { .. } => Status::Starting,
            Phase::Running(run) => Status::Running { started: run.started, ends: run.ends, levels: run.levels },
        }
    }

    /// The test that ended since the last call, if any.
    pub fn take_finished(&self) -> Option<Finished> {
        self.state.lock().unwrap().unseen.take()
    }

    /// Ends the test and stops its loads outside the lock: stopping the GPU
    /// worker can take a moment.
    fn finish(&self, reason: StopReason, ran: Duration) {
        let running = {
            let mut state = self.state.lock().unwrap();
            state.unseen = Some(Finished { reason, ran });
            std::mem::replace(&mut state.phase, Phase::Idle)
        };
        drop(running);
    }

    /// Called by the sensor thread after every sample. Returns the levels
    /// that were running, to be stored with the sample.
    pub fn after_sample(&self, sample: &Sample) -> Option<Levels> {
        // Asked outside the lock, and only while a test runs.
        let vifte = matches!(self.status(), Status::Running { .. }).then(vifte::state).flatten();
        let mut state = self.state.lock().unwrap();
        let Phase::Running(run) = &mut state.phase else { return None };
        let levels = run.levels;
        let gpu = levels.gpu.and_then(|i| sample.gpus.get(i));
        let vifte_limit = vifte.as_ref().and_then(|v| v.limit()).unwrap_or(f64::INFINITY);
        let (cpu_limit, gpu_limit) = (run.limits.cpu.min(vifte_limit), run.limits.gpu.min(vifte_limit));
        let reason = if Instant::now() >= run.ends {
            Some(StopReason::TimeUp)
        } else if let Some(t) = sample.cpu_temperature.filter(|t| *t >= cpu_limit) {
            Some(StopReason::CpuLimit(t))
        } else if let Some(t) = gpu.and_then(|g| g.temperature).filter(|t| *t >= gpu_limit) {
            Some(StopReason::GpuLimit(t))
        } else if vifte.as_ref().is_some_and(|v| v.emergency_active) {
            Some(StopReason::VifteEmergency)
        } else if vifte.as_ref().is_some_and(|v| v.sensor_lost) {
            Some(StopReason::VifteSensorLost)
        } else if vifte.as_ref().is_some_and(|v| v.paused) {
            Some(StopReason::ViftePaused)
        } else if run.limits.stop_on_battery && on_battery() {
            Some(StopReason::Battery)
        } else if run.gpu.as_mut().is_some_and(|(worker, _)| !worker.running()) {
            Some(StopReason::WorkerStopped)
        } else {
            None
        };
        if let Some(reason) = reason {
            let ran = run.started.elapsed();
            drop(state);
            self.finish(reason, ran);
            return Some(levels);
        }
        if let (Some((worker, command)), Some(utilization)) = (&mut run.gpu, gpu.and_then(|g| g.utilization)) {
            let target = levels.gpu_load as f64;
            if let Some(next) = steer(target, *command, utilization) {
                *command = next;
                worker.set(next.round() as u32);
            }
        }
        Some(levels)
    }
}

/// Next load to ask of the GPU worker so the measured utilization approaches
/// the target, or `None` to leave it. The duty cycle alone overshoots at low
/// loads because the GPU also finishes queued work in its pauses.
fn steer(target: f64, command: f64, utilization: f64) -> Option<f64> {
    if target <= 0.0 || target >= 100.0 {
        return None;
    }
    let next = (command + 0.5 * (target - utilization)).clamp(1.0, 100.0);
    ((next - command).abs() >= 0.5).then_some(next)
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

    #[test]
    fn steering_moves_toward_the_target() {
        assert_eq!(steer(60.0, 60.0, 68.0), Some(56.0));
        assert_eq!(steer(60.0, 56.0, 54.0), Some(59.0));
        assert_eq!(steer(60.0, 60.0, 60.2), None);
        assert_eq!(steer(100.0, 100.0, 95.0), None);
        assert_eq!(steer(5.0, 2.0, 40.0), Some(1.0));
    }
}
