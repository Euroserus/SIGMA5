//! The launcher window: state, window procedure, input handling.
//!
//! Re-entrancy rule: Win32 calls that synchronously send messages back to this
//! window (ShowWindow, DestroyWindow, modal menus, ...) are either made where
//! the state is not borrowed, or the resulting messages are handled by posting
//! a follow-up message instead of touching the state.

use std::cell::Cell;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND,
    DWMWCP_ROUNDSMALL,
    DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, CreateSolidBrush, DeleteObject, GetDC, HGDIOBJ, MonitorFromWindow, GetMonitorInfoW, GetTextMetricsW, HBRUSH, HDC, HFONT,
    InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, ReleaseDC, SelectObject,
    SetBkColor, SetTextColor, TEXTMETRICW,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    EM_GETRECT, EM_GETSEL, EM_POSFROMCHAR, EM_REPLACESEL, EM_SETCUEBANNER, EM_SETLIMITTEXT, EM_SETMARGINS, EM_SETSEL,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_NEXT, VK_PRIOR,
    VK_DELETE, VK_RETURN, VK_SHIFT, VK_TAB, VK_UP,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use super::icons::{self, IconPixels};
use super::draw::BackBuffer;
use super::paint::{self, Fonts};
use super::settings;
use super::input;
use super::{
    WM_APP_APP_PICKED, WM_APP_CHECK_UPDATE, WM_APP_CLEAR_HISTORY, WM_APP_CONFIG_CHANGED, WM_APP_DRAG_BEGIN,
    WM_APP_COLOR_PICKED, WM_APP_DRAG_END, WM_APP_SYSINFO, WM_APP_FIDGET_CHAR, WM_APP_FIDGET_FRAME, WM_APP_FILES, WM_APP_PICK_APP, WM_APP_UPDATE, WM_APP_WINGET, WM_APP_WINKEY,
};
use super::fidget::{Fidget, Pacer, Shake};
use super::theme::{self, Theme, metrics as m};
use super::{hotkey, tray};
use super::{
    WINDOW_CLASS, WM_APP_HIDE, WM_APP_ICONS, WM_APP_INDEX, WM_APP_NAV, WM_APP_QUERY, WM_APP_QUIT,
    WM_APP_RELAYOUT, WM_APP_SHOW, WM_APP_TRAY, WM_APP_TRAYCMD,
};
use crate::config::ConfigStore;
use crate::files::{FileIndex, Recent};
use crate::fuzzy::Query;
use crate::history::History;
use crate::launch;
use crate::providers::apps::{IndexResult, spawn_indexer};
use crate::providers::{Action, Command, Row};
use crate::util::{SendHwnd, data_dir, wide};

const REINDEX_AFTER: Duration = Duration::from_secs(5 * 60);
/// The file index is refreshed in the background when older than this.
const REFILE_AFTER: Duration = Duration::from_secs(10 * 60);
/// Recent files shown on the Files page.
const RECENT_FILES: usize = 40;

/// Posted by File Search worker threads.
pub enum FilesMsg {
    Index(Box<FileIndex>, Vec<String>),
    Recent(Vec<Recent>),
}

/// A result row for a file or folder.
fn file_item(path: &std::path::Path, name: &str, is_dir: bool) -> crate::providers::Item {
    let ext = path.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
    crate::providers::Item {
        key: format!("file:{}", path.display()),
        title: name.to_string(),
        subtitle: crate::files::display_dir(path),
        accessory: if is_dir { "Folder".into() } else if ext.is_empty() { "File".into() } else { ext },
        icon: crate::providers::Icon::Glyph(crate::files::glyph_for(path, is_dir)),
        action: Action::OpenFile { path: path.to_string_lossy().to_string(), is_dir },
        score: 0,
    }
}

fn post_files(hwnd: SendHwnd, msg: FilesMsg) {
    let raw = Box::into_raw(Box::new(msg));
    unsafe {
        if PostMessageW(Some(hwnd.get()), WM_APP_FILES, WPARAM(0), LPARAM(raw as isize)).is_err() {
            drop(Box::from_raw(raw));
        }
    }
}
/// Drives the launcher's pulse animations at ~60 fps.
const PULSE_TIMER: usize = 7;
/// Hides the launcher after a launch flash.
const FLASH_TIMER: usize = 13;
/// How long a launched row flashes before the launcher hides.
const FLASH_MS: u32 = 140;
const STATUS_TIMER: usize = 8;
/// Redraws the top bar's clock once a second while the launcher is open.
const CLOCK_TIMER: usize = 12;
const EDIT_ID: usize = 100;
/// Typed letters start again from the left after this long without typing.
const FIDGET_CARET_RESET: Duration = Duration::from_millis(1500);
const FIDGET_STATUS: &str = "Fidget mode: type to make pixels · Enter to scatter · Esc to stop";

/// The launcher being dragged by the mouse.
struct Drag {
    /// Cursor position minus window position when the drag started.
    grab: (i32, i32),
    started: Instant,
    shake: Shake,
}

thread_local! {
    /// Fidget mode is running: the search box hands typed characters to the
    /// launcher instead of inserting them.
    static FIDGET_ON: Cell<bool> = const { Cell::new(false) };

    /// Search box colours, read during WM_CTLCOLOREDIT without touching state.
    static EDIT_STYLE: Cell<(HBRUSH, COLORREF, COLORREF)> =
        const { Cell::new((HBRUSH(std::ptr::null_mut()), COLORREF(0), COLORREF(0))) };
}

pub struct Launcher {
    pub hwnd: HWND,
    pub edit: HWND,
    pub dpi: u32,
    /// Launcher size in percent (from settings).
    zoom: u32,
    pub theme: Theme,
    pub fonts: Fonts,
    pub logo: Option<HICON>,
    pub backbuf: BackBuffer,

    config: ConfigStore,
    /// Footer message: (text, is_warning, expires).
    status: Option<(String, bool, Option<Instant>)>,
    history: History,
    registry: crate::providers::Registry,

    pub rows: Vec<Row>,
    /// Content-space y of each row (physical px), plus total content height.
    pub offsets: Vec<i32>,
    pub content_h: i32,
    pub selected: Option<usize>,
    pub scroll: i32,

    query: String,
    indexing: bool,
    last_index: Option<Instant>,
    hidden_at: Option<Instant>,
    reset_on_show: bool,
    visible: bool,
    last_mouse: (i32, i32),
    press_row: Option<usize>,
    edit_brush: HBRUSH,
    taskbar_created: u32,
    /// Settings window waiting for the user to pick an app (pick mode).
    pick_for: Option<HWND>,
    /// Hotkeys another app owns, handed to the keyboard hook: (mods, key, id).
    taken: Vec<(u8, u32, i32)>,
    /// Number of app hotkeys currently registered (ids from APP_HOTKEY_BASE).
    app_hotkeys_registered: usize,
    /// Calculator answer left in the search box after Enter (shown until edited).
    calc_hold: Option<crate::calc::Calc>,
    pulse_start: Option<Instant>,

    /// Which page the launcher shows (root search, WinGet, clipboard).
    pub mode: Mode,
    /// Root search text to restore when going back.
    root_query: String,
    winget: WingetState,
    pub clip: crate::clipboard::History,
    clip_listening: bool,
    /// Shown in the list area when there are no rows (loading, errors, hints).
    pub list_message: Option<String>,
    /// Window that was active before the launcher opened (paste target).
    prev_foreground: HWND,
    /// A newer release found by the update check.
    update: Option<crate::update::UpdateInfo>,
    /// An update check or download is in flight.
    update_busy: bool,
    drag: Option<Drag>,
    /// Fidget mode (pixels orbiting the cursor), while it runs.
    fidget: Option<Box<Fidget>>,
    /// Appearance the fonts were last built for.
    fonts_look: crate::config::Appearance,
    /// Paces fidget frames to the monitor's refresh rate.
    pacer: Option<Pacer>,
    /// File Search index (built in the background) and Windows' recent files.
    files: Option<Box<FileIndex>>,
    files_indexing: bool,
    files_built: Option<Instant>,
    /// The extra folders the current index was asked to cover.
    files_folders: Vec<String>,
    recent_files: Vec<Recent>,
    /// Top bar readings and the thread that takes them (while it's shown).
    pub sysinfo: crate::sysinfo::Snapshot,
    sampler: Option<crate::sysinfo::Sampler>,
    /// Picked colours, newest first.
    colors: crate::colors::History,
    /// Row the "copied" pulse is playing on (calculator card or a list row).
    pub pulse_row: Option<usize>,
    /// Where the next typed letter appears (search box x) and when the last one was typed.
    fidget_caret: (i32, Option<Instant>),
}

/// Posted by update worker threads.
pub enum UpdateEvent {
    Checked { manual: bool, result: Result<Option<crate::update::UpdateInfo>, String> },
    Progress(u8),
    Downloaded(Result<std::path::PathBuf, String>),
}

const UPDATE_TIMER: usize = 10;
/// First check shortly after startup, then every 12 hours.
const UPDATE_FIRST_MS: u32 = 30_000;
const UPDATE_EVERY_MS: u32 = 12 * 60 * 60 * 1000;

/// The launcher's pages. Sub-pages have their own search box contents and
/// return to the root with Esc (or Backspace on an empty box).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Root,
    Winget(crate::winget::Query),
    Clipboard,
    Files,
    Colors,
    Suggest,
}

impl Mode {
    pub fn title(self) -> &'static str {
        match self {
            Mode::Root => "SIGMA5",
            Mode::Winget(crate::winget::Query::Search) => "WinGet · Search",
            Mode::Winget(crate::winget::Query::Installed) => "WinGet · Installed",
            Mode::Winget(crate::winget::Query::Upgradable) => "WinGet · Upgradable",
            Mode::Clipboard => "Clipboard History",
            Mode::Files => "Files",
            Mode::Colors => "Colours",
            Mode::Suggest => "Suggest a Feature",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Mode::Root => "Search for apps and commands…",
            Mode::Winget(crate::winget::Query::Search) => "Search WinGet packages…",
            Mode::Winget(_) => "Filter packages…",
            Mode::Clipboard => "Search clipboard history…",
            Mode::Files => "Search files and folders…",
            Mode::Colors => "Search picked colours…",
            Mode::Suggest => "Describe a feature you'd like…",
        }
    }
}

#[derive(Default)]
struct WingetState {
    packages: Vec<crate::winget::Package>,
    loading: bool,
    error: Option<String>,
    /// Bumped per query so stale results are ignored.
    generation: u64,
    /// Search text the current results are for.
    searched: String,
}

/// Result posted by a WinGet worker thread.
pub struct WingetResult {
    generation: u64,
    result: Result<Vec<crate::winget::Package>, String>,
}

const WINGET_TIMER: usize = 9;

impl Launcher {
    // ----- geometry -------------------------------------------------------

    /// Monitor DPI scaled by the "Launcher size" setting.
    fn eff_dpi(&self) -> u32 {
        self.dpi * self.zoom / 100
    }

    pub fn s(&self, v: i32) -> i32 {
        (v * self.eff_dpi() as i32 + 48) / 96
    }

    /// The top bar's settings, if it is shown.
    pub fn top_bar(&self) -> Option<&crate::config::TopBar> {
        let c = &self.config.config;
        (c.module_enabled("topbar") && !c.top_bar.items.is_empty()).then_some(&c.top_bar)
    }

    /// Height of the top bar (0 when it's off).
    pub fn top_h(&self) -> i32 {
        if self.top_bar().is_some() { self.s(m::TOPBAR_H) } else { 0 }
    }

    /// Launcher window size for the current settings.
    fn window_size(&self) -> (i32, i32) {
        let a = self.config.config.appearance.sane();
        (self.s(a.width as i32), self.s(a.height as i32) + self.top_h())
    }

    pub fn client_size(&self) -> (i32, i32) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        (rc.right - rc.left, rc.bottom - rc.top)
    }

    /// The area rows are drawn in (the left pane in Clipboard mode).
    pub fn list_rect(&self) -> RECT {
        let (w, h) = self.client_size();
        let right = if self.mode == Mode::Clipboard { w * 41 / 100 } else { w };
        RECT { left: 0, top: self.top_h() + self.s(m::SEARCH_H) + 1, right, bottom: h - self.s(m::FOOTER_H) }
    }

    /// Clipboard preview pane (right of the list).
    pub fn preview_rect(&self) -> RECT {
        let (w, _) = self.client_size();
        let lr = self.list_rect();
        RECT { left: lr.right + 1, right: w, ..lr }
    }

    /// Back arrow in the search bar (sub-pages only).
    pub fn back_rect(&self) -> Option<RECT> {
        (self.mode != Mode::Root).then(|| {
            let cy = self.top_h() + self.s(m::SEARCH_H) / 2;
            let x = self.s(12);
            RECT { left: x, top: cy - self.s(14), right: x + self.s(28), bottom: cy + self.s(14) }
        })
    }

    /// Friendly name for a source exe ("chrome.exe" → "Google Chrome").
    pub fn source_name(&self, exe: &str) -> String {
        if exe.is_empty() {
            return "Unknown".into();
        }
        if let Some(n) = self.registry.apps.name_for_exe(exe) {
            return n.to_string();
        }
        let stem = exe.strip_suffix(".exe").unwrap_or(exe);
        let mut c = stem.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    }

    /// The clipboard entry selected in Clipboard mode.
    pub fn selected_clip(&self) -> Option<&crate::clipboard::Entry> {
        match self.selected_action()? {
            Action::Clip(id) => self.clip.get(*id),
            _ => None,
        }
    }

    /// Secondary action shown in the footer: (label, needs Shift+Enter).
    pub fn secondary_hint(&self) -> Option<&'static str> {
        match self.selected_action()? {
            Action::Clip(_) => Some("Copy"),
            Action::OpenFile { .. } => Some("Show in Folder"),
            Action::Color(_) => Some("Copy RGB"),
            Action::Update => Some("Release Notes"),
            Action::Winget { op: crate::providers::WingetOp::Manage { has_update: true } | crate::providers::WingetOp::Upgrade, .. } => {
                Some("Uninstall")
            }
            _ => None,
        }
    }

    /// Height of a result row (Appearance › Row height).
    pub fn item_h(&self) -> i32 {
        self.s(self.config.config.appearance.sane().row_height as i32)
    }

    pub fn row_height(&self, row: &Row) -> i32 {
        match row {
            Row::Header(_) => self.s(m::HEADER_H),
            Row::Item(_) => self.item_h(),
            Row::Calc(_) => self.s(m::CALC_H),
        }
    }

    fn relayout_rows(&mut self) {
        let mut y = self.s(m::LIST_PAD_Y);
        self.offsets.clear();
        for r in &self.rows {
            self.offsets.push(y);
            y += self.row_height(r);
        }
        self.content_h = y + self.s(m::LIST_PAD_Y);
    }

    fn max_scroll(&self) -> i32 {
        let lr = self.list_rect();
        (self.content_h - (lr.bottom - lr.top)).max(0)
    }

    fn hit_test(&self, x: i32, y: i32) -> Option<usize> {
        let lr = self.list_rect();
        if y < lr.top || y >= lr.bottom || x < self.s(m::PAD_X) || x >= lr.right - self.s(m::PAD_X) {
            return None;
        }
        let cy = y - lr.top + self.scroll;
        let i = self.offsets.partition_point(|&o| o <= cy).checked_sub(1)?;
        let row = self.rows.get(i)?;
        (cy < self.offsets[i] + self.row_height(row) && row.selectable()).then_some(i)
    }

    fn ensure_visible(&mut self, idx: usize) {
        let lr = self.list_rect();
        let view_h = lr.bottom - lr.top;
        let pad = self.s(m::LIST_PAD_Y);
        // Keep a section header in view when selecting the first item under it.
        let mut top = self.offsets[idx];
        if idx > 0 && matches!(self.rows[idx - 1], Row::Header(_)) {
            top = self.offsets[idx - 1];
        }
        if idx <= 1 {
            top = 0;
        }
        let bottom = self.offsets[idx] + self.row_height(&self.rows[idx]) + pad;
        if top < self.scroll {
            self.scroll = top;
        } else if bottom > self.scroll + view_h {
            self.scroll = bottom - view_h;
        }
        self.scroll = self.scroll.clamp(0, self.max_scroll());
    }

    pub fn selected_action(&self) -> Option<&Action> {
        match self.rows.get(self.selected?)? {
            Row::Item(it) => Some(&it.action),
            _ => None,
        }
    }

    /// Footer label for Enter.
    pub fn action_verb(&self) -> Option<&'static str> {
        if matches!(self.rows.get(self.selected?)?, Row::Calc(_)) {
            return Some("Copy Answer");
        }
        let a = self.selected_action()?;
        Some(if self.pick_for.is_some() { "Choose" } else { a.verb() })
    }

    /// Whether animations are on (Appearance setting and Windows' own
    /// "Animation effects").
    pub fn animations(&self) -> bool {
        if !self.config.config.appearance.animations {
            return false;
        }
        let mut on = windows::core::BOOL(1);
        unsafe {
            let _ = SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        }
        on.as_bool()
    }

    /// Keeps the animation timer running while anything is moving.
    fn kick_anim(&self) {
        unsafe {
            SetTimer(Some(self.hwnd), PULSE_TIMER, 15, None);
        }
    }

    fn animating(&self) -> bool {
        self.pulse().is_some()
    }

    /// Flashes the activated row, then hides (the launch itself isn't delayed).
    fn flash_then_hide(&mut self, idx: usize) {
        self.reset_on_show = true;
        if !self.animations() {
            self.hide();
            return;
        }
        self.pulse_row = Some(idx);
        self.pulse_start = Some(Instant::now());
        self.kick_anim();
        unsafe {
            SetTimer(Some(self.hwnd), FLASH_TIMER, FLASH_MS, None);
        }
        self.invalidate();
    }

    /// Progress of the "copied" pulse, 0 → 1, while it is running.
    pub fn pulse(&self) -> Option<f32> {
        let t = self.pulse_start?.elapsed().as_millis() as f32 / m::PULSE_MS as f32;
        (t < 1.0).then_some(t)
    }

    pub fn status(&self) -> Option<(&str, bool)> {
        match &self.status {
            Some((t, warn, exp)) if exp.is_none_or(|e| Instant::now() < e) => Some((t.as_str(), *warn)),
            _ => None,
        }
    }

    pub fn is_indexing_empty(&self) -> bool {
        self.indexing && self.registry.apps.len() == 0
    }

    pub fn query_is_empty(&self) -> bool {
        self.query.trim().is_empty()
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn set_status(&mut self, text: impl Into<String>, warn: bool, ttl: Option<Duration>) {
        self.status = Some((text.into(), warn, ttl.map(|d| Instant::now() + d)));
        self.invalidate();
    }

    // ----- setup ----------------------------------------------------------

    fn new(hwnd: HWND) -> Box<Self> {
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        let config = ConfigStore::load();
        let theme = theme::resolve(config.config.theme, config.config.accent, &config.config.appearance);
        let zoom = config.config.size.percent();
        let mut me = Box::new(Self {
            hwnd,
            edit: HWND::default(),
            dpi,
            zoom,
            theme,
            fonts: Fonts::create(dpi * zoom / 100, &config.config.appearance),
            logo: None,
            backbuf: BackBuffer::default(),
            config,
            status: None,
            history: History::load(),
            registry: crate::providers::Registry::new(),
            rows: Vec::new(),
            offsets: Vec::new(),
            content_h: 0,
            selected: None,
            scroll: 0,
            query: String::new(),
            indexing: false,
            last_index: None,
            hidden_at: None,
            reset_on_show: false,
            visible: false,
            last_mouse: (-1, -1),
            press_row: None,
            edit_brush: HBRUSH::default(),
            taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
            pick_for: None,
            app_hotkeys_registered: 0,
            taken: Vec::new(),
            calc_hold: None,
            pulse_start: None,
            mode: Mode::Root,
            root_query: String::new(),
            winget: WingetState::default(),
            clip: crate::clipboard::History::load(),
            clip_listening: false,
            list_message: None,
            prev_foreground: HWND::default(),
            update: None,
            update_busy: false,
            drag: None,
            fidget: None,
            pacer: None,
            fonts_look: Default::default(),
            files: None,
            files_indexing: false,
            files_built: None,
            files_folders: Vec::new(),
            recent_files: Vec::new(),
            sysinfo: crate::sysinfo::Snapshot::default(),
            sampler: None,
            colors: crate::colors::History::load(),
            pulse_row: None,
            fidget_caret: (0, None),
        });
        me.create_edit();
        me.apply_dpi(dpi, false);
        me.apply_theme();
        tray::add(hwnd, "SIGMA5");
        me.apply_config(true);
        me.refresh_rows(false);
        me.start_index(false);
        me.start_file_index();
        me.after_start();
        me
    }

    /// Update housekeeping at startup: clean old downloads, announce a
    /// just-installed version, and schedule the first update check.
    fn after_start(&mut self) {
        crate::update::cleanup();
        let marker = data_dir().join("last_version.txt");
        let last = std::fs::read_to_string(&marker).unwrap_or_default();
        if !last.trim().is_empty() && last.trim() != crate::install::VERSION {
            tray::notify(self.hwnd, "SIGMA5 updated", &format!("You're now on {}", crate::install::version_label()), false);
        }
        let _ = std::fs::write(&marker, crate::install::VERSION);
        unsafe {
            SetTimer(Some(self.hwnd), UPDATE_TIMER, UPDATE_FIRST_MS, None);
        }
    }

    fn on_update_timer(&mut self) {
        if super::headless() {
            return;
        }
        unsafe {
            SetTimer(Some(self.hwnd), UPDATE_TIMER, UPDATE_EVERY_MS, None);
        }
        if self.config.config.auto_update {
            self.start_update_check(false);
        }
    }

    /// Checks GitHub on a worker thread; `manual` reports "up to date" too.
    fn start_update_check(&mut self, manual: bool) {
        if self.update_busy {
            return;
        }
        self.update_busy = true;
        if manual {
            self.set_status("Checking for updates…", false, None);
        }
        let hwnd = SendHwnd::new(self.hwnd);
        let _ = std::thread::Builder::new().name("sigma5-update".into()).spawn(move || {
            post_update(hwnd, UpdateEvent::Checked { manual, result: crate::update::check() });
        });
    }

    fn start_update_download(&mut self) {
        let Some(info) = self.update.clone() else { return };
        if self.update_busy {
            return;
        }
        self.update_busy = true;
        self.set_status(format!("Downloading {}…", info.label()), false, None);
        let hwnd = SendHwnd::new(self.hwnd);
        let _ = std::thread::Builder::new().name("sigma5-update".into()).spawn(move || {
            let mut last = 255u8;
            let result = crate::update::download(&info, |got, total| {
                let pct = if total > 0 { (got * 100 / total) as u8 } else { 0 };
                if pct != last && pct % 5 == 0 {
                    last = pct;
                    post_update(hwnd, UpdateEvent::Progress(pct));
                }
            });
            post_update(hwnd, UpdateEvent::Downloaded(result));
        });
    }

    fn on_update_event(&mut self, ev: UpdateEvent) {
        let tell = |me: &mut Self, title: &str, body: &str, warn: bool| {
            me.set_status(body.to_string(), warn, Some(Duration::from_secs(6)));
            unsafe {
                SetTimer(Some(me.hwnd), STATUS_TIMER, 6100, None);
            }
            if !me.visible {
                tray::notify(me.hwnd, title, body, warn);
            }
        };
        match ev {
            UpdateEvent::Checked { manual, result } => {
                self.update_busy = false;
                match result {
                    Ok(Some(info)) => {
                        let new = self.update.as_ref() != Some(&info);
                        let msg = format!("SIGMA5 {} is available", info.label());
                        self.update = Some(info);
                        if new || manual {
                            tell(self, "Update available", &format!("{msg}. Open SIGMA5 to update."), false);
                        }
                        self.refresh_rows(true);
                    }
                    Ok(None) if manual => {
                        let msg = format!("You're up to date: {}", crate::install::version_label());
                        tell(self, "SIGMA5", &msg, false);
                    }
                    Err(e) if manual => tell(self, "Update check failed", &e, true),
                    _ => {}
                }
            }
            UpdateEvent::Progress(p) => {
                let label = self.update.as_ref().map(|u| u.label()).unwrap_or_default();
                self.set_status(format!("Downloading {label}… {p}%"), false, None);
            }
            UpdateEvent::Downloaded(Ok(path)) => {
                self.set_status("Installing update…", false, None);
                if let Err(e) = crate::update::apply(&path) {
                    self.update_busy = false;
                    tell(self, "Update failed", &e, true);
                }
                // On success the new version's installer closes this copy.
            }
            UpdateEvent::Downloaded(Err(e)) => {
                self.update_busy = false;
                tell(self, "Update failed", &e, true);
            }
        }
    }

    fn create_edit(&mut self) {
        unsafe {
            let hinst = GetModuleHandleW(None).ok();
            let edit = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("EDIT"),
                PCWSTR::null(),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                0,
                0,
                10,
                10,
                Some(self.hwnd),
                Some(HMENU(EDIT_ID as *mut _)),
                hinst.map(Into::into),
                None,
            )
            .expect("create search box");
            let cue = wide("Search for apps and commands…");
            SendMessageW(edit, EM_SETCUEBANNER, Some(WPARAM(1)), Some(LPARAM(cue.as_ptr() as isize)));
            SendMessageW(edit, EM_SETMARGINS, Some(WPARAM((EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize)), Some(LPARAM(0)));
            SendMessageW(edit, EM_SETLIMITTEXT, Some(WPARAM(512)), Some(LPARAM(0)));
            let _ = SetWindowSubclass(edit, Some(edit_proc), 1, 0);
            self.edit = edit;
        }
    }

    fn apply_dpi(&mut self, dpi: u32, reload_icons: bool) {
        self.dpi = dpi;
        self.fonts.destroy();
        self.fonts = Fonts::create(self.eff_dpi(), &self.config.config.appearance);
        if let Some(old) = self.logo.take() {
            unsafe {
                let _ = DestroyIcon(old);
            }
        }
        self.logo = tray::load_app_icon(self.s(16));
        unsafe {
            SendMessageW(self.edit, WM_SETFONT, Some(WPARAM(self.fonts.search.0 as usize)), Some(LPARAM(1)));
            // Left margin keeps the caret from overlapping the first glyph.
            let margin = self.s(3) as isize;
            SendMessageW(self.edit, EM_SETMARGINS, Some(WPARAM((EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize)), Some(LPARAM(margin)));
        }
        self.layout_edit();
        self.relayout_rows();
        if reload_icons && !self.indexing {
            icons::with(|c| c.clear());
            self.start_index(true);
        }
    }

    fn layout_edit(&self) {
        let (w, _) = self.client_size();
        let w = if w <= 0 { self.window_size().0 } else { w };
        let th = text_height(self.fonts.search);
        let sub = self.mode != Mode::Root;
        // Sub-pages: back arrow on the left, page title on the right.
        let x = self.s(m::TEXT_X) - self.s(3) + if sub { self.s(30) } else { 0 };
        let right_pad = if sub { self.s(190) } else { self.s(m::TEXT_X) - self.s(3) };
        let y = self.top_h() + (self.s(m::SEARCH_H) - th) / 2;
        unsafe {
            let _ = SetWindowPos(self.edit, None, x, y, w - x - right_pad, th, SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }

    fn apply_theme(&mut self) {
        let t = self.theme;
        unsafe {
            if !self.edit_brush.is_invalid() {
                let _ = DeleteObject(self.edit_brush.into());
            }
            self.edit_brush = CreateSolidBrush(t.bg);
            EDIT_STYLE.with(|c| c.set((self.edit_brush, t.bg, t.text)));
            let dark: i32 = t.dark as i32;
            let _ = DwmSetWindowAttribute(self.hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const _, 4);
            // Square corners unless a radius is chosen.
            let corner = match t.radius {
                0 => DWMWCP_DONOTROUND,
                1..=5 => DWMWCP_ROUNDSMALL,
                _ => DWMWCP_ROUND,
            };
            let _ = DwmSetWindowAttribute(self.hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const _, 4);
            // Windows' own 1 px border takes the frame's colour so they blend.
            let border = if t.frame_w > 0 { t.frame } else { t.border };
            let _ = DwmSetWindowAttribute(self.hwnd, DWMWA_BORDER_COLOR, &border as *const _ as *const _, 4);
            let _ = InvalidateRect(Some(self.edit), None, true);
        }
        self.invalidate();
    }

    /// Applies every setting from `self.config` (startup, file change, or the
    /// Settings window).
    fn apply_config(&mut self, first: bool) {
        let cfg = self.config.config.clone();
        launch::set_start_with_windows(cfg.start_with_windows);
        self.taken.clear();
        self.register_app_hotkeys(&cfg);
        self.registry.disabled = cfg.disabled_modules.clone();
        // File Search: free the index when off; rebuild when its folders change.
        if !cfg.module_enabled("files") {
            self.files = None;
            self.files_built = None;
        } else if !first && (self.files.is_none() || self.files_folders != cfg.file_folders) {
            self.files_built = None;
            self.start_file_index();
        }
        self.files_folders = cfg.file_folders.clone();
        self.update_sampler();
        if !first && self.visible {
            // The top bar may have appeared or gone: resize and re-lay out.
            let (w, h) = self.window_size();
            unsafe {
                let _ = SetWindowPos(self.hwnd, None, 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
            self.layout_edit();
            self.relayout_rows();
        }

        // Clipboard history: listen only while the module is on.
        let want_clip = cfg.module_enabled("clipboard") && !super::headless();
        unsafe {
            use windows::Win32::System::DataExchange::{AddClipboardFormatListener, RemoveClipboardFormatListener};
            if want_clip && !self.clip_listening {
                self.clip_listening = AddClipboardFormatListener(self.hwnd).is_ok();
            } else if !want_clip && self.clip_listening {
                let _ = RemoveClipboardFormatListener(self.hwnd);
                self.clip_listening = false;
            }
        }
        hotkey::unregister(self.hwnd, hotkey::CLIPBOARD_HOTKEY_ID);
        if want_clip && !cfg.clipboard_hotkey.trim().is_empty() {
            if let Err(e) = self.grab_hotkey(hotkey::CLIPBOARD_HOTKEY_ID, &cfg.clipboard_hotkey) {
                self.set_status(format!("Clipboard hotkey: {e}"), true, None);
            }
        }

        let t = theme::resolve(cfg.theme, cfg.accent, &cfg.appearance);
        if t != self.theme || first {
            self.theme = t;
            self.apply_theme();
        }
        // New font or text size: rebuild fonts and re-lay out.
        let (old, new) = (&self.fonts_look, &cfg.appearance);
        if !first && (old.font != new.font || old.search_font != new.search_font || old.font_scale != new.font_scale) {
            self.apply_dpi(self.dpi, false);
        }
        self.fonts_look = cfg.appearance.clone();
        let zoom = cfg.size.percent();
        if zoom != self.zoom {
            self.zoom = zoom;
            self.apply_dpi(self.dpi, true);
        }

        self.status = None;
        match self.grab_hotkey(hotkey::HOTKEY_ID, &cfg.hotkey) {
            Ok(()) => tray::set_tip(self.hwnd, &format!("SIGMA5 ({})", cfg.hotkey)),
            Err(e) => {
                if first {
                    tray::notify(self.hwnd, "SIGMA5 hotkey unavailable", &e, true);
                }
                self.set_status(e, true, None);
            }
        }
        // The keyboard hook: remaps, Windows key, and hotkeys taken over above.
        input::configure(self.hwnd, input::Rules { win_tap: cfg.windows_key, remaps: cfg.key_rules(), hotkeys: self.taken.clone() });
        if let Some(err) = self.config.error.clone() {
            self.set_status(err, true, None);
        }
        if !first {
            self.refresh_rows(true);
        }
    }

    /// (Re)registers every app hotkey; failures are shown in the footer.
    fn register_app_hotkeys(&mut self, cfg: &crate::config::Config) {
        for i in 0..self.app_hotkeys_registered {
            hotkey::unregister(self.hwnd, hotkey::APP_HOTKEY_BASE + i as i32);
        }
        let mut failed = Vec::new();
        for (i, b) in cfg.app_hotkeys.iter().enumerate() {
            if b.hotkey.trim().is_empty() {
                continue;
            }
            if let Err(e) = self.grab_hotkey(hotkey::APP_HOTKEY_BASE + i as i32, &b.hotkey) {
                failed.push(format!("{} ({e})", b.name));
            }
        }
        self.app_hotkeys_registered = cfg.app_hotkeys.len();
        if !failed.is_empty() {
            self.set_status(format!("Invalid hotkey: {}", failed.join(", ")), true, None);
        }
    }

    /// Registers a hotkey. If another app already owns it, the keyboard hook
    /// takes it over instead (it sees keys before Windows dispatches hotkeys).
    fn grab_hotkey(&mut self, id: i32, spec: &str) -> Result<(), String> {
        let (mods, vk) = hotkey::parse(spec)?;
        if hotkey::register_id(self.hwnd, id, spec).is_err() {
            self.taken.push((mods, vk, id));
        }
        Ok(())
    }

    fn on_app_hotkey(&mut self, index: usize) {
        let Some(b) = self.config.config.app_hotkeys.get(index) else { return };
        // A SIGMA5 or module command bound to a hotkey.
        if let Some((module, _, _, cmd)) = crate::providers::commands::find(&b.app) {
            if self.config.config.module_enabled(module) {
                self.run_command_hotkey(cmd);
            }
            return;
        }
        let exe = self
            .registry
            .apps
            .find(&b.app)
            .and_then(|a| a.exe.clone())
            .or_else(|| b.app.rsplit('\\').next().map(str::to_ascii_lowercase).filter(|e| e.ends_with(".exe")));
        crate::focus::focus_or_launch(&b.app, exe.as_deref());
    }

    /// Runs a command from its hotkey. Commands that open a page show the
    /// launcher on it (or close it if that page is already in front).
    fn run_command_hotkey(&mut self, c: Command) {
        let page = match c {
            Command::SearchFiles => Some(Mode::Files),
            Command::SuggestFeature => Some(Mode::Suggest),
            Command::ColorHistory => Some(Mode::Colors),
            Command::ClipboardHistory => Some(Mode::Clipboard),
            Command::WingetSearch => Some(Mode::Winget(crate::winget::Query::Search)),
            Command::WingetInstalled => Some(Mode::Winget(crate::winget::Query::Installed)),
            Command::WingetUpgradable => Some(Mode::Winget(crate::winget::Query::Upgradable)),
            _ => None,
        };
        if let Some(mode) = page {
            let fg = unsafe { GetForegroundWindow() };
            if self.visible && fg == self.hwnd && self.mode == mode {
                self.hide();
                return;
            }
            if !self.visible || fg != self.hwnd {
                self.show();
            }
            if self.mode == mode {
                return;
            }
            if self.mode != Mode::Root {
                self.go_back();
            }
        }
        self.run_command(c);
    }

    /// Shows the launcher so the user can pick an app for the Settings window.
    fn start_pick(&mut self, settings: HWND) {
        self.reset_on_show = true;
        self.hidden_at = None;
        self.pick_for = Some(settings);
        self.show();
        self.set_status("Choose an app or command for the new hotkey", false, None);
        self.refresh_rows(false);
    }

    fn on_config_changed(&mut self) {
        self.config.reload();
        self.apply_config(false);
    }

    fn open_settings(&mut self) {
        // Open first (while we still own the foreground), then hide.
        settings::open(self.hwnd);
        self.hide();
    }

    // ----- data -------------------------------------------------------------

    fn start_index(&mut self, reload_icons: bool) {
        if self.indexing {
            return;
        }
        self.indexing = true;
        let have = if reload_icons { Default::default() } else { icons::with(|c| c.ids()) };
        spawn_indexer(SendHwnd::new(self.hwnd), self.s(m::ICON), have);
    }

    fn on_index(&mut self, res: IndexResult) {
        let n = res.apps.len();
        self.registry.apps.set_entries(res.apps);
        self.indexing = false;
        self.last_index = Some(Instant::now());
        tray::set_tip(self.hwnd, &format!("SIGMA5 ({}) · {n} apps", self.config.config.hotkey));
        if matches!(&self.status, Some((t, false, _)) if t.starts_with("Reloading")) {
            self.set_status(format!("Found {n} applications"), false, Some(Duration::from_secs(4)));
        }
        // Only preserve the selection if the user moved it off the top result.
        let first = self.rows.iter().position(Row::selectable);
        let user_moved = self.visible && self.selected != first;
        self.refresh_rows(user_moved);
    }

    fn on_icons(&mut self, batch: Vec<IconPixels>) {
        icons::with(|c| batch.into_iter().for_each(|ic| c.insert(ic)));
        if self.visible {
            self.invalidate();
        }
    }

    fn refresh_rows(&mut self, keep_selection: bool) {
        let prev_key = keep_selection
            .then(|| self.selected.and_then(|i| match self.rows.get(i) {
                Some(Row::Item(it)) => Some(it.key.clone()),
                _ => None,
            }))
            .flatten();
        let q = Query::new(&self.query);
        self.list_message = None;
        match self.mode {
            Mode::Root => self.rows = self.registry.rows(&q, &self.history, self.config.config.recent_count),
            Mode::Winget(kind) => self.rows = self.winget_rows(kind, &q),
            Mode::Clipboard => self.rows = self.clip_rows(&q),
            Mode::Files => self.rows = self.file_rows(&q),
            Mode::Colors => self.rows = self.color_rows(&q),
            Mode::Suggest => self.rows = self.suggest_rows(),
        }
        // A typed colour (#hex, rgb(), hsl()) at the top, in every format.
        if self.mode == Mode::Root && self.pick_for.is_none() && self.config.config.module_enabled("colors") {
            if let Some(c) = crate::colors::parse(&self.query) {
                let mut rows = vec![Row::Header("Colour")];
                for (label, text) in [("HEX", c.hex()), ("RGB", c.rgb()), ("HSL", c.hsl())] {
                    rows.push(Row::Item(crate::providers::Item {
                        key: format!("colorfmt:{label}"),
                        title: text.clone(),
                        subtitle: String::new(),
                        accessory: label.into(),
                        icon: crate::providers::Icon::Swatch(c.colorref()),
                        action: Action::CopyText(text),
                        score: 0,
                    }));
                }
                self.rows.splice(0..0, rows);
            }
        }
        // A few file matches under the root results.
        if self.mode == Mode::Root && self.pick_for.is_none() && q.chars().len() >= 2 && self.config.config.module_enabled("files") {
            if let Some(idx) = &self.files {
                let hits = idx.search(&q, 4);
                if !hits.is_empty() {
                    self.rows.push(Row::Header("Files"));
                    self.rows.extend(hits.into_iter().map(|h| Row::Item(file_item(&h.path, &h.name, h.is_dir))));
                }
            }
        }
        // Update banner at the top of the root page.
        if let (Mode::Root, None, Some(info)) = (self.mode, self.pick_for, &self.update) {
            if q.is_empty() || q.score("update sigma5 new version").is_some() {
                let item = crate::providers::Item {
                    key: "update".into(),
                    title: format!("Update to SIGMA5 {}", info.label()),
                    subtitle: format!("You have {}", crate::install::version_label()),
                    accessory: "Update".into(),
                    icon: crate::providers::Icon::Glyph('\u{E896}'),
                    action: Action::Update,
                    score: 0,
                };
                self.rows.splice(0..0, [Row::Header("Update Available"), Row::Item(item)]);
            }
        }
        if self.mode != Mode::Root {
            // Sub-pages have no calculator or pick mode.
        } else if self.pick_for.is_some() {
            // Pick mode: apps and commands only, and drop headers left with no items.
            let rows = std::mem::take(&mut self.rows);
            let kept: Vec<Row> = rows
                .into_iter()
                .filter(|r| match r {
                    Row::Item(it) => matches!(it.action, Action::LaunchApp { .. } | Action::Command(_)),
                    Row::Calc(_) => false,
                    Row::Header(_) => true,
                })
                .collect();
            for (i, r) in kept.iter().enumerate() {
                let has_item = matches!(kept.get(i + 1), Some(Row::Item(_)));
                if matches!(r, Row::Item(_)) || has_item {
                    self.rows.push(r.clone());
                }
            }
        } else if self.config.config.module_enabled("calculator") {
            // A chained answer stays on screen until the text changes.
            let calc = match &self.calc_hold {
                Some(h) if h.chain == self.query.trim() => Some(h.clone()),
                _ => crate::calc::evaluate(&self.query, false),
            };
            if let Some(c) = calc {
                self.rows.splice(0..0, [Row::Header("Calculator"), Row::Calc(Box::new(c))]);
            }
        }
        self.relayout_rows();
        let first = self.rows.iter().position(Row::selectable);
        self.selected = prev_key
            .and_then(|k| self.rows.iter().position(|r| matches!(r, Row::Item(it) if it.key == k)))
            .or(first);
        if !keep_selection {
            self.scroll = 0;
        }
        if let Some(i) = self.selected {
            self.ensure_visible(i);
        }
        self.scroll = self.scroll.clamp(0, self.max_scroll());
        self.invalidate();
    }

    fn on_query_changed(&mut self) {
        let len = unsafe { GetWindowTextLengthW(self.edit) };
        let mut buf = vec![0u16; len as usize + 1];
        let n = unsafe { GetWindowTextW(self.edit, &mut buf) };
        let text = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
        if self.fidget.is_some() && !text.is_empty() {
            // Pasted (or otherwise inserted) text dissolves too.
            let (at, font) = (self.edit_char_pos(0), self.fonts.search);
            if let Some(f) = &mut self.fidget {
                f.dissolve(&text, font, at, 0, (0.0, 0.0));
            }
            self.set_search_text("");
            return;
        }
        if text != self.query {
            if self.calc_hold.as_ref().is_some_and(|h| h.chain != text.trim()) {
                self.calc_hold = None;
            }
            self.query = text;
            if self.mode == Mode::Winget(crate::winget::Query::Search) {
                // Debounce: search once typing pauses.
                unsafe {
                    SetTimer(Some(self.hwnd), WINGET_TIMER, 350, None);
                }
            }
            self.refresh_rows(false);
        }
    }

    // ----- pages: WinGet & clipboard ---------------------------------------

    /// Opens a sub-page with an empty search box.
    fn enter_mode(&mut self, mode: Mode) {
        if self.mode == Mode::Root {
            self.root_query = self.query.clone();
        }
        self.mode = mode;
        self.set_search_text("");
        self.set_placeholder(mode.placeholder());
        self.layout_edit();
        if let Mode::Winget(kind) = mode {
            self.winget = WingetState::default();
            if kind != crate::winget::Query::Search {
                self.start_winget_query(kind, String::new());
            }
        }
        self.refresh_rows(false);
    }

    /// Back to the root search, restoring what was typed there.
    fn go_back(&mut self) {
        if self.mode == Mode::Root {
            return;
        }
        self.mode = Mode::Root;
        let q = std::mem::take(&mut self.root_query);
        self.set_search_text(&q);
        self.set_placeholder(Mode::Root.placeholder());
        self.layout_edit();
        self.refresh_rows(false);
    }

    fn set_search_text(&mut self, text: &str) {
        self.query = text.to_string();
        let w = wide(text);
        unsafe {
            let _ = SetWindowTextW(self.edit, PCWSTR(w.as_ptr()));
            let end = (w.len() - 1) as isize;
            SendMessageW(self.edit, EM_SETSEL, Some(WPARAM(end as usize)), Some(LPARAM(end)));
        }
    }

    fn set_placeholder(&self, text: &str) {
        let cue = wide(text);
        unsafe {
            SendMessageW(self.edit, EM_SETCUEBANNER, Some(WPARAM(1)), Some(LPARAM(cue.as_ptr() as isize)));
        }
    }

    fn start_winget_query(&mut self, kind: crate::winget::Query, text: String) {
        self.winget.generation += 1;
        self.winget.loading = true;
        self.winget.error = None;
        self.winget.searched = text.clone();
        let generation = self.winget.generation;
        let hwnd = SendHwnd::new(self.hwnd);
        let _ = std::thread::Builder::new().name("sigma5-winget".into()).spawn(move || {
            let result = crate::winget::query(kind, &text);
            let boxed = Box::into_raw(Box::new(WingetResult { generation, result }));
            unsafe {
                if PostMessageW(Some(hwnd.get()), WM_APP_WINGET, WPARAM(0), LPARAM(boxed as isize)).is_err() {
                    drop(Box::from_raw(boxed));
                }
            }
        });
    }

    fn on_winget_result(&mut self, r: WingetResult) {
        if r.generation != self.winget.generation {
            return; // superseded by a newer query
        }
        self.winget.loading = false;
        match r.result {
            Ok(p) => self.winget.packages = p,
            Err(e) => {
                self.winget.packages.clear();
                self.winget.error = Some(e);
            }
        }
        if matches!(self.mode, Mode::Winget(_)) {
            self.refresh_rows(false);
        }
    }

    fn on_winget_timer(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), WINGET_TIMER);
        }
        let text = self.query.trim().to_string();
        if self.mode == Mode::Winget(crate::winget::Query::Search) && text != self.winget.searched {
            if text.is_empty() {
                self.winget = WingetState::default();
                self.refresh_rows(false);
            } else {
                self.start_winget_query(crate::winget::Query::Search, text);
                self.refresh_rows(false);
            }
        }
    }

    fn winget_rows(&mut self, kind: crate::winget::Query, q: &Query) -> Vec<Row> {
        use crate::providers::{Icon, Item, WingetOp};
        use crate::winget::Query as K;
        let w = &self.winget;
        let mut rows = Vec::new();
        // Search results are already filtered by winget; local lists filter here.
        let mut pkgs: Vec<(i32, &crate::winget::Package)> = w
            .packages
            .iter()
            .filter_map(|p| {
                if kind == K::Search || q.is_empty() {
                    return Some((0, p));
                }
                q.score(&p.name).max(q.score(&p.id).map(|s| s - 10)).map(|s| (s, p))
            })
            .collect();
        if kind != K::Search && !q.is_empty() {
            pkgs.sort_by(|a, b| b.0.cmp(&a.0));
        }
        let glyph = '\u{E7B8}';
        if kind == K::Upgradable && q.is_empty() && pkgs.len() > 1 {
            rows.push(Row::Header("Actions"));
            rows.push(Row::Item(Item {
                key: "winget:all".into(),
                title: format!("Upgrade All ({})", pkgs.len()),
                subtitle: "WinGet".into(),
                accessory: String::new(),
                icon: Icon::Glyph('\u{E777}'),
                action: Action::Winget { op: WingetOp::UpgradeAll, id: String::new(), name: String::new() },
                score: 0,
            }));
        }
        if !pkgs.is_empty() {
            rows.push(Row::Header(match kind {
                K::Search => "Packages",
                K::Installed => "Installed",
                K::Upgradable => "Updates Available",
            }));
        }
        for (_, p) in pkgs {
            let has_update = !p.available.is_empty();
            let op = match kind {
                K::Search => WingetOp::Install,
                K::Installed => WingetOp::Manage { has_update },
                K::Upgradable => WingetOp::Upgrade,
            };
            let accessory = if has_update { format!("{} → {}", p.version, p.available) } else { p.version.clone() };
            rows.push(Row::Item(Item {
                key: format!("winget:{}", p.id),
                title: p.name.clone(),
                subtitle: p.id.clone(),
                accessory,
                icon: Icon::Glyph(glyph),
                action: Action::Winget { op, id: p.id.clone(), name: p.name.clone() },
                score: 0,
            }));
        }
        if rows.is_empty() {
            self.list_message = Some(if let Some(e) = &w.error {
                e.clone()
            } else if w.loading {
                match kind {
                    K::Search => format!("Searching WinGet for “{}”…", w.searched),
                    K::Installed => "Loading installed packages…".into(),
                    K::Upgradable => "Checking for updates…".into(),
                }
            } else if kind == K::Search && self.query.trim().is_empty() {
                "Type to search the WinGet catalogue".into()
            } else if kind == K::Upgradable && q.is_empty() {
                "Everything is up to date".into()
            } else {
                "No packages found".into()
            });
        }
        rows
    }

    // ----- Top bar ------------------------------------------------------------

    /// Starts, retargets or stops the top bar's sampler to match the settings.
    fn update_sampler(&mut self) {
        let needs = self.top_bar().map_or(0, |tb| crate::sysinfo::needs(&tb.items));
        match (&self.sampler, needs) {
            (Some(_), 0) => {
                self.sampler = None;
                self.sysinfo = Default::default();
            }
            (Some(s), n) => s.set_needs(n),
            (None, 0) => {}
            (None, n) => {
                let s = crate::sysinfo::Sampler::start(self.hwnd, WM_APP_SYSINFO, n);
                s.set_active(self.visible);
                self.sampler = Some(s);
            }
        }
    }

    fn on_sysinfo(&mut self, snap: crate::sysinfo::Snapshot) {
        if snap != self.sysinfo {
            self.sysinfo = snap;
            if self.visible {
                let rc = RECT { left: 0, top: 0, right: self.client_size().0, bottom: self.top_h() };
                unsafe {
                    let _ = InvalidateRect(Some(self.hwnd), Some(&rc), false);
                }
            }
        }
    }

    // ----- File Search ------------------------------------------------------

    /// Builds the file index on a worker thread (no-op if already running).
    fn start_file_index(&mut self) {
        if self.files_indexing || !self.config.config.module_enabled("files") {
            return;
        }
        self.files_indexing = true;
        let folders = self.config.config.file_folders.clone();
        let hwnd = SendHwnd::new(self.hwnd);
        let _ = std::thread::Builder::new().name("sigma5-files".into()).spawn(move || {
            let idx = FileIndex::build(&crate::files::default_roots(&folders));
            post_files(hwnd, FilesMsg::Index(Box::new(idx), folders));
        });
    }

    /// Reads Windows' recent files on a worker thread.
    fn load_recent_files(&mut self) {
        let hwnd = SendHwnd::new(self.hwnd);
        let _ = std::thread::Builder::new().name("sigma5-recent".into()).spawn(move || unsafe {
            use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let r = crate::files::recent_files(RECENT_FILES);
            CoUninitialize();
            post_files(hwnd, FilesMsg::Recent(r));
        });
    }

    fn on_files_msg(&mut self, msg: FilesMsg) {
        match msg {
            FilesMsg::Index(idx, folders) => {
                self.files_indexing = false;
                if !self.config.config.module_enabled("files") {
                    return; // turned off meanwhile
                }
                self.files = Some(idx);
                self.files_built = Some(Instant::now());
                if folders != self.config.config.file_folders {
                    self.start_file_index(); // folders changed while indexing
                }
            }
            FilesMsg::Recent(r) => self.recent_files = r,
        }
        if self.visible && (self.mode == Mode::Files || !self.query.trim().is_empty()) {
            self.refresh_rows(true);
        }
    }

    fn file_rows(&mut self, q: &Query) -> Vec<Row> {
        let mut rows = Vec::new();
        if q.is_empty() {
            if !self.recent_files.is_empty() {
                rows.push(Row::Header("Recent Files"));
                rows.extend(self.recent_files.iter().map(|r| Row::Item(file_item(&r.path, &r.name, r.is_dir))));
            } else {
                let n = self.files.as_ref().map_or(0, |f| f.len());
                self.list_message = Some(if n > 0 { format!("Type to search {n} files and folders") } else { "Type to search your files".into() });
            }
            return rows;
        }
        match &self.files {
            Some(idx) => {
                let hits = idx.search(q, 60);
                if hits.is_empty() {
                    self.list_message = Some("No matching files".into());
                } else {
                    rows.push(Row::Header("Files"));
                    rows.extend(hits.into_iter().map(|h| Row::Item(file_item(&h.path, &h.name, h.is_dir))));
                }
            }
            None => self.list_message = Some("Indexing your files…".into()),
        }
        rows
    }

    // ----- Suggest a Feature ------------------------------------------------

    /// The suggestion page: what's typed becomes the suggestion.
    fn suggest_rows(&mut self) -> Vec<Row> {
        let idea = self.query.trim().to_string();
        if idea.is_empty() {
            self.list_message = Some("Type your idea, then press Enter to send it on WhatsApp".into());
            return Vec::new();
        }
        vec![
            Row::Header("Suggestion"),
            Row::Item(crate::providers::Item {
                key: "suggest".into(),
                title: idea.clone(),
                subtitle: "Opens WhatsApp with your idea typed in; press Send there".into(),
                accessory: "WhatsApp".into(),
                icon: crate::providers::Icon::Glyph('\u{E82F}'),
                action: Action::Suggest(idea),
                score: 0,
            }),
        ]
    }

    // ----- Colours -----------------------------------------------------------

    fn color_rows(&mut self, q: &Query) -> Vec<Row> {
        use crate::providers::Item;
        let mut rows = Vec::new();
        let items: Vec<Item> = self
            .colors
            .items()
            .iter()
            .filter(|p| q.is_empty() || q.score(&format!("{} {} {}", p.color.hex(), p.color.rgb(), p.color.hsl())).is_some())
            .map(|p| Item {
                key: format!("color:{}", p.color.hex()),
                title: p.color.hex(),
                subtitle: format!("{}  ·  {}", p.color.rgb(), p.color.hsl()),
                accessory: crate::util::ago(p.at),
                icon: crate::providers::Icon::Swatch(p.color.colorref()),
                action: Action::Color(p.color.colorref()),
                score: 0,
            })
            .collect();
        if q.is_empty() {
            rows.push(Row::Header("Actions"));
            if let Some(it) = crate::providers::Provider::lookup(&self.registry.colors, "cmd:PickColor") {
                rows.push(Row::Item(it));
            }
        }
        if !items.is_empty() {
            rows.push(Row::Header("Picked Colours"));
            rows.extend(items.into_iter().map(Row::Item));
        } else if !q.is_empty() {
            self.list_message = Some("No matching colours".into());
        }
        rows
    }

    fn start_color_pick(&mut self) {
        self.reset_on_show = false;
        self.hide();
        super::picker::start(self.hwnd, self.theme.accent);
    }

    /// Picker result: copy it, remember it, and show it in Colour History.
    fn on_color_picked(&mut self, colorref: u32, ok: bool) {
        if !ok {
            return;
        }
        let c = crate::colors::Color::from_colorref(colorref);
        self.colors.add(c);
        self.hidden_at = None;
        self.reset_on_show = true;
        self.show();
        self.enter_mode(Mode::Colors);
        let row = self.rows.iter().position(|r| matches!(r, Row::Item(it) if matches!(it.action, Action::Color(v) if v == colorref)));
        if let Some(i) = row {
            self.selected = Some(i);
            self.ensure_visible(i);
        }
        self.copy_with_pulse(&c.hex(), row);
    }

    /// Copies `text`, plays the pulse on `row`, and confirms in the footer.
    fn copy_with_pulse(&mut self, text: &str, row: Option<usize>) {
        if crate::util::copy_to_clipboard(self.hwnd, text) {
            self.pulse_row = row;
            self.pulse_start = Some(Instant::now());
            unsafe {
                SetTimer(Some(self.hwnd), PULSE_TIMER, 15, None);
                SetTimer(Some(self.hwnd), STATUS_TIMER, 2600, None);
            }
            self.set_status(format!("Copied {text}"), false, Some(Duration::from_millis(2500)));
        } else {
            self.set_status("Clipboard is busy; try again", true, Some(Duration::from_secs(3)));
        }
    }

    fn delete_selected_color(&mut self) {
        if let Some(Action::Color(c)) = self.selected_action().cloned() {
            self.colors.remove(crate::colors::Color::from_colorref(c));
            self.refresh_rows(true);
        }
    }

    fn clip_rows(&mut self, q: &Query) -> Vec<Row> {
        use crate::clipboard::Content;
        use crate::providers::{Icon, Item};
        let mut rows = Vec::new();
        let now = crate::util::now_unix();
        let mut section = "";
        for e in self.clip.entries() {
            if !q.is_empty() && q.score(&e.content.search_text()).is_none() {
                continue;
            }
            let age = now.saturating_sub(e.copied);
            let s = if age < 86_400 { "Today" } else if age < 2 * 86_400 { "Yesterday" } else { "Earlier" };
            if s != section {
                rows.push(Row::Header(s));
                section = s;
            }
            let glyph = match e.content {
                Content::Text(_) => '\u{E8C4}',
                Content::Files(_) => '\u{E8B7}',
                Content::Image { .. } => '\u{EB9F}',
            };
            rows.push(Row::Item(Item {
                key: format!("clip:{}", e.id),
                title: e.content.title(),
                subtitle: String::new(),
                accessory: crate::util::ago(e.copied),
                icon: Icon::Glyph(glyph),
                action: Action::Clip(e.id),
                score: 0,
            }));
        }
        if rows.is_empty() {
            self.list_message = Some(if self.clip.entries().is_empty() {
                "Copy something and it will appear here".into()
            } else {
                "No matches".into()
            });
        }
        rows
    }

    fn on_clipboard_update(&mut self) {
        if self.clip.capture(self.hwnd) && self.visible && self.mode == Mode::Clipboard {
            self.refresh_rows(true);
        }
    }

    /// Enter / Shift+Enter on a WinGet package.
    fn activate_winget(&mut self, op: crate::providers::WingetOp, id: &str, name: &str, shift: bool) {
        use crate::providers::WingetOp;
        self.allow_foreground();
        match (op, shift) {
            (WingetOp::UpgradeAll, _) => crate::winget::upgrade_all(),
            (WingetOp::Install, _) => crate::winget::install(id, name),
            (WingetOp::Manage { has_update: false }, _) | (WingetOp::Manage { .. } | WingetOp::Upgrade, true) => {
                crate::winget::uninstall(id, name)
            }
            (WingetOp::Manage { .. } | WingetOp::Upgrade, false) => crate::winget::upgrade(id, name),
        }
        self.reset_on_show = true;
        self.hide();
    }

    /// Enter pastes into the previous window; Shift+Enter only copies.
    fn activate_clip(&mut self, id: u64, shift: bool) {
        if !self.clip.restore(id, self.hwnd) {
            self.set_status("Clipboard is busy; try again", true, Some(Duration::from_secs(3)));
            return;
        }
        if shift {
            self.set_status("Copied to clipboard", false, Some(Duration::from_millis(1800)));
            unsafe {
                SetTimer(Some(self.hwnd), STATUS_TIMER, 1900, None);
            }
            return;
        }
        let target = self.prev_foreground;
        self.reset_on_show = true;
        self.hide();
        if !target.is_invalid() {
            crate::focus::activate(target);
            // Give focus a moment to land, then press Ctrl+V.
            let _ = std::thread::Builder::new().name("sigma5-paste".into()).spawn(|| {
                std::thread::sleep(Duration::from_millis(60));
                input::send_paste();
            });
        }
    }

    fn delete_selected_clip(&mut self) {
        if let Some(Action::Clip(id)) = self.selected_action().cloned() {
            self.clip.remove(id);
            self.refresh_rows(true);
        }
    }

    fn allow_foreground(&self) {
        unsafe {
            let _ = AllowSetForegroundWindow(ASFW_ANY);
        }
    }

    // ----- visibility ------------------------------------------------------

    fn show(&mut self) {
        // Remember where the user was, so clipboard entries can be pasted there.
        let fg = unsafe { GetForegroundWindow() };
        if fg != self.hwnd && !fg.is_invalid() {
            self.prev_foreground = fg;
        }
        if self.config.reload_if_changed() {
            self.apply_config(false);
        }
        // System theme/accent may have changed while hidden.
        let t = theme::resolve(self.config.config.theme, self.config.config.accent, &self.config.config.appearance);
        if t != self.theme {
            self.theme = t;
            self.apply_theme();
        }

        unsafe {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            let (mut dx, mut dy) = (96u32, 96u32);
            let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            if dx != self.dpi {
                self.apply_dpi(dx, true);
            }
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(mon, &mut mi);
            let wa = mi.rcWork;
            let (w, h) = self.window_size();
            let x = wa.left + ((wa.right - wa.left) - w) / 2;
            let y = (wa.top + ((wa.bottom - wa.top) as f32 * m::TOP_FRACTION) as i32).min(wa.bottom - h).max(wa.top);
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
        }
        self.layout_edit();

        let expired = self
            .hidden_at
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(self.config.config.keep_query_secs));
        if self.reset_on_show || expired {
            if self.mode != Mode::Root {
                self.mode = Mode::Root;
                self.root_query.clear();
                self.set_placeholder(Mode::Root.placeholder());
                self.layout_edit();
            }
            if !self.query.is_empty() {
                self.query.clear();
                unsafe {
                    let _ = SetWindowTextW(self.edit, w!(""));
                }
            }
        }
        self.reset_on_show = false;
        self.refresh_rows(false);
        unsafe {
            SendMessageW(self.edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        }

        self.visible = true;
        // Ignore the synthetic WM_MOUSEMOVE sent when the window appears under
        // a stationary cursor, so hovering doesn't steal the selection.
        unsafe {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(self.hwnd, &mut pt);
            self.last_mouse = (pt.x, pt.y);
        }
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let _ = SetForegroundWindow(self.hwnd);
            if GetForegroundWindow() != self.hwnd {
                // Foreground lock refused us (e.g. opened from the Windows-key
                // hook): briefly share input state with the foreground thread.
                let fg = GetForegroundWindow();
                let fg_thread = GetWindowThreadProcessId(fg, None);
                use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
                let me = GetCurrentThreadId();
                if fg_thread != 0 && fg_thread != me {
                    let _ = AttachThreadInput(me, fg_thread, true);
                    let _ = SetForegroundWindow(self.hwnd);
                    let _ = AttachThreadInput(me, fg_thread, false);
                }
            }
            let _ = SetFocus(Some(self.edit));
        }
        if let Some(s) = &self.sampler {
            s.set_active(true);
        }
        unsafe {
            SetTimer(Some(self.hwnd), CLOCK_TIMER, 1000, None);
        }
        if self.files_built.is_none_or(|t| t.elapsed() > REFILE_AFTER) {
            self.start_file_index();
        }
        if self.mode == Mode::Files {
            self.load_recent_files();
        }
        if !self.indexing && self.last_index.is_none_or(|t| t.elapsed() > REINDEX_AFTER) {
            self.start_index(false);
        }
    }

    fn hide(&mut self) {
        if !self.visible {
            return;
        }
        self.visible = false;
        self.hidden_at = Some(Instant::now());
        self.press_row = None;
        self.drag = None;
        self.stop_fidget();
        unsafe {
            let _ = KillTimer(Some(self.hwnd), FLASH_TIMER);
        }
        if let Some(s) = &self.sampler {
            s.set_active(false);
        }
        unsafe {
            let _ = KillTimer(Some(self.hwnd), CLOCK_TIMER);
        }
        if self.pick_for.take().is_some() {
            // Pick cancelled (or finished): back to normal mode next time.
            self.status = None;
            self.reset_on_show = true;
        }
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    fn toggle(&mut self) {
        let fg = unsafe { GetForegroundWindow() };
        if self.visible && fg == self.hwnd {
            self.hide();
        } else {
            self.show();
        }
    }

    // ----- input ----------------------------------------------------------

    fn move_selection(&mut self, delta: i32, wrap: bool) {
        let items: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.selectable())
            .map(|(i, _)| i)
            .collect();
        if items.is_empty() {
            return;
        }
        let cur = self.selected.and_then(|s| items.iter().position(|&i| i == s)).unwrap_or(0) as i32;
        let n = items.len() as i32;
        let next = if wrap { (cur + delta).rem_euclid(n) } else { (cur + delta).clamp(0, n - 1) };
        self.selected = Some(items[next as usize]);
        self.ensure_visible(items[next as usize]);
        self.invalidate();
    }

    fn on_nav(&mut self, vk: VIRTUAL_KEY, ctrl: bool, shift: bool) {
        let page = {
            let lr = self.list_rect();
            ((lr.bottom - lr.top) / self.item_h()).max(1)
        };
        if self.fidget.is_some() {
            match vk {
                VK_ESCAPE if !ctrl => return self.stop_fidget(),
                VK_RETURN => return self.fidget_burst(),
                _ => {}
            }
        }
        match vk {
            VK_DOWN => self.move_selection(1, true),
            VK_UP => self.move_selection(-1, true),
            VK_TAB => self.move_selection(if shift { -1 } else { 1 }, true),
            VK_NEXT => self.move_selection(page, false),
            VK_PRIOR => self.move_selection(-page, false),
            VK_RETURN => {
                if let Some(i) = self.selected {
                    self.activate(i, shift);
                }
            }
            VK_ESCAPE => {
                if !self.query.is_empty() && !ctrl {
                    self.set_search_text("");
                    if self.mode == Mode::Winget(crate::winget::Query::Search) {
                        self.winget = WingetState::default();
                    }
                    self.refresh_rows(false);
                } else if self.mode != Mode::Root && !ctrl {
                    self.go_back();
                } else {
                    self.hide();
                }
            }
            // Backspace on an empty box (sent by the search box) goes back.
            VK_BACK => self.go_back(),
            VK_DELETE if shift && self.mode == Mode::Clipboard => self.delete_selected_clip(),
            VK_DELETE if shift && self.mode == Mode::Colors => self.delete_selected_color(),
            // Ctrl+, opens settings.
            VIRTUAL_KEY(0xBC) if ctrl => self.open_settings(),
            _ => {}
        }
    }

    /// Enter on the calculator card: copy the answer, leave it in the search
    /// box for the next step, and pulse.
    fn activate_calc(&mut self, c: crate::calc::Calc) {
        let copied = crate::util::copy_to_clipboard(self.hwnd, &c.copy);
        self.query = c.chain.clone();
        self.calc_hold = Some(c.clone());
        unsafe {
            let text = wide(&c.chain);
            let _ = SetWindowTextW(self.edit, PCWSTR(text.as_ptr()));
            // Caret at the end, ready for "* 2" or " to m".
            let end = (text.len() - 1) as isize;
            SendMessageW(self.edit, EM_SETSEL, Some(WPARAM(end as usize)), Some(LPARAM(end)));
        }
        self.refresh_rows(false);
        if copied {
            self.pulse_row = self.rows.iter().position(|r| matches!(r, Row::Calc(_)));
            self.pulse_start = Some(Instant::now());
            unsafe {
                SetTimer(Some(self.hwnd), PULSE_TIMER, 15, None);
            }
            self.set_status(format!("Copied {}", c.copy), false, Some(Duration::from_millis(2500)));
            unsafe {
                SetTimer(Some(self.hwnd), STATUS_TIMER, 2600, None);
            }
        } else {
            self.set_status("Clipboard is busy; try again", true, Some(Duration::from_secs(3)));
        }
    }

    fn on_timer(&mut self, id: usize) {
        if id == PULSE_TIMER && !self.animating() {
            self.pulse_start = None;
            unsafe {
                let _ = KillTimer(Some(self.hwnd), PULSE_TIMER);
            }
        }
        if id == FLASH_TIMER {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), FLASH_TIMER);
            }
            self.hide();
            return;
        }
        if id == STATUS_TIMER {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), STATUS_TIMER);
            }
        }
        self.invalidate();
    }

    fn activate(&mut self, idx: usize, shift: bool) {
        if let Some(Row::Calc(c)) = self.rows.get(idx) {
            let c = (**c).clone();
            self.activate_calc(c);
            return;
        }
        let Some(Row::Item(it)) = self.rows.get(idx) else { return };
        let (key, action) = (it.key.clone(), it.action.clone());
        match &action {
            Action::Winget { op, id, name } => {
                let (op, id, name) = (*op, id.clone(), name.clone());
                self.activate_winget(op, &id, &name, shift);
                return;
            }
            Action::Clip(id) => {
                let id = *id;
                self.activate_clip(id, shift);
                return;
            }
            Action::Color(c) => {
                let c = crate::colors::Color::from_colorref(*c);
                let text = if shift { c.rgb() } else { c.hex() };
                self.copy_with_pulse(&text, Some(idx));
                return;
            }
            Action::CopyText(t) => {
                let t = t.clone();
                self.copy_with_pulse(&t, Some(idx));
                return;
            }
            Action::Suggest(idea) => {
                let text = crate::util::suggestion_text(idea, &crate::install::version_label());
                self.allow_foreground();
                launch::open_whatsapp(crate::util::SUGGEST_PHONE, &text);
                self.flash_then_hide(idx);
                return;
            }
            Action::OpenFile { path, .. } => {
                let p = std::path::PathBuf::from(path);
                self.allow_foreground();
                if shift { launch::reveal(&p) } else { launch::open_file(&p) }
                self.flash_then_hide(idx);
                return;
            }
            Action::Update => {
                if shift {
                    // Shift+Enter: read the release notes first.
                    if let Some(u) = &self.update {
                        self.allow_foreground();
                        crate::launch::open_url(&u.page_url);
                        self.hide();
                    }
                } else {
                    self.start_update_download();
                }
                return;
            }
            _ => {}
        }
        let pick_id = match &action {
            Action::LaunchApp { id } => Some(id.clone()),
            Action::Command(_) => Some(key.clone()),
            _ => None,
        };
        if let (Some(settings), Some(id)) = (self.pick_for, pick_id) {
            // Pick mode: hand the app (or command) to Settings instead of running it.
            let picked = Box::into_raw(Box::new((id, it.title.clone())));
            unsafe {
                if PostMessageW(Some(settings), WM_APP_APP_PICKED, WPARAM(0), LPARAM(picked as isize)).is_err() {
                    drop(Box::from_raw(picked));
                }
            }
            crate::focus::activate(settings);
            self.hide();
            return;
        }
        self.history.record(&key);
        match action {
            Action::LaunchApp { id } => {
                unsafe {
                    let _ = AllowSetForegroundWindow(ASFW_ANY);
                }
                launch::launch_app(&id);
                self.flash_then_hide(idx);
            }
            Action::Command(c) => self.run_command(c),
            Action::Winget { .. } | Action::Clip(_) | Action::Update | Action::OpenFile { .. } | Action::Color(_) | Action::CopyText(_) | Action::Suggest(_) => {}
        }
    }

    fn run_command(&mut self, c: Command) {
        match c {
            Command::ReloadApps => {
                self.set_status("Reloading applications…", false, None);
                self.start_index(false);
            }
            Command::OpenSettings => {
                self.reset_on_show = true;
                self.open_settings();
            }
            Command::OpenDataFolder => {
                unsafe {
                    let _ = AllowSetForegroundWindow(ASFW_ANY);
                }
                launch::open_folder(&data_dir());
                self.reset_on_show = true;
                self.hide();
            }
            Command::Quit => unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_APP_QUIT, WPARAM(0), LPARAM(0));
            },
            Command::WingetSearch => self.enter_mode(Mode::Winget(crate::winget::Query::Search)),
            Command::WingetInstalled => self.enter_mode(Mode::Winget(crate::winget::Query::Installed)),
            Command::WingetUpgradable => self.enter_mode(Mode::Winget(crate::winget::Query::Upgradable)),
            Command::WingetUpgradeAll | Command::WingetExport | Command::WingetImport => {
                self.allow_foreground();
                match c {
                    Command::WingetUpgradeAll => crate::winget::upgrade_all(),
                    Command::WingetExport => crate::winget::export(),
                    _ => crate::winget::import(),
                }
                self.reset_on_show = true;
                self.hide();
            }
            Command::ClipboardHistory => self.enter_mode(Mode::Clipboard),
            Command::PickColor => self.start_color_pick(),
            Command::SuggestFeature => self.enter_mode(Mode::Suggest),
            Command::ColorHistory => self.enter_mode(Mode::Colors),
            Command::ClearColors => {
                self.colors.clear();
                self.refresh_rows(false);
                self.set_status("Colour history cleared", false, Some(Duration::from_secs(3)));
                unsafe {
                    SetTimer(Some(self.hwnd), STATUS_TIMER, 3100, None);
                }
            }
            Command::SearchFiles => {
                self.load_recent_files();
                self.enter_mode(Mode::Files);
            }
            Command::CheckUpdates => self.start_update_check(true),
            Command::ClearClipboard => {
                self.clip.clear();
                self.set_search_text("");
                self.refresh_rows(false);
                self.set_status("Clipboard history cleared", false, Some(Duration::from_secs(3)));
                unsafe {
                    SetTimer(Some(self.hwnd), STATUS_TIMER, 3100, None);
                }
            }
        }
    }

    /// The clipboard hotkey: open straight on Clipboard History (or close it).
    fn toggle_clipboard(&mut self) {
        let fg = unsafe { GetForegroundWindow() };
        if self.visible && fg == self.hwnd && self.mode == Mode::Clipboard {
            self.hide();
            return;
        }
        if !self.visible || fg != self.hwnd {
            self.show();
        }
        if self.mode != Mode::Clipboard {
            self.enter_mode(Mode::Clipboard);
        }
    }

    // ----- dragging & fidget mode ------------------------------------------

    fn unit(&self) -> f32 {
        self.eff_dpi() as f32 / 96.0
    }

    /// Parts of the launcher that pick it up: the search bar (outside the
    /// text and back arrow) and the footer.
    fn is_drag_handle(&self, x: i32, y: i32) -> bool {
        let (_, h) = self.client_size();
        let on_back = self.back_rect().is_some_and(|r| super::draw::contains(&r, x, y));
        (y < self.top_h() + self.s(m::SEARCH_H) && !on_back) || y >= h - self.s(m::FOOTER_H)
    }

    fn begin_drag(&mut self) {
        let mut pt = POINT::default();
        let mut rc = RECT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
            let _ = GetWindowRect(self.hwnd, &mut rc);
            SetCapture(self.hwnd);
        }
        self.drag = Some(Drag { grab: (pt.x - rc.left, pt.y - rc.top), started: Instant::now(), shake: Shake::new(self.unit()) });
    }

    fn on_drag_move(&mut self) {
        let Some(d) = &mut self.drag else { return };
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
            let _ = SetWindowPos(
                self.hwnd,
                None,
                pt.x - d.grab.0,
                pt.y - d.grab.1,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        let fidget_on = self.config.config.module_enabled("fidget");
        if d.shake.sample(d.started.elapsed().as_secs_f32(), pt.x as f32, pt.y as f32) && fidget_on {
            let (vx, vy) = d.shake.velocity();
            let u = self.unit();
            if self.fidget.is_some() {
                self.fidget_burst();
            } else {
                // The letters fly off with some of the shake's momentum.
                self.start_fidget((vx / u * 0.5, vy / u * 0.5));
            }
        }
    }

    /// Dissolves what the search box shows (the typed text, or else the
    /// placeholder) into pixels and starts the simulation.
    fn start_fidget(&mut self, vel: (f32, f32)) {
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let mon = unsafe { MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST) };
        unsafe {
            let _ = GetMonitorInfoW(mon, &mut mi);
        }
        let t = self.theme;
        let Some(mut f) = Fidget::start(self.hwnd, mi.rcMonitor, self.unit(), [t.text, t.subtext], t.accent) else {
            return;
        };
        if self.query.is_empty() {
            f.dissolve(self.mode.placeholder(), self.fonts.search, self.edit_text_origin(), 1, vel);
        } else {
            f.dissolve(&self.query, self.fonts.search, self.edit_char_pos(0), 0, vel);
            self.set_search_text("");
            self.refresh_rows(false);
        }
        self.fidget = Some(f);
        FIDGET_ON.with(|c| c.set(true));
        self.fidget_caret = (0, None);
        self.set_placeholder("");
        self.set_status(FIDGET_STATUS, false, None);
        self.pacer = Some(Pacer::start(self.hwnd, WM_APP_FIDGET_FRAME, mon));
    }

    fn stop_fidget(&mut self) {
        if self.fidget.take().is_none() {
            return;
        }
        FIDGET_ON.with(|c| c.set(false));
        self.pacer = None;
        self.set_placeholder(self.mode.placeholder());
        if matches!(&self.status, Some((s, _, _)) if s == FIDGET_STATUS) {
            self.status = None;
        }
        self.invalidate();
    }

    fn fidget_burst(&mut self) {
        let Some(f) = &mut self.fidget else { return };
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        let (x, y) = f.to_sim(pt.x, pt.y);
        f.sim.burst(x, y);
    }

    fn on_fidget_frame(&mut self) {
        let Some(f) = &mut self.fidget else { return };
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        f.tick(pt);
        if let Some(p) = &self.pacer {
            p.frame_done();
        }
    }

    /// A letter typed in fidget mode: it appears where the next letter would
    /// go and dissolves at once.
    fn on_fidget_char(&mut self, code: u16) {
        // Lone surrogate halves (emoji) are skipped.
        let Some(c) = char::from_u32(code as u32) else { return };
        let (origin, box_w) = (self.edit_text_origin(), self.edit_text_rect().right - self.edit_text_rect().left);
        let font = self.fonts.search;
        let now = Instant::now();
        let (mut x, last) = self.fidget_caret;
        if last.is_none_or(|t| now - t > FIDGET_CARET_RESET) {
            x = 0;
        }
        let Some(f) = &mut self.fidget else { return };
        x += f.dissolve(&c.to_string(), font, POINT { x: origin.x + x, y: origin.y }, 0, (0.0, -60.0));
        if x > box_w - self.s(24) {
            x = 0;
        }
        self.fidget_caret = (x, Some(now));
    }

    /// The search box's formatting rectangle (where text is drawn), in its client coordinates.
    fn edit_text_rect(&self) -> RECT {
        let mut rc = RECT::default();
        unsafe {
            SendMessageW(self.edit, EM_GETRECT, None, Some(LPARAM(&mut rc as *mut RECT as isize)));
        }
        rc
    }

    /// Screen position where the search box starts drawing text.
    fn edit_text_origin(&self) -> POINT {
        let rc = self.edit_text_rect();
        let mut pt = POINT { x: rc.left, y: rc.top };
        unsafe {
            let _ = ClientToScreen(self.edit, &mut pt);
        }
        pt
    }

    /// Screen position of character `i` in the search box.
    fn edit_char_pos(&self, i: usize) -> POINT {
        let r = unsafe { SendMessageW(self.edit, EM_POSFROMCHAR, Some(WPARAM(i)), None) }.0;
        if r == -1 {
            return self.edit_text_origin();
        }
        let mut pt = POINT { x: (r & 0xFFFF) as i16 as i32, y: ((r >> 16) & 0xFFFF) as i16 as i32 };
        unsafe {
            let _ = ClientToScreen(self.edit, &mut pt);
        }
        pt
    }

    fn on_mouse_move(&mut self, x: i32, y: i32) {
        if (x, y) == self.last_mouse {
            return;
        }
        self.last_mouse = (x, y);
        if let Some(i) = self.hit_test(x, y) {
            if self.selected != Some(i) {
                self.selected = Some(i);
                self.invalidate();
            }
        }
    }

    fn on_wheel(&mut self, delta: i32) {
        let step = self.item_h() * 3 * delta / 120;
        let s = (self.scroll - step).clamp(0, self.max_scroll());
        if s != self.scroll {
            self.scroll = s;
            self.invalidate();
        }
    }

    fn on_tray_cmd(&mut self, id: usize) {
        match id {
            tray::MENU_OPEN => self.show(),
            tray::MENU_SETTINGS => self.open_settings(),
            tray::MENU_RELOAD => {
                self.set_status("Reloading applications…", false, None);
                self.start_index(false);
            }
            tray::MENU_QUIT => self.run_command(Command::Quit),
            _ => {}
        }
    }

    fn on_relayout(&mut self) {
        let dpi = unsafe { GetDpiForWindow(self.hwnd) };
        if dpi != 0 && dpi != self.dpi {
            self.apply_dpi(dpi, true);
            // Dragged onto a monitor with a different scale: resize to match.
            unsafe {
                let _ = SetWindowPos(
                    self.hwnd,
                    None,
                    0,
                    0,
                    self.window_size().0,
                    self.window_size().1,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
        let t = theme::resolve(self.config.config.theme, self.config.config.accent, &self.config.config.appearance);
        if t != self.theme {
            self.theme = t;
            self.apply_theme();
        }
        self.invalidate();
    }

    fn destroy(&mut self) {
        input::shutdown();
        tray::remove(self.hwnd);
        self.fonts.destroy();
        self.backbuf.release();
        unsafe {
            if !self.edit_brush.is_invalid() {
                let _ = DeleteObject(self.edit_brush.into());
            }
            if let Some(l) = self.logo.take() {
                let _ = DestroyIcon(l);
            }
        }
    }
}

fn text_height(font: HFONT) -> i32 {
    unsafe {
        let hdc: HDC = GetDC(None);
        let old = SelectObject(hdc, font.into());
        let mut tm = TEXTMETRICW::default();
        let _ = GetTextMetricsW(hdc, &mut tm);
        SelectObject(hdc, old);
        ReleaseDC(None, hdc);
        tm.tmHeight
    }
}

#[inline]
fn loword(v: usize) -> u32 {
    (v & 0xFFFF) as u32
}
#[inline]
fn hiword(v: usize) -> u32 {
    ((v >> 16) & 0xFFFF) as u32
}
#[inline]
fn point_from_lparam(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32)
}

// ---------------------------------------------------------------------------
// Window creation & message loop.

pub fn create() -> HWND {
    unsafe {
        let hinst = GetModuleHandleW(None).expect("module handle");
        let class = wide(WINDOW_CLASS);
        let big = tray::load_app_icon(32).unwrap_or_default();
        let small = tray::load_app_icon(16).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: big,
            hIconSm: small,
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let title = wide("SIGMA5");
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_POPUP | WS_CLIPCHILDREN,
            0,
            0,
            m::WIDTH,
            m::HEIGHT,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .expect("create window");
        let state = Launcher::new(hwnd);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
        hwnd
    }
}

pub fn run_message_loop() {
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn post_update(hwnd: SendHwnd, ev: UpdateEvent) {
    let raw = Box::into_raw(Box::new(ev));
    unsafe {
        if PostMessageW(Some(hwnd.get()), WM_APP_UPDATE, WPARAM(0), LPARAM(raw as isize)).is_err() {
            drop(Box::from_raw(raw));
        }
    }
}

fn post(hwnd: HWND, msg: u32) {
    unsafe {
        let _ = PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(0));
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Launcher;
    if ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
    }

    // Messages handled without borrowing state (may arrive re-entrantly).
    match msg {
        WM_ACTIVATE => {
            if loword(wp.0) == WA_INACTIVE {
                post(hwnd, WM_APP_HIDE);
            }
            return LRESULT(0);
        }
        WM_COMMAND => {
            if loword(wp.0) as usize == EDIT_ID && hiword(wp.0) == EN_CHANGE {
                post(hwnd, WM_APP_QUERY);
            }
            return LRESULT(0);
        }
        WM_CTLCOLOREDIT => {
            let (brush, bg, fg) = EDIT_STYLE.with(|c| c.get());
            let hdc = HDC(wp.0 as *mut _);
            unsafe {
                SetTextColor(hdc, fg);
                SetBkColor(hdc, bg);
            }
            return LRESULT(brush.0 as isize);
        }
        WM_DPICHANGED | WM_SETTINGCHANGE | WM_THEMECHANGED => {
            post(hwnd, WM_APP_RELAYOUT);
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        // Sent re-entrantly by ReleaseCapture.
        WM_CAPTURECHANGED => {
            post(hwnd, WM_APP_DRAG_END);
            return LRESULT(0);
        }
        WM_CLOSE => {
            post(hwnd, WM_APP_HIDE);
            return LRESULT(0);
        }
        WM_APP_TRAY => {
            let event = loword(lp.0 as usize);
            if event == WM_LBUTTONUP {
                post(hwnd, WM_APP_SHOW);
            } else if event == WM_RBUTTONUP || event == WM_CONTEXTMENU {
                let hk = unsafe { (*ptr).config.config.hotkey.clone() };
                let id = tray::show_menu(hwnd, &hk);
                if id != 0 {
                    unsafe {
                        let _ = PostMessageW(Some(hwnd), WM_APP_TRAYCMD, WPARAM(id), LPARAM(0));
                    }
                }
            }
            return LRESULT(0);
        }
        WM_DESTROY => {
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                let mut state = Box::from_raw(ptr);
                state.destroy();
                drop(state);
                PostQuitMessage(0);
            }
            return LRESULT(0);
        }
        _ => {}
    }

    let ui = unsafe { &mut *ptr };
    if msg == ui.taskbar_created && msg != 0 {
        tray::add(hwnd, &format!("SIGMA5 ({})", ui.config.config.hotkey));
        return LRESULT(0);
    }
    match msg {
        WM_PAINT => {
            paint::paint(ui);
            LRESULT(0)
        }
        WM_HOTKEY => {
            let id = wp.0 as i32;
            if id == hotkey::HOTKEY_ID {
                ui.toggle();
            } else if id == hotkey::CLIPBOARD_HOTKEY_ID {
                ui.toggle_clipboard();
            } else if id >= hotkey::APP_HOTKEY_BASE {
                ui.on_app_hotkey((id - hotkey::APP_HOTKEY_BASE) as usize);
            }
            LRESULT(0)
        }
        WM_APP_PICK_APP => {
            ui.start_pick(HWND(wp.0 as *mut _));
            LRESULT(0)
        }
        WM_APP_SHOW => {
            ui.show();
            LRESULT(0)
        }
        WM_APP_HIDE => {
            ui.hide();
            LRESULT(0)
        }
        WM_APP_QUERY => {
            ui.on_query_changed();
            LRESULT(0)
        }
        WM_APP_NAV => {
            let ctrl = lp.0 & 1 != 0;
            let shift = lp.0 & 2 != 0;
            ui.on_nav(VIRTUAL_KEY(wp.0 as u16), ctrl, shift);
            LRESULT(0)
        }
        WM_APP_INDEX => {
            let res = unsafe { Box::from_raw(lp.0 as *mut IndexResult) };
            ui.on_index(*res);
            LRESULT(0)
        }
        WM_APP_ICONS => {
            let batch = unsafe { Box::from_raw(lp.0 as *mut Vec<IconPixels>) };
            ui.on_icons(*batch);
            LRESULT(0)
        }
        WM_APP_TRAYCMD => {
            ui.on_tray_cmd(wp.0);
            LRESULT(0)
        }
        WM_APP_RELAYOUT => {
            ui.on_relayout();
            LRESULT(0)
        }
        WM_APP_WINKEY => {
            ui.toggle();
            LRESULT(0)
        }
        WM_APP_SYSINFO => {
            let s = unsafe { Box::from_raw(lp.0 as *mut crate::sysinfo::Snapshot) };
            ui.on_sysinfo(*s);
            LRESULT(0)
        }
        WM_TIMER if wp.0 == CLOCK_TIMER => {
            if ui.top_bar().is_some() {
                let rc = RECT { left: 0, top: 0, right: ui.client_size().0, bottom: ui.top_h() };
                unsafe {
                    let _ = InvalidateRect(Some(hwnd), Some(&rc), false);
                }
            }
            LRESULT(0)
        }
        WM_APP_COLOR_PICKED => {
            ui.on_color_picked(wp.0 as u32, lp.0 != 0);
            LRESULT(0)
        }
        WM_APP_FILES => {
            let m = unsafe { Box::from_raw(lp.0 as *mut FilesMsg) };
            ui.on_files_msg(*m);
            LRESULT(0)
        }
        WM_APP_WINGET => {
            let r = unsafe { Box::from_raw(lp.0 as *mut WingetResult) };
            ui.on_winget_result(*r);
            LRESULT(0)
        }
        WM_CLIPBOARDUPDATE => {
            ui.on_clipboard_update();
            LRESULT(0)
        }
        WM_APP_CONFIG_CHANGED => {
            ui.on_config_changed();
            LRESULT(0)
        }
        WM_APP_CLEAR_HISTORY => {
            ui.history.clear();
            ui.refresh_rows(false);
            LRESULT(0)
        }
        WM_APP_QUIT => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_APP_DRAG_BEGIN => {
            ui.begin_drag();
            LRESULT(0)
        }
        WM_APP_DRAG_END => {
            ui.drag = None;
            LRESULT(0)
        }
        WM_APP_FIDGET_CHAR => {
            ui.on_fidget_char(wp.0 as u16);
            LRESULT(0)
        }
        WM_APP_FIDGET_FRAME => {
            ui.on_fidget_frame();
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if ui.drag.is_some() {
                ui.on_drag_move();
                return LRESULT(0);
            }
            let (x, y) = point_from_lparam(lp);
            ui.on_mouse_move(x, y);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = point_from_lparam(lp);
            if ui.is_drag_handle(x, y) {
                unsafe {
                    let _ = SetFocus(Some(ui.edit));
                }
                ui.begin_drag();
                return LRESULT(0);
            }
            ui.press_row = ui.hit_test(x, y);
            if let Some(i) = ui.press_row {
                ui.selected = Some(i);
                ui.invalidate();
            }
            unsafe {
                let _ = SetFocus(Some(ui.edit));
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if ui.drag.take().is_some() {
                unsafe {
                    let _ = ReleaseCapture();
                }
                return LRESULT(0);
            }
            let (x, y) = point_from_lparam(lp);
            if ui.back_rect().is_some_and(|r| super::draw::contains(&r, x, y)) {
                ui.go_back();
                return LRESULT(0);
            }
            let hit = ui.hit_test(x, y);
            if hit.is_some() && hit == ui.press_row.take() {
                ui.activate(hit.unwrap(), key_down(VK_SHIFT));
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = hiword(wp.0) as u16 as i16 as i32;
            ui.on_wheel(delta);
            LRESULT(0)
        }
        WM_TIMER if wp.0 == WINGET_TIMER => {
            ui.on_winget_timer();
            LRESULT(0)
        }
        WM_TIMER if wp.0 == UPDATE_TIMER => {
            ui.on_update_timer();
            LRESULT(0)
        }
        WM_APP_UPDATE => {
            let ev = unsafe { Box::from_raw(lp.0 as *mut UpdateEvent) };
            ui.on_update_event(*ev);
            LRESULT(0)
        }
        WM_APP_CHECK_UPDATE => {
            ui.start_update_check(true);
            LRESULT(0)
        }
        WM_TIMER => {
            ui.on_timer(wp.0);
            LRESULT(0)
        }
        WM_SIZE => {
            ui.layout_edit();
            ui.invalidate();
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

// ---------------------------------------------------------------------------
// Search box subclass: routes navigation keys to the launcher and adds
// Ctrl+Backspace / Ctrl+A editing.

fn key_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) < 0 }
}

unsafe extern "system" fn edit_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    match msg {
        WM_KEYDOWN => {
            let vk = VIRTUAL_KEY(wp.0 as u16);
            let ctrl = key_down(VK_CONTROL);
            let shift = key_down(VK_SHIFT);
            let nav = |k: VIRTUAL_KEY| unsafe {
                let parent = GetParent(hwnd).unwrap_or_default();
                let flags = (ctrl as isize) | ((shift as isize) << 1);
                SendMessageW(parent, WM_APP_NAV, Some(WPARAM(k.0 as usize)), Some(LPARAM(flags)));
                LRESULT(0)
            };
            match vk {
                VK_UP | VK_DOWN | VK_RETURN | VK_ESCAPE | VK_PRIOR | VK_NEXT | VK_TAB => return nav(vk),
                _ if ctrl && (vk.0 == b'N' as u16 || vk.0 == b'J' as u16) => return nav(VK_DOWN),
                _ if ctrl && (vk.0 == b'P' as u16 || vk.0 == b'K' as u16) => return nav(VK_UP),
                _ if ctrl && vk.0 == 0xBC => return nav(vk), // Ctrl+, → settings
                _ if ctrl && vk.0 == b'A' as u16 => unsafe {
                    SendMessageW(hwnd, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
                    return LRESULT(0);
                },
                VK_BACK if ctrl => {
                    unsafe { delete_word_back(hwnd) };
                    return LRESULT(0);
                }
                // Backspace with nothing typed: back out of a sub-page.
                VK_BACK if unsafe { GetWindowTextLengthW(hwnd) } == 0 => return nav(vk),
                // Shift+Delete removes the selected clipboard entry.
                VK_DELETE if shift => return nav(vk),
                _ => {}
            }
        }
        WM_CHAR => {
            // Swallow control characters produced by the keys handled above
            // (Enter, Esc, Tab, Ctrl+Backspace, Ctrl+A/N/P/J/K) to avoid beeps.
            let c = wp.0 as u32;
            if matches!(c, 0x0D | 0x1B | 0x09 | 0x7F | 0x01 | 0x0E | 0x10 | 0x0A | 0x0B) {
                return LRESULT(0);
            }
            // Fidget mode: letters dissolve instead of being typed.
            if c >= 0x20 && FIDGET_ON.with(|f| f.get()) {
                unsafe {
                    let parent = GetParent(hwnd).unwrap_or_default();
                    let _ = PostMessageW(Some(parent), WM_APP_FIDGET_CHAR, wp, LPARAM(0));
                }
                return LRESULT(0);
            }
        }
        // Pressing past the end of the text picks the launcher up.
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK if unsafe { past_text_end(hwnd, (lp.0 & 0xFFFF) as i16 as i32) } => unsafe {
            let _ = SetFocus(Some(hwnd));
            let len = GetWindowTextLengthW(hwnd).max(0) as usize;
            SendMessageW(hwnd, EM_SETSEL, Some(WPARAM(len)), Some(LPARAM(len as isize)));
            let parent = GetParent(hwnd).unwrap_or_default();
            SendMessageW(parent, WM_APP_DRAG_BEGIN, None, None);
            return LRESULT(0);
        },
        WM_SETFOCUS => {
            let r = unsafe { DefSubclassProc(hwnd, msg, wp, lp) };
            // A slightly thicker caret than the 1px default.
            unsafe {
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let dpi = GetDpiForWindow(hwnd).max(96);
                let w = ((dpi as f32 / 96.0) * 2.0).round().max(1.0) as i32;
                let _ = CreateCaret(hwnd, None, w, rc.bottom - rc.top);
                let _ = ShowCaret(Some(hwnd));
            }
            return r;
        }
        WM_NCDESTROY => unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(edit_proc), 1);
        },
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
}

/// Whether client x `x` in the search box is clearly past the end of its text.
unsafe fn past_text_end(edit: HWND, x: i32) -> bool {
    unsafe {
        let len = GetWindowTextLengthW(edit);
        if len <= 0 {
            return true;
        }
        let r = SendMessageW(edit, EM_POSFROMCHAR, Some(WPARAM(len as usize - 1)), None).0;
        if r == -1 {
            return false;
        }
        let last_x = (r & 0xFFFF) as i16 as i32;
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(edit, &mut buf) as usize;
        let Some(&last) = buf[..n].last() else { return true };
        let font = SendMessageW(edit, WM_GETFONT, None, None).0;
        let hdc = GetDC(Some(edit));
        let old = SelectObject(hdc, HGDIOBJ(font as *mut _));
        let mut sz = windows::Win32::Foundation::SIZE::default();
        let _ = windows::Win32::Graphics::Gdi::GetTextExtentPoint32W(hdc, &[last], &mut sz);
        SelectObject(hdc, old);
        ReleaseDC(Some(edit), hdc);
        let slack = 8 * GetDpiForWindow(edit).max(96) as i32 / 96;
        x > last_x + sz.cx + slack
    }
}

unsafe fn delete_word_back(edit: HWND) {
    unsafe {
        let mut start = 0u32;
        let mut end = 0u32;
        SendMessageW(
            edit,
            EM_GETSEL,
            Some(WPARAM(&mut start as *mut u32 as usize)),
            Some(LPARAM(&mut end as *mut u32 as isize)),
        );
        if start == end {
            let len = GetWindowTextLengthW(edit);
            let mut buf = vec![0u16; len as usize + 1];
            let n = GetWindowTextW(edit, &mut buf) as usize;
            let chars = &buf[..n];
            let mut i = (end as usize).min(n);
            while i > 0 && chars[i - 1] == b' ' as u16 {
                i -= 1;
            }
            while i > 0 && chars[i - 1] != b' ' as u16 {
                i -= 1;
            }
            start = i as u32;
        }
        SendMessageW(edit, EM_SETSEL, Some(WPARAM(start as usize)), Some(LPARAM(end as isize)));
        let empty = [0u16];
        SendMessageW(edit, EM_REPLACESEL, Some(WPARAM(1)), Some(LPARAM(empty.as_ptr() as isize)));
    }
}

#[cfg(test)]
#[path = "harness.rs"]
mod harness;
