//! Colours: parsing what people type (#hex, rgb(), hsl()), converting
//! between formats, and the history of picked colours
//! (`%APPDATA%\SIGMA5\colors.txt`, one "#RRGGBB unix-time" per line).

use std::path::PathBuf;

use crate::util::{data_dir, now_unix};

/// Colours kept in the history.
pub const HISTORY_MAX: usize = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// From a Win32 COLORREF (0x00BBGGRR).
    pub fn from_colorref(c: u32) -> Self {
        Self { r: c as u8, g: (c >> 8) as u8, b: (c >> 16) as u8 }
    }

    pub fn colorref(self) -> u32 {
        self.r as u32 | (self.g as u32) << 8 | (self.b as u32) << 16
    }

    pub fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    pub fn rgb(self) -> String {
        format!("rgb({}, {}, {})", self.r, self.g, self.b)
    }

    /// Hue (degrees), saturation and lightness (percent), rounded.
    pub fn hsl_parts(self) -> (u32, u32, u32) {
        let (r, g, b) = (self.r as f32 / 255.0, self.g as f32 / 255.0, self.b as f32 / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;
        let d = max - min;
        if d < 1e-6 {
            return (0, 0, (l * 100.0).round() as u32);
        }
        let s = d / (1.0 - (2.0 * l - 1.0).abs());
        let h = if max == r {
            60.0 * ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            60.0 * ((b - r) / d + 2.0)
        } else {
            60.0 * ((r - g) / d + 4.0)
        };
        ((h.round() as u32) % 360, (s * 100.0).round() as u32, (l * 100.0).round() as u32)
    }

    pub fn hsl(self) -> String {
        let (h, s, l) = self.hsl_parts();
        format!("hsl({h}, {s}%, {l}%)")
    }

    /// Perceived brightness 0–1 (for choosing readable text on a swatch).
    pub fn luminance(self) -> f32 {
        (0.299 * self.r as f32 + 0.587 * self.g as f32 + 0.114 * self.b as f32) / 255.0
    }

    fn from_hsl(h: f32, s: f32, l: f32) -> Self {
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let hp = (h.rem_euclid(360.0)) / 60.0;
        let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
        let (r, g, b) = match hp as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = l - c / 2.0;
        let f = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
        Self::new(f(r), f(g), f(b))
    }
}

/// Parses `#RGB`, `#RRGGBB`, `rgb(r, g, b)` and `hsl(h, s%, l%)`. Plain
/// words are never colours, so searches like "facade" aren't hijacked.
pub fn parse(s: &str) -> Option<Color> {
    let s = s.trim().to_ascii_lowercase();
    if let Some(hex) = s.strip_prefix('#') {
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let v = |i: usize, n: usize| u8::from_str_radix(&hex[i..i + n], 16).ok();
        return match hex.len() {
            3 => Some(Color::new(v(0, 1)? * 17, v(1, 1)? * 17, v(2, 1)? * 17)),
            6 => Some(Color::new(v(0, 2)?, v(2, 2)?, v(4, 2)?)),
            _ => None,
        };
    }
    let args = |prefix: &str| -> Option<Vec<f32>> {
        let inner = s.strip_prefix(prefix)?.trim_start().strip_prefix('(')?.strip_suffix(')')?;
        let parts: Vec<f32> = inner
            .split([',', ' ', '/'])
            .filter(|p| !p.is_empty())
            .map(|p| p.trim_end_matches(['%', '°']).parse().ok())
            .collect::<Option<_>>()?;
        (parts.len() == 3).then_some(parts)
    };
    if let Some(p) = args("rgb") {
        let ok = p.iter().all(|v| (0.0..=255.0).contains(v));
        return ok.then(|| Color::new(p[0].round() as u8, p[1].round() as u8, p[2].round() as u8));
    }
    if let Some(p) = args("hsl") {
        let ok = (0.0..=100.0).contains(&p[1]) && (0.0..=100.0).contains(&p[2]);
        return ok.then(|| Color::from_hsl(p[0], p[1] / 100.0, p[2] / 100.0));
    }
    None
}

/// A picked colour and when.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Picked {
    pub color: Color,
    pub at: u64,
}

pub struct History {
    path: PathBuf,
    items: Vec<Picked>,
}

impl History {
    pub fn load() -> Self {
        Self::load_from(data_dir().join("colors.txt"))
    }

    fn load_from(path: PathBuf) -> Self {
        let items = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let (hex, at) = l.split_once(' ').unwrap_or((l, "0"));
                Some(Picked { color: parse(hex)?, at: at.trim().parse().unwrap_or(0) })
            })
            .take(HISTORY_MAX)
            .collect();
        Self { path, items }
    }

    /// Newest first.
    pub fn items(&self) -> &[Picked] {
        &self.items
    }

    /// Adds (or moves to the top) a colour and saves.
    pub fn add(&mut self, c: Color) {
        self.items.retain(|p| p.color != c);
        self.items.insert(0, Picked { color: c, at: now_unix() });
        self.items.truncate(HISTORY_MAX);
        self.save();
    }

    pub fn remove(&mut self, c: Color) {
        self.items.retain(|p| p.color != c);
        self.save();
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.save();
    }

    fn save(&self) {
        let text: String = self.items.iter().map(|p| format!("{} {}\n", p.color.hex(), p.at)).collect();
        let _ = std::fs::write(&self.path, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_formats() {
        assert_eq!(parse("#ff8800"), Some(Color::new(255, 136, 0)));
        assert_eq!(parse("#F80"), Some(Color::new(255, 136, 0)));
        assert_eq!(parse(" rgb(255, 136, 0) "), Some(Color::new(255, 136, 0)));
        assert_eq!(parse("rgb(255 136 0)"), Some(Color::new(255, 136, 0)));
        assert_eq!(parse("hsl(32, 100%, 50%)"), Some(Color::new(255, 136, 0)));
        for bad in ["facade", "#ff88", "#gg0000", "rgb(300, 0, 0)", "rgb(1,2)", "hsl(0, 120%, 50%)", "ff8800"] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn formats_round_trip() {
        let c = Color::new(171, 114, 255);
        assert_eq!(c.hex(), "#AB72FF");
        assert_eq!(c.rgb(), "rgb(171, 114, 255)");
        assert_eq!(c.hsl(), "hsl(264, 100%, 72%)");
        assert_eq!(parse(&c.hex()), Some(c));
        assert_eq!(Color::from_colorref(c.colorref()), c);
        // HSL survives a round trip to within rounding.
        let back = parse(&c.hsl()).unwrap();
        assert!((back.r as i32 - c.r as i32).abs() <= 2 && (back.b as i32 - c.b as i32).abs() <= 2);
        assert_eq!(Color::new(128, 128, 128).hsl(), "hsl(0, 0%, 50%)");
    }

    #[test]
    fn history_dedupes_and_persists() {
        let path = std::env::temp_dir().join(format!("sigma5-colors-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut h = History::load_from(path.clone());
        h.add(Color::new(1, 2, 3));
        h.add(Color::new(4, 5, 6));
        h.add(Color::new(1, 2, 3));
        assert_eq!(h.items().iter().map(|p| p.color).collect::<Vec<_>>(), vec![Color::new(1, 2, 3), Color::new(4, 5, 6)]);
        let again = History::load_from(path.clone());
        assert_eq!(again.items().len(), 2);
        assert_eq!(again.items()[0].color, Color::new(1, 2, 3));
        let _ = std::fs::remove_file(path);
    }
}
