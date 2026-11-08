//! Screen colour picker.
//!
//! Two windows exist only while picking:
//! - a click-catching layer over every monitor (alpha 1, so effectively
//!   invisible, with a crosshair cursor), and
//! - a loupe next to the cursor showing the pixels around it magnified, the
//!   centre pixel outlined, and its hex code.
//!
//! Both are excluded from screen capture, so they never tint what's sampled.
//! Click picks, Esc or right-click cancels, arrow keys nudge by one pixel
//! (Shift: ten). The result is posted to the launcher as
//! `WM_APP_COLOR_PICKED` (WPARAM = COLORREF, LPARAM = 1 if picked).

use std::cell::RefCell;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, SetFocus, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_UP};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use super::WM_APP_COLOR_PICKED;
use super::draw::{self, BackBuffer, fill, mix, text};
use crate::colors::Color;

/// Pixels across the loupe (odd, so there's a centre pixel).
const GRID: i32 = 15;
/// Size of each magnified pixel, in logical px.
const ZOOM: i32 = 9;
const LAYER_CLASS: PCWSTR = w!("SIGMA5.PickerLayer");
const LOUPE_CLASS: PCWSTR = w!("SIGMA5.PickerLoupe");
const WDA_EXCLUDEFROMCAPTURE: u32 = 0x11;
const WM_PICK_DONE: u32 = WM_APP + 1;

struct Picker {
    layer: HWND,
    loupe: HWND,
    launcher: HWND,
    accent: COLORREF,
    dpi: u32,
    font: HFONT,
    pixels: [u32; (GRID * GRID) as usize],
    backbuf: BackBuffer,
    /// Set when finishing: the colour, or None if cancelled.
    result: Option<Option<Color>>,
}

thread_local! {
    static PICKER: RefCell<Option<Box<Picker>>> = const { RefCell::new(None) };
}

pub fn is_active() -> bool {
    PICKER.with(|p| p.borrow().is_some())
}

fn s(dpi: u32, v: i32) -> i32 {
    (v * dpi as i32 + 48) / 96
}

fn register() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let layer = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(layer_proc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
            lpszClassName: LAYER_CLASS,
            ..Default::default()
        };
        RegisterClassExW(&layer);
        let loupe = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(loupe_proc),
            hInstance: hinst.into(),
            lpszClassName: LOUPE_CLASS,
            ..Default::default()
        };
        RegisterClassExW(&loupe);
    });
}

/// Loupe size in physical px: the grid plus a caption bar.
fn loupe_size(dpi: u32) -> (i32, i32) {
    let g = GRID * s(dpi, ZOOM) + 2;
    (g, g + s(dpi, 30))
}

/// Starts picking. Does nothing if a pick is already running.
pub fn start(launcher: HWND, accent: COLORREF) {
    if is_active() {
        return;
    }
    register();
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let (vx, vy) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        let (vw, vh) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let (mut dpi, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dpi, &mut dy);

        let Ok(layer) = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            LAYER_CLASS,
            w!("SIGMA5 Colour Picker"),
            WS_POPUP,
            vx,
            vy,
            vw,
            vh,
            None,
            None,
            Some(hinst.into()),
            None,
        ) else {
            return;
        };
        let (lw, lh) = loupe_size(dpi);
        let Ok(loupe) = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            LOUPE_CLASS,
            PCWSTR::null(),
            WS_POPUP,
            pt.x,
            pt.y,
            lw,
            lh,
            Some(layer),
            None,
            Some(hinst.into()),
            None,
        ) else {
            let _ = DestroyWindow(layer);
            return;
        };
        let _ = SetLayeredWindowAttributes(layer, COLORREF(0), 1, LWA_ALPHA);
        let _ = SetWindowDisplayAffinity(layer, WINDOW_DISPLAY_AFFINITY(WDA_EXCLUDEFROMCAPTURE));
        let _ = SetWindowDisplayAffinity(loupe, WINDOW_DISPLAY_AFFINITY(WDA_EXCLUDEFROMCAPTURE));

        let picker = Box::new(Picker {
            layer,
            loupe,
            launcher,
            accent,
            dpi,
            font: draw::font(s(dpi, 13), 600, &["Cascadia Mono", "Consolas"]),
            pixels: [0; (GRID * GRID) as usize],
            backbuf: BackBuffer::default(),
            result: None,
        });
        PICKER.with(|p| *p.borrow_mut() = Some(picker));
        update();
        let _ = ShowWindow(layer, SW_SHOW);
        let _ = ShowWindow(loupe, SW_SHOWNOACTIVATE);
        let _ = SetForegroundWindow(layer);
        let _ = SetFocus(Some(layer));
    }
}

/// Samples the pixels round the cursor and moves the loupe next to it.
fn update() {
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    PICKER.with(|p| {
        let mut p = p.borrow_mut();
        let Some(pk) = p.as_mut() else { return };
        pk.pixels = sample(pt);
        let (lw, lh) = loupe_size(pk.dpi);
        let gap = s(pk.dpi, 22);
        // Below-right of the cursor, flipped near the screen edges.
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        unsafe {
            let _ = GetMonitorInfoW(MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST), &mut mi);
        }
        let m = mi.rcMonitor;
        let x = if pt.x + gap + lw > m.right { pt.x - gap - lw } else { pt.x + gap };
        let y = if pt.y + gap + lh > m.bottom { pt.y - gap - lh } else { pt.y + gap };
        unsafe {
            let _ = SetWindowPos(pk.loupe, Some(HWND_TOPMOST), x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
            let _ = InvalidateRect(Some(pk.loupe), None, false);
        }
    });
}

/// The GRID×GRID pixels centred on `pt`, as COLORREFs.
fn sample(pt: POINT) -> [u32; (GRID * GRID) as usize] {
    let mut out = [0u32; (GRID * GRID) as usize];
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: GRID,
                biHeight: -GRID,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        if let Ok(bmp) = CreateDIBSection(Some(mem), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            let old = SelectObject(mem, bmp.into());
            let r = GRID / 2;
            let _ = BitBlt(mem, 0, 0, GRID, GRID, Some(screen), pt.x - r, pt.y - r, SRCCOPY);
            let _ = GdiFlush();
            if !bits.is_null() {
                let px = std::slice::from_raw_parts(bits as *const u32, (GRID * GRID) as usize);
                for (o, &v) in out.iter_mut().zip(px) {
                    // DIB is 0x00RRGGBB; COLORREF is 0x00BBGGRR.
                    *o = (v >> 16) & 0xFF | (v & 0xFF00) | (v & 0xFF) << 16;
                }
            }
            SelectObject(mem, old);
            let _ = DeleteObject(bmp.into());
        }
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
    }
    out
}

fn center(pk: &Picker) -> Color {
    Color::from_colorref(pk.pixels[(GRID * GRID / 2) as usize])
}

/// Finishes (from a posted message, so no window is mid-message).
fn finish(result: Option<Color>) {
    PICKER.with(|p| {
        if let Some(pk) = p.borrow_mut().as_mut() {
            pk.result = Some(result);
        }
    });
    let layer = PICKER.with(|p| p.borrow().as_ref().map(|pk| pk.layer));
    if let Some(layer) = layer {
        unsafe {
            let _ = PostMessageW(Some(layer), WM_PICK_DONE, WPARAM(0), LPARAM(0));
        }
    }
}

fn done() {
    let Some(pk) = PICKER.with(|p| p.borrow_mut().take()) else { return };
    unsafe {
        let _ = DestroyWindow(pk.loupe);
        let _ = DestroyWindow(pk.layer);
        let _ = DeleteObject(pk.font.into());
        let (c, ok) = match pk.result.flatten() {
            Some(c) => (c.colorref(), 1),
            None => (0, 0),
        };
        let _ = PostMessageW(Some(pk.launcher), WM_APP_COLOR_PICKED, WPARAM(c as usize), LPARAM(ok));
    }
}

fn paint_loupe(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        PICKER.with(|p| {
            let mut p = p.borrow_mut();
            let Some(pk) = p.as_mut() else { return };
            let (w, h) = loupe_size(pk.dpi);
            let dc = pk.backbuf.ensure(hdc, w, h);
            let z = s(pk.dpi, ZOOM);
            let bg = COLORREF(0x001F1C1C);
            fill(dc, &RECT { left: 0, top: 0, right: w, bottom: h }, mix(bg, COLORREF(0x00FFFFFF), 0.18));
            for gy in 0..GRID {
                for gx in 0..GRID {
                    let c = COLORREF(pk.pixels[(gy * GRID + gx) as usize]);
                    let rc = RECT { left: 1 + gx * z, top: 1 + gy * z, right: 1 + (gx + 1) * z, bottom: 1 + (gy + 1) * z };
                    fill(dc, &rc, c);
                }
            }
            // Centre pixel: a two-tone square outline readable on any colour.
            let c0 = 1 + (GRID / 2) * z;
            let cc = center(pk);
            let ring = if cc.luminance() > 0.5 { COLORREF(0) } else { COLORREF(0x00FFFFFF) };
            for (inset, col) in [(-2, ring), (-1, pk.accent)] {
                let r = RECT { left: c0 + inset, top: c0 + inset, right: c0 + z - inset, bottom: c0 + z - inset };
                let b = CreateSolidBrush(col);
                let _ = FrameRect(dc, &r, b);
                let _ = DeleteObject(b.into());
            }
            // Caption: swatch + hex.
            let top = GRID * z + 2;
            fill(dc, &RECT { left: 0, top, right: w, bottom: h }, bg);
            let sw = s(pk.dpi, 16);
            let sy = top + (h - top - sw) / 2;
            let pad = s(pk.dpi, 8);
            fill(dc, &RECT { left: pad, top: sy, right: pad + sw, bottom: sy + sw }, COLORREF(cc.colorref()));
            let tr = RECT { left: pad + sw + s(pk.dpi, 8), top, right: w - pad, bottom: h };
            text(dc, &cc.hex(), &tr, pk.font, COLORREF(0x00F1EEEE), DT_LEFT);
            let _ = BitBlt(hdc, 0, 0, w, h, Some(dc), 0, 0, SRCCOPY);
        });
        let _ = EndPaint(hwnd, &ps);
    }
}

unsafe extern "system" fn layer_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            update();
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            update();
            let c = PICKER.with(|p| p.borrow().as_ref().map(|pk| center(pk)));
            finish(c);
            LRESULT(0)
        }
        WM_RBUTTONDOWN => {
            finish(None);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = wp.0 as u16;
            let step = if unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0 { 10 } else { 1 };
            let (dx, dy) = match vk {
                v if v == VK_LEFT.0 => (-step, 0),
                v if v == VK_RIGHT.0 => (step, 0),
                v if v == VK_UP.0 => (0, -step),
                v if v == VK_DOWN.0 => (0, step),
                v if v == VK_ESCAPE.0 => {
                    finish(None);
                    return LRESULT(0);
                }
                v if v == VK_RETURN.0 => {
                    let c = PICKER.with(|p| p.borrow().as_ref().map(|pk| center(pk)));
                    finish(c);
                    return LRESULT(0);
                }
                _ => (0, 0),
            };
            if (dx, dy) != (0, 0) {
                let mut pt = POINT::default();
                unsafe {
                    let _ = GetCursorPos(&mut pt);
                    let _ = SetCursorPos(pt.x + dx, pt.y + dy);
                }
                update();
            }
            LRESULT(0)
        }
        // Another window took over: give up.
        WM_ACTIVATE if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE => {
            if PICKER.with(|p| p.borrow().as_ref().is_some_and(|pk| pk.result.is_none())) {
                finish(None);
            }
            LRESULT(0)
        }
        WM_PICK_DONE => {
            done();
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

unsafe extern "system" fn loupe_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint_loupe(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a whole pick on a private desktop (never visible, no real input:
    /// the "click" is a message sent to our own window).
    /// `cargo test --release picker_offscreen -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn picker_offscreen() {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn CreateDesktopW(name: PCWSTR, dev: PCWSTR, mode: *const core::ffi::c_void, flags: u32, access: u32, sa: *const core::ffi::c_void) -> isize;
            fn SetThreadDesktop(desk: isize) -> i32;
            fn CloseDesktop(desk: isize) -> i32;
        }
        std::thread::spawn(|| unsafe {
            let desk = CreateDesktopW(w!("SIGMA5.PickerTest"), PCWSTR::null(), std::ptr::null(), 0, 0x1000_0000, std::ptr::null());
            assert!(desk != 0 && SetThreadDesktop(desk) != 0);
            // A stand-in launcher that records what it's sent.
            let launcher = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_POPUP, 0, 0, 1, 1, None, None, None, None).unwrap();
            start(launcher, COLORREF(0x00FF72AB));
            assert!(is_active());
            let layer = PICKER.with(|p| p.borrow().as_ref().unwrap().layer);
            SendMessageW(layer, WM_MOUSEMOVE, None, None);
            // Paint the loupe once to exercise drawing.
            let loupe = PICKER.with(|p| p.borrow().as_ref().unwrap().loupe);
            let _ = RedrawWindow(Some(loupe), None, None, RDW_INVALIDATE | RDW_UPDATENOW);
            SendMessageW(layer, WM_LBUTTONDOWN, None, None);
            // Pump until the launcher receives the result.
            let mut msg = MSG::default();
            let mut got = None;
            for _ in 0..200 {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_APP_COLOR_PICKED {
                        got = Some((msg.wParam.0, msg.lParam.0));
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                if got.is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let (c, ok) = got.expect("result posted to the launcher");
            println!("picked {} (ok={ok})", Color::from_colorref(c as u32).hex());
            assert_eq!(ok, 1);
            assert!(!is_active(), "windows cleaned up");
            let _ = DestroyWindow(launcher);
            CloseDesktop(desk);
        })
        .join()
        .unwrap();
    }
}
