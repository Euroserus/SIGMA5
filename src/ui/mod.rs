//! Native Win32 UI: one pre-created popup window that is shown/hidden on the
//! global hotkey, so opening it is instant.

pub mod draw;
pub mod hotkey;
pub mod icons;
pub mod input;
pub mod installer;
pub mod paint;
pub mod settings;
pub mod theme;
pub mod tray;
pub mod window;

use windows::Win32::UI::WindowsAndMessaging::WM_APP;

pub const WINDOW_CLASS: &str = "SIGMA5.Launcher";

/// Show the launcher (sent by a second instance or the tray).
pub const WM_APP_SHOW: u32 = WM_APP + 1;
/// Indexer finished: LPARAM = Box<IndexResult>.
pub const WM_APP_INDEX: u32 = WM_APP + 2;
/// Icon batch: LPARAM = Box<Vec<IconPixels>>.
pub const WM_APP_ICONS: u32 = WM_APP + 3;
/// Tray icon callback.
pub const WM_APP_TRAY: u32 = WM_APP + 4;
/// Search text changed (posted from EN_CHANGE to avoid re-entrancy).
pub const WM_APP_QUERY: u32 = WM_APP + 5;
/// Navigation key from the search box: WPARAM = virtual key, LPARAM = 1 if Ctrl.
pub const WM_APP_NAV: u32 = WM_APP + 6;
/// Deferred hide (e.g. after losing focus).
pub const WM_APP_HIDE: u32 = WM_APP + 7;
/// Run a tray-menu command: WPARAM = menu id.
pub const WM_APP_TRAYCMD: u32 = WM_APP + 8;
/// DPI or system theme changed; re-layout.
pub const WM_APP_RELAYOUT: u32 = WM_APP + 9;
/// Exit the app.
pub const WM_APP_QUIT: u32 = WM_APP + 10;
/// Settings window saved config.toml; reload and apply it.
pub const WM_APP_CONFIG_CHANGED: u32 = WM_APP + 11;
/// Settings window asked to clear launch history.
pub const WM_APP_CLEAR_HISTORY: u32 = WM_APP + 12;
/// The Windows key was tapped on its own (see `input`).
pub const WM_APP_WINKEY: u32 = WM_APP + 13;
/// Key captured for the settings recorder: WPARAM = key code, LPARAM = modifier bits.
pub const WM_APP_CAPTURED: u32 = WM_APP + 14;
/// Modifiers currently held while capturing: WPARAM = modifier bits.
pub const WM_APP_CAPTURE_MODS: u32 = WM_APP + 15;
/// Settings asks the launcher to let the user pick an app: WPARAM = settings HWND.
pub const WM_APP_PICK_APP: u32 = WM_APP + 16;
/// Launcher returns the picked app to settings: LPARAM = Box<(id, name)>.
pub const WM_APP_APP_PICKED: u32 = WM_APP + 17;
/// Settings: show a deferred popup menu (avoids holding state during the menu loop).
pub const WM_APP_SETTINGS_MENU: u32 = WM_APP + 18;
/// WinGet query finished: LPARAM = Box<WingetResult>.
pub const WM_APP_WINGET: u32 = WM_APP + 19;
/// Update check / download progress: LPARAM = Box<UpdateEvent>.
pub const WM_APP_UPDATE: u32 = WM_APP + 20;
/// Settings asks the launcher to check for updates now.
pub const WM_APP_CHECK_UPDATE: u32 = WM_APP + 21;
