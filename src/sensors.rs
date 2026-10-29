//! Live readings: CPU temperature, utilization and power (RAPL), each GPU, and
//! the power supply when it reports itself (Corsair HXi/RMi/AXi over USB).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::devices::{Gpu, NVIDIA};
use crate::nvml;

#[derive(Clone, Default, Debug)]
pub struct GpuReading {
    pub temperature: Option<f64>,
    pub utilization: Option<f64>,
    pub power: Option<f64>,
    pub fan_percent: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct Rail {
    pub label: String,
    pub volts: Option<f64>,
    pub amps: Option<f64>,
    pub watts: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct PsuReading {
    pub name: String,
    /// Power delivered to the computer (DC side).
    pub output: f64,
    /// Power drawn from the wall, estimated from the efficiency class; the PSU
    /// does not measure it itself.
    pub input_estimate: Option<f64>,
    pub input_volts: Option<f64>,
    pub temperature: Option<f64>,
    pub fan_rpm: Option<f64>,
    pub rails: Vec<Rail>,
}

#[derive(Clone, Debug, Default)]
pub struct Sample {
    pub cpu_temperature: Option<f64>,
    pub cpu_utilization: Option<f64>,
    pub cpu_power: Option<f64>,
    /// Same order as the GPUs given to [`Sensors::new`].
    pub gpus: Vec<GpuReading>,
    pub psu: Option<PsuReading>,
}

enum GpuSource {
    Nvidia(String),
    /// amdgpu and Intel: hwmon for temperature and power, `gpu_busy_percent`
    /// for utilization where the driver has it.
    Sysfs { hwmon: Option<PathBuf>, busy: PathBuf, energy: Option<(f64, Instant)> },
}

struct Rapl {
    energy: PathBuf,
    max: f64,
    last: Option<(f64, Instant)>,
}

struct Psu {
    hwmon: PathBuf,
    name: String,
    rated_watts: Option<f64>,
}

pub struct Sensors {
    cpu_temperature: Option<PathBuf>,
    rapl: Vec<Rapl>,
    /// The CPU has RAPL but its counters are only readable by root.
    pub rapl_denied: bool,
    last_stat: Option<(u64, u64)>,
    gpus: Vec<GpuSource>,
    psu: Option<Psu>,
}

impl Sensors {
    pub fn new(gpus: &[Gpu]) -> Sensors {
        let hwmons = hwmons();
        let (rapl, rapl_denied) = find_rapl();
        let gpus = gpus
            .iter()
            .map(|g| {
                if g.vendor == NVIDIA && nvml::get().is_some() {
                    return GpuSource::Nvidia(g.pci.clone());
                }
                let dir = Path::new("/sys/bus/pci/devices").join(&g.pci);
                let hwmon = fs::read_dir(dir.join("hwmon")).ok().and_then(|d| d.flatten().next()).map(|e| e.path());
                GpuSource::Sysfs { hwmon, busy: dir.join("gpu_busy_percent"), energy: None }
            })
            .collect();
        let psu = hwmons.iter().find(|(_, name)| name == "corsairpsu").map(|(path, _)| {
            let product = product_name(path);
            Psu {
                hwmon: path.clone(),
                rated_watts: product.as_deref().and_then(rated_watts),
                name: product.unwrap_or_else(|| "Corsair PSU".into()),
            }
        });
        let mut sensors = Sensors {
            cpu_temperature: find_cpu_temperature(&hwmons),
            rapl,
            rapl_denied,
            last_stat: None,
            gpus,
            psu,
        };
        // Prime the counters so the first real sample has a time span.
        sensors.sample();
        sensors
    }

    pub fn sample(&mut self) -> Sample {
        let now = Instant::now();
        // Access may be granted while running (the RAPL helper).
        if self.rapl_denied && self.rapl.is_empty() {
            (self.rapl, self.rapl_denied) = find_rapl();
        }
        let cpu_power = self
            .rapl
            .iter_mut()
            .map(|r| {
                let energy = read_f64(&r.energy)?;
                let last = r.last.replace((energy, now));
                let (e0, t0) = last?;
                // The counter wraps at max_energy_range_uj.
                let delta = if energy >= e0 { energy - e0 } else { energy + r.max - e0 };
                Some(delta / 1e6 / now.duration_since(t0).as_secs_f64())
            })
            .try_fold(0.0, |sum, w| w.map(|w| sum + w))
            .filter(|_| !self.rapl.is_empty());
        Sample {
            cpu_temperature: self.cpu_temperature.as_deref().and_then(read_f64).map(|v| v / 1000.0),
            cpu_utilization: self.cpu_utilization(),
            cpu_power,
            gpus: self.gpus.iter_mut().map(|g| read_gpu(g, now)).collect(),
            psu: self.psu.as_ref().and_then(read_psu),
        }
    }

    fn cpu_utilization(&mut self) -> Option<f64> {
        let stat = fs::read_to_string("/proc/stat").ok()?;
        let fields: Vec<u64> =
            stat.lines().next()?.split_whitespace().skip(1).take(8).filter_map(|v| v.parse().ok()).collect();
        if fields.len() < 5 {
            return None;
        }
        let idle = fields[3] + fields[4];
        let total: u64 = fields.iter().sum();
        let (idle0, total0) = self.last_stat.replace((idle, total))?;
        let span = total.saturating_sub(total0);
        (span > 0).then(|| 100.0 * (1.0 - idle.saturating_sub(idle0) as f64 / span as f64))
    }
}

fn read_gpu(source: &mut GpuSource, now: Instant) -> GpuReading {
    match source {
        GpuSource::Nvidia(pci) => nvml::get().map(|n| n.read(pci)).unwrap_or_default(),
        GpuSource::Sysfs { hwmon, busy, energy } => {
            let mut reading = GpuReading { utilization: read_f64(busy), ..Default::default() };
            if let Some(h) = hwmon {
                reading.temperature = read_f64(&h.join("temp1_input")).map(|v| v / 1000.0);
                reading.power = read_f64(&h.join("power1_average"))
                    .or_else(|| read_f64(&h.join("power1_input")))
                    .map(|v| v / 1e6);
                // Intel reports an energy counter (µJ) instead of power.
                if reading.power.is_none() {
                    if let Some(e) = read_f64(&h.join("energy1_input")) {
                        if let Some((e0, t0)) = energy.replace((e, now)) {
                            let secs = now.duration_since(t0).as_secs_f64();
                            if e >= e0 && secs > 0.0 {
                                reading.power = Some((e - e0) / 1e6 / secs);
                            }
                        }
                    }
                }
            }
            reading
        }
    }
}

fn read_psu(psu: &Psu) -> Option<PsuReading> {
    let channels = labelled(&psu.hwmon);
    let value = |label: &str| channels.iter().find(|(l, _)| l == label).map(|(_, v)| *v);
    let output = value("power total")?;
    // Rails are named by their voltage: "power +12v", "v_out +12v", "curr +12v".
    let mut rails: Vec<Rail> = channels
        .iter()
        .filter_map(|(l, _)| l.strip_prefix("power +"))
        .map(|rail| Rail {
            label: format!("+{}", rail.to_uppercase()),
            volts: value(&format!("v_out +{rail}")),
            amps: value(&format!("curr +{rail}")),
            watts: value(&format!("power +{rail}")),
        })
        .collect();
    rails.sort_by(|a, b| b.volts.partial_cmp(&a.volts).unwrap_or(std::cmp::Ordering::Equal));
    Some(PsuReading {
        name: psu.name.clone(),
        output,
        input_estimate: psu.rated_watts.and_then(|rated| {
            efficiency(&psu.name, output / rated).map(|eff| output / eff)
        }),
        input_volts: value("v_in"),
        temperature: read_f64(&psu.hwmon.join("temp1_input")).map(|v| v / 1000.0),
        fan_rpm: read_f64(&psu.hwmon.join("fan1_input")),
        rails,
    })
}

/// Every hwmon channel with a label, in watts, volts, amperes or as is.
fn labelled(hwmon: &Path) -> Vec<(String, f64)> {
    let Ok(entries) = fs::read_dir(hwmon) else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|e| {
            let file = e.file_name().to_string_lossy().into_owned();
            let channel = file.strip_suffix("_label")?;
            let label = fs::read_to_string(e.path()).ok()?.trim().to_string();
            let raw = read_f64(&hwmon.join(format!("{channel}_input")))?;
            let scale = if channel.starts_with("power") { 1e6 } else if channel.starts_with("in") || channel.starts_with("curr") { 1e3 } else { 1.0 };
            Some((label, raw / scale))
        })
        .collect()
}

/// Typical 80 PLUS efficiency at 230 V by load fraction, for Corsair's naming:
/// AXi is Titanium, HXi Platinum, RMi Gold. Below 10 % load the curves are
/// extrapolated, so the estimate is rougher at idle.
fn efficiency(name: &str, load: f64) -> Option<f64> {
    let model = name.trim_start_matches("Corsair ").to_uppercase();
    let curve: &[(f64, f64)] = if model.starts_with("AX") {
        &[(0.02, 0.80), (0.05, 0.88), (0.10, 0.92), (0.20, 0.94), (0.50, 0.96), (1.0, 0.94)]
    } else if model.starts_with("HX") {
        &[(0.02, 0.75), (0.05, 0.85), (0.10, 0.89), (0.20, 0.92), (0.50, 0.94), (1.0, 0.91)]
    } else if model.starts_with("RM") {
        &[(0.02, 0.72), (0.05, 0.82), (0.10, 0.87), (0.20, 0.90), (0.50, 0.92), (1.0, 0.89)]
    } else {
        return None;
    };
    let load = load.clamp(curve[0].0, 1.0);
    curve.windows(2).find(|w| load <= w[1].0).map(|w| {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        y0 + (y1 - y0) * (load - x0) / (x1 - x0)
    })
}

/// "HX1000i Power Supply" -> 1000 W.
fn rated_watts(product: &str) -> Option<f64> {
    let model = product.trim_start_matches("Corsair ").split_whitespace().next()?;
    let digits: String = model.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok().filter(|w| *w >= 200.0)
}

/// USB product string of the device behind a hwmon, e.g. "HX1000i Power Supply".
fn product_name(hwmon: &Path) -> Option<String> {
    let mut dir = fs::canonicalize(hwmon.join("device")).ok()?;
    loop {
        if let Ok(product) = fs::read_to_string(dir.join("product")) {
            let product = product.trim().trim_end_matches(" Power Supply").to_string();
            return Some(format!("Corsair {product}"));
        }
        if !dir.pop() || dir == Path::new("/sys/devices") {
            return None;
        }
    }
}

fn hwmons() -> Vec<(PathBuf, String)> {
    let Ok(entries) = fs::read_dir("/sys/class/hwmon") else { return Vec::new() };
    let mut list: Vec<_> = entries
        .flatten()
        .filter_map(|e| {
            let name = fs::read_to_string(e.path().join("name")).ok()?;
            Some((e.path(), name.trim().to_string()))
        })
        .collect();
    list.sort();
    list
}

/// Tctl/Tdie on AMD (k10temp, zenpower), the package sensor on Intel (coretemp).
fn find_cpu_temperature(hwmons: &[(PathBuf, String)]) -> Option<PathBuf> {
    for (path, name) in hwmons {
        let wanted: &[&str] = match name.as_str() {
            "k10temp" => &["Tctl", "Tdie"],
            "zenpower" => &["Tdie", "Tctl"],
            "coretemp" => &["Package id 0"],
            _ => continue,
        };
        for label in wanted {
            for i in 1..=16 {
                if fs::read_to_string(path.join(format!("temp{i}_label"))).is_ok_and(|l| l.trim() == *label) {
                    return Some(path.join(format!("temp{i}_input")));
                }
            }
        }
        return Some(path.join("temp1_input"));
    }
    None
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RaplAccess {
    Readable,
    /// The counters exist but only root can read them.
    Denied,
    /// The processor does not report its energy use.
    Missing,
}

pub fn rapl_access() -> RaplAccess {
    match find_rapl() {
        (domains, _) if !domains.is_empty() => RaplAccess::Readable,
        (_, true) => RaplAccess::Denied,
        _ => RaplAccess::Missing,
    }
}

/// The package domains under /sys/class/powercap (also used by AMD), and
/// whether they exist but are unreadable without root.
fn find_rapl() -> (Vec<Rapl>, bool) {
    let Ok(entries) = fs::read_dir("/sys/class/powercap") else { return (Vec::new(), false) };
    let mut denied = false;
    let mut domains: Vec<Rapl> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with("intel-rapl:") && name.matches(':').count() == 1
        })
        .filter(|e| fs::read_to_string(e.path().join("name")).is_ok_and(|n| n.starts_with("package")))
        .filter_map(|e| {
            let energy = e.path().join("energy_uj");
            if read_f64(&energy).is_none() {
                denied |= energy.exists();
                return None;
            }
            Some(Rapl { max: read_f64(&e.path().join("max_energy_range_uj")).unwrap_or(f64::MAX), energy, last: None })
        })
        .collect();
    domains.sort_by(|a, b| a.energy.cmp(&b.energy));
    (domains, denied)
}

/// "55 °C", or "– °C" when the reading is missing.
pub fn format(value: Option<f64>, unit: &str, decimals: usize) -> String {
    match value {
        Some(v) => format!("{v:.decimals$} {unit}"),
        None => format!("– {unit}"),
    }
}

fn read_f64(path: &Path) -> Option<f64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rated_watts_from_product() {
        assert_eq!(rated_watts("HX1000i Power Supply"), Some(1000.0));
        assert_eq!(rated_watts("RM850x"), Some(850.0));
        assert_eq!(rated_watts("Corsair HX1000i"), Some(1000.0));
        assert_eq!(rated_watts("Something"), None);
    }

    #[test]
    fn efficiency_interpolates() {
        let eff = efficiency("Corsair HX1000i", 0.35).unwrap();
        assert!(eff > 0.92 && eff < 0.94);
        assert_eq!(efficiency("Corsair HX1000i", 0.0), Some(0.75));
        assert_eq!(efficiency("Unknown PSU", 0.5), None);
    }
}
