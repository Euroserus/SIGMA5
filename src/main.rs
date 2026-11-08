//! SIGMA5 (S5): a fast, lightweight launcher for Windows.
//!
//! Architecture:
//! - `ui`        native Win32 window (GDI), tray icon, global hotkey
//! - `providers` result sources ("plugins"): apps, built-in commands, ...
//! - `fuzzy`     ranking of results against the query
//! - `history`   frecency data used to boost frequently used items
//! - `launch`    starting apps/files via the shell
//! - `config`    user settings in %APPDATA%\SIGMA5\config.toml
//! - `install`   self-install / update / uninstall (no separate setup program)

#![windows_subsystem = "windows"]

mod calc;
mod clipboard;
mod colors;
mod config;
mod files;
mod focus;
mod fuzzy;
mod history;
mod install;
mod keys;
mod launch;
mod providers;
mod sysinfo;
mod ui;
mod update;
mod util;
mod winget;

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, LPARAM, WPARAM};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, FindWindowW, GetWindowThreadProcessId, PostMessageW,
};
use windows::core::{PCWSTR, w};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let background = has("--background");

    // COM on the UI thread (shell links, window property stores, shell items).
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED | windows::Win32::System::Com::COINIT_DISABLE_OLE1DDE,
        );
    }

    // Install / uninstall (no separate setup program).
    if has("--uninstall") {
        install::uninstall_interactive();
        return;
    }
    if has("--install") {
        // Quiet install with defaults, then start the installed copy in the tray.
        if install::install(install::Options { launch_at_login: true, shortcut: true }).is_ok() {
            install::launch_installed(true);
        }
        return;
    }
    if let install::Startup::Installer { update } = install::startup_mode(&args) {
        match ui::installer::run(update) {
            ui::installer::Outcome::RunPortable => {}
            _ => return,
        }
    }

    // Single instance: if already running, ask it to open and exit.
    let _mutex = unsafe { CreateMutexW(None, true, w!("Local\\SIGMA5.SingleInstance")) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        if !background {
            let class = util::wide(ui::WINDOW_CLASS);
            if let Ok(hwnd) = unsafe { FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()) } {
                unsafe {
                    let mut pid = 0u32;
                    GetWindowThreadProcessId(hwnd, Some(&mut pid));
                    let _ = AllowSetForegroundWindow(pid);
                    let _ = PostMessageW(Some(hwnd), ui::WM_APP_SHOW, WPARAM(0), LPARAM(0));
                }
            }
        }
        return;
    }

    let hwnd = ui::window::create();
    if !background {
        unsafe {
            let _ = PostMessageW(Some(hwnd), ui::WM_APP_SHOW, WPARAM(0), LPARAM(0));
        }
    }
    ui::window::run_message_loop();
}
