//! NVIDIA GPU name, temperature, utilization and power through NVML, loaded at
//! run time. Read-only: none of these calls need root.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::sync::OnceLock;

use crate::dl::Lib;
use crate::sensors::GpuReading;

type Device = *mut c_void;

#[repr(C)]
struct Utilization {
    gpu: c_uint,
    memory: c_uint,
}

pub struct Nvml {
    by_pci: unsafe extern "C" fn(*const c_char, *mut Device) -> c_int,
    name: unsafe extern "C" fn(Device, *mut c_char, c_uint) -> c_int,
    power: unsafe extern "C" fn(Device, *mut c_uint) -> c_int,
    temperature: unsafe extern "C" fn(Device, c_int, *mut c_uint) -> c_int,
    utilization: unsafe extern "C" fn(Device, *mut Utilization) -> c_int,
    fan: unsafe extern "C" fn(Device, *mut c_uint) -> c_int,
    power_limit: unsafe extern "C" fn(Device, *mut c_uint) -> c_int,
}

/// NVML, or `None` when the NVIDIA driver is not installed.
pub fn get() -> Option<&'static Nvml> {
    static NVML: OnceLock<Option<Nvml>> = OnceLock::new();
    NVML.get_or_init(Nvml::load).as_ref()
}

impl Nvml {
    fn load() -> Option<Nvml> {
        let lib = Lib::open("libnvidia-ml.so.1")?;
        unsafe {
            let init: unsafe extern "C" fn() -> c_int = lib.sym("nvmlInit_v2")?;
            if init() != 0 {
                return None;
            }
            Some(Nvml {
                by_pci: lib.sym("nvmlDeviceGetHandleByPciBusId_v2")?,
                name: lib.sym("nvmlDeviceGetName")?,
                power: lib.sym("nvmlDeviceGetPowerUsage")?,
                temperature: lib.sym("nvmlDeviceGetTemperature")?,
                utilization: lib.sym("nvmlDeviceGetUtilizationRates")?,
                fan: lib.sym("nvmlDeviceGetFanSpeed")?,
                power_limit: lib.sym("nvmlDeviceGetEnforcedPowerLimit")?,
            })
        }
    }

    fn device(&self, pci: &str) -> Option<Device> {
        let pci = CString::new(pci).ok()?;
        let mut dev = std::ptr::null_mut();
        (unsafe { (self.by_pci)(pci.as_ptr(), &mut dev) } == 0).then_some(dev)
    }

    pub fn name(&self, pci: &str) -> Option<String> {
        let dev = self.device(pci)?;
        let mut buf = [0 as c_char; 96];
        if unsafe { (self.name)(dev, buf.as_mut_ptr(), buf.len() as c_uint) } != 0 {
            return None;
        }
        Some(unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned())
    }

    pub fn read(&self, pci: &str) -> GpuReading {
        let Some(dev) = self.device(pci) else { return GpuReading::default() };
        let mut reading = GpuReading::default();
        unsafe {
            let mut v: c_uint = 0;
            if (self.temperature)(dev, 0, &mut v) == 0 {
                reading.temperature = Some(v as f64);
            }
            if (self.power)(dev, &mut v) == 0 {
                reading.power = Some(v as f64 / 1000.0);
            }
            if (self.power_limit)(dev, &mut v) == 0 && v > 0 {
                reading.power_limit = Some(v as f64 / 1000.0);
            }
            if (self.fan)(dev, &mut v) == 0 {
                reading.fan_percent = Some(v as f64);
            }
            let mut util = Utilization { gpu: 0, memory: 0 };
            if (self.utilization)(dev, &mut util) == 0 {
                reading.utilization = Some(util.gpu as f64);
            }
        }
        reading
    }
}
