//! Launching things through the Windows shell, plus start-at-login.

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW};
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

pub fn open_folder(path: &std::path::Path) {
    let p = path.to_string_lossy().to_string();
    spawn_shell(move || {
        shell_execute(&p, None);
    });
}

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("SIGMA5");

/// Adds or removes SIGMA5's own value under HKCU\...\Run.
pub fn set_start_with_windows(enabled: bool) {
    unsafe {
        if enabled {
            let Ok(exe) = std::env::current_exe() else { return };
            let cmd = wide(&format!("\"{}\" --background", exe.display()));
            let _ = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(cmd.as_ptr() as *const _),
                (cmd.len() * 2) as u32,
            );
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
        }
    }
}
