//! Launcher rendering. Everything is drawn with GDI into a back buffer and
//! blitted in one go, so there is no flicker and no GPU/driver overhead.

use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{DI_NORMAL, DrawIconEx};

use super::draw::{
    self, DISPLAY_FACES, ICON_FACES, UI_FACES, bordered_round_rect, circle, fill, inflate, mix, round_rect, text,
    text_width,
};
use super::theme::{Theme, WHITE, metrics as m, rgb};
use super::window::{ArtBitmap, Launcher, Mode, Zone};
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
    /// Typing Test words.
    pub typing: HFONT,
}

impl Fonts {
    /// `dpi` is the effective DPI (monitor DPI × launcher size); fonts and
    /// text size follow the Appearance settings.
    pub fn create(dpi: u32, look: &crate::config::Appearance) -> Self {
        let scale = look.sane().font_scale as i32;
        let s = |v: i32| (v * dpi as i32 + 48) / 96;
        let t = |v: i32| s(v * scale / 100);
        let ui = draw::faces(&look.font, UI_FACES);
        let display = draw::faces(if look.search_font.trim().is_empty() { &look.font } else { &look.search_font }, DISPLAY_FACES);
        Self {
            search: draw::font(t(19), 400, &display),
            title: draw::font(t(14), 400, &ui),
            small: draw::font(t(13), 400, &ui),
            header: draw::font(t(12), 600, &ui),
            glyph: draw::font(s(15), 400, ICON_FACES),
            kbd_glyph: draw::font(s(11), 400, ICON_FACES),
            letter: draw::font(t(13), 600, &ui),
            calc_big: draw::font(t(30), 600, &display),
            calc_expr: draw::font(t(19), 400, &display),
            calc_glyph: draw::font(s(12), 400, ICON_FACES),
            mono: draw::font(t(13), 400, &["Cascadia Mono", "Consolas"]),
            typing: draw::font(t(22), 400, &["Cascadia Mono", "Consolas"]),
        }
    }

    pub fn destroy(&mut self) {
        draw::delete_fonts(&[
            self.search, self.title, self.small, self.header, self.glyph, self.kbd_glyph, self.letter,
            self.calc_big, self.calc_expr, self.calc_glyph, self.mono, self.typing,
        ]);
    }
}

pub fn paint(ui: &mut Launcher) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = unsafe { BeginPaint(ui.hwnd, &mut ps) };
    let (w, h) = ui.client_size();
    let dirty = ps.rcPaint;
    if let Some(dc) = ui.backbuf.current(w, h).filter(|_| ui.top_h() > 0 && dirty.bottom <= ui.top_h()) {
        // Only the top bar changed (clock, readings, the media waveform):
        // redraw just that strip over the last frame.
        unsafe {
            SaveDC(dc);
            IntersectClipRect(dc, dirty.left, dirty.top, dirty.right, dirty.bottom);
        }
        draw::set_corner_cap(ui.s(ui.theme.radius));
        ui.zones.borrow_mut().retain(|(_, z)| *z != Zone::MediaChip);
        draw_top_bar(ui, dc, w);
        draw_frame(dc, w, h, ui.s(ui.theme.frame_w).max(ui.theme.frame_w.min(1)), ui.theme.frame);
        unsafe {
            let _ = RestoreDC(dc, -1);
            let (dw, dh) = (dirty.right - dirty.left, dirty.bottom - dirty.top);
            let _ = BitBlt(hdc, dirty.left, dirty.top, dw, dh, Some(dc), dirty.left, dirty.top, SRCCOPY);
        }
    } else if w > 0 && h > 0 {
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

pub(crate) fn draw(ui: &Launcher, dc: HDC, w: i32, h: i32) {
    let t: &Theme = &ui.theme;
    draw::set_corner_cap(ui.s(t.radius));
    ui.zones.borrow_mut().clear();
    fill(dc, &RECT { left: 0, top: 0, right: w, bottom: h }, t.bg);

    let search_h = ui.s(m::SEARCH_H);
    let footer_h = ui.s(m::FOOTER_H);
    let bar_top = ui.top_h();
    fill(dc, &RECT { left: 0, top: bar_top + search_h, right: w, bottom: bar_top + search_h + 1 }, t.divider);
    fill(dc, &RECT { left: 0, top: h - footer_h - 1, right: w, bottom: h - footer_h }, t.divider);

    draw_top_bar(ui, dc, w);

    // Sub-pages: back arrow (left) and page title chip (right) in the search bar.
    if let Some(back) = ui.back_rect() {
        text(dc, "\u{E72B}", &back, ui.fonts.glyph, t.subtext, DT_CENTER);
        let title = ui.mode.title();
        let tw = text_width(dc, title, ui.fonts.header);
        let ch = ui.s(24);
        let cy = bar_top + search_h / 2;
        let chip = RECT { left: w - ui.s(18) - tw - ui.s(20), top: cy - ch / 2, right: w - ui.s(18), bottom: cy - ch / 2 + ch };
        round_rect(dc, &chip, ui.s(6), t.kbd_bg);
        text(dc, title, &chip, ui.fonts.header, t.kbd_text, DT_CENTER);
    }

    match ui.mode {
        Mode::Media => draw_media_page(ui, dc),
        Mode::Typing => draw_typing_page(ui, dc),
        _ => draw_list(ui, dc),
    }
    if ui.mode == super::window::Mode::Clipboard {
        draw_clip_preview(ui, dc);
    }
    draw_footer(ui, dc, w, h);
    draw_frame(dc, w, h, ui.s(t.frame_w).max(t.frame_w.min(1)), t.frame);
}

/// The coloured border round the whole window.
pub fn draw_frame(dc: HDC, w: i32, h: i32, px: i32, c: windows::Win32::Foundation::COLORREF) {
    if px <= 0 {
        return;
    }
    fill(dc, &RECT { left: 0, top: 0, right: w, bottom: px }, c);
    fill(dc, &RECT { left: 0, top: h - px, right: w, bottom: h }, c);
    fill(dc, &RECT { left: 0, top: 0, right: px, bottom: h }, c);
    fill(dc, &RECT { left: w - px, top: 0, right: w, bottom: h }, c);
}

/// One top bar component: (glyph, label, value, meter fraction).
fn top_item(ui: &Launcher, id: &str, tb: &crate::config::TopBar) -> Option<(char, &'static str, String, Option<f32>)> {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let info = crate::sysinfo::item(id)?;
    let si = &ui.sysinfo;
    let dash = || "—".to_string();
    let pct = |v: Option<f32>| v.map_or_else(dash, |p| format!("{p:.0}%"));
    let deg = |v: Option<f32>| v.map_or_else(dash, |c| format!("{c:.0}°C"));
    Some(match id {
        "clock" | "date" => {
            let t = unsafe { GetLocalTime() };
            if id == "date" {
                const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
                const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
                let d = format!("{} {} {}", DAYS[t.wDayOfWeek as usize % 7], t.wDay, MONTHS[(t.wMonth as usize).clamp(1, 12) - 1]);
                (info.glyph, "", d, None)
            } else {
                let secs = if tb.show_seconds { format!(":{:02}", t.wSecond) } else { String::new() };
                let s = if tb.clock_24h {
                    format!("{:02}:{:02}{secs}", t.wHour, t.wMinute)
                } else {
                    let h = match t.wHour % 12 { 0 => 12, h => h };
                    format!("{h}:{:02}{secs} {}", t.wMinute, if t.wHour < 12 { "AM" } else { "PM" })
                };
                (info.glyph, "", s, None)
            }
        }
        "cpu" => (info.glyph, "CPU", pct(si.cpu), si.cpu.map(|v| v / 100.0)),
        "gpu" => (info.glyph, "GPU", pct(si.gpu), si.gpu.map(|v| v / 100.0)),
        "ram" => (
            info.glyph,
            "RAM",
            si.ram.map_or_else(dash, |(u, t, _)| format!("{u:.1}/{t:.0} GB")),
            si.ram.map(|(_, _, p)| p / 100.0),
        ),
        "cpu_temp" => (info.glyph, "CPU", deg(si.cpu_temp), None),
        "gpu_temp" => (info.glyph, "GPU", deg(si.gpu_temp), None),
        "battery" => {
            let glyph = if si.battery.is_some_and(|(_, c)| c) { '\u{EBB5}' } else { info.glyph };
            (glyph, "", si.battery.map_or_else(dash, |(p, _)| format!("{p}%")), si.battery.map(|(p, _)| p as f32 / 100.0))
        }
        "net" => (
            info.glyph,
            "",
            si.net.map_or_else(dash, |(d, u)| format!("↓{}  ↑{}", crate::sysinfo::rate(d), crate::sysinfo::rate(u))),
            None,
        ),
        "disk" => (info.glyph, "", si.disk.map_or_else(dash, |(f, _)| format!("{f:.0} GB free")), si.disk.map(|(f, t)| 1.0 - f / t.max(1.0))),
        _ => return None,
    })
}

/// The bar above the search box: components left to right.
fn draw_top_bar(ui: &Launcher, dc: HDC, w: i32) {
    let Some(tb) = ui.top_bar() else { return };
    let t = &ui.theme;
    let h = ui.top_h();
    let bar = RECT { left: 0, top: 0, right: w, bottom: h };
    fill(dc, &bar, mix(t.bg, t.selection, 0.45));
    fill(dc, &RECT { left: 0, top: h - 1, right: w, bottom: h }, t.divider);
    let mut x = ui.s(14);
    let gap = ui.s(18);
    for id in &tb.items {
        if x >= w - ui.s(14) {
            break;
        }
        if id == "media" {
            if ui.media_in_top_bar() {
                x = draw_media_chip(ui, dc, x, h, w - ui.s(14)) + gap;
            }
            continue;
        }
        let Some((glyph, label, value, meter)) = top_item(ui, id, tb) else { continue };
        let rc = RECT { left: x, top: 0, right: w - ui.s(10), bottom: h };
        let gw = ui.s(14);
        text(dc, &glyph.to_string(), &RECT { right: x + gw, ..rc }, ui.fonts.kbd_glyph, t.subtext, DT_CENTER);
        x += gw + ui.s(6);
        if !label.is_empty() {
            text(dc, label, &RECT { left: x, ..rc }, ui.fonts.header, t.subtext, DT_LEFT);
            x += text_width(dc, label, ui.fonts.header) + ui.s(5);
        }
        text(dc, &value, &RECT { left: x, ..rc }, ui.fonts.small, t.text, DT_LEFT);
        x += text_width(dc, &value, ui.fonts.small);
        if let Some(f) = meter {
            // A small square-ended meter.
            let (mw, mh) = (ui.s(26), ui.s(4));
            let my = (h - mh) / 2;
            let track = RECT { left: x + ui.s(7), top: my, right: x + ui.s(7) + mw, bottom: my + mh };
            fill(dc, &track, t.divider);
            let fw = ((mw as f32) * f.clamp(0.0, 1.0)).round() as i32;
            let col = if f > 0.9 { t.warn } else { t.accent };
            fill(dc, &RECT { right: track.left + fw, ..track }, col);
            x = track.right;
        }
        x += gap;
    }
}

// ----- Now Playing ------------------------------------------------------------

/// Album art scaled into `rc`, or a music-note tile without it.
fn draw_art(ui: &Launcher, dc: HDC, rc: &RECT, art: Option<&ArtBitmap>, glyph_font: HFONT) {
    let t = &ui.theme;
    match art {
        Some(a) => unsafe {
            let mem = CreateCompatibleDC(Some(dc));
            let old = SelectObject(mem, a.bmp.into());
            SetStretchBltMode(dc, HALFTONE);
            let _ = SetBrushOrgEx(dc, 0, 0, None);
            let n = a.src.size;
            let _ = StretchBlt(dc, rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top, Some(mem), 0, 0, n, n, SRCCOPY);
            SelectObject(mem, old);
            let _ = DeleteDC(mem);
        },
        None => {
            round_rect(dc, rc, ui.s(4), mix(t.bg, t.text, 0.14));
            text(dc, "\u{E8D6}", rc, glyph_font, t.subtext, DT_CENTER);
        }
    }
}

/// Vertical bars of the recent output level, mirrored round the middle of
/// `rc`, newest on the right. Paused or silent, they rest as short stubs.
fn draw_waveform(ui: &Launcher, dc: HDC, rc: &RECT, bar: i32, gap: i32, playing: bool) {
    let t = &ui.theme;
    let n = ((rc.right - rc.left + gap) / (bar + gap)).max(1) as usize;
    let levels = ui.levels.last(n);
    let h = rc.bottom - rc.top;
    let cy = (rc.top + rc.bottom) / 2;
    let rest = bar.max(ui.s(2));
    // A fixed ripple so neighbouring bars differ, like a real waveform.
    const SHAPE: [f32; 7] = [0.72, 1.0, 0.84, 0.6, 0.93, 0.78, 0.66];
    for (i, v) in levels.iter().enumerate() {
        let x = rc.left + i as i32 * (bar + gap);
        let amp = if playing { v * SHAPE[i % SHAPE.len()] } else { 0.0 };
        let bh = ((h as f32 * amp) as i32).max(rest);
        let fade = i as f32 / n as f32; // older bars are dimmer
        let col = if playing { mix(t.bg, t.accent, 0.45 + 0.55 * fade) } else { mix(t.bg, t.subtext, 0.6) };
        round_rect(dc, &RECT { left: x, top: cy - bh / 2, right: x + bar, bottom: cy - bh / 2 + bh }, bar, col);
    }
}

/// The Now Playing chip in the top bar. Returns its right edge.
fn draw_media_chip(ui: &Launcher, dc: HDC, x0: i32, h: i32, max_right: i32) -> i32 {
    let t = &ui.theme;
    let mut x = x0;
    let Some(np) = &ui.media else {
        let gw = ui.s(14);
        text(dc, "\u{E8D6}", &RECT { left: x, top: 0, right: x + gw, bottom: h }, ui.fonts.kbd_glyph, t.subtext, DT_CENTER);
        x += gw + ui.s(6);
        let label = "Nothing playing";
        let tw = text_width(dc, label, ui.fonts.small);
        text(dc, label, &RECT { left: x, top: 0, right: max_right, bottom: h }, ui.fonts.small, t.subtext, DT_LEFT);
        let right = (x + tw).min(max_right);
        ui.zones.borrow_mut().push((RECT { left: x0 - ui.s(6), top: 0, right: right + ui.s(6), bottom: h }, Zone::MediaChip));
        return right;
    };
    let a = ui.s(18);
    let art = RECT { left: x, top: (h - a) / 2, right: x + a, bottom: (h - a) / 2 + a };
    draw_art(ui, dc, &art, ui.art.as_ref(), ui.fonts.kbd_glyph);
    x = art.right + ui.s(8);
    let (bar, gap) = (ui.s(2), ui.s(2));
    let wave = RECT { left: x, top: (h - ui.s(14)) / 2, right: x + 9 * (bar + gap) - gap, bottom: (h - ui.s(14)) / 2 + ui.s(14) };
    draw_waveform(ui, dc, &wave, bar, gap, np.playing);
    x = wave.right + ui.s(8);
    let line = np.line();
    let tw = text_width(dc, &line, ui.fonts.small).min(ui.s(210)).min(max_right - x);
    text(dc, &line, &RECT { left: x, top: 0, right: x + tw, bottom: h }, ui.fonts.small, if np.playing { t.text } else { t.subtext }, DT_LEFT | DT_END_ELLIPSIS);
    let right = x + tw;
    ui.zones.borrow_mut().push((RECT { left: x0 - ui.s(6), top: 0, right: right + ui.s(6), bottom: h }, Zone::MediaChip));
    right
}

/// A round transport button; `primary` is the filled play/pause one.
fn media_button(ui: &Launcher, dc: HDC, cx: i32, cy: i32, d: i32, glyph: &str, primary: bool, zone: Zone) -> i32 {
    let t = &ui.theme;
    let rc = RECT { left: cx - d / 2, top: cy - d / 2, right: cx - d / 2 + d, bottom: cy - d / 2 + d };
    if primary {
        circle(dc, cx, cy, d, t.accent);
        text(dc, glyph, &rc, ui.fonts.glyph, WHITE, DT_CENTER);
    } else {
        circle(dc, cx, cy, d, t.kbd_bg);
        text(dc, glyph, &rc, ui.fonts.kbd_glyph, t.text, DT_CENTER);
    }
    ui.zones.borrow_mut().push((rc, zone));
    rc.right
}

/// The Now Playing page: art, track, waveform, progress and controls.
fn draw_media_page(ui: &Launcher, dc: HDC) {
    let t = &ui.theme;
    let lr = ui.list_rect();
    let card = RECT { left: lr.left + ui.s(16), top: lr.top + ui.s(16), right: lr.right - ui.s(16), bottom: lr.bottom - ui.s(16) };
    let Some(np) = &ui.media else {
        let mid = (lr.top + lr.bottom) / 2;
        text(dc, "\u{E8D6}", &RECT { top: mid - ui.s(56), bottom: mid - ui.s(16), ..lr }, ui.fonts.calc_glyph, t.subtext, DT_CENTER);
        let msg = if ui.config_module_on("media") { "Nothing is playing" } else { "Now Playing is turned off in Settings › Modules" };
        text(dc, msg, &RECT { top: mid - ui.s(12), bottom: mid + ui.s(14), ..lr }, ui.fonts.title, t.text, DT_CENTER);
        let sub = "Play something in Spotify, a browser, or any app that shows in Windows' media controls";
        text(dc, sub, &RECT { left: lr.left + ui.s(24), right: lr.right - ui.s(24), top: mid + ui.s(14), bottom: mid + ui.s(38), ..lr }, ui.fonts.small, t.subtext, DT_CENTER | DT_END_ELLIPSIS);
        return;
    };
    let tint = ui.art.as_ref().map_or(t.selection, |a| a.avg);
    let fill_c = mix(t.bg, tint, if t.dark { 0.22 } else { 0.14 });
    draw::bordered_round_rect(dc, &card, ui.s(10), 1, fill_c, mix(fill_c, t.text, 0.08));

    let pad = ui.s(22);
    let art_sz = (card.bottom - card.top - 2 * pad).min(ui.s(176));
    let at = card.top + (card.bottom - card.top - art_sz) / 2;
    let art = RECT { left: card.left + pad, top: at, right: card.left + pad + art_sz, bottom: at + art_sz };
    draw_art(ui, dc, &art, ui.art.as_ref(), ui.fonts.calc_glyph);

    let tx = art.right + ui.s(26);
    let tr = card.right - pad;
    let row = |top: i32, hgt: i32| RECT { left: tx, top: art.top + top, right: tr, bottom: art.top + top + hgt };
    let title = if np.title.is_empty() { "Unknown track" } else { &np.title };
    text(dc, title, &row(0, ui.s(30)), ui.fonts.calc_expr, t.text, DT_LEFT | DT_END_ELLIPSIS);
    let by = match (np.artist.is_empty(), np.album.is_empty()) {
        (false, false) => format!("{} · {}", np.artist, np.album),
        (false, true) => np.artist.clone(),
        (true, false) => np.album.clone(),
        _ => String::new(),
    };
    text(dc, &by, &row(ui.s(31), ui.s(22)), ui.fonts.title, t.subtext, DT_LEFT | DT_END_ELLIPSIS);
    let app = ui.media_app_name();
    let state = if np.playing { format!("Playing on {app}") } else { format!("Paused · {app}") };
    text(dc, &state, &row(ui.s(54), ui.s(20)), ui.fonts.small, mix(t.subtext, fill_c, 0.2), DT_LEFT | DT_END_ELLIPSIS);

    // Waveform across the text column.
    draw_waveform(ui, dc, &row(ui.s(82), ui.s(24)), ui.s(3), ui.s(3), np.playing);

    // Progress: elapsed · bar · length (click to seek).
    let py = art.top + ui.s(120);
    if let (Some(pos), Some(len)) = (np.position_now(), np.duration_ms) {
        let tw = ui.s(46);
        text(dc, &crate::media::clock(pos), &RECT { left: tx, top: py - ui.s(10), right: tx + tw, bottom: py + ui.s(10) }, ui.fonts.small, t.subtext, DT_LEFT);
        text(dc, &crate::media::clock(len), &RECT { left: tr - tw, top: py - ui.s(10), right: tr, bottom: py + ui.s(10) }, ui.fonts.small, t.subtext, DT_RIGHT);
        let bar = RECT { left: tx + tw, top: py - ui.s(2), right: tr - tw, bottom: py + ui.s(2) };
        round_rect(dc, &bar, ui.s(2), mix(fill_c, t.text, 0.16));
        let f = if len > 0 { pos as f32 / len as f32 } else { 0.0 };
        let fx = bar.left + ((bar.right - bar.left) as f32 * f.clamp(0.0, 1.0)) as i32;
        round_rect(dc, &RECT { right: fx.max(bar.left + ui.s(2)), ..bar }, ui.s(2), t.accent);
        circle(dc, fx, py, ui.s(10), t.accent);
        ui.zones.borrow_mut().push((RECT { top: py - ui.s(9), bottom: py + ui.s(9), ..bar }, Zone::MediaSeek));
    }

    // Controls.
    let cy = art.bottom - ui.s(19);
    let mut x = tx + ui.s(16);
    x = media_button(ui, dc, x, cy, ui.s(32), "\u{E892}", false, Zone::MediaPrev);
    let play = if np.playing { "\u{E769}" } else { "\u{E768}" };
    x = media_button(ui, dc, x + ui.s(12) + ui.s(19), cy, ui.s(38), play, true, Zone::MediaPlay);
    media_button(ui, dc, x + ui.s(12) + ui.s(16), cy, ui.s(32), "\u{E893}", false, Zone::MediaNext);
    if !app.is_empty() {
        let label = format!("Open {app}");
        let bw = text_width(dc, &label, ui.fonts.small) + ui.s(24);
        let b = RECT { left: tr - bw, top: cy - ui.s(15), right: tr, bottom: cy + ui.s(15) };
        round_rect(dc, &b, ui.s(6), t.kbd_bg);
        text(dc, &label, &b, ui.fonts.small, t.text, DT_CENTER | DT_END_ELLIPSIS);
        ui.zones.borrow_mut().push((b, Zone::MediaApp));
    }
}

// ----- Typing Test --------------------------------------------------------------

fn typing_error_color(t: &Theme) -> COLORREF {
    if t.dark { rgb(242, 104, 104) } else { rgb(200, 40, 40) }
}

fn kind_label(k: crate::typing::Kind) -> String {
    match k {
        crate::typing::Kind::Time(s) => format!("{s} seconds"),
        crate::typing::Kind::Words(n) => format!("{n} words"),
    }
}

/// The Typing Test page: just the words (with a length menu above them
/// until you start), then a short result.
fn draw_typing_page(ui: &Launcher, dc: HDC) {
    use crate::typing::Kind;
    let t = &ui.theme;
    let lr = ui.list_rect();
    let Some(test) = &ui.typing else { return };
    if let Some(score) = test.finished {
        draw_typing_results(ui, dc, &lr, &score);
        return;
    }
    let left = lr.left + ui.s(40);
    let right = lr.right - ui.s(40);
    let block = 3 * ui.s(42);
    let top = lr.top + (lr.bottom - lr.top - block) / 2;
    let hy = top - ui.s(46);
    if test.running() {
        // Time left (or words done), quietly above the words.
        let s = match test.kind {
            Kind::Time(_) => test.remaining().unwrap_or(0).to_string(),
            Kind::Words(n) => format!("{}/{n}", test.current),
        };
        text(dc, &s, &RECT { left, top: hy, right, bottom: hy + ui.s(32) }, ui.fonts.calc_expr, t.accent, DT_LEFT);
    } else {
        // The length menu.
        let label = kind_label(test.kind);
        let lw = text_width(dc, &label, ui.fonts.title);
        let pill = RECT { left: left - ui.s(10), top: hy, right: left + lw + ui.s(32), bottom: hy + ui.s(30) };
        round_rect(dc, &pill, ui.s(6), t.kbd_bg);
        text(dc, &label, &RECT { left: pill.left + ui.s(10), ..pill }, ui.fonts.title, t.text, DT_LEFT);
        text(dc, "\u{E70D}", &RECT { left: pill.right - ui.s(24), right: pill.right - ui.s(6), ..pill }, ui.fonts.kbd_glyph, t.subtext, DT_CENTER);
        ui.zones.borrow_mut().push((pill, Zone::TypingMenu));
        if let Some(b) = ui.typing_store.as_ref().and_then(|st| st.best(test.kind)) {
            text(dc, &format!("Best {:.0} wpm", b.wpm), &RECT { left: pill.right, top: hy, right, bottom: hy + ui.s(30) }, ui.fonts.small, t.subtext, DT_RIGHT);
        }
    }
    draw_typing_words(ui, dc, test, top, left, right);
    // Before starting: does this run feed the Solar System?
    if let (false, Some((wait, at))) = (test.running(), ui.harvest_wait) {
        let wait = wait.saturating_sub(at.elapsed().as_secs());
        let (msg, col) = if wait == 0 {
            ("This run feeds your solar system: every word typed right becomes material".to_string(), t.accent)
        } else {
            (format!("Practice run · the next one that feeds your solar system is in {}", super::solar::wait_label(wait)), t.subtext)
        };
        let y = top + block + ui.s(16);
        text(dc, &msg, &RECT { left, top: y, right, bottom: y + ui.s(22) }, ui.fonts.small, col, DT_LEFT | DT_END_ELLIPSIS);
    }
}

/// Three lines of words; the line being typed is kept first or second.
fn draw_typing_words(ui: &Launcher, dc: HDC, test: &crate::typing::Test, top: i32, left: i32, right: i32) {
    let t = &ui.theme;
    let font = ui.fonts.typing;
    let cw = text_width(dc, "m", font).max(1);
    let line_h = ui.s(42);
    let cap = ((right - left) / cw).max(10) as usize;
    let err = typing_error_color(t);

    // Lay out words into lines: (first word, end) by display length.
    let shown_len = |i: usize| test.words[i].chars().count().max(test.typed.get(i).map_or(0, |s| s.chars().count()));
    let mut lines: Vec<(usize, usize)> = Vec::new();
    let (mut start, mut len) = (0usize, 0usize);
    let limit = test.words.len().min(test.current + 80);
    for i in 0..limit {
        let l = shown_len(i);
        if len > 0 && len + 1 + l > cap {
            lines.push((start, i));
            start = i;
            len = 0;
        }
        len += if len > 0 { 1 } else { 0 } + l;
    }
    lines.push((start, limit));
    let cur_line = lines.iter().position(|&(a, b)| test.current >= a && test.current < b).unwrap_or(0);
    let first = cur_line.saturating_sub(1);

    let mut placed = ui.typing_words.borrow_mut();
    placed.clear();
    for (li, &(a, b)) in lines.iter().enumerate().skip(first).take(3) {
        let ly = top + (li - first) as i32 * line_h;
        let mut x = left;
        for i in a..b {
            placed.push((i, x, ly, line_h));
            let word: Vec<char> = test.words[i].chars().collect();
            let typed: Vec<char> = test.typed.get(i).map(|s| s.chars().collect()).unwrap_or_default();
            let past = i < test.current;
            let word_x = x;
            for j in 0..word.len().max(typed.len()) {
                let (c, col) = match (word.get(j), typed.get(j)) {
                    (Some(w), Some(ty)) if w == ty => (*w, t.text),
                    (Some(w), Some(_)) => (*w, err),
                    (Some(w), None) => (*w, if past { mix(err, t.bg, 0.45) } else { t.subtext }),
                    (None, Some(ty)) => (*ty, mix(err, t.bg, 0.3)),
                    (None, None) => break,
                };
                let rc = RECT { left: x, top: ly, right: x + cw * 2, bottom: ly + line_h };
                text(dc, &c.to_string(), &rc, font, col, DT_LEFT);
                x += cw;
            }
            if past && typed != word {
                let uy = ly + line_h - ui.s(9);
                fill(dc, &RECT { left: word_x, top: uy, right: x, bottom: uy + ui.s(2) }, mix(err, t.bg, 0.25));
            }
            if i == test.current {
                let cx = word_x + typed.len() as i32 * cw - ui.s(1);
                let ch = line_h * 3 / 5;
                fill(dc, &RECT { left: cx, top: ly + (line_h - ch) / 2, right: cx + ui.s(2), bottom: ly + (line_h + ch) / 2 }, t.accent);
            }
            x += cw; // the space
        }
    }
}

/// The result, centred: speed, accuracy, best, and what was harvested.
fn draw_typing_results(ui: &Launcher, dc: HDC, lr: &RECT, s: &crate::typing::Score) {
    let t = &ui.theme;
    let cx = (lr.left + lr.right) / 2;
    let row = |y: i32, h: i32| RECT { left: lr.left + ui.s(24), top: y, right: lr.right - ui.s(24), bottom: y + h };
    let top = (lr.top + lr.bottom) / 2 - ui.s(96);

    // "74 wpm"
    let num = format!("{:.0}", s.wpm);
    let nw = text_width(dc, &num, ui.fonts.calc_big);
    let unit = " wpm";
    let uw = text_width(dc, unit, ui.fonts.title);
    let x0 = cx - (nw + uw) / 2;
    text(dc, &num, &RECT { left: x0, top, right: x0 + nw + 2, bottom: top + ui.s(52) }, ui.fonts.calc_big, t.accent, DT_LEFT);
    text(dc, unit, &RECT { left: x0 + nw, top: top + ui.s(10), right: x0 + nw + uw + 2, bottom: top + ui.s(52) }, ui.fonts.title, t.subtext, DT_LEFT);

    let line = format!("{:.0}% accuracy · {:.0} raw · {:.0} s", s.acc, s.raw, s.secs);
    text(dc, &line, &row(top + ui.s(58), ui.s(24)), ui.fonts.title, t.text, DT_CENTER);

    let best = ui.typing.as_ref().and_then(|tt| ui.typing_store.as_ref().and_then(|st| st.best(tt.kind)));
    let (msg, col) = if ui.typing_new_best {
        ("New personal best!".to_string(), t.accent)
    } else if s.acc < 75.0 {
        ("Under 75% accuracy, so it doesn't count towards your best".to_string(), t.subtext)
    } else {
        (best.map_or(String::new(), |b| format!("Your best is {:.0} wpm", b.wpm)), t.subtext)
    };
    text(dc, &msg, &row(top + ui.s(86), ui.s(22)), ui.fonts.small, col, DT_CENTER | DT_END_ELLIPSIS);

    // What the Solar System got.
    let hy = top + ui.s(124);
    match ui.harvest {
        Some(Ok(got)) if got.iter().sum::<u32>() > 0 => {
            let n: u32 = got.iter().sum();
            let head = format!("+{n} particles for your solar system");
            let parts: Vec<(crate::solar::Material, String)> = crate::solar::Material::ALL
                .iter()
                .filter(|m| got[m.index()] > 0)
                .map(|m| (*m, format!("{} {}", got[m.index()], m.name().to_lowercase())))
                .collect();
            let sw = ui.s(9);
            let gap = ui.s(14);
            let total = text_width(dc, &head, ui.fonts.title)
                + parts.iter().map(|(_, l)| gap + sw + ui.s(5) + text_width(dc, l, ui.fonts.small)).sum::<i32>();
            let mut x = cx - total / 2;
            let hw = text_width(dc, &head, ui.fonts.title);
            text(dc, &head, &RECT { left: x, top: hy, right: x + hw + 2, bottom: hy + ui.s(24) }, ui.fonts.title, t.accent, DT_LEFT);
            x += hw;
            for (m, label) in parts {
                x += gap;
                let my = hy + ui.s(12);
                fill(dc, &RECT { left: x, top: my - sw / 2, right: x + sw, bottom: my + sw / 2 }, material_color(m));
                x += sw + ui.s(5);
                let lw = text_width(dc, &label, ui.fonts.small);
                text(dc, &label, &RECT { left: x, top: hy, right: x + lw + 2, bottom: hy + ui.s(24) }, ui.fonts.small, t.subtext, DT_LEFT);
                x += lw;
            }
        }
        Some(Ok(_)) => {
            text(dc, "No words typed right, so nothing to harvest", &row(hy, ui.s(24)), ui.fonts.small, t.subtext, DT_CENTER);
        }
        Some(Err(wait)) => {
            let msg = format!("Practice run · next harvest in {}", super::solar::wait_label(wait));
            text(dc, &msg, &row(hy, ui.s(24)), ui.fonts.small, t.subtext, DT_CENTER);
        }
        None => {}
    }
    text(dc, "Tab or Enter to go again", &row(hy + ui.s(44), ui.s(22)), ui.fonts.small, t.subtext, DT_CENTER);
}

fn material_color(m: crate::solar::Material) -> COLORREF {
    let c = m.rgb();
    COLORREF((c >> 16) & 0xFF | (c & 0xFF00) | (c & 0xFF) << 16)
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

    // Selection highlight, drawn first so the row draws over it.
    if let Some(i) = ui.selected.filter(|&i| matches!(ui.rows.get(i), Some(Row::Item(_)))) {
        let (top, h) = (ui.offsets[i], ui.row_height(&ui.rows[i]));
        let y = lr.top + top - ui.scroll;
        let rc = RECT { left: lr.left + pad, top: y, right: lr.right - pad, bottom: y + h };
        round_rect(dc, &rc, radius, t.selection);
        // A thin accent edge marks the selected row (square style).
        fill(dc, &RECT { left: rc.left, top: rc.top, right: rc.left + ui.s(3), bottom: rc.bottom }, t.accent);
    }

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
            Row::Calc(c) => draw_calc(ui, dc, &rc, c, ui.selected == Some(i), ui.pulse().filter(|_| ui.pulse_row == Some(i))),
            Row::Item(it) => {
                if let Some(p) = ui.pulse().filter(|_| ui.pulse_row == Some(i)) {
                    draw_row_pulse(dc, &rc, p, t, ui.s(1), ui.selected == Some(i));
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
                    Icon::Swatch(c) => {
                        fill(dc, &irc, t.divider);
                        fill(dc, &draw::inset(&irc, 1), windows::Win32::Foundation::COLORREF(*c));
                    }
                    Icon::Char(c) => {
                        round_rect(dc, &irc, ui.s(6), t.accent);
                        text(dc, &c.to_string(), &irc, ui.fonts.title, WHITE, DT_CENTER);
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
        let x = lr.right - ui.s(5) - ui.s(t.frame_w);
        round_rect(dc, &RECT { left: x - ui.s(3), top: y, right: x, bottom: y + thumb }, ui.s(3), t.scrollbar);
    }

    unsafe {
        let _ = RestoreDC(dc, -1);
    }
}

fn draw_calc(ui: &Launcher, dc: HDC, row: &RECT, c: &crate::calc::Calc, selected: bool, pulse: Option<f32>) {
    let dpi = ((ui.s(960) as f32) / 10.0).round() as u32; // effective DPI incl. launcher size
    draw_calc_card(dc, row, c, selected, &ui.theme, &ui.fonts, dpi, pulse);
}

/// The calculator's "copied" pulse, for any row: an accent ring that grows
/// and fades while the row's fill settles back from accent.
fn draw_row_pulse(dc: HDC, rc: &RECT, p: f32, t: &Theme, px: i32, selected: bool) {
    let e = 1.0 - (1.0 - p).powi(3);
    let base = if selected { t.selection } else { t.bg };
    let grow = 2 * px + (e * 10.0 * px as f32) as i32;
    round_rect(dc, &inflate(rc, grow), grow * 2, mix(t.accent, t.bg, e));
    round_rect(dc, &inflate(rc, px), px * 2, mix(t.accent, t.bg, 0.4 + 0.6 * e));
    round_rect(dc, rc, px * 6, mix(t.accent, base, 0.55 + 0.45 * e));
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
        None if ui.mode == Mode::Media => {
            let msg = "Play/Pause: Space · Skip: ← → · Back: Esc";
            text(dc, msg, &RECT { right: w - ui.s(14), ..left_rc }, ui.fonts.small, t.subtext, DT_LEFT | DT_END_ELLIPSIS)
        }
        None if ui.mode == Mode::Typing => {
            let msg = "Restart: Tab · Back: Esc";
            text(dc, msg, &RECT { right: w - ui.s(14), ..left_rc }, ui.fonts.small, t.subtext, DT_LEFT | DT_END_ELLIPSIS)
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
            let mut fonts = Fonts::create(dpi, &crate::config::Appearance::default());
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
            let mut fonts = Fonts::create(dpi, &crate::config::Appearance::default());
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
