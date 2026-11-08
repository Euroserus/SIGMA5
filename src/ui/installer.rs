//! The welcome / install window shown when `sigma5.exe` runs from outside its
//! install folder (e.g. straight from Downloads).

use std::cell::Cell;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

use super::draw::{
    self, BackBuffer, DISPLAY_FACES, UI_FACES, bordered_round_rect, circle, contains, fill, mix, round_rect, text,
};
use super::theme::{self, Theme, WHITE};
use super::tray;
use crate::config::{Accent, ConfigStore, ThemeMode};
use crate::install::{self, Options};
use crate::util::wide;

const CLASS: &str = "SIGMA5.Installer";
const CLIENT_W: i32 = 480;
const CLIENT_H: i32 = 452;
const WORK_TIMER: usize = 1;

/// How the installer ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Installed and the installed copy was launched; this process should exit.
    Installed,
    /// Run this exe as a portable launcher without installing.
    RunPortable,
    Cancelled,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ctl {
    AtLogin,
    Shortcut,
    Primary,
    Secondary,
}

/// Everything the installer draws (kept separate so it can render off-screen).
pub struct View {
    pub update: bool,
    pub old_version: Option<String>,
    pub at_login: bool,
    pub shortcut: bool,
    pub hover: Option<Ctl>,
    pub busy: bool,
    pub error: Option<String>,
}

pub struct Fonts {
    title: HFONT,
    label: HFONT,
    small: HFONT,
    button: HFONT,
}

impl Fonts {
    pub fn create(dpi: u32) -> Self {
        let s = |v: i32| (v * dpi as i32 + 48) / 96;
        Self {
            title: draw::font(s(28), 600, DISPLAY_FACES),
            label: draw::font(s(14), 400, UI_FACES),
            small: draw::font(s(12), 400, UI_FACES),
            button: draw::font(s(14), 600, UI_FACES),
        }
    }
    pub fn destroy(&self) {
        draw::delete_fonts(&[self.title, self.label, self.small, self.button]);
    }
}

fn toggle(dc: HDC, t: &Theme, s: &dyn Fn(i32) -> i32, right: i32, cy: i32, on: bool, hovered: bool) -> RECT {
    let (tw, th) = (s(40), s(20));
    let track = RECT { left: right - tw, top: cy - th / 2, right, bottom: cy - th / 2 + th };
    let col = match (on, hovered) {
        (true, false) => t.accent,
        (true, true) => mix(t.accent, WHITE, 0.12),
        (false, false) => t.toggle_off,
        (false, true) => mix(t.toggle_off, t.text, 0.15),
    };
    round_rect(dc, &track, th, col);
    circle(dc, if on { track.right - th / 2 } else { track.left + th / 2 }, cy, s(14), WHITE);
    track
}

/// Draws the whole window; returns clickable zones.
pub fn draw(dc: HDC, w: i32, h: i32, v: &View, t: &Theme, f: &Fonts, dpi: u32, logo: Option<HICON>) -> Vec<(RECT, Ctl)> {
    let s = |x: i32| (x * dpi as i32 + 48) / 96;
    let mut zones = Vec::new();
    super::draw::set_corner_cap(s(t.radius));
    fill(dc, &RECT { left: 0, top: 0, right: w, bottom: h }, t.bg);

    // Logo + title.
    let lsz = s(72);
    if let Some(l) = logo {
        unsafe {
            let _ = DrawIconEx(dc, (w - lsz) / 2, s(30), l, lsz, lsz, 0, None, DI_NORMAL);
        }
    }
    let title = if v.update { "Update SIGMA5" } else { "Welcome to SIGMA5" };
    text(dc, title, &RECT { left: 0, top: s(112), right: w, bottom: s(150) }, f.title, t.text, DT_CENTER);
    let sub = match (&v.old_version, v.update) {
        (Some(old), true) => format!("Version {} is ready (you have {old})", install::version_label()),
        _ => format!("A fast launcher for engineers · v{}", install::version_label()),
    };
    text(dc, &sub, &RECT { left: s(20), top: s(150), right: w - s(20), bottom: s(172) }, f.small, t.subtext, DT_CENTER | DT_END_ELLIPSIS);

    // Options card.
    let (x0, x1) = (s(32), w - s(32));
    let card = RECT { left: x0, top: s(194), right: x1, bottom: s(194) + s(124) };
    bordered_round_rect(dc, &card, s(8), 1, t.card, t.card_border);
    fill(dc, &RECT { left: x0 + s(16), top: card.top + s(62), right: x1 - s(16), bottom: card.top + s(63) }, t.divider);
    let rows = [
        (Ctl::AtLogin, v.at_login, "Launch at login", "Start quietly in the tray when you sign in"),
        (Ctl::Shortcut, v.shortcut, "Start menu shortcut", "Find SIGMA5 in Start like any other app"),
    ];
    for (i, (ctl, on, label, desc)) in rows.into_iter().enumerate() {
        let top = card.top + i as i32 * s(62);
        let cy = top + s(31);
        let tr = toggle(dc, t, &s, x1 - s(18), cy, on, v.hover == Some(ctl));
        let lr = RECT { left: x0 + s(18), top: cy - s(19), right: tr.left - s(12), bottom: cy };
        text(dc, label, &lr, f.label, t.text, DT_LEFT | DT_END_ELLIPSIS);
        let dr = RECT { left: x0 + s(18), top: cy + s(1), right: tr.left - s(12), bottom: cy + s(19) };
        text(dc, desc, &dr, f.small, t.subtext, DT_LEFT | DT_END_ELLIPSIS);
        zones.push((RECT { left: x0, top, right: x1, bottom: top + s(62) }, ctl));
    }

    // Where it goes / status.
    let (msg, col): (String, COLORREF) = match (&v.error, v.busy) {
        (Some(e), _) => (e.clone(), t.warn),
        (None, true) => (if v.update { "Updating…" } else { "Installing…" }.into(), t.accent),
        _ => ("Installs to %LOCALAPPDATA%\\Programs\\SIGMA5 · no admin rights needed".into(), t.subtext),
    };
    text(dc, &msg, &RECT { left: s(20), top: s(330), right: w - s(20), bottom: s(350) }, f.small, col, DT_CENTER | DT_END_ELLIPSIS);

    // Buttons.
    let bh = s(42);
    let by = h - s(32) - bh;
    let gap = s(12);
    let bw = (x1 - x0 - gap) / 2;
    let secondary = RECT { left: x0, top: by, right: x0 + bw, bottom: by + bh };
    let primary = RECT { left: x1 - bw, top: by, right: x1, bottom: by + bh };
    let sec_label = if v.update { "Not now" } else { "Just run it" };
    round_rect(dc, &secondary, s(8), if v.hover == Some(Ctl::Secondary) { t.hover } else { t.kbd_bg });
    text(dc, sec_label, &secondary, f.button, t.text, DT_CENTER);
    let pri_fill = if v.busy {
        mix(t.accent, t.bg, 0.4)
    } else if v.hover == Some(Ctl::Primary) {
        mix(t.accent, WHITE, 0.12)
    } else {
        t.accent
    };
    round_rect(dc, &primary, s(8), pri_fill);
    let pri_label = if v.update { "Update" } else { "Install" };
    text(dc, pri_label, &primary, f.button, WHITE, DT_CENTER);
    zones.push((secondary, Ctl::Secondary));
    zones.push((primary, Ctl::Primary));
    zones
}

// ---------------------------------------------------------------------------
// Window

struct State {
    hwnd: HWND,
    dpi: u32,
    theme: Theme,
    fonts: Fonts,
    view: View,
    zones: Vec<(RECT, Ctl)>,
    tracking: bool,
    backbuf: BackBuffer,
    logo: Option<HICON>,
}

thread_local! {
    static OUTCOME: Cell<Outcome> = const { Cell::new(Outcome::Cancelled) };
}

impl State {
    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn close(&self, outcome: Outcome) {
        OUTCOME.with(|o| o.set(outcome));
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    fn act(&mut self, ctl: Ctl) {
        if self.view.busy {
            return;
        }
        match ctl {
            Ctl::AtLogin => self.view.at_login ^= true,
            Ctl::Shortcut => self.view.shortcut ^= true,
            Ctl::Secondary => self.close(if self.view.update { Outcome::Cancelled } else { Outcome::RunPortable }),
            Ctl::Primary => {
                // Show "Installing…" first, then do the work on a timer tick.
                self.view.busy = true;
                self.view.error = None;
                unsafe {
                    SetTimer(Some(self.hwnd), WORK_TIMER, 30, None);
                }
            }
        }
        self.invalidate();
    }

    fn do_install(&mut self) {
        let opts = Options { launch_at_login: self.view.at_login, shortcut: self.view.shortcut };
        match install::install(opts) {
            Ok(_) => {
                install::launch_installed(false);
                self.close(Outcome::Installed);
            }
            Err(e) => {
                self.view.busy = false;
                self.view.error = Some(e);
            }
        }
        self.invalidate();
    }

    fn paint(&mut self) {
        let mut ps = PAINTSTRUCT::default();
        let hdc = unsafe { BeginPaint(self.hwnd, &mut ps) };
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        if rc.right > 0 && rc.bottom > 0 {
            let dc = self.backbuf.ensure(hdc, rc.right, rc.bottom);
            self.zones = draw(dc, rc.right, rc.bottom, &self.view, &self.theme, &self.fonts, self.dpi, self.logo);
            unsafe {
                let _ = BitBlt(hdc, 0, 0, rc.right, rc.bottom, Some(dc), 0, 0, SRCCOPY);
            }
        }
        unsafe {
            let _ = EndPaint(self.hwnd, &ps);
        }
    }
}

fn point(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut State;
    if ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
    }
    if msg == WM_DESTROY {
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            let st = Box::from_raw(ptr);
            st.fonts.destroy();
            if let Some(l) = st.logo {
                let _ = DestroyIcon(l);
            }
            PostQuitMessage(0);
        }
        return LRESULT(0);
    }
    let st = unsafe { &mut *ptr };
    let zone_at = |st: &State, x: i32, y: i32| st.zones.iter().find(|(r, _)| contains(r, x, y)).map(|(_, c)| *c);
    match msg {
        WM_PAINT => {
            st.paint();
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEMOVE => {
            if !st.tracking {
                let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                unsafe {
                    let _ = TrackMouseEvent(&mut tme);
                }
                st.tracking = true;
            }
            let (x, y) = point(lp);
            let h = zone_at(st, x, y);
            if h != st.view.hover {
                st.view.hover = h;
                st.invalidate();
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            st.tracking = false;
            st.view.hover = None;
            st.invalidate();
            LRESULT(0)
        }
        WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT && st.view.hover.is_some() => unsafe {
            if let Ok(c) = LoadCursorW(None, IDC_HAND) {
                SetCursor(Some(c));
            }
            LRESULT(1)
        },
        WM_LBUTTONUP => {
            let (x, y) = point(lp);
            if let Some(c) = zone_at(st, x, y) {
                st.act(c);
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            match wp.0 as u32 {
                0x0D => st.act(Ctl::Primary),
                0x1B => st.close(Outcome::Cancelled),
                _ => {}
            }
            LRESULT(0)
        }
        WM_TIMER if wp.0 == WORK_TIMER => {
            unsafe {
                let _ = KillTimer(Some(hwnd), WORK_TIMER);
            }
            st.do_install();
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Shows the installer and runs its message loop until it closes.
pub fn run(update: bool) -> Outcome {
    let store = ConfigStore::load();
    let theme = theme::resolve(ThemeMode::System, Accent::Violet, &crate::config::Appearance::default());
    let old_version = install::installed_version();
    let view = View {
        update,
        old_version,
        at_login: if update { store.config.start_with_windows } else { true },
        shortcut: true,
        hover: None,
        busy: false,
        error: None,
    };
    unsafe {
        let hinst = GetModuleHandleW(None).expect("module handle");
        let class = wide(CLASS);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: tray::load_app_icon(32).unwrap_or_default(),
            hIconSm: tray::load_app_icon(16).unwrap_or_default(),
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let (mut dpi, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dpi, &mut dy);
        let sc = |v: i32| (v * dpi as i32 + 48) / 96;
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
        let mut wr = RECT { left: 0, top: 0, right: sc(CLIENT_W), bottom: sc(CLIENT_H) };
        let _ = AdjustWindowRectExForDpi(&mut wr, style, false, WINDOW_EX_STYLE(0), dpi);
        let (ww, wh) = (wr.right - wr.left, wr.bottom - wr.top);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let wa = mi.rcWork;
        let x = wa.left + ((wa.right - wa.left) - ww) / 2;
        let y = (wa.top + ((wa.bottom - wa.top) - wh) / 2).max(wa.top);
        let title = wide(if update { "Update SIGMA5" } else { "Install SIGMA5" });
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(0), PCWSTR(class.as_ptr()), PCWSTR(title.as_ptr()), style,
            x, y, ww, wh, None, None, Some(hinst.into()), None,
        ) else {
            return Outcome::RunPortable;
        };
        let dark: i32 = theme.dark as i32;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const _, 4);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR, &theme.bg as *const _ as *const _, 4);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_TEXT_COLOR, &theme.text as *const _ as *const _, 4);

        let st = Box::new(State {
            hwnd,
            dpi,
            theme,
            fonts: Fonts::create(dpi),
            view,
            zones: Vec::new(),
            tracking: false,
            backbuf: BackBuffer::default(),
            logo: tray::load_app_icon(sc(72)),
        });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(st) as isize);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    OUTCOME.with(|o| o.get())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Off-screen render of the installer (fresh + update) to a BMP.
    /// `cargo test --release render_installer -- --ignored`
    #[test]
    #[ignore]
    fn render_installer() {
        let out = std::env::var("SIGMA5_RENDER_OUT").unwrap_or_else(|_| "installer.bmp".into());
        let dpi = 144u32;
        let s = |v: i32| (v * dpi as i32 + 48) / 96;
        let (w, h) = (s(CLIENT_W), s(CLIENT_H));
        let views = [
            View { update: false, old_version: None, at_login: true, shortcut: true, hover: Some(Ctl::Primary), busy: false, error: None },
            View { update: true, old_version: Some("0.1.0".into()), at_login: true, shortcut: false, hover: None, busy: true, error: None },
        ];
        let total_w = w * views.len() as i32;
        unsafe {
            let dc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: total_w, biHeight: -h,
                    biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bmp = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
            SelectObject(dc, bmp.into());
            SetBkMode(dc, TRANSPARENT);
            let fonts = Fonts::create(dpi);
            let logo = tray::load_app_icon(s(72));
            let mut t = theme::DARK;
            t.accent = theme::rgb(171, 114, 255);
            for (i, v) in views.iter().enumerate() {
                let _ = SetViewportOrgEx(dc, i as i32 * w, 0, None);
                draw(dc, w, h, v, &t, &fonts, dpi, logo);
            }
            fonts.destroy();
            let px = std::slice::from_raw_parts(bits as *const u8, (total_w * h * 4) as usize);
            let mut f = Vec::new();
            f.extend_from_slice(b"BM");
            f.extend_from_slice(&((54 + px.len()) as u32).to_le_bytes());
            f.extend_from_slice(&[0u8; 4]);
            f.extend_from_slice(&54u32.to_le_bytes());
            f.extend_from_slice(&40u32.to_le_bytes());
            f.extend_from_slice(&total_w.to_le_bytes());
            f.extend_from_slice(&(-h).to_le_bytes());
            f.extend_from_slice(&1u16.to_le_bytes());
            f.extend_from_slice(&32u16.to_le_bytes());
            f.extend_from_slice(&[0u8; 24]);
            f.extend_from_slice(px);
            std::fs::write(&out, f).unwrap();
        }
    }
}
