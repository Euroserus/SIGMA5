//! Installing SIGMA5 like a normal app, without a separate setup program.
//!
//! The same `sigma5.exe` is the installer: run from anywhere else, it offers to
//! copy itself to `%LOCALAPPDATA%\Programs\SIGMA5` (per-user, no admin), add a
//! Start menu shortcut, set up launch at login, and register in
//! Settings › Apps so it can be uninstalled normally (`sigma5.exe --uninstall`).
//! Running a newer `sigma5.exe` offers to update the installed copy.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{CloseHandle, LPARAM, WPARAM};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_DWORD, REG_SZ, RRF_RT_REG_SZ, RegDeleteTreeW, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetWindowThreadProcessId, IDYES, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK, MB_YESNO, MessageBoxW,
    PostMessageW,
};
use windows::core::{Interface, PCWSTR, w};

use crate::util::wide;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Each release is named after a famous engineer or researcher.
pub const CODENAME: &str = "Charpy";

/// "0.3.0 “Charpy”"
pub fn version_label() -> String {
    format!("{VERSION} “{CODENAME}”")
}
const UNINSTALL_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\SIGMA5");

fn local_app_data() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

pub fn install_dir() -> PathBuf {
    local_app_data().join("Programs").join("SIGMA5")
}

pub fn installed_exe() -> PathBuf {
    install_dir().join("sigma5.exe")
}

fn shortcut_path() -> PathBuf {
    let roaming = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    roaming.join("Microsoft\\Windows\\Start Menu\\Programs\\SIGMA5.lnk")
}

pub fn is_installed() -> bool {
    installed_exe().exists()
}

fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().to_lowercase();
    norm(a) == norm(b)
}

/// True when this process is the installed copy.
pub fn running_installed() -> bool {
    std::env::current_exe().is_ok_and(|e| same_path(&e, &installed_exe()))
}

/// The exe that should start at login: the installed copy if there is one.
pub fn autostart_exe() -> PathBuf {
    if is_installed() { installed_exe() } else { std::env::current_exe().unwrap_or_else(|_| installed_exe()) }
}

pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let mut it = s.trim().trim_start_matches('v').split('.').map(|p| p.parse::<u32>().ok());
    Some((it.next()??, it.next().flatten().unwrap_or(0), it.next().flatten().unwrap_or(0)))
}

/// Version recorded by the installed copy (from its Apps entry).
pub fn installed_version() -> Option<String> {
    let mut buf = [0u16; 64];
    let mut size = (buf.len() * 2) as u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            UNINSTALL_KEY,
            w!("DisplayVersion"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
        .is_ok()
    };
    ok.then(|| String::from_utf16_lossy(&buf[..(size as usize / 2).saturating_sub(1)]))
}

/// What to do when the exe starts.
#[derive(Debug, PartialEq, Eq)]
pub enum Startup {
    /// Run the launcher.
    Normal,
    /// Show the installer (fresh install, or update of an older install).
    Installer { update: bool },
}

/// Pure decision logic (unit-tested).
pub fn decide(args_portable: bool, running_installed: bool, installed: Option<&str>, this: &str) -> Startup {
    if args_portable || running_installed {
        return Startup::Normal;
    }
    match installed {
        None => Startup::Installer { update: false },
        Some(v) if parse_version(this) > parse_version(v) => Startup::Installer { update: true },
        // Same or older build run from elsewhere (e.g. a dev build): just run.
        Some(_) => Startup::Normal,
    }
}

pub fn startup_mode(args: &[String]) -> Startup {
    let portable = args.iter().any(|a| a == "--portable" || a == "--background");
    let installed = if is_installed() { Some(installed_version().unwrap_or_else(|| "0.0.0".into())) } else { None };
    decide(portable, running_installed(), installed.as_deref(), VERSION)
}

#[derive(Clone, Copy)]
pub struct Options {
    pub launch_at_login: bool,
    pub shortcut: bool,
}

/// Asks a running SIGMA5 to quit and waits for it, so its exe can be replaced.
pub fn stop_running_instance() {
    let class = wide(crate::ui::WINDOW_CLASS);
    unsafe {
        let Ok(hwnd) = FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()) else { return };
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == std::process::id() {
            return;
        }
        let proc_ = OpenProcess(PROCESS_SYNCHRONIZE, false, pid).ok();
        let _ = PostMessageW(Some(hwnd), crate::ui::WM_APP_QUIT, WPARAM(0), LPARAM(0));
        if let Some(h) = proc_ {
            let _ = WaitForSingleObject(h, 5000);
            let _ = CloseHandle(h);
        }
    }
}

fn reg_sz(name: PCWSTR, value: &str) {
    let v = wide(value);
    unsafe {
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, UNINSTALL_KEY, name, REG_SZ.0, Some(v.as_ptr() as *const _), (v.len() * 2) as u32);
    }
}

fn reg_dword(name: PCWSTR, value: u32) {
    unsafe {
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, UNINSTALL_KEY, name, REG_DWORD.0, Some(&value as *const u32 as *const _), 4);
    }
}

fn create_shortcut(target: &Path, lnk: &Path) -> Result<(), String> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| e.to_string())?;
        let t = wide(&target.to_string_lossy());
        let dir = wide(&target.parent().unwrap_or(target).to_string_lossy());
        let desc = wide("A fast, lightweight launcher for Windows");
        link.SetPath(PCWSTR(t.as_ptr())).map_err(|e| e.to_string())?;
        link.SetWorkingDirectory(PCWSTR(dir.as_ptr())).map_err(|e| e.to_string())?;
        link.SetDescription(PCWSTR(desc.as_ptr())).map_err(|e| e.to_string())?;
        link.SetIconLocation(PCWSTR(t.as_ptr()), 0).map_err(|e| e.to_string())?;
        let file: IPersistFile = link.cast().map_err(|e| e.to_string())?;
        let p = wide(&lnk.to_string_lossy());
        file.Save(PCWSTR(p.as_ptr()), true).map_err(|e| e.to_string())
    }
}

/// Installs (or updates) this exe. Returns the installed exe path.
pub fn install(opts: Options) -> Result<PathBuf, String> {
    let src = std::env::current_exe().map_err(|e| e.to_string())?;
    let dst = installed_exe();
    std::fs::create_dir_all(install_dir()).map_err(|e| format!("Couldn't create {}: {e}", install_dir().display()))?;
    if !same_path(&src, &dst) {
        stop_running_instance();
        // The old exe can stay locked for a moment after its process exits.
        let mut last = String::new();
        for _ in 0..20 {
            match std::fs::copy(&src, &dst) {
                Ok(_) => {
                    last.clear();
                    break;
                }
                Err(e) => {
                    last = e.to_string();
                    std::thread::sleep(std::time::Duration::from_millis(150));
                }
            }
        }
        if !last.is_empty() {
            return Err(format!("Couldn't copy SIGMA5 into place: {last}"));
        }
    }

    let lnk = shortcut_path();
    if opts.shortcut {
        create_shortcut(&dst, &lnk)?;
    } else {
        let _ = std::fs::remove_file(&lnk);
    }

    // Settings › Apps entry.
    let exe = dst.to_string_lossy().to_string();
    reg_sz(w!("DisplayName"), "SIGMA5");
    reg_sz(w!("DisplayVersion"), VERSION);
    reg_sz(w!("Publisher"), "Nathan Smith");
    reg_sz(w!("DisplayIcon"), &format!("{exe},0"));
    reg_sz(w!("InstallLocation"), &install_dir().to_string_lossy());
    reg_sz(w!("UninstallString"), &format!("\"{exe}\" --uninstall"));
    reg_sz(w!("URLInfoAbout"), "https://github.com/Euroserus/SIGMA5");
    reg_dword(w!("NoModify"), 1);
    reg_dword(w!("NoRepair"), 1);
    let kb = std::fs::metadata(&dst).map(|m| (m.len() / 1024) as u32).unwrap_or(0);
    reg_dword(w!("EstimatedSize"), kb);

    // Launch at login lives in the shared config; the installed copy applies
    // it (Run entry + startup approval) when it starts.
    let mut store = crate::config::ConfigStore::load();
    if store.config.start_with_windows != opts.launch_at_login {
        store.config.start_with_windows = opts.launch_at_login;
        store.save();
    }
    Ok(dst)
}

/// Starts the installed copy. `background` keeps it hidden in the tray.
pub fn launch_installed(background: bool) {
    let mut cmd = std::process::Command::new(installed_exe());
    if background {
        cmd.arg("--background");
    }
    let _ = cmd.current_dir(install_dir()).spawn();
}

/// `sigma5.exe --uninstall` (what Settings › Apps runs).
pub fn uninstall_interactive() {
    let yes = unsafe {
        MessageBoxW(
            None,
            w!("Uninstall SIGMA5?\n\nYour settings in %APPDATA%\\SIGMA5 are kept, so reinstalling picks up where you left off."),
            w!("SIGMA5"),
            MB_YESNO | MB_ICONQUESTION,
        )
    };
    if yes != IDYES {
        return;
    }
    stop_running_instance();
    crate::launch::set_start_with_windows(false);
    let _ = std::fs::remove_file(shortcut_path());
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, UNINSTALL_KEY);
    }
    // A running exe can't delete itself: remove the folder from a hidden
    // helper once this process has exited.
    let dir = install_dir();
    if dir.exists() {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let script = format!("ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"", dir.display());
        let _ = std::process::Command::new("cmd")
            .args(["/c", &script])
            .current_dir(std::env::temp_dir())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    unsafe {
        MessageBoxW(None, w!("SIGMA5 has been uninstalled."), w!("SIGMA5"), MB_OK | MB_ICONINFORMATION);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_version("v1.10"), Some((1, 10, 0)));
        assert!(parse_version("0.10.0") > parse_version("0.9.9"));
        assert_eq!(parse_version("x"), None);
    }

    #[test]
    fn startup_decisions() {
        use Startup::*;
        // First run from Downloads → install.
        assert_eq!(decide(false, false, None, "0.2.0"), Installer { update: false });
        // The installed copy, or autostart / --portable → just run.
        assert_eq!(decide(false, true, Some("0.2.0"), "0.2.0"), Normal);
        assert_eq!(decide(true, false, None, "0.2.0"), Normal);
        // Newer download → offer update; same/older (e.g. dev build) → run.
        assert_eq!(decide(false, false, Some("0.1.0"), "0.2.0"), Installer { update: true });
        assert_eq!(decide(false, false, Some("0.2.0"), "0.2.0"), Normal);
        assert_eq!(decide(false, false, Some("0.3.0"), "0.2.0"), Normal);
    }
}
