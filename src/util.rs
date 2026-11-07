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
