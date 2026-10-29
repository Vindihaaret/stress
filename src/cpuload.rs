//! CPU load: one worker thread per logical CPU, of which the first `threads`
//! run dense FMA/AVX arithmetic for `load` percent of every 100 ms. Both can be
//! changed while running.

use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const PERIOD: Duration = Duration::from_millis(100);

struct Shared {
    threads: AtomicUsize,
    load: AtomicU32,
    stop: AtomicBool,
}

pub struct CpuLoad {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

impl CpuLoad {
    pub fn start(threads: usize, load: u32) -> CpuLoad {
        let shared = Arc::new(Shared {
            threads: AtomicUsize::new(0),
            load: AtomicU32::new(0),
            stop: AtomicBool::new(false),
        });
        let count = std::thread::available_parallelism().map_or(1, |n| n.get());
        let workers = (0..count)
            .map(|index| {
                let shared = shared.clone();
                std::thread::Builder::new()
                    .name(format!("cpu-load-{index}"))
                    .spawn(move || worker(index, &shared))
                    .expect("failed to start a CPU load thread")
            })
            .collect();
        let cpu = CpuLoad { shared, workers };
        cpu.set(threads, load);
        cpu
    }

    pub fn set(&self, threads: usize, load: u32) {
        self.shared.threads.store(threads.min(self.workers.len()), Ordering::Relaxed);
        self.shared.load.store(load.min(100), Ordering::Relaxed);
    }

    pub fn stop(mut self) {
        self.join();
    }

    fn join(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

impl Drop for CpuLoad {
    fn drop(&mut self) {
        self.join();
    }
}

fn worker(index: usize, shared: &Shared) {
    let fma = is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma");
    let mut acc = seeds();
    while !shared.stop.load(Ordering::Relaxed) {
        let start = Instant::now();
        let load = shared.load.load(Ordering::Relaxed);
        if index >= shared.threads.load(Ordering::Relaxed) || load == 0 {
            std::thread::sleep(PERIOD);
            continue;
        }
        let busy = PERIOD * load / 100;
        while start.elapsed() < busy && !shared.stop.load(Ordering::Relaxed) {
            if fma {
                unsafe { burn_fma(&mut acc) };
            } else {
                burn(&mut acc);
            }
            black_box(&mut acc);
        }
        if load < 100 {
            std::thread::sleep(PERIOD.saturating_sub(start.elapsed()));
        }
    }
}

/// About 50 µs of work: 128 independent chains of the logistic map
/// x' = 3.9·x·(1 − x). It stays between 0 and 1 but is chaotic, so the bits keep
/// changing; settling values would let the CPU draw noticeably less power.
const LANES: usize = 128;
const ROUNDS: usize = 1024;
const R: f32 = 3.9;

fn seeds() -> [f32; LANES] {
    std::array::from_fn(|i| 0.1 + 0.8 * i as f32 / LANES as f32)
}

#[inline(always)]
fn burn(acc: &mut [f32; LANES]) {
    for _ in 0..ROUNDS {
        for x in acc.iter_mut() {
            let t = *x * R;
            *x = t - t * *x;
        }
    }
}

#[target_feature(enable = "avx2,fma")]
unsafe fn burn_fma(acc: &mut [f32; LANES]) {
    for _ in 0..ROUNDS {
        for x in acc.iter_mut() {
            let t = *x * R;
            *x = (-t).mul_add(*x, t);
        }
    }
}
