//! Launcher rendering. Everything is drawn with GDI into a back buffer and
//! blitted in one go, so there is no flicker and no GPU/driver overhead.

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{DI_NORMAL, DrawIconEx};

use super::draw::{
    self, DISPLAY_FACES, ICON_FACES, UI_FACES, bordered_round_rect, circle, fill, inflate, mix, round_rect, text,
    text_width,
};
use super::theme::{Theme, WHITE, metrics as m};
use super::window::Launcher;
use crate::providers::{Icon, Row};

pub struct Fonts {
    pub search: HFONT,
    pub title: HFONT,
    pub small: HFONT,
    pub header: HFONT,
    pub glyph: HFONT,
    pub kbd_glyph: HFONT,
    pub letter: HFONT,
    pub calc_big: HFONT,
    pub calc_expr: HFONT,
    pub calc_glyph: HFONT,
    /// Clipboard text preview.
    pub mono: HFONT,
}

impl Fonts {
    /// `dpi` is the effective DPI (monitor DPI × launcher size).
    pub fn create(dpi: u32) -> Self {
        let s = |v: i32| (v * dpi as i32 + 48) / 96;
        Self {
            search: draw::font(s(19), 400, DISPLAY_FACES),
            title: draw::font(s(14), 400, UI_FACES),
            small: draw::font(s(13), 400, UI_FACES),
            header: draw::font(s(12), 600, UI_FACES),
            glyph: draw::font(s(15), 400, ICON_FACES),
            kbd_glyph: draw::font(s(11), 400, ICON_FACES),
            letter: draw::font(s(13), 600, UI_FACES),
            calc_big: draw::font(s(30), 600, DISPLAY_FACES),
            calc_expr: draw::font(s(19), 400, DISPLAY_FACES),
            calc_glyph: draw::font(s(12), 400, ICON_FACES),
            mono: draw::font(s(13), 400, &["Cascadia Mono", "Consolas"]),
        }
    }

    pub fn destroy(&mut self) {
        draw::delete_fonts(&[
            self.search, self.title, self.small, self.header, self.glyph, self.kbd_glyph, self.letter,
            self.calc_big, self.calc_expr, self.calc_glyph, self.mono,
        ]);
    }
}

pub fn paint(ui: &mut Launcher) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = unsafe { BeginPaint(ui.hwnd, &mut ps) };
    let (w, h) = ui.client_size();
    if w > 0 && h > 0 {
        let dc = ui.backbuf.ensure(hdc, w, h);
        draw(ui, dc, w, h);
        unsafe {
            let _ = BitBlt(hdc, 0, 0, w, h, Some(dc), 0, 0, SRCCOPY);
        }
    }
    unsafe {
        let _ = EndPaint(ui.hwnd, &ps);
    }
}

fn draw(ui: &Launcher, dc: HDC, w: i32, h: i32) {
    let t: &Theme = &ui.theme;
    fill(dc, &RECT { left: 0, top: 0, right: w, bottom: h }, t.bg);

    let search_h = ui.s(m::SEARCH_H);
    let footer_h = ui.s(m::FOOTER_H);
    fill(dc, &RECT { left: 0, top: search_h, right: w, bottom: search_h + 1 }, t.divider);
    fill(dc, &RECT { left: 0, top: h - footer_h - 1, right: w, bottom: h - footer_h }, t.divider);

    // Sub-pages: back arrow (left) and page title chip (right) in the search bar.
    if let Some(back) = ui.back_rect() {
        text(dc, "\u{E72B}", &back, ui.fonts.glyph, t.subtext, DT_CENTER);
        let title = ui.mode.title();
        let tw = text_width(dc, title, ui.fonts.header);
        let ch = ui.s(24);
        let cy = search_h / 2;
        let chip = RECT { left: w - ui.s(18) - tw - ui.s(20), top: cy - ch / 2, right: w - ui.s(18), bottom: cy - ch / 2 + ch };
        round_rect(dc, &chip, ui.s(6), t.kbd_bg);
        text(dc, title, &chip, ui.fonts.header, t.kbd_text, DT_CENTER);
    }

    draw_list(ui, dc);
    if ui.mode == super::window::Mode::Clipboard {
        draw_clip_preview(ui, dc);
    }
    draw_footer(ui, dc, w, h);
}

fn ago_long(then: u64) -> String {
    let s = crate::util::now_unix().saturating_sub(then);
    let n = |v: u64, unit: &str| format!("{v} {unit}{} ago", if v == 1 { "" } else { "s" });
    match s {
        0..60 => "Just now".into(),
        60..3600 => n(s / 60, "minute"),
        3600..86_400 => n(s / 3600, "hour"),
        _ => n(s / 86_400, "day"),
    }
}

fn draw_clip_preview(ui: &Launcher, dc: HDC) {
    let dpi = ((ui.s(960) as f32) / 10.0).round() as u32; // effective DPI incl. launcher size
    let e = ui.selected_clip();
    let source = e.map(|e| ui.source_name(&e.source)).unwrap_or_default();
    draw_clip_preview_into(dc, ui.preview_rect(), e, &source, &ui.theme, &ui.fonts, dpi);
}

/// Right-hand pane on the Clipboard page: the selected entry in full, then an
/// Information table. Window-independent so it can be rendered in tests.
pub fn draw_clip_preview_into(
    dc: HDC, pr: RECT, e: Option<&crate::clipboard::Entry>, source: &str, t: &Theme, fonts: &Fonts, dpi: u32,
) {
    use crate::clipboard::Content;
    let s = |v: i32| (v * dpi as i32 + 48) / 96;
    fill(dc, &RECT { left: pr.left - 1, right: pr.left, ..pr }, t.divider);
    let Some(e) = e else {
        text(dc, "Nothing selected", &pr, fonts.title, t.subtext, DT_CENTER);
        return;
    };

    let mut info: Vec<(&str, String)> =
        vec![("Source", source.to_string()), ("Content type", e.content.kind().into()), ("Copied", ago_long(e.copied))];
    match &e.content {
        Content::Text(txt) => {
            info.push(("Characters", txt.chars().count().to_string()));
            info.push(("Words", txt.split_whitespace().count().to_string()));
        }
        Content::Files(f) => info.push(("Files", f.len().to_string())),
        Content::Image { width, height, .. } => info.push(("Dimensions", format!("{width} × {height}"))),
    }
    let row_h = s(26);
    let info_top = pr.bottom - s(12) - row_h * info.len() as i32 - s(28);
    let pad = s(16);
    let content = RECT { left: pr.left + pad, top: pr.top + s(14), right: pr.right - pad, bottom: info_top - s(10) };

    unsafe {
        SaveDC(dc);
        IntersectClipRect(dc, content.left, content.top, content.right, content.bottom);
    }
    match &e.content {
        Content::Text(txt) => {
            let shown: String = txt.chars().take(4000).collect();
            draw_wrapped(dc, &shown, &content, fonts.mono, t.text);
        }
        Content::Files(f) => draw_wrapped(dc, &f.join("\n"), &content, fonts.mono, t.text),
        Content::Image { width, height, dib, .. } => draw_dib(dc, &content, *width, *height, dib, t),
    }
    unsafe {
        let _ = RestoreDC(dc, -1);
    }

    fill(dc, &RECT { left: pr.left + pad, top: info_top, right: pr.right - pad, bottom: info_top + 1 }, t.divider);
    let hr = RECT { left: pr.left + pad, top: info_top + s(4), right: pr.right - pad, bottom: info_top + s(28) };
    text(dc, "Information", &hr, fonts.header, t.header, DT_LEFT);
    for (i, (label, value)) in info.iter().enumerate() {
        let top = info_top + s(28) + i as i32 * row_h;
        let r = RECT { left: pr.left + pad, top, right: pr.right - pad, bottom: top + row_h };
        text(dc, label, &r, fonts.small, t.subtext, DT_LEFT);
        text(dc, value, &r, fonts.small, t.text, DT_RIGHT | DT_END_ELLIPSIS);
        if i + 1 < info.len() {
            fill(dc, &RECT { left: r.left, top: r.bottom - 1, right: r.right, bottom: r.bottom }, mix(t.divider, t.bg, 0.4));
        }
    }
}

/// Multi-line, word-wrapped text.
fn draw_wrapped(dc: HDC, s: &str, rc: &RECT, font: HFONT, color: windows::Win32::Foundation::COLORREF) {
    let mut buf: Vec<u16> = s.replace("\r\n", "\n").encode_utf16().collect();
    let mut r = *rc;
    unsafe {
        let old = SelectObject(dc, font.into());
        SetTextColor(dc, color);
        DrawTextW(dc, &mut buf, &mut r, DT_LEFT | DT_WORDBREAK | DT_EDITCONTROL | DT_NOPREFIX | DT_EXPANDTABS);
        SelectObject(dc, old);
    }
}

/// Draws a packed DIB scaled to fit `area`, preserving aspect ratio.
fn draw_dib(dc: HDC, area: &RECT, w: i32, h: i32, dib: &[u8], t: &Theme) {
    if w <= 0 || h <= 0 || dib.len() < 40 {
        return;
    }
    let u32_at = |o: usize| u32::from_le_bytes(dib[o..o + 4].try_into().unwrap());
    let header = u32_at(0) as usize;
    let bits = u16::from_le_bytes([dib[14], dib[15]]) as usize;
    let compression = u32_at(16);
    let used = u32_at(32) as usize;
    let masks = if compression == 3 && header == 40 { 12 } else { 0 };
    let palette = if bits <= 8 { if used > 0 { used } else { 1 << bits } } else { used } * 4;
    let offset = header + masks + palette;
    if offset >= dib.len() {
        return;
    }
    let (aw, ah) = (area.right - area.left, area.bottom - area.top);
    let scale = (aw as f32 / w as f32).min(ah as f32 / h as f32).min(1.0);
    let (dw, dh) = ((w as f32 * scale) as i32, (h as f32 * scale) as i32);
    let x = area.left + (aw - dw) / 2;
    let y = area.top + (ah - dh) / 2;
    fill(dc, &RECT { left: x - 1, top: y - 1, right: x + dw + 1, bottom: y + dh + 1 }, t.divider);
    unsafe {
        SetStretchBltMode(dc, HALFTONE);
        StretchDIBits(
            dc, x, y, dw, dh, 0, 0, w, h,
            Some(dib[offset..].as_ptr() as *const _),
            dib.as_ptr() as *const BITMAPINFO,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
    }
}

fn draw_list(ui: &Launcher, dc: HDC) {
    let t = &ui.theme;
    let lr = ui.list_rect();
    unsafe {
        SaveDC(dc);
        IntersectClipRect(dc, lr.left, lr.top, lr.right, lr.bottom);
    }

    if ui.rows.is_empty() {
        let msg = if let Some(m) = &ui.list_message {
            m.as_str()
        } else if ui.is_indexing_empty() {
            "Indexing applications…"
        } else if ui.query_is_empty() {
            "Nothing here yet"
        } else {
            "No results"
        };
        text(dc, msg, &lr, ui.fonts.title, t.subtext, DT_CENTER | DT_END_ELLIPSIS);
    }

    let pad = ui.s(m::PAD_X);
    let inner = ui.s(12);
    let icon = ui.s(m::ICON);
    let radius = ui.s(m::RADIUS);

    let icon_dc = unsafe { CreateCompatibleDC(Some(dc)) };
    let icon_old = unsafe { SelectObject(icon_dc, GetStockObject(NULL_BRUSH)) };

    for (i, row) in ui.rows.iter().enumerate() {
        let rh = ui.row_height(row);
        let top = lr.top + ui.offsets[i] - ui.scroll;
        if top + rh < lr.top {
            continue;
        }
        if top > lr.bottom {
            break;
        }
        let rc = RECT { left: lr.left + pad, top, right: lr.right - pad, bottom: top + rh };
        match row {
            Row::Header(title) => {
                let r = RECT { left: rc.left + inner, top: rc.top + ui.s(8), ..rc };
                text(dc, title, &r, ui.fonts.header, t.header, DT_LEFT);
            }
            Row::Calc(c) => draw_calc(ui, dc, &rc, c, ui.selected == Some(i)),
            Row::Item(it) => {
                if ui.selected == Some(i) {
                    round_rect(dc, &rc, radius, t.selection);
                }
                let ix = rc.left + inner;
                let iy = rc.top + (rh - icon) / 2;
                let irc = RECT { left: ix, top: iy, right: ix + icon, bottom: iy + icon };
                match &it.icon {
                    Icon::App(id) => match super::icons::get(id) {
                        Some(ic) => unsafe {
                            SelectObject(icon_dc, ic.bmp.into());
                            let bf = BLENDFUNCTION {
                                BlendOp: AC_SRC_OVER as u8,
                                BlendFlags: 0,
                                SourceConstantAlpha: 255,
                                AlphaFormat: AC_SRC_ALPHA as u8,
                            };
                            let _ = AlphaBlend(dc, ix, iy, icon, icon, icon_dc, 0, 0, ic.w, ic.h, bf);
                        },
                        None => {
                            round_rect(dc, &irc, ui.s(6), t.tile);
                            let letter: String =
                                it.title.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                            text(dc, &letter, &irc, ui.fonts.letter, t.tile_text, DT_CENTER);
                        }
                    },
                    Icon::Glyph(g) => {
                        round_rect(dc, &irc, ui.s(6), t.accent);
                        text(dc, &g.to_string(), &irc, ui.fonts.glyph, WHITE, DT_CENTER);
                    }
                }

                // Accessory (right) then title/subtitle (left, truncated).
                let acc_w = text_width(dc, &it.accessory, ui.fonts.small);
                let acc_rc = RECT { left: rc.right - inner - acc_w, right: rc.right - inner, ..rc };
                text(dc, &it.accessory, &acc_rc, ui.fonts.small, t.subtext, DT_RIGHT);

                let tx = ix + icon + ui.s(12);
                let max_right = acc_rc.left - ui.s(16);
                let title_w = text_width(dc, &it.title, ui.fonts.title).min(max_right - tx);
                let trc = RECT { left: tx, right: tx + title_w, ..rc };
                text(dc, &it.title, &trc, ui.fonts.title, t.text, DT_LEFT | DT_END_ELLIPSIS);
                if !it.subtitle.is_empty() && tx + title_w + ui.s(10) < max_right {
                    let src = RECT { left: tx + title_w + ui.s(10), right: max_right, ..rc };
                    text(dc, &it.subtitle, &src, ui.fonts.small, t.subtext, DT_LEFT | DT_END_ELLIPSIS);
                }
            }
        }
    }
    unsafe {
        SelectObject(icon_dc, icon_old);
        let _ = DeleteDC(icon_dc);
    }

    // Overlay scrollbar.
    let view_h = lr.bottom - lr.top;
    if ui.content_h > view_h {
        let track = view_h - ui.s(8);
        let thumb = (track * view_h / ui.content_h).max(ui.s(24));
        let max_scroll = ui.content_h - view_h;
        let y = lr.top + ui.s(4) + (track - thumb) * ui.scroll / max_scroll.max(1);
        let x = lr.right - ui.s(5);
        round_rect(dc, &RECT { left: x - ui.s(3), top: y, right: x, bottom: y + thumb }, ui.s(3), t.scrollbar);
    }

    unsafe {
        let _ = RestoreDC(dc, -1);
    }
}

fn draw_calc(ui: &Launcher, dc: HDC, row: &RECT, c: &crate::calc::Calc, selected: bool) {
    let dpi = ((ui.s(960) as f32) / 10.0).round() as u32; // effective DPI incl. launcher size
    draw_calc_card(dc, row, c, selected, &ui.theme, &ui.fonts, dpi, ui.pulse());
}

/// The calculator card: expression → answer, with the "copied" pulse
/// (`pulse_t` = raw progress 0 → 1 while running). Independent of the window
/// so it can be rendered off-screen in tests.
#[allow(clippy::too_many_arguments)]
pub fn draw_calc_card(
    dc: HDC, row: &RECT, c: &crate::calc::Calc, selected: bool, t: &Theme, fonts: &Fonts, dpi: u32, pulse_t: Option<f32>,
) {
    let s = |v: i32| (v * dpi as i32 + 48) / 96;
    let card = RECT { left: row.left, top: row.top + s(2), right: row.right, bottom: row.bottom - s(4) };
    let radius = s(12);
    let base = if selected { t.selection } else { mix(t.bg, t.selection, 0.55) };

    // Pulse: ease-out progress 0 → 1.
    let pulse = pulse_t.map(|p| 1.0 - (1.0 - p).powi(3));
    let mut fill_c = base;
    if let Some(e) = pulse {
        // Outer ring expands and fades into the background...
        let grow = s(2) + (e * s(12) as f32) as i32;
        round_rect(dc, &inflate(&card, grow), radius + grow, mix(t.accent, t.bg, e));
        // ...over a tighter glow that settles back onto the card.
        let g2 = s(1) + (e * s(3) as f32) as i32;
        round_rect(dc, &inflate(&card, g2), radius + g2, mix(t.accent, t.bg, 0.35 + 0.65 * e));
        fill_c = mix(t.accent, base, 0.72 + 0.28 * e);
    }
    let border = if selected { mix(t.accent, base, 0.6) } else { t.divider };
    bordered_round_rect(dc, &card, radius, 1, fill_c, border);

    let mid = (card.left + card.right) / 2;
    let cy = (card.top + card.bottom) / 2 - s(9);

    // Divider with an arrow badge.
    fill(dc, &RECT { left: mid, top: card.top + s(16), right: mid + 1, bottom: card.bottom - s(16) }, mix(t.divider, fill_c, 0.2));
    circle(dc, mid, cy, s(30), mix(t.divider, fill_c, 0.1));
    circle(dc, mid, cy, s(28), fill_c);
    let badge = RECT { left: mid - s(14), top: cy - s(14), right: mid + s(14), bottom: cy + s(14) };
    text(dc, "\u{E72A}", &badge, fonts.calc_glyph, t.subtext, DT_CENTER);

    // Expression (left) and answer (right).
    let left = RECT { left: card.left + s(20), right: mid - s(26), top: cy - s(20), bottom: cy + s(20) };
    text(dc, &c.expr, &left, fonts.calc_expr, t.text, DT_CENTER | DT_END_ELLIPSIS);
    let answer_c = pulse.map_or(t.text, |e| mix(t.accent, t.text, e));
    let right = RECT { left: mid + s(26), right: card.right - s(20), top: cy - s(24), bottom: cy + s(24) };
    text(dc, &c.display, &right, fonts.calc_big, answer_c, DT_CENTER | DT_END_ELLIPSIS);

    // Captions.
    let ly = card.bottom - s(32);
    let lcap = RECT { left: card.left + s(16), right: mid - s(16), top: ly, bottom: ly + s(20) };
    text(dc, "Expression", &lcap, fonts.small, t.subtext, DT_CENTER);
    let rcap = RECT { left: mid + s(16), right: card.right - s(16), top: ly, bottom: ly + s(20) };
    text(dc, &c.label, &rcap, fonts.small, t.subtext, DT_CENTER | DT_END_ELLIPSIS);

    // "✓ Copied" chip, fading out at the end of the pulse.
    if let Some(p) = pulse_t {
        let fade = ((p - 0.55) / 0.45).clamp(0.0, 1.0);
        let col = mix(t.accent, fill_c, fade);
        let label = "Copied";
        let tw = text_width(dc, label, fonts.header);
        let gw = s(14);
        let right_x = card.right - s(14);
        let top = card.top + s(8);
        let chip_l = right_x - tw - gw - s(6);
        text(dc, "\u{E73E}", &RECT { left: chip_l, top, right: chip_l + gw, bottom: top + s(18) }, fonts.calc_glyph, col, DT_CENTER);
        text(dc, label, &RECT { left: chip_l + gw + s(6), top, right: right_x, bottom: top + s(18) }, fonts.header, col, DT_LEFT);
    }
}

fn kbd(ui: &Launcher, dc: HDC, right: i32, cy: i32, label: &str, glyph: bool) -> i32 {
    let t = &ui.theme;
    let font = if glyph { ui.fonts.kbd_glyph } else { ui.fonts.header };
    let kh = ui.s(20);
    let kw = (text_width(dc, label, font) + ui.s(10)).max(kh);
    let rc = RECT { left: right - kw, top: cy - kh / 2, right, bottom: cy + kh / 2 };
    round_rect(dc, &rc, ui.s(5), t.kbd_bg);
    text(dc, label, &rc, font, t.kbd_text, DT_CENTER);
    rc.left
}

fn draw_footer(ui: &Launcher, dc: HDC, w: i32, h: i32) {
    let t = &ui.theme;
    let footer_h = ui.s(m::FOOTER_H);
    let top = h - footer_h;
    let cy = top + footer_h / 2;
    let x0 = ui.s(14);

    // Left: logo + name, or a status message.
    let mut x = x0;
    if let Some(logo) = ui.logo {
        let sz = ui.s(16);
        unsafe {
            let _ = DrawIconEx(dc, x, cy - sz / 2, logo, sz, sz, 0, None, DI_NORMAL);
        }
        x += sz + ui.s(8);
    }
    let left_rc = RECT { left: x, top, right: w / 2, bottom: h };
    match ui.status() {
        Some((msg, warn)) => {
            text(dc, msg, &left_rc, ui.fonts.small, if warn { t.warn } else { t.subtext }, DT_LEFT | DT_END_ELLIPSIS)
        }
        None if ui.mode == super::window::Mode::Clipboard => {
            text(dc, "Delete: Shift+Del · Back: Esc", &left_rc, ui.fonts.small, t.subtext, DT_LEFT | DT_END_ELLIPSIS)
        }
        None if ui.mode != super::window::Mode::Root => {
            text(dc, "Back: Esc", &left_rc, ui.fonts.small, t.subtext, DT_LEFT | DT_END_ELLIPSIS)
        }
        None => text(dc, "SIGMA5", &left_rc, ui.fonts.header, t.subtext, DT_LEFT),
    }

    // Right: primary action + Enter, then an optional Shift+Enter action.
    if let Some(label) = ui.action_verb() {
        let right = w - ui.s(12);
        // U+E751 = "ReturnKey" glyph in Segoe Fluent Icons / MDL2.
        let kx = kbd(ui, dc, right, cy, "\u{E751}", true);
        let lw = text_width(dc, label, ui.fonts.small);
        let lx = kx - ui.s(8) - lw;
        text(dc, label, &RECT { left: lx, top, right: kx - ui.s(8), bottom: h }, ui.fonts.small, t.text, DT_RIGHT);
        if let Some(sec) = ui.secondary_hint() {
            let sep = lx - ui.s(14);
            fill(dc, &RECT { left: sep, top: cy - ui.s(8), right: sep + 1, bottom: cy + ui.s(8) }, t.divider);
            let k2 = kbd(ui, dc, sep - ui.s(14), cy, "\u{E751}", true);
            let k1 = kbd(ui, dc, k2 - ui.s(4), cy, "Shift", false);
            let sw = text_width(dc, sec, ui.fonts.small);
            text(dc, sec, &RECT { left: k1 - ui.s(8) - sw, top, right: k1 - ui.s(8), bottom: h }, ui.fonts.small, t.subtext, DT_RIGHT);
        }
    }
}

#[cfg(test)]
mod clip_render {
    use super::*;
    use crate::clipboard::{Content, Entry};

    /// Off-screen render of the clipboard preview (text + image) to a BMP.
    /// `cargo test --release render_clip -- --ignored`
    #[test]
    #[ignore]
    fn render_clip_preview() {
        let out = std::env::var("SIGMA5_RENDER_OUT").unwrap_or_else(|_| "clip.bmp".into());
        let dpi = 144u32;
        let s = |v: i32| (v * dpi as i32 + 48) / 96;
        let (pw, ph) = (s(440), s(372));
        // A 320×180 gradient image as a packed 32-bit DIB.
        let (iw, ih) = (320i32, 180i32);
        let mut dib = Vec::new();
        for v in [40u32, iw as u32, ih as u32] {
            dib.extend_from_slice(&v.to_le_bytes());
        }
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&[0u8; 24]);
        for y in 0..ih {
            for x in 0..iw {
                dib.extend_from_slice(&[(x * 255 / iw) as u8, (y * 255 / ih) as u8, 200, 255]);
            }
        }
        let entries = [
            Entry {
                id: 1,
                content: Content::Text("fn main() {\n    println!(\"Hello, SIGMA5!\");\n}\n\nA longer paragraph to check that word wrapping behaves nicely inside the preview pane on the right.".into()),
                copied: crate::util::now_unix() - 300,
                source: String::new(),
            },
            Entry { id: 2, content: Content::Image { width: iw, height: ih, dib, hash: 1 }, copied: crate::util::now_unix() - 7200, source: String::new() },
        ];
        let total_w = pw * 2;
        unsafe {
            let dc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: total_w, biHeight: -ph,
                    biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bmp = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
            SelectObject(dc, bmp.into());
            SetBkMode(dc, TRANSPARENT);
            let mut fonts = Fonts::create(dpi);
            let t = crate::ui::theme::DARK;
            fill(dc, &RECT { left: 0, top: 0, right: total_w, bottom: ph }, t.bg);
            for (i, (e, src)) in entries.iter().zip(["Visual Studio Code", "Snipping Tool"]).enumerate() {
                let pr = RECT { left: i as i32 * pw + 1, top: 0, right: (i as i32 + 1) * pw, bottom: ph };
                draw_clip_preview_into(dc, pr, Some(e), src, &t, &fonts, dpi);
            }
            fonts.destroy();
            let px = std::slice::from_raw_parts(bits as *const u8, (total_w * ph * 4) as usize);
            let mut f = Vec::new();
            f.extend_from_slice(b"BM");
            f.extend_from_slice(&((54 + px.len()) as u32).to_le_bytes());
            f.extend_from_slice(&[0u8; 4]);
            f.extend_from_slice(&54u32.to_le_bytes());
            f.extend_from_slice(&40u32.to_le_bytes());
            f.extend_from_slice(&total_w.to_le_bytes());
            f.extend_from_slice(&(-ph).to_le_bytes());
            f.extend_from_slice(&1u16.to_le_bytes());
            f.extend_from_slice(&32u16.to_le_bytes());
            f.extend_from_slice(&[0u8; 24]);
            f.extend_from_slice(px);
            std::fs::write(&out, f).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::{DARK, LIGHT};

    /// Renders the calculator card at several pulse stages into a BMP, entirely
    /// off-screen. `cargo test --release render_calc -- --ignored`
    #[test]
    #[ignore]
    fn render_calc_card_frames() {
        let out = std::env::var("SIGMA5_RENDER_OUT").unwrap_or_else(|_| "calc_frames.bmp".into());
        let dpi = 144u32;
        let s = |v: i32| (v * dpi as i32 + 48) / 96;
        let (w, row_h, gap) = (s(734), s(m::CALC_H), s(26));
        let frames: [(Option<f32>, bool, &Theme); 6] = [
            (None, true, &DARK), (Some(0.06), true, &DARK), (Some(0.22), true, &DARK),
            (Some(0.5), true, &DARK), (Some(0.85), true, &DARK), (None, false, &LIGHT),
        ];
        let h = frames.len() as i32 * (row_h + gap) + gap;
        let calc = crate::calc::evaluate("3 kN * 0.5 m", false).unwrap();
        unsafe {
            let dc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h,
                    biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bmp = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
            SelectObject(dc, bmp.into());
            SetBkMode(dc, TRANSPARENT);
            let mut fonts = Fonts::create(dpi);
            for (i, (pulse, selected, theme)) in frames.iter().enumerate() {
                let top = gap + i as i32 * (row_h + gap);
                fill(dc, &RECT { left: 0, top: top - gap, right: w, bottom: top + row_h }, theme.bg);
                let mut t = **theme;
                t.accent = crate::ui::theme::rgb(171, 114, 255);
                let row = RECT { left: s(16), top, right: w - s(16), bottom: top + row_h };
                draw_calc_card(dc, &row, &calc, *selected, &t, &fonts, dpi, *pulse);
            }
            fonts.destroy();
            // BMP: file header + info header + top-down 32-bit pixels.
            let px = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
            let mut f = Vec::new();
            f.extend_from_slice(b"BM");
            f.extend_from_slice(&((54 + px.len()) as u32).to_le_bytes());
            f.extend_from_slice(&[0u8; 4]);
            f.extend_from_slice(&54u32.to_le_bytes());
            f.extend_from_slice(&40u32.to_le_bytes());
            f.extend_from_slice(&w.to_le_bytes());
            f.extend_from_slice(&(-h).to_le_bytes());
            f.extend_from_slice(&1u16.to_le_bytes());
            f.extend_from_slice(&32u16.to_le_bytes());
            f.extend_from_slice(&[0u8; 24]);
            f.extend_from_slice(px);
            std::fs::write(&out, f).unwrap();
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(dc);
        }
    }
}
