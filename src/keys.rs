//! Keyboard vocabulary shared by hotkeys, remaps and the settings UI.
//!
//! Keys are identified by a `code`: the Windows virtual-key code, except that
//! numpad keys are always reported as their numpad codes (even with Num Lock
//! off, when Windows sends them as arrows/Home/etc.) and Numpad Enter gets its
//! own code. Each key has a stable `id` (used in config.toml) and a display
//! `label` (used on keycaps).

/// Numpad Enter (Windows sends it as VK_RETURN with the extended flag).
pub const NUMPAD_ENTER: u32 = 0x10D;

pub const VK_ESCAPE: u32 = 0x1B;
pub const VK_CAPITAL: u32 = 0x14;
pub const VK_LSHIFT: u32 = 0xA0;
pub const VK_RSHIFT: u32 = 0xA1;
pub const VK_LCONTROL: u32 = 0xA2;
pub const VK_RCONTROL: u32 = 0xA3;
pub const VK_LMENU: u32 = 0xA4;
pub const VK_RMENU: u32 = 0xA5;
pub const VK_LWIN: u32 = 0x5B;
pub const VK_RWIN: u32 = 0x5C;

/// Modifier bit flags (same values as RegisterHotKey's MOD_*).
pub const ALT: u8 = 1;
pub const CTRL: u8 = 2;
pub const SHIFT: u8 = 4;
pub const WIN: u8 = 8;
pub const HYPER: u8 = CTRL | SHIFT | ALT;

pub struct KeyDef {
    pub code: u32,
    pub id: &'static str,
    pub label: &'static str,
}

macro_rules! keys {
    ($($code:expr => $id:literal, $label:literal;)*) => {
        pub const KEYS: &[KeyDef] = &[$(KeyDef { code: $code, id: $id, label: $label }),*];
    };
}

keys! {
    0x41 => "A", "A"; 0x42 => "B", "B"; 0x43 => "C", "C"; 0x44 => "D", "D"; 0x45 => "E", "E";
    0x46 => "F", "F"; 0x47 => "G", "G"; 0x48 => "H", "H"; 0x49 => "I", "I"; 0x4A => "J", "J";
    0x4B => "K", "K"; 0x4C => "L", "L"; 0x4D => "M", "M"; 0x4E => "N", "N"; 0x4F => "O", "O";
    0x50 => "P", "P"; 0x51 => "Q", "Q"; 0x52 => "R", "R"; 0x53 => "S", "S"; 0x54 => "T", "T";
    0x55 => "U", "U"; 0x56 => "V", "V"; 0x57 => "W", "W"; 0x58 => "X", "X"; 0x59 => "Y", "Y";
    0x5A => "Z", "Z";
    0x30 => "0", "0"; 0x31 => "1", "1"; 0x32 => "2", "2"; 0x33 => "3", "3"; 0x34 => "4", "4";
    0x35 => "5", "5"; 0x36 => "6", "6"; 0x37 => "7", "7"; 0x38 => "8", "8"; 0x39 => "9", "9";
    0x70 => "F1", "F1"; 0x71 => "F2", "F2"; 0x72 => "F3", "F3"; 0x73 => "F4", "F4";
    0x74 => "F5", "F5"; 0x75 => "F6", "F6"; 0x76 => "F7", "F7"; 0x77 => "F8", "F8";
    0x78 => "F9", "F9"; 0x79 => "F10", "F10"; 0x7A => "F11", "F11"; 0x7B => "F12", "F12";
    0x7C => "F13", "F13"; 0x7D => "F14", "F14"; 0x7E => "F15", "F15"; 0x7F => "F16", "F16";
    0x80 => "F17", "F17"; 0x81 => "F18", "F18"; 0x82 => "F19", "F19"; 0x83 => "F20", "F20";
    0x84 => "F21", "F21"; 0x85 => "F22", "F22"; 0x86 => "F23", "F23"; 0x87 => "F24", "F24";
    0x60 => "Numpad0", "Num 0"; 0x61 => "Numpad1", "Num 1"; 0x62 => "Numpad2", "Num 2";
    0x63 => "Numpad3", "Num 3"; 0x64 => "Numpad4", "Num 4"; 0x65 => "Numpad5", "Num 5";
    0x66 => "Numpad6", "Num 6"; 0x67 => "Numpad7", "Num 7"; 0x68 => "Numpad8", "Num 8";
    0x69 => "Numpad9", "Num 9";
    0x6A => "NumpadMultiply", "Num *"; 0x6B => "NumpadAdd", "Num +"; 0x6D => "NumpadSubtract", "Num -";
    0x6E => "NumpadDecimal", "Num ."; 0x6F => "NumpadDivide", "Num /"; 0x10D => "NumpadEnter", "Num Enter";
    0x20 => "Space", "Space"; 0x0D => "Enter", "Enter"; 0x09 => "Tab", "Tab"; 0x1B => "Escape", "Esc";
    0x08 => "Backspace", "Backspace"; 0x2E => "Delete", "Del"; 0x2D => "Insert", "Ins";
    0x24 => "Home", "Home"; 0x23 => "End", "End"; 0x21 => "PageUp", "PgUp"; 0x22 => "PageDown", "PgDn";
    0x26 => "Up", "Up"; 0x28 => "Down", "Down"; 0x25 => "Left", "Left"; 0x27 => "Right", "Right";
    0x14 => "CapsLock", "Caps Lock"; 0x90 => "NumLock", "Num Lock"; 0x91 => "ScrollLock", "Scroll Lock";
    0x2C => "PrintScreen", "PrtSc"; 0x13 => "Pause", "Pause"; 0x5D => "Menu", "Menu";
    0xC0 => "`", "`"; 0xBD => "-", "-"; 0xBB => "=", "="; 0xDB => "[", "["; 0xDD => "]", "]";
    0xDC => "\\", "\\"; 0xBA => ";", ";"; 0xDE => "'", "'"; 0xBC => ",", ","; 0xBE => ".", "."; 0xBF => "/", "/";
    0xA2 => "LCtrl", "Ctrl"; 0xA3 => "RCtrl", "Right Ctrl"; 0xA0 => "LShift", "Shift"; 0xA1 => "RShift", "Right Shift";
    0xA4 => "LAlt", "Alt"; 0xA5 => "RAlt", "Right Alt"; 0x5B => "LWin", "Win"; 0x5C => "RWin", "Right Win";
    0xB3 => "MediaPlayPause", "Play/Pause"; 0xB0 => "MediaNext", "Next Track"; 0xB1 => "MediaPrevious", "Previous Track";
    0xB2 => "MediaStop", "Stop"; 0xAF => "VolumeUp", "Volume Up"; 0xAE => "VolumeDown", "Volume Down";
    0xAD => "VolumeMute", "Mute";
}

/// Older/alternate spellings accepted when parsing.
const ALIASES: &[(&str, u32)] = &[
    ("return", 0x0D), ("esc", 0x1B), ("del", 0x2E), ("ins", 0x2D), ("pgup", 0x21), ("pgdn", 0x22),
    ("backquote", 0xC0), ("grave", 0xC0), ("tilde", 0xC0), ("minus", 0xBD), ("equals", 0xBB),
    ("comma", 0xBC), ("period", 0xBE), ("caps", 0x14), ("prtsc", 0x2C), ("apps", 0x5D),
    ("playpause", 0xB3), ("play", 0xB3), ("next", 0xB0), ("previous", 0xB1), ("prev", 0xB1),
    ("mute", 0xAD),
];

pub fn by_code(code: u32) -> Option<&'static KeyDef> {
    KEYS.iter().find(|k| k.code == code)
}

/// Looks up a key by id, label or alias (case-insensitive). Does not accept
/// bare modifier names; see [`modifier_bit`].
pub fn by_name(name: &str) -> Option<u32> {
    let n = name.trim();
    KEYS.iter()
        .find(|k| k.id.eq_ignore_ascii_case(n) || k.label.eq_ignore_ascii_case(n))
        .map(|k| k.code)
        .or_else(|| ALIASES.iter().find(|(a, _)| a.eq_ignore_ascii_case(n)).map(|(_, c)| *c))
}

pub fn label(code: u32) -> String {
    by_code(code).map(|k| k.label.to_string()).unwrap_or_else(|| format!("Key {code:#04X}"))
}

pub fn id(code: u32) -> Option<&'static str> {
    by_code(code).map(|k| k.id)
}

/// Modifier bit for a generic modifier name ("Ctrl", "Alt", ...).
pub fn modifier_bit(name: &str) -> Option<u8> {
    match name.trim().to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(CTRL),
        "alt" => Some(ALT),
        "shift" => Some(SHIFT),
        "win" | "super" | "meta" | "windows" => Some(WIN),
        _ => None,
    }
}

/// Modifier bit for a modifier key code (either side), or 0.
pub fn modifier_of(code: u32) -> u8 {
    match code {
        0x10 | VK_LSHIFT | VK_RSHIFT => SHIFT,
        0x11 | VK_LCONTROL | VK_RCONTROL => CTRL,
        0x12 | VK_LMENU | VK_RMENU => ALT,
        VK_LWIN | VK_RWIN => WIN,
        _ => 0,
    }
}

pub fn is_modifier(code: u32) -> bool {
    modifier_of(code) != 0
}

/// Keycap labels for a modifier mask, in conventional order.
pub fn modifier_labels(mods: u8) -> Vec<String> {
    let mut v = Vec::new();
    for (bit, name) in [(CTRL, "Ctrl"), (ALT, "Alt"), (SHIFT, "Shift"), (WIN, "Win")] {
        if mods & bit != 0 {
            v.push(name.to_string());
        }
    }
    v
}

/// Normalises a raw (vk, extended-flag) pair from the keyboard into a code.
/// With Num Lock off the numpad sends navigation keys *without* the extended
/// flag; the dedicated navigation keys always have it.
pub fn canonical(vk: u32, extended: bool) -> u32 {
    if vk == 0x0D && extended {
        return NUMPAD_ENTER;
    }
    if !extended {
        let mapped = match vk {
            0x2D => 0x60, // Insert -> Num 0
            0x23 => 0x61, // End -> Num 1
            0x28 => 0x62, // Down
            0x22 => 0x63, // PgDn
            0x25 => 0x64, // Left
            0x0C => 0x65, // Clear (Num 5)
            0x27 => 0x66, // Right
            0x24 => 0x67, // Home
            0x26 => 0x68, // Up
            0x21 => 0x69, // PgUp
            0x2E => 0x6E, // Delete -> Num .
            _ => vk,
        };
        return mapped;
    }
    vk
}

/// Virtual-key code to inject for a code.
pub fn vk(code: u32) -> u16 {
    if code == NUMPAD_ENTER { 0x0D } else { code as u16 }
}

/// Whether injecting this key needs the extended-key flag.
pub fn is_extended(code: u32) -> bool {
    matches!(
        code,
        NUMPAD_ENTER | 0x21..=0x28 | 0x2D | 0x2E | 0x2C | 0x6F | 0x90 | VK_RCONTROL | VK_RMENU | VK_LWIN | VK_RWIN | 0x5D
            | 0xAD..=0xB3
    )
}

/// What a key is remapped to (or a shortcut's target).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// Press these modifiers and (optionally) a key.
    Keys { mods: u8, key: Option<u32> },
    /// Swallow the key entirely.
    Disabled,
}

impl Target {
    pub const HYPER: Target = Target::Keys { mods: HYPER, key: None };

    /// Parses "Ctrl+C", "MediaPlayPause", "Hyper", "Ctrl", "Disabled".
    pub fn parse(spec: &str) -> Result<Target, String> {
        let s = spec.trim();
        if s.eq_ignore_ascii_case("disabled") || s.eq_ignore_ascii_case("none") {
            return Ok(Target::Disabled);
        }
        if s.eq_ignore_ascii_case("hyper") {
            return Ok(Target::HYPER);
        }
        let mut mods = 0u8;
        let mut key = None;
        for part in s.split('+').map(str::trim) {
            if part.is_empty() {
                return Err(format!("invalid key \"{spec}\""));
            }
            if let Some(b) = modifier_bit(part) {
                mods |= b;
            } else if let Some(code) = by_name(part) {
                if key.is_some() {
                    return Err(format!("\"{spec}\" has more than one key"));
                }
                key = Some(code);
            } else {
                return Err(format!("unknown key \"{part}\""));
            }
        }
        if mods == 0 && key.is_none() {
            return Err(format!("invalid key \"{spec}\""));
        }
        Ok(Target::Keys { mods, key })
    }

    /// Config-file spelling.
    pub fn to_spec(self) -> String {
        match self {
            Target::Disabled => "Disabled".into(),
            Target::Keys { mods: HYPER, key: None } => "Hyper".into(),
            Target::Keys { mods, key } => {
                let mut parts = modifier_labels(mods);
                if let Some(k) = key {
                    parts.push(id(k).unwrap_or("?").to_string());
                }
                parts.join("+")
            }
        }
    }

    /// Keycap labels for display.
    pub fn labels(self) -> Vec<String> {
        match self {
            Target::Disabled => vec!["Disabled".into()],
            Target::Keys { mods: HYPER, key: None } => vec!["Hyper".into()],
            Target::Keys { mods, key } => {
                let mut v = modifier_labels(mods);
                if let Some(k) = key {
                    v.push(label(k));
                }
                v
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numpad_canonicalisation() {
        assert_eq!(canonical(0x26, false), 0x68); // Num 8 with Num Lock off
        assert_eq!(canonical(0x26, true), 0x26); // real Up arrow
        assert_eq!(canonical(0x68, false), 0x68); // Num 8 with Num Lock on
        assert_eq!(canonical(0x0D, true), NUMPAD_ENTER);
        assert_eq!(canonical(0x0D, false), 0x0D);
        assert_eq!(canonical(0x0C, false), 0x65);
    }

    #[test]
    fn target_round_trips() {
        for s in ["Ctrl+C", "MediaPlayPause", "Hyper", "Disabled", "Ctrl+Shift+Alt+K", "Win+Shift+S", "Ctrl", "Numpad5"] {
            let t = Target::parse(s).unwrap();
            assert_eq!(Target::parse(&t.to_spec()).unwrap(), t, "{s}");
        }
        assert_eq!(Target::parse("ctrl+shift+alt").unwrap(), Target::HYPER);
        assert!(Target::parse("Ctrl+Nope").is_err());
        assert!(Target::parse("A+B").is_err());
    }

    #[test]
    fn names_and_labels() {
        assert_eq!(by_name("numpad5"), Some(0x65));
        assert_eq!(by_name("Num 5"), Some(0x65));
        assert_eq!(by_name("esc"), Some(0x1B));
        assert_eq!(by_name("`"), Some(0xC0));
        assert_eq!(label(0xB3), "Play/Pause");
        assert_eq!(Target::parse("Ctrl+Alt+Shift+C").unwrap().labels(), vec!["Ctrl", "Alt", "Shift", "C"]);
    }

    #[test]
    fn every_key_id_is_unique_and_parses() {
        for k in KEYS {
            assert_eq!(KEYS.iter().filter(|o| o.id.eq_ignore_ascii_case(k.id)).count(), 1, "{}", k.id);
            assert_eq!(by_name(k.id), Some(k.code), "{}", k.id);
        }
    }
}
