//! Small helpers shared across the app.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use windows::Win32::System::Com::CoTaskMemFree;
use windows::core::PWSTR;

/// UTF-16, NUL-terminated, for passing to Win32 as a `PCWSTR`.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 without the trailing NUL (for APIs that take a slice + length).
pub fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Converts a shell-allocated string to a `String` and frees it.
pub unsafe fn take_pwstr(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

/// `%APPDATA%\SIGMA5`, created on first use.
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir());
    let dir = base.join("SIGMA5");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Lowercase exe file name of a process, e.g. "chrome.exe".
pub fn process_exe_name(pid: u32) -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        path.rsplit('\\').next().map(|s| s.to_ascii_lowercase())
    }
}

/// Today's date (UTC) as "YYYY-MM-DD".
pub fn today() -> String {
    let (y, m, d) = civil_from_days((now_unix() / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 → (year, month, day). (Howard Hinnant's algorithm.)
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// "now", "5m", "3h", "2d", "6w" since a unix timestamp.
pub fn ago(then: u64) -> String {
    let s = now_unix().saturating_sub(then);
    match s {
        0..60 => "now".into(),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h", s / 3600),
        86_400..1_209_600 => format!("{}d", s / 86_400),
        _ => format!("{}w", s / 604_800),
    }
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Puts Unicode text on the clipboard. Returns false if the clipboard was busy.
pub fn copy_to_clipboard(owner: windows::Win32::Foundation::HWND, text: &str) -> bool {
    use windows::Win32::Foundation::{GlobalFree, HANDLE};
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    const CF_UNICODETEXT: u32 = 13;

    let data = wide(text);
    let bytes = data.len() * 2;
    unsafe {
        if OpenClipboard(Some(owner)).is_err() {
            return false;
        }
        let mut ok = false;
        if EmptyClipboard().is_ok() {
            if let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, bytes) {
                let p = GlobalLock(mem) as *mut u16;
                if !p.is_null() {
                    std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
                    let _ = GlobalUnlock(mem);
                    // On success the clipboard owns the memory.
                    ok = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(mem.0))).is_ok();
                }
                if !ok {
                    let _ = GlobalFree(Some(mem));
                }
            }
        }
        let _ = CloseClipboard();
        ok
    }
}

/// Wraps a raw window handle so it can be moved to a worker thread. HWNDs are
/// plain identifiers and are valid to use (e.g. `PostMessageW`) from any thread.
#[derive(Clone, Copy)]
pub struct SendHwnd(pub isize);
unsafe impl Send for SendHwnd {}

impl SendHwnd {
    pub fn new(h: windows::Win32::Foundation::HWND) -> Self {
        Self(h.0 as isize)
    }
    pub fn get(self) -> windows::Win32::Foundation::HWND {
        windows::Win32::Foundation::HWND(self.0 as *mut _)
    }
}
