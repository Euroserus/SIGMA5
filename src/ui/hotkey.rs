//! Global hotkeys ("Alt+Space", "Ctrl+Alt+Shift+C", ...) via RegisterHotKey.
//!
//! Id 1 is the launcher hotkey; app hotkeys use `APP_HOTKEY_BASE + index`.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
};

use crate::keys;

pub const HOTKEY_ID: i32 = 1;
pub const APP_HOTKEY_BASE: i32 = 100;
const PROBE_ID: i32 = 999;

/// Parses a hotkey into (modifier bits, virtual key).
pub fn parse(spec: &str) -> Result<(u8, u32), String> {
    let mut mods = 0u8;
    let mut key = None;
    for part in spec.split('+').map(str::trim) {
        if part.is_empty() {
            return Err(format!("invalid hotkey \"{spec}\""));
        }
        if let Some(b) = keys::modifier_bit(part) {
            mods |= b;
            continue;
        }
        let code = keys::by_name(part).ok_or_else(|| format!("unknown key \"{part}\" in hotkey"))?;
        if keys::is_modifier(code) || code > 0xFF {
            return Err(format!("\"{part}\" can't be the main key of a hotkey"));
        }
        if key.replace(code).is_some() {
            return Err(format!("hotkey \"{spec}\" has more than one key"));
        }
    }
    let key = key.ok_or_else(|| format!("hotkey \"{spec}\" has no key"))?;
    if mods == 0 {
        return Err(format!("hotkey \"{spec}\" needs a modifier (Ctrl, Alt, Shift or Win)"));
    }
    Ok((mods, key))
}

/// Builds a hotkey string from captured modifiers and key.
pub fn format(mods: u8, code: u32) -> Result<String, String> {
    if keys::is_modifier(code) {
        return Err("Add a regular key to the modifiers".into());
    }
    if code > 0xFF {
        return Err(format!("{} can't be used in a hotkey", keys::label(code)));
    }
    let id = keys::id(code).ok_or("That key can't be used in a shortcut")?;
    if mods == 0 {
        return Err("Add Ctrl, Alt, Shift or Win to the key".into());
    }
    let mut parts = keys::modifier_labels(mods);
    parts.push(id.to_string());
    Ok(parts.join("+"))
}

/// Keycap labels, e.g. "ctrl+shift+k" → ["Ctrl", "Shift", "K"].
pub fn parts(spec: &str) -> Vec<String> {
    match parse(spec) {
        Ok((mods, vk)) => {
            let mut v = keys::modifier_labels(mods);
            v.push(keys::label(vk));
            v
        }
        Err(_) if spec.trim().is_empty() => Vec::new(),
        Err(_) => vec![spec.to_string()],
    }
}

/// Canonical spelling, so "alt + space" and "Alt+Space" compare equal.
pub fn normalize(spec: &str) -> Option<String> {
    let (m, k) = parse(spec).ok()?;
    format(m, k).ok()
}

fn reg(hwnd: HWND, id: i32, mods: u8, vk: u32) -> bool {
    unsafe { RegisterHotKey(Some(hwnd), id, HOT_KEY_MODIFIERS(mods as u32) | MOD_NOREPEAT, vk).is_ok() }
}

pub fn unregister(hwnd: HWND, id: i32) {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), id);
    }
}

/// Registers `spec` under `id`, replacing any previous registration of that id.
pub fn register_id(hwnd: HWND, id: i32, spec: &str) -> Result<(), String> {
    unregister(hwnd, id);
    let (mods, vk) = parse(spec)?;
    if reg(hwnd, id, mods, vk) {
        Ok(())
    } else {
        Err(format!("{spec} is already used by another app"))
    }
}

pub fn register(hwnd: HWND, spec: &str) -> Result<(), String> {
    register_id(hwnd, HOTKEY_ID, spec)
}

/// True if no app (including this one) currently owns the combination.
pub fn is_free(hwnd: HWND, spec: &str) -> bool {
    let Ok((mods, vk)) = parse(spec) else { return false };
    let ok = reg(hwnd, PROBE_ID, mods, vk);
    if ok {
        unregister(hwnd, PROBE_ID);
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{ALT, CTRL, SHIFT, WIN};

    #[test]
    fn parses_common() {
        assert_eq!(parse("Alt+Space").unwrap(), (ALT, 0x20));
        assert_eq!(parse("ctrl + shift + k").unwrap(), (CTRL | SHIFT, 'K' as u32));
        assert_eq!(parse("Win+F5").unwrap().1, 0x74);
        assert_eq!(parse("Ctrl+Alt+Shift+Numpad5").unwrap(), (CTRL | ALT | SHIFT, 0x65));
        assert!(parse("Space").is_err());
        assert!(parse("Alt+Nope").is_err());
        assert!(parse("Ctrl+NumpadEnter").is_err());
    }

    #[test]
    fn formats_round_trip() {
        assert_eq!(format(CTRL | SHIFT, 'K' as u32).unwrap(), "Ctrl+Shift+K");
        assert_eq!(parts("alt + space"), vec!["Alt", "Space"]);
        assert!(format(0, 'K' as u32).is_err());
        assert!(format(CTRL, 0xA2).is_err());
        let s = format(ALT | WIN, 0x70).unwrap();
        assert_eq!(parse(&s).unwrap(), (ALT | WIN, 0x70));
        assert_eq!(normalize("shift+alt+ctrl+c").unwrap(), "Ctrl+Alt+Shift+C");
    }
}
