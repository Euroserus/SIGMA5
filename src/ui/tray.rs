//! Notification-area (system tray) icon and its context menu.

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIIF_WARNING, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, GetSystemMetrics, HICON, IMAGE_ICON,
    LR_DEFAULTCOLOR, LoadImageW, MF_SEPARATOR, MF_STRING, SM_CXSMICON, SM_CYSMICON,
    SetForegroundWindow, TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu,
};
use windows::core::PCWSTR;

use super::WM_APP_TRAY;
use crate::util::wide;

pub const MENU_OPEN: usize = 1;
pub const MENU_SETTINGS: usize = 2;
pub const MENU_RELOAD: usize = 3;
pub const MENU_QUIT: usize = 4;

/// Loads the embedded app icon (resource id 1) at a given pixel size.
pub fn load_app_icon(size: i32) -> Option<HICON> {
    unsafe {
        let hinst = GetModuleHandleW(None).ok()?;
        LoadImageW(Some(hinst.into()), PCWSTR(1 as *const u16), IMAGE_ICON, size, size, LR_DEFAULTCOLOR)
            .ok()
            .map(|h| HICON(h.0))
    }
}

fn copy_into(dst: &mut [u16], s: &str) {
    let w: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..w.len()].copy_from_slice(&w);
    dst[w.len()] = 0;
}

fn base(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    }
}

pub fn add(hwnd: HWND, tip: &str) {
    let mut nid = base(hwnd);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_APP_TRAY;
    let sz = unsafe { GetSystemMetrics(SM_CXSMICON).max(GetSystemMetrics(SM_CYSMICON)) };
    nid.hIcon = load_app_icon(sz).unwrap_or_default();
    copy_into(&mut nid.szTip, tip);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
    }
}

pub fn set_tip(hwnd: HWND, tip: &str) {
    let mut nid = base(hwnd);
    nid.uFlags = NIF_TIP;
    copy_into(&mut nid.szTip, tip);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

pub fn notify(hwnd: HWND, title: &str, body: &str, warning: bool) {
    let mut nid = base(hwnd);
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = if warning { NIIF_WARNING } else { NIIF_INFO };
    copy_into(&mut nid.szInfoTitle, title);
    copy_into(&mut nid.szInfo, body);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

pub fn remove(hwnd: HWND) {
    let nid = base(hwnd);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

/// Shows the context menu and returns the chosen id (0 if dismissed).
/// Must be called without holding the window state: it runs a modal loop.
pub fn show_menu(hwnd: HWND, hotkey: &str) -> usize {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return 0 };
        let open = wide(&format!("Open SIGMA5\t{hotkey}"));
        let _ = AppendMenuW(menu, MF_STRING, MENU_OPEN, PCWSTR(open.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let settings = wide("Settings…");
        let _ = AppendMenuW(menu, MF_STRING, MENU_SETTINGS, PCWSTR(settings.as_ptr()));
        let reload = wide("Reload Applications");
        let _ = AppendMenuW(menu, MF_STRING, MENU_RELOAD, PCWSTR(reload.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let quit = wide("Quit SIGMA5");
        let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, PCWSTR(quit.as_ptr()));

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Required so the menu closes when clicking elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, None, hwnd, None);
        let _ = DestroyMenu(menu);
        cmd.0 as usize
    }
}
