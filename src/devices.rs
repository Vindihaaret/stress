//! What can be loaded: the CPU and every display controller on the PCI bus,
//! including those without a working driver, so the GPU picker can explain why.

use std::fs;
use std::path::Path;

use crate::i18n::{tr, trf};
use crate::nvml;

pub struct Cpu {
    pub model: String,
    pub threads: usize,
}

pub fn cpu() -> Cpu {
    let model = fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|info| {
            info.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split_once(':'))
                .map(|(_, v)| v.trim().to_string())
        })
        .unwrap_or_else(|| "CPU".into());
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    Cpu { model, threads }
}

pub struct Gpu {
    /// PCI address, e.g. `0000:01:00.0`; the stable id used everywhere.
    pub pci: String,
    pub name: String,
    pub vendor: u16,
    pub driver: Option<String>,
    /// Drives the screen the system booted on.
    pub boot_vga: bool,
    /// DRM node such as `card1`, present when a driver is bound and working.
    pub drm_card: Option<String>,
}

pub const NVIDIA: u16 = 0x10de;
pub const AMD: u16 = 0x1002;
pub const INTEL: u16 = 0x8086;

impl Gpu {
    /// Why this GPU cannot be loaded, or `None` when it can be tried.
    pub fn unavailable_reason(&self) -> Option<String> {
        if self.drm_card.is_some() {
            return None;
        }
        let nomodeset = fs::read_to_string("/proc/cmdline")
            .is_ok_and(|c| c.split_whitespace().any(|w| w == "nomodeset"));
        Some(match (&self.driver, nomodeset) {
            (None, true) => tr("No driver is active (the system was started with nomodeset)").to_string(),
            (None, false) => tr("No driver is active").to_string(),
            (Some(d), _) => trf("The {} driver offers no graphics device for it", &[d]),
        })
    }
}

pub fn gpus() -> Vec<Gpu> {
    let Ok(entries) = fs::read_dir("/sys/bus/pci/devices") else { return Vec::new() };
    let mut gpus: Vec<Gpu> = entries
        .flatten()
        .filter_map(|e| {
            let dir = e.path();
            // Class 0x03xxxx: display controllers (VGA, 3D, other).
            if !read(&dir.join("class"))?.starts_with("0x03") {
                return None;
            }
            let pci = e.file_name().to_string_lossy().into_owned();
            let vendor = hex(&read(&dir.join("vendor"))?)?;
            let device = hex(&read(&dir.join("device"))?)?;
            let driver = fs::read_link(dir.join("driver"))
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
            let drm_card = fs::read_dir(dir.join("drm")).ok().and_then(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .find(|n| n.starts_with("card"))
            });
            let name = (vendor == NVIDIA)
                .then(|| nvml::get().and_then(|n| n.name(&pci)))
                .flatten()
                .unwrap_or_else(|| pci_name(vendor, device));
            Some(Gpu {
                pci,
                name,
                vendor,
                driver,
                boot_vga: read(&dir.join("boot_vga")).as_deref() == Some("1"),
                drm_card,
            })
        })
        .collect();
    gpus.sort_by(|a, b| a.pci.cmp(&b.pci));
    gpus
}

fn read(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn hex(s: &str) -> Option<u16> {
    u16::from_str_radix(s.trim_start_matches("0x"), 16).ok()
}

fn vendor_name(vendor: u16) -> &'static str {
    match vendor {
        NVIDIA => "NVIDIA",
        AMD => "AMD",
        INTEL => "Intel",
        _ => "GPU",
    }
}

/// "AMD Radeon 610M" from pci.ids ("Raphael [Radeon 610M]"), preferring the
/// marketing name in brackets.
fn pci_name(vendor: u16, device: u16) -> String {
    let vendor_label = vendor_name(vendor);
    let fallback = || format!("{vendor_label} {device:04x}");
    let Some(ids) = ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids", "/usr/share/pci.ids"]
        .iter()
        .find_map(|p| fs::read_to_string(p).ok())
    else {
        return fallback();
    };
    let vendor_key = format!("{vendor:04x}  ");
    let device_key = format!("\t{device:04x}  ");
    let mut in_vendor = false;
    for line in ids.lines() {
        if !line.starts_with('\t') && !line.starts_with('#') && !line.is_empty() {
            if in_vendor {
                break;
            }
            in_vendor = line.starts_with(&vendor_key);
        } else if in_vendor {
            if let Some(name) = line.strip_prefix(&device_key) {
                let name = match (name.find('['), name.rfind(']')) {
                    (Some(a), Some(b)) if a < b => &name[a + 1..b],
                    _ => name,
                };
                return format!("{vendor_label} {name}");
            }
        }
    }
    fallback()
}
