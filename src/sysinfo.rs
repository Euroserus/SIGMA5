//! System readings for the top bar: CPU, RAM, GPU, temperatures, battery,
//! network and disk.
//!
//! A worker thread samples once a second, but only while the launcher is
//! open (it sleeps otherwise), and only the readings the top bar shows.
//! Sources:
//! - CPU: `GetSystemTimes` deltas. RAM: `GlobalMemoryStatusEx`.
//! - GPU use, network and CPU temperature: Windows performance counters (PDH):
//!   `GPU Engine(*engtype_3D)`, `Network Interface(*)`, and the ACPI
//!   `Thermal Zone Information(*)` (not every PC exposes a useful one).
//! - GPU temperature: NVIDIA's `nvml.dll` when present (loaded on demand).
//! - Battery: `GetSystemPowerStatus`. Disk: `GetDiskFreeSpaceExW`.
//!
//! Anything a PC doesn't provide is `None` and shows as "—".

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use windows::Win32::Foundation::{FILETIME, HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// A top bar component: (config id, label in settings, Fluent glyph).
pub struct ItemInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub glyph: char,
    /// Which reading it needs (0 = none, e.g. the clock).
    pub needs: u32,
}

pub const NEED_CPU: u32 = 1;
pub const NEED_RAM: u32 = 2;
pub const NEED_GPU: u32 = 4;
pub const NEED_CPU_TEMP: u32 = 8;
pub const NEED_GPU_TEMP: u32 = 16;
pub const NEED_BATTERY: u32 = 32;
pub const NEED_NET: u32 = 64;
pub const NEED_DISK: u32 = 128;

pub const ITEMS: &[ItemInfo] = &[
    ItemInfo { id: "clock", name: "Clock", glyph: '\u{E823}', needs: 0 },
    ItemInfo { id: "date", name: "Date", glyph: '\u{E787}', needs: 0 },
    ItemInfo { id: "media", name: "Now playing", glyph: '\u{E8D6}', needs: 0 },
    ItemInfo { id: "cpu", name: "CPU usage", glyph: '\u{E950}', needs: NEED_CPU },
    ItemInfo { id: "ram", name: "Memory", glyph: '\u{E964}', needs: NEED_RAM },
    ItemInfo { id: "gpu", name: "GPU usage", glyph: '\u{F211}', needs: NEED_GPU },
    ItemInfo { id: "cpu_temp", name: "CPU temperature", glyph: '\u{E9CA}', needs: NEED_CPU_TEMP },
    ItemInfo { id: "gpu_temp", name: "GPU temperature (NVIDIA)", glyph: '\u{E9CA}', needs: NEED_GPU_TEMP },
    ItemInfo { id: "battery", name: "Battery", glyph: '\u{E83F}', needs: NEED_BATTERY },
    ItemInfo { id: "net", name: "Network speed", glyph: '\u{E968}', needs: NEED_NET },
    ItemInfo { id: "disk", name: "Free disk space", glyph: '\u{EDA2}', needs: NEED_DISK },
];

pub fn item(id: &str) -> Option<&'static ItemInfo> {
    ITEMS.iter().find(|i| i.id == id)
}

/// Readings needed for a list of item ids.
pub fn needs(items: &[String]) -> u32 {
    items.iter().filter_map(|i| item(i)).fold(0, |m, i| m | i.needs)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// Percent.
    pub cpu: Option<f32>,
    /// (used GB, total GB, percent).
    pub ram: Option<(f32, f32, f32)>,
    pub gpu: Option<f32>,
    /// Degrees C.
    pub cpu_temp: Option<f32>,
    pub gpu_temp: Option<f32>,
    /// (percent, charging).
    pub battery: Option<(u8, bool)>,
    /// (down, up) in bytes per second.
    pub net: Option<(f64, f64)>,
    /// (free GB, total GB) of the system drive.
    pub disk: Option<(f32, f32)>,
}

/// "1.2 MB/s"-style rate.
pub fn rate(bps: f64) -> String {
    match bps {
        b if b >= 1e9 => format!("{:.1} GB/s", b / 1e9),
        b if b >= 1e6 => format!("{:.1} MB/s", b / 1e6),
        b if b >= 1e3 => format!("{:.0} KB/s", b / 1e3),
        b => format!("{b:.0} B/s"),
    }
}

// ---------------------------------------------------------------------------
// Readers

fn ft(f: FILETIME) -> u64 {
    (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64
}

#[derive(Default)]
struct CpuTimes {
    last: Option<(u64, u64)>,
}

impl CpuTimes {
    fn read(&mut self) -> Option<f32> {
        use windows::Win32::System::Threading::GetSystemTimes;
        let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).ok()? };
        let (idle, total) = (ft(idle), ft(kernel) + ft(user)); // kernel time includes idle
        let prev = self.last.replace((idle, total));
        let (pi, pt) = prev?;
        let dt = total.saturating_sub(pt);
        (dt > 0).then(|| (100.0 * (1.0 - idle.saturating_sub(pi) as f32 / dt as f32)).clamp(0.0, 100.0))
    }
}

fn ram() -> Option<(f32, f32, f32)> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe { GlobalMemoryStatusEx(&mut m).ok()? };
    let gb = |b: u64| b as f32 / (1u64 << 30) as f32;
    Some((gb(m.ullTotalPhys - m.ullAvailPhys), gb(m.ullTotalPhys), m.dwMemoryLoad as f32))
}

fn battery() -> Option<(u8, bool)> {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s).ok()? };
    // 128 = no battery, 255 = unknown.
    if s.BatteryFlag & 128 != 0 || s.BatteryFlag == 255 || s.BatteryLifePercent > 100 {
        return None;
    }
    Some((s.BatteryLifePercent, s.ACLineStatus == 1))
}

fn disk() -> Option<(f32, f32)> {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::core::PCWSTR;
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()) + "\\";
    let w = crate::util::wide(&drive);
    let (mut free, mut total) = (0u64, 0u64);
    unsafe { GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), None, Some(&mut total), Some(&mut free)).ok()? };
    let gb = |b: u64| b as f32 / 1e9;
    Some((gb(free), gb(total)))
}

/// Performance-counter readings (GPU use, network, ACPI temperature).
struct Pdh {
    query: isize,
    gpu: Option<isize>,
    net_in: Option<isize>,
    net_out: Option<isize>,
    temp: Option<isize>,
}

impl Pdh {
    fn open(needs: u32) -> Option<Self> {
        use windows::Win32::System::Performance::{PdhAddEnglishCounterW, PdhCollectQueryData, PdhOpenQueryW};
        use windows::core::w;
        let mut q = Default::default();
        if unsafe { PdhOpenQueryW(None, 0, &mut q) } != 0 {
            return None;
        }
        let add = |path: windows::core::PCWSTR| {
            let mut c = Default::default();
            (unsafe { PdhAddEnglishCounterW(q, path, 0, &mut c) } == 0).then_some(c.0 as isize)
        };
        let gpu = if needs & NEED_GPU != 0 { add(w!("\\GPU Engine(*engtype_3D)\\Utilization Percentage")) } else { None };
        let (net_in, net_out) = if needs & NEED_NET != 0 {
            (add(w!("\\Network Interface(*)\\Bytes Received/sec")), add(w!("\\Network Interface(*)\\Bytes Sent/sec")))
        } else {
            (None, None)
        };
        let temp = if needs & NEED_CPU_TEMP != 0 { add(w!("\\Thermal Zone Information(*)\\Temperature")) } else { None };
        unsafe {
            PdhCollectQueryData(q);
        }
        Some(Self { query: q.0 as isize, gpu, net_in, net_out, temp })
    }

    fn collect(&self) {
        use windows::Win32::System::Performance::{PDH_HQUERY, PdhCollectQueryData};
        unsafe {
            PdhCollectQueryData(PDH_HQUERY(self.query as *mut _));
        }
    }

    /// Every instance's value of a wildcard counter.
    fn values(counter: Option<isize>) -> Option<Vec<f64>> {
        use windows::Win32::System::Performance::{PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PdhGetFormattedCounterArrayW};
        let c = PDH_HCOUNTER(counter? as *mut _);
        let (mut size, mut count) = (0u32, 0u32);
        unsafe {
            PdhGetFormattedCounterArrayW(c, PDH_FMT_DOUBLE, &mut size, &mut count, None);
        }
        if size == 0 {
            return None;
        }
        // u64 storage keeps the items 8-byte aligned.
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
        if unsafe { PdhGetFormattedCounterArrayW(c, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) } != 0 {
            return None;
        }
        let items = unsafe { std::slice::from_raw_parts(items, count as usize) };
        Some(items.iter().filter(|i| i.FmtValue.CStatus == 0).map(|i| unsafe { i.FmtValue.Anonymous.doubleValue }).collect())
    }

    fn gpu(&self) -> Option<f32> {
        // Sum of every process's 3D engine use (what Task Manager shows), capped.
        Self::values(self.gpu).map(|v| (v.iter().sum::<f64>() as f32).clamp(0.0, 100.0))
    }

    fn net(&self) -> Option<(f64, f64)> {
        Some((Self::values(self.net_in)?.iter().sum(), Self::values(self.net_out)?.iter().sum()))
    }

    fn cpu_temp(&self) -> Option<f32> {
        // Kelvin; the hottest zone. Zones reading implausible values are ignored.
        let t = Self::values(self.temp)?.into_iter().map(|k| k as f32 - 273.15).filter(|c| (5.0..120.0).contains(c)).fold(f32::NAN, f32::max);
        (!t.is_nan()).then_some(t)
    }
}

impl Drop for Pdh {
    fn drop(&mut self) {
        use windows::Win32::System::Performance::{PDH_HQUERY, PdhCloseQuery};
        unsafe {
            PdhCloseQuery(PDH_HQUERY(self.query as *mut _));
        }
    }
}

/// NVIDIA GPU temperature through nvml.dll, if the driver provides it.
struct Nvml {
    get_temp: unsafe extern "C" fn(*mut core::ffi::c_void, u32, *mut u32) -> i32,
    device: *mut core::ffi::c_void,
}

impl Nvml {
    fn open() -> Option<Self> {
        use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
        use windows::core::{s, w};
        unsafe {
            let dll = LoadLibraryW(w!("nvml.dll")).ok()?;
            let init: unsafe extern "C" fn() -> i32 = std::mem::transmute(GetProcAddress(dll, s!("nvmlInit_v2"))?);
            let by_index: unsafe extern "C" fn(u32, *mut *mut core::ffi::c_void) -> i32 =
                std::mem::transmute(GetProcAddress(dll, s!("nvmlDeviceGetHandleByIndex_v2"))?);
            let get_temp = std::mem::transmute(GetProcAddress(dll, s!("nvmlDeviceGetTemperature"))?);
            if init() != 0 {
                return None;
            }
            let mut device = std::ptr::null_mut();
            (by_index(0, &mut device) == 0).then_some(Self { get_temp, device })
        }
    }

    fn temp(&self) -> Option<f32> {
        let mut t = 0u32;
        (unsafe { (self.get_temp)(self.device, 0, &mut t) } == 0).then_some(t as f32)
    }
}

/// Everything needed to take readings; lives on the sampler thread.
struct Readers {
    needs: u32,
    cpu: CpuTimes,
    pdh: Option<Pdh>,
    nvml: Option<Nvml>,
    nvml_tried: bool,
}

impl Readers {
    fn new(needs: u32) -> Self {
        let pdh_needs = needs & (NEED_GPU | NEED_NET | NEED_CPU_TEMP);
        Self { needs, cpu: CpuTimes::default(), pdh: (pdh_needs != 0).then(|| Pdh::open(pdh_needs)).flatten(), nvml: None, nvml_tried: false }
    }

    fn sample(&mut self) -> Snapshot {
        let n = self.needs;
        if let Some(p) = &self.pdh {
            p.collect();
        }
        if n & NEED_GPU_TEMP != 0 && !self.nvml_tried {
            self.nvml_tried = true;
            self.nvml = Nvml::open();
        }
        let pdh = self.pdh.as_ref();
        Snapshot {
            cpu: if n & NEED_CPU != 0 { self.cpu.read() } else { None },
            ram: if n & NEED_RAM != 0 { ram() } else { None },
            gpu: if n & NEED_GPU != 0 { pdh.and_then(Pdh::gpu) } else { None },
            cpu_temp: if n & NEED_CPU_TEMP != 0 { pdh.and_then(Pdh::cpu_temp) } else { None },
            gpu_temp: self.nvml.as_ref().and_then(Nvml::temp),
            battery: if n & NEED_BATTERY != 0 { battery() } else { None },
            net: if n & NEED_NET != 0 { pdh.and_then(Pdh::net) } else { None },
            disk: if n & NEED_DISK != 0 { disk() } else { None },
        }
    }
}

// ---------------------------------------------------------------------------
// Sampler thread

struct Shared {
    active: AtomicBool,
    stop: AtomicBool,
    needs: AtomicU32,
    wake: (Mutex<()>, Condvar),
}

/// Background sampler. Posts `msg` with LPARAM = Box<Snapshot> to `hwnd`.
pub struct Sampler {
    shared: Arc<Shared>,
}

impl Sampler {
    pub fn start(hwnd: HWND, msg: u32, needs: u32) -> Self {
        let shared = Arc::new(Shared { active: AtomicBool::new(false), stop: AtomicBool::new(false), needs: AtomicU32::new(needs), wake: (Mutex::new(()), Condvar::new()) });
        let sh = shared.clone();
        let target = hwnd.0 as isize;
        let _ = std::thread::Builder::new().name("sigma5-sysinfo".into()).spawn(move || {
            let mut readers = Readers::new(sh.needs.load(Ordering::Relaxed));
            let wait = |d: Option<Duration>| {
                let g = sh.wake.0.lock().unwrap_or_else(|e| e.into_inner());
                match d {
                    Some(d) => drop(sh.wake.1.wait_timeout(g, d)),
                    None => drop(sh.wake.1.wait(g)),
                }
            };
            while !sh.stop.load(Ordering::Relaxed) {
                if !sh.active.load(Ordering::Relaxed) {
                    wait(None);
                    continue;
                }
                let needs = sh.needs.load(Ordering::Relaxed);
                if needs != readers.needs {
                    readers = Readers::new(needs);
                }
                // Rates need two samples: take a quick first one on opening.
                if readers.cpu.last.is_none() {
                    readers.sample();
                    wait(Some(Duration::from_millis(250)));
                }
                let snap = readers.sample();
                let raw = Box::into_raw(Box::new(snap));
                unsafe {
                    if PostMessageW(Some(HWND(target as *mut _)), msg, WPARAM(0), LPARAM(raw as isize)).is_err() {
                        drop(Box::from_raw(raw));
                    }
                }
                wait(Some(Duration::from_secs(1)));
            }
        });
        Self { shared }
    }

    /// Sample while the launcher is open; sleep while it's hidden.
    pub fn set_active(&self, on: bool) {
        self.shared.active.store(on, Ordering::Relaxed);
        self.shared.wake.1.notify_all();
    }

    pub fn set_needs(&self, needs: u32) {
        self.shared.needs.store(needs, Ordering::Relaxed);
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.wake.1.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_and_rates() {
        assert_eq!(needs(&["clock".into(), "cpu".into(), "gpu_temp".into(), "nope".into()]), NEED_CPU | NEED_GPU_TEMP);
        assert_eq!(rate(512.0), "512 B/s");
        assert_eq!(rate(2_600.0), "3 KB/s");
        assert_eq!(rate(1_260_000.0), "1.3 MB/s");
    }

    /// Takes real readings from this PC (read-only):
    /// `cargo test --release sysinfo_live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn sysinfo_live() {
        let mut r = Readers::new(u32::MAX);
        r.sample();
        std::thread::sleep(Duration::from_millis(1000));
        let t = std::time::Instant::now();
        let s = r.sample();
        println!("sample took {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
        println!("{s:#?}");
        assert!(s.cpu.is_some() && s.ram.is_some() && s.disk.is_some());
    }
}
