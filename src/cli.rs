//! Command line: `stresstest probe`, `monitor` and `run` for scripts and
//! testing, and the internal `gpu-worker` used by GPU loads.

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::sensors::{format, Sample, Sensors};
use crate::{cpuload, devices, gpuload};

const USAGE: &str = "Usage:
  stresstest                           open the window
  stresstest probe                     list CPU, GPUs and sensors
  stresstest monitor [--seconds N]     print sensor readings every second
  stresstest run [options]             run a load test from the command line
      --cpu-threads N   CPU threads to load (default: all)
      --cpu-load P      CPU load in percent (default: 0)
      --gpu PCI         GPU to load, e.g. 0000:01:00.0 (see probe)
      --gpu-load P      GPU load in percent (default: 0)
      --seconds N       duration (default: 60)
      --cpu-limit C     stop when the CPU reaches C °C (default: 95)
      --gpu-limit C     stop when the GPU reaches C °C (default: 90)";

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Runs a command-line subcommand, or returns `None` when the arguments are
/// for the window.
pub fn run_command(args: &[String]) -> Option<ExitCode> {
    if matches!(args.first().map(String::as_str), Some("probe" | "monitor" | "run" | "--help" | "-h" | "help")) {
        // Rust ignores SIGPIPE, which turns `stresstest probe | head` into a
        // panic once head has read enough; end quietly like other tools.
        unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    }
    let result = match args.first().map(String::as_str) {
        Some("gpu-worker") => std::process::exit(gpuload::worker_main(args.get(1).map_or("", String::as_str))),
        Some("probe") => probe(),
        Some("monitor") => Options::parse(&args[1..]).and_then(|o| monitor(o.seconds)),
        Some("run") => Options::parse(&args[1..]).and_then(run),
        Some("--help" | "-h" | "help") => {
            println!("{USAGE}");
            Ok(())
        }
        _ => return None,
    };
    Some(match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    })
}

struct Options {
    cpu_threads: Option<usize>,
    cpu_load: u32,
    gpu: Option<String>,
    gpu_load: u32,
    seconds: u64,
    cpu_limit: f64,
    gpu_limit: f64,
}

impl Options {
    fn parse(args: &[String]) -> Result<Options, String> {
        let mut o = Options {
            cpu_threads: None,
            cpu_load: 0,
            gpu: None,
            gpu_load: 0,
            seconds: 60,
            cpu_limit: 95.0,
            gpu_limit: 90.0,
        };
        let mut it = args.iter();
        while let Some(flag) = it.next() {
            let value = it.next().ok_or_else(|| format!("{flag} needs a value\n\n{USAGE}"))?;
            let bad = || format!("invalid value for {flag}: {value}");
            match flag.as_str() {
                "--cpu-threads" => o.cpu_threads = Some(value.parse().map_err(|_| bad())?),
                "--cpu-load" => o.cpu_load = value.parse().map_err(|_| bad())?,
                "--gpu" => o.gpu = Some(value.clone()),
                "--gpu-load" => o.gpu_load = value.parse().map_err(|_| bad())?,
                "--seconds" => o.seconds = value.parse().map_err(|_| bad())?,
                "--cpu-limit" => o.cpu_limit = value.parse().map_err(|_| bad())?,
                "--gpu-limit" => o.gpu_limit = value.parse().map_err(|_| bad())?,
                _ => return Err(format!("unknown option {flag}\n\n{USAGE}")),
            }
        }
        if o.cpu_load > 100 || o.gpu_load > 100 {
            return Err("loads are given in percent, 0–100".into());
        }
        Ok(o)
    }
}

fn probe() -> Result<(), String> {
    let cpu = devices::cpu();
    println!("CPU   {} ({} threads)", cpu.model, cpu.threads);
    let gpus = devices::gpus();
    let loadable = gpuload::loadable();
    for g in &gpus {
        let mut tags = vec![g.driver.clone().unwrap_or_else(|| "no driver".into())];
        if g.boot_vga {
            tags.push("display".into());
        }
        let status = match g.unavailable_reason() {
            Some(reason) => format!("cannot be loaded: {reason}"),
            None if loadable.contains(&g.pci) => "can be loaded".into(),
            None => "cannot be loaded: EGL cannot open it".into(),
        };
        println!("GPU   {}  {}  [{}]  {}", g.pci, g.name, tags.join(", "), status);
    }
    let mut sensors = Sensors::new(&gpus);
    if sensors.rapl_denied {
        println!("Note  CPU power (RAPL) is only readable by root on this system");
    }
    std::thread::sleep(Duration::from_secs(1));
    print_sample(&gpus, &sensors.sample());
    Ok(())
}

fn monitor(seconds: u64) -> Result<(), String> {
    catch_interrupt();
    let gpus = devices::gpus();
    let mut sensors = Sensors::new(&gpus);
    let end = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < end && !INTERRUPTED.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_secs(1));
        print_sample(&gpus, &sensors.sample());
    }
    Ok(())
}

fn run(o: Options) -> Result<(), String> {
    catch_interrupt();
    let gpus = devices::gpus();
    let cpu = devices::cpu();
    let gpu_index = match &o.gpu {
        Some(pci) => {
            let index = gpus.iter().position(|g| &g.pci == pci).ok_or_else(|| format!("no GPU at {pci} (see probe)"))?;
            if let Some(reason) = gpus[index].unavailable_reason() {
                return Err(format!("{} cannot be loaded: {reason}", gpus[index].name));
            }
            Some(index)
        }
        None if o.gpu_load > 0 => return Err("--gpu-load needs --gpu".into()),
        None => None,
    };
    let mut sensors = Sensors::new(&gpus);

    let threads = o.cpu_threads.unwrap_or(cpu.threads).min(cpu.threads);
    let cpu_load = (o.cpu_load > 0 && threads > 0).then(|| cpuload::CpuLoad::start(threads, o.cpu_load));
    if cpu_load.is_some() {
        println!("CPU load: {threads} of {} threads at {} %", cpu.threads, o.cpu_load);
    }
    let mut gpu_load = match gpu_index {
        Some(i) if o.gpu_load > 0 => {
            let load = gpuload::GpuLoad::start(&gpus[i].pci, o.gpu_load)?;
            println!("GPU load: {} at {} % (renderer: {})", gpus[i].name, o.gpu_load, load.renderer);
            Some(load)
        }
        _ => None,
    };

    let start = Instant::now();
    let end = start + Duration::from_secs(o.seconds);
    let reason = loop {
        if INTERRUPTED.load(Ordering::Relaxed) {
            break "interrupted".to_string();
        }
        if Instant::now() >= end {
            break "time is up".to_string();
        }
        std::thread::sleep(Duration::from_secs(1));
        let sample = sensors.sample();
        print!("{:>4}s  ", start.elapsed().as_secs());
        print_sample(&gpus, &sample);
        if let Some(t) = sample.cpu_temperature.filter(|t| *t >= o.cpu_limit) {
            break format!("the CPU reached {t:.0} °C (limit {:.0} °C)", o.cpu_limit);
        }
        if let Some(i) = gpu_index {
            if let Some(t) = sample.gpus[i].temperature.filter(|t| *t >= o.gpu_limit) {
                break format!("the GPU reached {t:.0} °C (limit {:.0} °C)", o.gpu_limit);
            }
            if gpu_load.as_mut().is_some_and(|g| !g.running()) {
                break "the GPU worker stopped unexpectedly".to_string();
            }
        }
    };
    if let Some(c) = cpu_load {
        c.stop();
    }
    if let Some(g) = gpu_load {
        g.stop();
    }
    println!("Stopped: {reason}");
    Ok(())
}

fn print_sample(gpus: &[devices::Gpu], s: &Sample) {
    let mut parts = vec![format!(
        "CPU {} {} {}",
        format(s.cpu_temperature, "°C", 0),
        format(s.cpu_utilization, "%", 0),
        format(s.cpu_power, "W", 0)
    )];
    for (g, r) in gpus.iter().zip(&s.gpus) {
        if g.drm_card.is_none() {
            continue;
        }
        parts.push(format!(
            "{} {} {} {}",
            short_name(&g.name),
            format(r.temperature, "°C", 0),
            format(r.utilization, "%", 0),
            format(r.power, "W", 0)
        ));
    }
    if let Some(p) = &s.psu {
        let rails: Vec<String> =
            p.rails.iter().map(|r| format!("{} {}", r.label, format(r.amps, "A", 1))).collect();
        parts.push(format!(
            "PSU out {} in ≈{} ({})",
            format(Some(p.output), "W", 0),
            format(p.input_estimate, "W", 0),
            rails.join(" ")
        ));
    }
    println!("{}", parts.join("  |  "));
}

fn short_name(name: &str) -> &str {
    name.trim_start_matches("NVIDIA ").trim_start_matches("GeForce ")
}

fn catch_interrupt() {
    extern "C" fn on_signal(_: libc::c_int) {
        INTERRUPTED.store(true, Ordering::Relaxed);
    }
    unsafe {
        libc::signal(libc::SIGINT, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
    }
}
