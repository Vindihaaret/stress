//! Samples every sensor once a second on a background thread and keeps the
//! last 30 minutes, so the graphs have history and slow sensors (NVML, the
//! power supply over USB) never hold up the window.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::sensors::{Sample, Sensors};

pub const INTERVAL: Duration = Duration::from_secs(1);
pub const KEEP: Duration = Duration::from_secs(30 * 60);

pub struct Entry {
    pub at: Instant,
    pub sample: Sample,
}

#[derive(Clone)]
pub struct History(Arc<Mutex<VecDeque<Entry>>>);

impl History {
    /// Starts sampling with `sensors` for as long as the program runs.
    pub fn start(mut sensors: Sensors) -> History {
        let history = History(Arc::new(Mutex::new(VecDeque::with_capacity(KEEP.as_secs() as usize + 1))));
        let shared = history.0.clone();
        std::thread::Builder::new()
            .name("sensors".into())
            .spawn(move || {
                let mut next = Instant::now();
                loop {
                    let sample = sensors.sample();
                    let at = Instant::now();
                    {
                        let mut entries = shared.lock().unwrap();
                        entries.push_back(Entry { at, sample });
                        while entries.front().is_some_and(|e| at.duration_since(e.at) > KEEP) {
                            entries.pop_front();
                        }
                    }
                    next += INTERVAL;
                    let now = Instant::now();
                    if next < now {
                        next = now;
                    }
                    std::thread::sleep(next - now);
                }
            })
            .expect("failed to start the sensor thread");
        history
    }

    pub fn latest(&self) -> Option<Sample> {
        self.0.lock().unwrap().back().map(|e| e.sample.clone())
    }

    /// One value per entry, as (seconds ago, value), oldest first.
    pub fn series(&self, value: impl Fn(&Sample) -> Option<f64>) -> Vec<(f64, Option<f64>)> {
        let now = Instant::now();
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|e| (now.duration_since(e.at).as_secs_f64(), value(&e.sample)))
            .collect()
    }
}
