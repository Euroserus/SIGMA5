//! Drawing primitives shared by the launcher and settings windows.
//!
//! Text uses GDI (ClearType). Rounded shapes are rasterised in software with
//! a signed-distance function and alpha-blended, giving anti-aliased edges
//! without pulling in GDI+ or Direct2D.

use windows::Win32::Foundation::{COLORREF, RECT, SIZE};
use windows::Win32::Graphics::Gdi::*;
use windows::core::PCWSTR;

use crate::util::{utf16, wide};

// ----- fonts ----------------------------------------------------------------

fn try_font(px: i32, weight: i32, face: &str) -> Option<HFONT> {
    let fw = wide(face);
    let f = unsafe {
        CreateFontW(
            -px, 0, 0, 0, weight, 0, 0, 0,
            DEFAULT_CHARSET, OUT_TT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY,
            0, PCWSTR(fw.as_ptr()),
        )
    };
    if f.is_invalid() {
        return None;
    }
    // Verify the face actually exists (GDI silently substitutes otherwise).
    let mut buf = [0u16; 64];
    let n = unsafe {
        let hdc = GetDC(None);
        let old = SelectObject(hdc, f.into());
        let n = GetTextFaceW(hdc, Some(&mut buf));
        SelectObject(hdc, old);
        ReleaseDC(None, hdc);
        n
    };
    let got = String::from_utf16_lossy(&buf[..(n.max(1) - 1) as usize]);
    if got.eq_ignore_ascii_case(face) {
        Some(f)
    } else {
        unsafe {
            let _ = DeleteObject(f.into());
        }
        None
    }
}

/// Creates a font from the first installed face in `faces`.
pub fn font(px: i32, weight: i32, faces: &[&str]) -> HFONT {
    faces
        .iter()
        .find_map(|f| try_font(px, weight, f))
        .unwrap_or_else(|| try_font(px, weight, "Segoe UI").unwrap_or_default())
}

pub const UI_FACES: &[&str] = &["Segoe UI Variable Text", "Segoe UI"];
pub const DISPLAY_FACES: &[&str] = &["Segoe UI Variable Display", "Segoe UI"];
pub const ICON_FACES: &[&str] = &["Segoe Fluent Icons", "Segoe MDL2 Assets"];

pub fn delete_fonts(fonts: &[HFONT]) {
    for f in fonts {
        if !f.is_invalid() {
            unsafe {
                let _ = DeleteObject((*f).into());
            }
        }
    }
}

// ----- back buffer ------------------------------------------------------------

#[derive(Default)]
pub struct BackBuffer {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    w: i32,
    h: i32,
}

impl BackBuffer {
    pub fn ensure(&mut self, target: HDC, w: i32, h: i32) -> HDC {
        if self.dc.is_invalid() || self.w != w || self.h != h {
            self.release();
            unsafe {
                self.dc = CreateCompatibleDC(Some(target));
                self.bmp = CreateCompatibleBitmap(target, w, h);
                self.old = SelectObject(self.dc, self.bmp.into());
                SetBkMode(self.dc, TRANSPARENT);
            }
            self.w = w;
            self.h = h;
        }
        self.dc
    }

    pub fn release(&mut self) {
        if !self.dc.is_invalid() {
            unsafe {
                SelectObject(self.dc, self.old);
                let _ = DeleteObject(self.bmp.into());
                let _ = DeleteDC(self.dc);
            }
        }
        // Reset fields individually: assigning `*self` would run Drop again.
        self.dc = HDC::default();
        self.bmp = HBITMAP::default();
        self.old = HGDIOBJ::default();
        self.w = 0;
        self.h = 0;
    }
}

impl Drop for BackBuffer {
    fn drop(&mut self) {
        self.release();
    }
}

// ----- shapes -------------------------------------------------------------------

/// Linear blend from `a` (t = 0) to `b` (t = 1).
pub fn mix(a: COLORREF, b: COLORREF, t: f32) -> COLORREF {
    let t = t.clamp(0.0, 1.0);
    let ch = |s: u32| {
        let (x, y) = ((a.0 >> s) & 0xFF, (b.0 >> s) & 0xFF);
        ((x as f32 + (y as f32 - x as f32) * t).round() as u32) << s
    };
    COLORREF(ch(0) | ch(8) | ch(16))
}

pub fn inflate(rc: &RECT, by: i32) -> RECT {
    RECT { left: rc.left - by, top: rc.top - by, right: rc.right + by, bottom: rc.bottom + by }
}

pub fn fill(hdc: HDC, rc: &RECT, c: COLORREF) {
    unsafe {
        let b = CreateSolidBrush(c);
        FillRect(hdc, rc, b);
        let _ = DeleteObject(b.into());
    }
}

/// Anti-aliased filled rounded rectangle; `radius` is the corner radius in px
/// (clamped to half the shorter side, so a large value draws a pill/circle).
pub fn round_rect(hdc: HDC, rc: &RECT, radius: i32, color: COLORREF) {
    let w = rc.right - rc.left;
    let h = rc.bottom - rc.top;
    if w <= 0 || h <= 0 {
        return;
    }
    let r = (radius as f32).min(w as f32 / 2.0).min(h as f32 / 2.0).max(0.0);
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let Ok(bmp) = (unsafe { CreateDIBSection(Some(hdc), &info, DIB_RGB_COLORS, &mut bits, None, 0) }) else {
        return;
    };
    if bits.is_null() {
        unsafe {
            let _ = DeleteObject(bmp.into());
        }
        return;
    }
    let px = unsafe { std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize) };
    let (cr, cg, cb) = (color.0 & 0xFF, (color.0 >> 8) & 0xFF, (color.0 >> 16) & 0xFF);
    let (hw, hh) = (w as f32 / 2.0, h as f32 / 2.0);
    for y in 0..h {
        let py = (y as f32 + 0.5 - hh).abs() - (hh - r);
        for x in 0..w {
            let qx = (x as f32 + 0.5 - hw).abs() - (hw - r);
            // Signed distance to the rounded rectangle; 1px coverage ramp.
            let d = qx.max(py).min(0.0) + (qx.max(0.0).powi(2) + py.max(0.0).powi(2)).sqrt() - r;
            let a = (0.5 - d).clamp(0.0, 1.0);
            let a8 = (a * 255.0 + 0.5) as u32;
            let m = |c: u32| (c * a8 + 127) / 255;
            // BGRA, premultiplied.
            px[(y * w + x) as usize] = a8 << 24 | m(cr) << 16 | m(cg) << 8 | m(cb);
        }
    }
    unsafe {
        let mem = CreateCompatibleDC(Some(hdc));
        let old = SelectObject(mem, bmp.into());
        let bf = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let _ = AlphaBlend(hdc, rc.left, rc.top, w, h, mem, 0, 0, w, h, bf);
        SelectObject(mem, old);
        let _ = DeleteDC(mem);
        let _ = DeleteObject(bmp.into());
    }
}

/// Rounded rectangle with a 1px (scaled) border.
pub fn bordered_round_rect(hdc: HDC, rc: &RECT, radius: i32, border_w: i32, fill_c: COLORREF, border_c: COLORREF) {
    round_rect(hdc, rc, radius, border_c);
    let inner = inset(rc, border_w);
    round_rect(hdc, &inner, (radius - border_w).max(0), fill_c);
}

pub fn circle(hdc: HDC, cx: i32, cy: i32, d: i32, color: COLORREF) {
    let rc = RECT { left: cx - d / 2, top: cy - d / 2, right: cx - d / 2 + d, bottom: cy - d / 2 + d };
    round_rect(hdc, &rc, d, color);
}

pub fn inset(rc: &RECT, by: i32) -> RECT {
    RECT { left: rc.left + by, top: rc.top + by, right: rc.right - by, bottom: rc.bottom - by }
}

pub fn contains(rc: &RECT, x: i32, y: i32) -> bool {
    x >= rc.left && x < rc.right && y >= rc.top && y < rc.bottom
}

// ----- text ---------------------------------------------------------------------

/// Single-line text, vertically centred in `rc`.
pub fn text(hdc: HDC, s: &str, rc: &RECT, font: HFONT, color: COLORREF, flags: DRAW_TEXT_FORMAT) {
    let mut buf = utf16(s);
    if buf.is_empty() {
        return;
    }
    let mut r = *rc;
    unsafe {
        let old = SelectObject(hdc, font.into());
        SetTextColor(hdc, color);
        DrawTextW(hdc, &mut buf, &mut r, flags | DT_SINGLELINE | DT_NOPREFIX | DT_VCENTER);
        SelectObject(hdc, old);
    }
}

pub fn text_width(hdc: HDC, s: &str, font: HFONT) -> i32 {
    let buf = utf16(s);
    let mut sz = SIZE::default();
    unsafe {
        let old = SelectObject(hdc, font.into());
        let _ = GetTextExtentPoint32W(hdc, &buf, &mut sz);
        SelectObject(hdc, old);
    }
    sz.cx
}
