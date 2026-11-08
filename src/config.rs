//! User settings, stored as TOML in `%APPDATA%\SIGMA5\config.toml`.
//!
//! Edited through the Settings window (which rewrites the file) or by hand.
//! The launcher re-reads it when it changes.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::Deserialize;

use crate::keys::{self, Target};
use crate::util::data_dir;

/// Declares a lowercase string-backed enum usable from TOML and the settings UI.
macro_rules! str_enum {
    ($name:ident { $($variant:ident => $s:literal),+ $(,)? } default $def:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
        #[serde(rename_all = "lowercase")]
        pub enum $name {
            $( $variant, )+
        }
        impl Default for $name {
            fn default() -> Self { $name::$def }
        }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];
            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $s),+ }
            }
        }
    };
}

str_enum!(ThemeMode { System => "system", Dark => "dark", Light => "light" } default System);
str_enum!(Accent {
    System => "system", Violet => "violet", Blue => "blue", Teal => "teal",
    Green => "green", Orange => "orange", Red => "red", Pink => "pink"
} default Violet);
str_enum!(UiSize { Compact => "compact", Default => "default", Large => "large" } default Default);
str_enum!(CapsMode {
    Default => "default", Hyper => "hyper", Ctrl => "ctrl", Escape => "escape", Off => "off"
} default Default);

impl CapsMode {
    pub fn label(self) -> &'static str {
        match self {
            CapsMode::Default => "Default",
            CapsMode::Hyper => "Hyper",
            CapsMode::Ctrl => "Ctrl",
            CapsMode::Escape => "Esc",
            CapsMode::Off => "Off",
        }
    }

    /// What Caps Lock becomes, or None to leave it alone.
    pub fn target(self) -> Option<Target> {
        match self {
            CapsMode::Default => None,
            CapsMode::Hyper => Some(Target::HYPER),
            CapsMode::Ctrl => Some(Target::Keys { mods: keys::CTRL, key: None }),
            CapsMode::Escape => Some(Target::Keys { mods: 0, key: Some(keys::VK_ESCAPE) }),
            CapsMode::Off => Some(Target::Disabled),
        }
    }
}

/// A global hotkey that focuses (or launches) an app.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AppHotkey {
    pub hotkey: String,
    /// Apps-folder id (see `providers::apps`).
    pub app: String,
    /// Display name, kept so the binding still reads well if the app is gone.
    #[serde(default)]
    pub name: String,
}

/// A key remap: `from` is a single key id, `to` a key, shortcut, "Hyper" or "Disabled".
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Remap {
    pub from: String,
    pub to: String,
}

impl UiSize {
    /// Launcher scale in percent.
    pub fn percent(self) -> u32 {
        match self {
            UiSize::Compact => 90,
            UiSize::Default => 100,
            UiSize::Large => 115,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            UiSize::Compact => "Compact",
            UiSize::Default => "Default",
            UiSize::Large => "Large",
        }
    }
}

impl ThemeMode {
    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::System => "System",
            ThemeMode::Dark => "Dark",
            ThemeMode::Light => "Light",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Global shortcut that toggles the launcher, e.g. "Alt+Space".
    pub hotkey: String,
    /// Optional shortcut that opens Clipboard History directly (empty = none).
    pub clipboard_hotkey: String,
    pub theme: ThemeMode,
    pub accent: Accent,
    pub size: UiSize,
    /// Tapping the Windows key opens SIGMA5 instead of the Start menu.
    pub windows_key: bool,
    /// Adds/removes SIGMA5 from HKCU\...\Run so it starts at login.
    pub start_with_windows: bool,
    /// Check GitHub for new versions in the background.
    pub auto_update: bool,
    /// Reopening within this many seconds keeps your last search.
    pub keep_query_secs: u64,
    /// Number of recently used items shown when the search box is empty.
    pub recent_count: usize,
    /// Provider ids that are turned off (see `providers::EXTENSIONS`).
    pub disabled_extensions: Vec<String>,
    /// What Caps Lock does.
    pub caps_lock: CapsMode,
    pub app_hotkeys: Vec<AppHotkey>,
    pub remaps: Vec<Remap>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: "Alt+Space".into(),
            clipboard_hotkey: String::new(),
            theme: ThemeMode::default(),
            accent: Accent::default(),
            size: UiSize::default(),
            windows_key: false,
            start_with_windows: false,
            auto_update: true,
            keep_query_secs: 30,
            recent_count: 6,
            disabled_extensions: Vec::new(),
            caps_lock: CapsMode::Default,
            app_hotkeys: Vec::new(),
            remaps: Vec::new(),
        }
    }
}

impl Config {
    pub fn extension_enabled(&self, id: &str) -> bool {
        !self.disabled_extensions.iter().any(|d| d == id)
    }

    /// Effective key remaps as (source key code, target). Caps Lock's own
    /// setting wins over a remap of Caps Lock; invalid entries are skipped.
    pub fn key_rules(&self) -> Vec<(u32, Target)> {
        let mut rules = Vec::new();
        if let Some(t) = self.caps_lock.target() {
            rules.push((keys::VK_CAPITAL, t));
        }
        for r in &self.remaps {
            let (Some(from), Ok(to)) = (keys::by_name(&r.from), Target::parse(&r.to)) else { continue };
            if !rules.iter().any(|(f, _)| *f == from) {
                rules.push((from, to));
            }
        }
        rules
    }

    /// Serializes to a commented TOML file.
    pub fn to_toml(&self) -> String {
        let mut out = self.base_toml();
        out.push_str(
            "\n# Caps Lock: \"default\", \"hyper\" (Ctrl+Shift+Alt), \"ctrl\", \"escape\" or \"off\"\n",
        );
        out.push_str(&format!("caps_lock = \"{}\"\n", self.caps_lock.as_str()));
        out.push_str(
            "\n# App hotkeys: focus the app if it's running (press again to cycle its windows),\n\
             # otherwise launch it. `app` is the app's id from the Start menu's app list.\n",
        );
        for h in &self.app_hotkeys {
            out.push_str(&format!(
                "[[app_hotkeys]]\nhotkey = {:?}\napp = {:?}\nname = {:?}\n\n",
                h.hotkey, h.app, h.name
            ));
        }
        out.push_str(
            "\n# Key remaps. `from` is one key (e.g. \"Numpad5\", \"RAlt\", \"F13\");\n\
             # `to` is a key, a shortcut (\"Ctrl+C\"), a media key (\"MediaPlayPause\",\n\
             # \"MediaNext\", \"MediaPrevious\", \"VolumeUp\", \"VolumeDown\", \"VolumeMute\"),\n\
             # \"Hyper\" or \"Disabled\".\n",
        );
        for r in &self.remaps {
            out.push_str(&format!("[[remaps]]\nfrom = {:?}\nto = {:?}\n\n", r.from, r.to));
        }
        out
    }

    fn base_toml(&self) -> String {
        let accents: Vec<&str> = Accent::ALL.iter().map(|a| a.as_str()).collect();
        let disabled: Vec<String> = self.disabled_extensions.iter().map(|d| format!("{d:?}")).collect();
        format!(
            r#"# SIGMA5 settings. Easiest to change from SIGMA5 Settings (Ctrl+, in the launcher);
# edits here also work and apply the next time the launcher opens.

# Global shortcut to open/close SIGMA5.
# Modifiers: Ctrl, Alt, Shift, Win. Keys: A-Z, 0-9, F1-F24, Space, Enter, Tab, `
hotkey = {hotkey:?}

# Optional shortcut that opens Clipboard History directly, e.g. "Ctrl+Shift+V" (empty = none).
clipboard_hotkey = {clip_hotkey:?}

# "system", "dark" or "light"
theme = "{theme}"

# One of: {accents}
accent = "{accent}"

# Launcher size: "compact", "default" or "large"
size = "{size}"

# Tap the Windows key to open SIGMA5 instead of Start (Ctrl+Esc still opens Start).
windows_key = {wk}

# Launch SIGMA5 automatically when you sign in to Windows.
start_with_windows = {sww}

# Check for new versions in the background (about once every 12 hours).
auto_update = {au}

# Reopening within this many seconds keeps your last search (0 = never).
keep_query_secs = {kq}

# How many recently used items to show when the search box is empty.
recent_count = {rc}

# Extensions to turn off, e.g. ["commands"]
disabled_extensions = [{disabled}]
"#,
            hotkey = self.hotkey,
            clip_hotkey = self.clipboard_hotkey,
            theme = self.theme.as_str(),
            accents = accents.join(", "),
            accent = self.accent.as_str(),
            size = self.size.as_str(),
            wk = self.windows_key,
            sww = self.start_with_windows,
            au = self.auto_update,
            kq = self.keep_query_secs,
            rc = self.recent_count,
            disabled = disabled.join(", "),
        )
    }
}

pub fn config_path() -> PathBuf {
    data_dir().join("config.toml")
}

fn mtime(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

pub struct ConfigStore {
    pub config: Config,
    /// Last parse error, surfaced in the launcher footer.
    pub error: Option<String>,
    path: PathBuf,
    loaded_mtime: Option<SystemTime>,
}

impl ConfigStore {
    pub fn load() -> Self {
        let path = config_path();
        if !path.exists() {
            let _ = std::fs::write(&path, Config::default().to_toml());
        }
        let mut s = Self {
            config: Config::default(),
            error: None,
            path,
            loaded_mtime: None,
        };
        s.reload();
        s
    }

    /// Re-reads the file unconditionally.
    pub fn reload(&mut self) {
        self.loaded_mtime = mtime(&self.path);
        match std::fs::read_to_string(&self.path) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(c) => {
                    self.config = c;
                    self.error = None;
                }
                Err(e) => {
                    let msg = e.message().to_string();
                    self.error = Some(format!("config.toml: {msg}"));
                }
            },
            Err(e) => self.error = Some(format!("config.toml: {e}")),
        }
    }

    /// Re-reads the file if it was modified. Returns true if settings were reloaded.
    pub fn reload_if_changed(&mut self) -> bool {
        if mtime(&self.path) != self.loaded_mtime {
            self.reload();
            true
        } else {
            false
        }
    }

    /// Writes the current settings to disk.
    pub fn save(&mut self) {
        let _ = std::fs::write(&self.path, self.config.to_toml());
        self.loaded_mtime = mtime(&self.path);
        self.error = None;
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_round_trip() {
        let mut c = Config::default();
        c.hotkey = "Ctrl+Shift+K".into();
        c.clipboard_hotkey = "Ctrl+Alt+Shift+V".into();
        c.theme = ThemeMode::Light;
        c.accent = Accent::Teal;
        c.size = UiSize::Large;
        c.start_with_windows = true;
        c.windows_key = true;
        c.auto_update = false;
        c.keep_query_secs = 120;
        c.recent_count = 3;
        c.disabled_extensions = vec!["commands".into()];
        c.caps_lock = CapsMode::Hyper;
        c.app_hotkeys = vec![
            AppHotkey { hotkey: "Ctrl+Alt+Shift+C".into(), app: "Chrome".into(), name: "Google Chrome".into() },
            AppHotkey {
                hotkey: "Ctrl+Alt+Shift+I".into(),
                app: "{6D809377-6AF0-444B-8957-A3773F02200E}\\Autodesk\\Inventor 2027\\Bin\\Inventor.exe".into(),
                name: "Autodesk \"Inventor\" 2027".into(),
            },
        ];
        c.remaps = vec![
            Remap { from: "Numpad5".into(), to: "MediaPlayPause".into() },
            Remap { from: "\\".into(), to: "Disabled".into() },
        ];
        let back: Config = toml::from_str(&c.to_toml()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn key_rules_caps_wins_and_invalid_skipped() {
        let mut c = Config::default();
        c.caps_lock = CapsMode::Hyper;
        c.remaps = vec![
            Remap { from: "CapsLock".into(), to: "Escape".into() },
            Remap { from: "Numpad8".into(), to: "VolumeUp".into() },
            Remap { from: "NotAKey".into(), to: "A".into() },
            Remap { from: "Numpad2".into(), to: "Nope+X".into() },
        ];
        let rules = c.key_rules();
        assert_eq!(rules, vec![(keys::VK_CAPITAL, Target::HYPER), (0x68, Target::Keys { mods: 0, key: Some(0xAF) })]);
        c.caps_lock = CapsMode::Default;
        assert_eq!(c.key_rules()[0], (keys::VK_CAPITAL, Target::Keys { mods: 0, key: Some(0x1B) }));
    }

    #[test]
    fn missing_fields_use_defaults() {
        let c: Config = toml::from_str("hotkey = \"Alt+F1\"").unwrap();
        assert_eq!(c.hotkey, "Alt+F1");
        assert_eq!(c.accent, Accent::Violet);
        assert_eq!(c.size, UiSize::Default);
    }
}
