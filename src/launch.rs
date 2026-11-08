//! Launching things through the Windows shell, plus start-at-login.

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_BINARY, REG_SZ, RRF_RT_REG_BINARY, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::UI::Shell::{SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, w};

use crate::util::wide;

fn shell_execute(file: &str, params: Option<&str>) -> bool {
    let file_w = wide(file);
    let params_w = params.map(wide);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: w!("open"),
        lpFile: PCWSTR(file_w.as_ptr()),
        lpParameters: params_w.as_ref().map_or(PCWSTR::null(), |p| PCWSTR(p.as_ptr())),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info).is_ok() }
}

/// Runs a shell launch on a short-lived thread so the UI never blocks.
fn spawn_shell(f: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new().name("sigma5-launch".into()).spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        f();
        CoUninitialize();
    });
}

/// Launches an entry of the Apps folder by its parsing name.
pub fn launch_app(id: &str) {
    let target = format!("shell:AppsFolder\\{id}");
    spawn_shell(move || {
        shell_execute(&target, None);
    });
}

/// Opens a file in its default app, falling back to Notepad for unassociated types.
pub fn open_file(path: &std::path::Path) {
    let p = path.to_string_lossy().to_string();
    spawn_shell(move || {
        if !shell_execute(&p, None) {
            shell_execute("notepad.exe", Some(&format!("\"{p}\"")));
        }
    });
}

/// Opens an https:// link in the default browser.
pub fn open_url(url: &str) {
    if !url.starts_with("https://") {
        return;
    }
    let u = url.to_string();
    spawn_shell(move || {
        shell_execute(&u, None);
    });
}

pub fn open_folder(path: &std::path::Path) {
    let p = path.to_string_lossy().to_string();
    spawn_shell(move || {
        shell_execute(&p, None);
    });
}

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("SIGMA5");
/// Task Manager's "Startup apps" state. Recent Windows builds only launch
/// per-user Run entries that have a record here (02… = enabled, 03… = disabled).
const APPROVED_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run");

/// Adds or removes SIGMA5's own startup entry (HKCU\...\Run plus its
/// "startup approved" record).
pub fn set_start_with_windows(enabled: bool) {
    unsafe {
        if enabled {
            // Prefer the installed copy, so a dev/portable build never
            // hijacks autostart.
            let exe = crate::install::autostart_exe();
            let cmd = wide(&format!("\"{}\" --background", exe.display()));
            let _ = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(cmd.as_ptr() as *const _),
                (cmd.len() * 2) as u32,
            );
            // Add an "enabled" record only if none exists, so switching
            // SIGMA5 off in Task Manager is respected.
            let mut size = 0u32;
            let exists = RegGetValueW(HKEY_CURRENT_USER, APPROVED_KEY, RUN_VALUE, RRF_RT_REG_BINARY, None, None, Some(&mut size)).is_ok();
            if !exists {
                let enabled_record = [2u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
                let _ = RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    APPROVED_KEY,
                    RUN_VALUE,
                    REG_BINARY.0,
                    Some(enabled_record.as_ptr() as *const _),
                    enabled_record.len() as u32,
                );
            }
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, APPROVED_KEY, RUN_VALUE);
        }
    }
}
