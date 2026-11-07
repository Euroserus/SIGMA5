//! "Focus or launch" for app hotkeys.
//!
//! Finds the app's top-level windows (Alt+Tab-style: visible, unowned, not a
//! tool window, not cloaked). A window belongs to the app if its
//! AppUserModelID matches the app's id (Store apps, Chrome, Explorer, ...) or
//! its process exe matches the app's exe. Then (like Raycast):
//! - no window: launch the app;
//! - app in front: minimise all its windows ("hide");
//! - app hidden by us: restore exactly those windows;
//! - otherwise: bring its most recent window to the front.

use std::cell::RefCell;
use std::collections::HashMap;

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, TRUE};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_TRANSITIONS_FORCEDISABLED, DwmGetWindowAttribute, DwmSetWindowAttribute,
};
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, EnumWindows, GA_ROOTOWNER, GW_OWNER, GWL_EXSTYLE, GetAncestor,
    GetForegroundWindow, GetWindow, GetWindowLongW, GetWindowTextLengthW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, SW_MINIMIZE, SW_RESTORE, SetForegroundWindow, ShowWindowAsync, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, PWSTR};

use crate::launch;
use crate::util::take_pwstr;

thread_local! {
    /// Windows we minimised per app id, so the next press restores them.
    static HIDDEN: RefCell<HashMap<String, Vec<isize>>> = RefCell::new(HashMap::new());
}

/// What a hotkey press should do (pure, unit-tested). Windows are opaque ids,
/// front-most first.
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    Launch,
    Focus(isize),
    Hide(Vec<isize>),
    Unhide(Vec<isize>),
}

fn plan(windows: &[isize], foreground: isize, is_minimized: impl Fn(isize) -> bool, hidden: Option<&[isize]>) -> Plan {
    if windows.is_empty() {
        return Plan::Launch;
    }
    if windows.contains(&foreground) {
        let visible: Vec<isize> = windows.iter().copied().filter(|&w| !is_minimized(w)).collect();
        return Plan::Hide(visible);
    }
    if let Some(h) = hidden {
        let still: Vec<isize> = h.iter().copied().filter(|w| windows.contains(w) && is_minimized(*w)).collect();
        if !still.is_empty() {
            return Plan::Unhide(still);
        }
    }
    Plan::Focus(windows[0])
}

/// Brings the app to the front, hides it if it's already in front, or launches it.
pub fn focus_or_launch(id: &str, exe: Option<&str>) {
    let windows: Vec<isize> = app_windows(id, exe).into_iter().map(|h| h.0 as isize).collect();
    // A focused dialog (e.g. Save As) counts as its app being in front.
    let fg = unsafe { GetAncestor(GetForegroundWindow(), GA_ROOTOWNER) }.0 as isize;
    let minimized = |w: isize| unsafe { IsIconic(HWND(w as *mut _)).as_bool() };
    let hidden = HIDDEN.with(|h| h.borrow().get(id).cloned());

    match plan(&windows, fg, minimized, hidden.as_deref()) {
        Plan::Launch => {
            unsafe {
                let _ = AllowSetForegroundWindow(ASFW_ANY);
            }
            launch::launch_app(id);
        }
        Plan::Focus(w) => {
            // Windows we hid earlier but the user restored by hand: animations back on.
            if let Some(old) = HIDDEN.with(|h| h.borrow_mut().remove(id)) {
                set_transitions(&old, true);
            }
            activate(HWND(w as *mut _));
        }
        Plan::Hide(ws) => {
            // Instant, like Raycast: no minimise animation. Transitions stay
            // off while hidden so the restore is instant too.
            set_transitions(&ws, false);
            for &w in &ws {
                unsafe {
                    let _ = ShowWindowAsync(HWND(w as *mut _), SW_MINIMIZE);
                }
            }
            HIDDEN.with(|h| h.borrow_mut().insert(id.to_string(), ws));
        }
        Plan::Unhide(ws) => {
            HIDDEN.with(|h| h.borrow_mut().remove(id));
            // Restore back-to-front so the original front window ends on top.
            for &w in ws.iter().rev().skip(1) {
                unsafe {
                    let _ = ShowWindowAsync(HWND(w as *mut _), SW_RESTORE);
                }
            }
            activate(HWND(ws[0] as *mut _));
            // Give the restores a moment to happen, then re-enable animations.
            let later: Vec<isize> = ws.clone();
            let _ = std::thread::Builder::new().name("sigma5-anim".into()).spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(600));
                set_transitions(&later, true);
            });
        }
    }
}

/// Turns DWM's minimise/restore animations on or off for windows (of any
/// process). Only affects those windows, never the system setting.
fn set_transitions(windows: &[isize], enabled: bool) {
    let disabled: BOOL = (!enabled).into();
    for &w in windows {
        unsafe {
            let _ = DwmSetWindowAttribute(
                HWND(w as *mut _),
                DWMWA_TRANSITIONS_FORCEDISABLED,
                &disabled as *const BOOL as *const _,
                std::mem::size_of::<BOOL>() as u32,
            );
        }
    }
}

/// Brings a window to the front, restoring it if minimised.
pub fn activate(hwnd: HWND) {
    unsafe {
        if IsIconic(hwnd).as_bool() {
            // Async so a frozen app can never stall SIGMA5.
            let _ = ShowWindowAsync(hwnd, SW_RESTORE);
        }
        if SetForegroundWindow(hwnd).as_bool() && GetForegroundWindow() == hwnd {
            return;
        }
        // Foreground lock: briefly share input state with the foreground thread.
        let fg_thread = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let me = GetCurrentThreadId();
        if fg_thread != 0 && fg_thread != me {
            let _ = AttachThreadInput(me, fg_thread, true);
            let _ = SetForegroundWindow(hwnd);
            let _ = AttachThreadInput(me, fg_thread, false);
        }
    }
}

/// The app's switchable windows, front-most first.
fn app_windows(id: &str, exe: Option<&str>) -> Vec<HWND> {
    let mut all: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut all as *mut Vec<HWND> as isize));
    }
    let me = std::process::id();
    all.into_iter()
        .filter(|&h| is_switchable(h))
        .filter(|&h| {
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
            if pid == me {
                return false;
            }
            if window_aumid(h).is_some_and(|a| same_app(&a, id)) {
                return true;
            }
            if exe.is_some_and(|e| process_exe(pid).is_some_and(|p| p == e)) {
                return true;
            }
            // Packaged (MSIX/Store) desktop apps: identity is on the process.
            id.contains('!') && process_aumid(pid).is_some_and(|a| same_app(&a, id))
        })
        .collect()
}

unsafe extern "system" fn collect(hwnd: HWND, lp: LPARAM) -> BOOL {
    let v = unsafe { &mut *(lp.0 as *mut Vec<HWND>) };
    v.push(hwnd);
    TRUE
}

/// Roughly the Alt+Tab criteria.
fn is_switchable(h: HWND) -> bool {
    unsafe {
        if !IsWindowVisible(h).as_bool() || GetWindowTextLengthW(h) == 0 {
            return false;
        }
        if GetWindow(h, GW_OWNER).is_ok_and(|o| !o.is_invalid()) {
            return false;
        }
        if GetWindowLongW(h, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
            return false;
        }
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4);
        cloaked == 0
    }
}

fn window_aumid(h: HWND) -> Option<String> {
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(h).ok()?;
        let pv = store.GetValue(&PKEY_AppUserModel_ID).ok()?;
        let s = take_pwstr(PropVariantToStringAlloc(&pv).ok()?);
        (!s.is_empty()).then_some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::{Plan, plan};

    #[test]
    fn toggle_like_raycast() {
        let none_min = |_: isize| false;
        // Not running → launch.
        assert_eq!(plan(&[], 9, none_min, None), Plan::Launch);
        // Running, not in front → focus the most recent window.
        assert_eq!(plan(&[1, 2], 9, none_min, None), Plan::Focus(1));
        // In front → hide every visible window (already-minimised ones untouched).
        assert_eq!(plan(&[1, 2, 3], 1, |w| w == 3, None), Plan::Hide(vec![1, 2]));
        // Pressed again → restore exactly what we hid.
        assert_eq!(plan(&[1, 2, 3], 9, |_| true, Some(&[1, 2])), Plan::Unhide(vec![1, 2]));
        // Something we hid was closed or restored by hand → restore the rest.
        assert_eq!(plan(&[2, 3], 9, |w| w == 2, Some(&[1, 2])), Plan::Unhide(vec![2]));
        // All restored by hand → plain focus.
        assert_eq!(plan(&[1, 2], 9, none_min, Some(&[1, 2])), Plan::Focus(1));
        // Only minimised windows and no record → focus (restores) the top one.
        assert_eq!(plan(&[4], 9, |_| true, None), Plan::Focus(4));
    }

    /// Cross-process check for the no-animation attribute. Needs a window
    /// handle from another process in SIGMA5_TEST_HWND (never shown on screen).
    #[test]
    #[ignore]
    fn diag_transitions_cross_process() {
        let Ok(v) = std::env::var("SIGMA5_TEST_HWND") else { return };
        let hwnd = windows::Win32::Foundation::HWND(v.trim().parse::<isize>().unwrap() as *mut _);
        for disabled in [true, false] {
            let b: windows::core::BOOL = disabled.into();
            let r = unsafe {
                super::DwmSetWindowAttribute(hwnd, super::DWMWA_TRANSITIONS_FORCEDISABLED, &b as *const _ as *const _, 4)
            };
            println!("set disabled={disabled}: {r:?}");
            assert!(r.is_ok());
        }
    }

    #[test]
    fn package_family_matching() {
        assert!(super::same_app("Claude_pzs8sxrjxfjjc!Claude", "claude_pzs8sxrjxfjjc!claude"));
        assert!(super::same_app("Foo_abc!Helper", "Foo_abc!App"));
        assert!(!super::same_app("Foo_abc!App", "Bar_abc!App"));
        assert!(!super::same_app("MSEdge.UserData.Profile1", "MSEdge"));
        assert!(super::same_app("Chrome", "chrome"));
    }

    /// Read-only diagnostic: which indexed apps currently have windows?
    /// Run with `cargo test --release diag_ -- --ignored --nocapture`.
    /// It only enumerates windows; it never focuses or launches anything.
    #[test]
    #[ignore]
    fn diag_matches_running_apps() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
        }
        let cache = std::fs::read_to_string(crate::util::data_dir().join("apps.cache")).unwrap_or_default();
        let mut found = 0;
        for line in cache.lines() {
            let mut it = line.split('\t');
            let (Some(name), Some(id)) = (it.next(), it.next()) else { continue };
            let exe = it.next().filter(|e| !e.is_empty());
            let n = super::app_windows(id, exe).len();
            if n > 0 {
                found += 1;
                println!("{n} window(s): {name}  [id={id}, exe={}]", exe.unwrap_or("-"));
            }
        }
        println!("{found} running apps matched");

        println!("--- all switchable windows:");
        let mut all = Vec::new();
        unsafe {
            let _ = super::EnumWindows(Some(super::collect), super::LPARAM(&mut all as *mut Vec<super::HWND> as isize));
        }
        for h in all.into_iter().filter(|&h| super::is_switchable(h)) {
            let mut pid = 0u32;
            unsafe { super::GetWindowThreadProcessId(h, Some(&mut pid)) };
            println!("  aumid={:?} exe={:?}", super::window_aumid(h), super::process_exe(pid));
        }
    }
}

/// AppUserModelIDs match exactly, or by package family for packaged apps
/// ("Family_hash!App" vs "Family_hash!OtherEntry").
fn same_app(a: &str, b: &str) -> bool {
    if a.eq_ignore_ascii_case(b) {
        return true;
    }
    match (a.split_once('!'), b.split_once('!')) {
        (Some((fa, _)), Some((fb, _))) => fa.eq_ignore_ascii_case(fb),
        _ => false,
    }
}

/// AppUserModelID of a packaged process, if it has one.
fn process_aumid(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let rc = GetApplicationUserModelId(h, &mut len, Some(PWSTR(buf.as_mut_ptr())));
        let _ = CloseHandle(h);
        if rc.is_err() || len == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1)]))
    }
}

/// Lowercase exe file name of a process.
fn process_exe(pid: u32) -> Option<String> {
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
