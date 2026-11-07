//! SIGMA5 (S5): a fast, lightweight launcher for Windows.
//!
//! Architecture:
//! - `ui`        native Win32 window (GDI), tray icon, global hotkey
//! - `providers` result sources ("plugins"): apps, built-in commands, ...
//! - `fuzzy`     ranking of results against the query
//! - `history`   frecency data used to boost frequently used items
//! - `launch`    starting apps/files via the shell
//! - `config`    user settings in %APPDATA%\SIGMA5\config.toml

#![windows_subsystem = "windows"]

mod calc;
mod config;
mod focus;
mod fuzzy;
mod keys;
mod history;
mod launch;
mod providers;
mod ui;
mod util;

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, LPARAM, WPARAM};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, FindWindowW, GetWindowThreadProcessId, PostMessageW,
};
use windows::core::{PCWSTR, w};

fn main() {
    let background = std::env::args().any(|a| a == "--background");

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

    // COM on the UI thread (window property stores for app hotkeys, shell items).
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED | windows::Win32::System::Com::COINIT_DISABLE_OLE1DDE,
        );
    }
    let hwnd = ui::window::create();
    if !background {
        unsafe {
            let _ = PostMessageW(Some(hwnd), ui::WM_APP_SHOW, WPARAM(0), LPARAM(0));
        }
    }
    ui::window::run_message_loop();
}
