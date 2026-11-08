//! Colours and layout metrics.

use windows::Win32::Foundation::COLORREF;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::core::{PCWSTR, w};

use crate::config::{Accent, Appearance, ThemeMode};
use super::draw::mix;

pub const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

pub const WHITE: COLORREF = rgb(255, 255, 255);

#[derive(Clone, Copy, PartialEq)]
pub struct Theme {
    pub dark: bool,
    pub bg: COLORREF,
    pub border: COLORREF,
    pub divider: COLORREF,
    pub text: COLORREF,
    pub subtext: COLORREF,
    pub header: COLORREF,
    pub selection: COLORREF,
    pub tile: COLORREF,
    pub tile_text: COLORREF,
    pub accent: COLORREF,
    pub kbd_bg: COLORREF,
    pub kbd_text: COLORREF,
    pub scrollbar: COLORREF,
    pub warn: COLORREF,
    // Settings window
    pub card: COLORREF,
    pub card_border: COLORREF,
    pub hover: COLORREF,
    pub toggle_off: COLORREF,
    pub seg_selected: COLORREF,
    /// Corner radius in logical px (0 = square).
    pub radius: i32,
    /// Border round the launcher window: colour and thickness (logical px).
    pub frame: COLORREF,
    pub frame_w: i32,
}

pub const DARK: Theme = Theme {
    dark: true,
    bg: rgb(28, 28, 31),
    border: rgb(64, 64, 70),
    divider: rgb(44, 44, 48),
    text: rgb(238, 238, 241),
    subtext: rgb(140, 140, 148),
    header: rgb(126, 126, 134),
    selection: rgb(50, 50, 56),
    tile: rgb(62, 62, 70),
    tile_text: rgb(220, 220, 226),
    accent: rgb(124, 92, 255),
    kbd_bg: rgb(52, 52, 58),
    kbd_text: rgb(196, 196, 204),
    scrollbar: rgb(78, 78, 86),
    warn: rgb(255, 170, 80),
    card: rgb(36, 36, 41),
    card_border: rgb(46, 46, 52),
    hover: rgb(64, 64, 72),
    toggle_off: rgb(86, 86, 96),
    seg_selected: rgb(78, 78, 88),
    radius: 0,
    frame: rgb(124, 92, 255),
    frame_w: 0,
};

pub const LIGHT: Theme = Theme {
    dark: false,
    bg: rgb(250, 250, 252),
    border: rgb(206, 206, 212),
    divider: rgb(228, 228, 233),
    text: rgb(24, 24, 28),
    subtext: rgb(108, 108, 118),
    header: rgb(118, 118, 128),
    selection: rgb(230, 230, 236),
    tile: rgb(218, 218, 226),
    tile_text: rgb(60, 60, 70),
    accent: rgb(110, 80, 245),
    kbd_bg: rgb(234, 234, 240),
    kbd_text: rgb(70, 70, 80),
    scrollbar: rgb(198, 198, 206),
    warn: rgb(200, 110, 0),
    card: rgb(255, 255, 255),
    card_border: rgb(226, 226, 232),
    hover: rgb(222, 222, 230),
    toggle_off: rgb(184, 184, 194),
    seg_selected: rgb(255, 255, 255),
    radius: 0,
    frame: rgb(110, 80, 245),
    frame_w: 0,
};

fn reg_dword(key: PCWSTR, value: PCWSTR) -> Option<u32> {
    let mut v: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            value,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    r.is_ok().then_some(v)
}

fn system_prefers_light() -> bool {
    reg_dword(w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"), w!("AppsUseLightTheme")) == Some(1)
}

/// The Windows accent colour (Settings > Personalization > Colors).
pub fn system_accent() -> COLORREF {
    // Stored as 0xAABBGGRR, which matches COLORREF's byte order.
    reg_dword(w!("Software\\Microsoft\\Windows\\DWM"), w!("AccentColor"))
        .map(|v| COLORREF(v & 0x00FF_FFFF))
        .unwrap_or(rgb(0, 120, 212))
}

pub fn accent_color(a: Accent) -> COLORREF {
    match a {
        Accent::System => system_accent(),
        // Matches the SIGMA5 logo.
        Accent::Violet => rgb(171, 114, 255),
        Accent::Blue => rgb(40, 120, 255),
        Accent::Teal => rgb(18, 170, 156),
        Accent::Green => rgb(46, 175, 84),
        Accent::Orange => rgb(245, 130, 32),
        Accent::Red => rgb(235, 72, 72),
        Accent::Pink => rgb(230, 70, 150),
    }
}

/// "#RRGGBB" → COLORREF.
pub fn parse_hex(s: &str) -> Option<COLORREF> {
    crate::colors::parse(s).map(|c| COLORREF(c.colorref()))
}

fn luminance(c: COLORREF) -> f32 {
    crate::colors::Color::from_colorref(c.0).luminance()
}

pub fn resolve(mode: ThemeMode, accent: Accent, look: &Appearance) -> Theme {
    let mut t = match mode {
        ThemeMode::Dark => DARK,
        ThemeMode::Light => LIGHT,
        ThemeMode::System => {
            if system_prefers_light() { LIGHT } else { DARK }
        }
    };
    t.accent = parse_hex(&look.accent_color).unwrap_or_else(|| accent_color(accent));
    t.radius = look.corner_radius.min(16) as i32;

    // A custom background re-derives every surface from it (and from the
    // text colour), so contrast holds whatever is chosen.
    if let Some(bg) = parse_hex(&look.background) {
        let dark = luminance(bg) < 0.5;
        let base = if dark { DARK } else { LIGHT };
        let text = parse_hex(&look.text).unwrap_or(base.text);
        t = Theme {
            dark,
            bg,
            border: mix(bg, text, 0.22),
            divider: mix(bg, text, 0.10),
            text,
            subtext: mix(bg, text, 0.58),
            header: mix(bg, text, 0.52),
            selection: mix(bg, text, if dark { 0.11 } else { 0.08 }),
            tile: mix(bg, text, 0.20),
            tile_text: mix(bg, text, 0.85),
            kbd_bg: mix(bg, text, 0.12),
            kbd_text: mix(bg, text, 0.78),
            scrollbar: mix(bg, text, 0.25),
            card: mix(bg, text, if dark { 0.04 } else { 0.0 }),
            card_border: mix(bg, text, 0.11),
            hover: mix(bg, text, 0.16),
            toggle_off: mix(bg, text, 0.32),
            seg_selected: if dark { mix(bg, text, 0.22) } else { mix(bg, rgb(255, 255, 255), 1.0) },
            ..t
        };
    }
    if let Some(c) = parse_hex(&look.text) {
        t.text = c;
    }
    if let Some(c) = parse_hex(&look.subtext) {
        t.subtext = c;
        t.header = c;
    }
    if let Some(c) = parse_hex(&look.selection) {
        t.selection = c;
    }
    if let Some(c) = parse_hex(&look.border) {
        t.border = c;
        t.divider = c;
        t.card_border = c;
    }
    t.frame = parse_hex(&look.window_border_color).unwrap_or(t.accent);
    t.frame_w = look.window_border.min(8) as i32;
    t
}

/// Launcher layout metrics in logical (96-DPI) pixels; scaled at draw time.
pub mod metrics {
    pub const WIDTH: i32 = 750;
    pub const HEIGHT: i32 = 474;
    pub const SEARCH_H: i32 = 60;
    /// The optional top bar above the search box.
    pub const TOPBAR_H: i32 = 30;
    pub const FOOTER_H: i32 = 40;
    pub const PAD_X: i32 = 8;
    pub const LIST_PAD_Y: i32 = 6;
    pub const HEADER_H: i32 = 30;
    /// Calculator result card.
    pub const CALC_H: i32 = 122;
    /// Length of the "copied" pulse, in ms.
    pub const PULSE_MS: u64 = 650;
    pub const ICON: i32 = 24;
    pub const RADIUS: i32 = 8;
    pub const TEXT_X: i32 = 20;
    /// Vertical position of the window: fraction of work-area height above it.
    pub const TOP_FRACTION: f32 = 0.22;
}
