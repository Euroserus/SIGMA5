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
    DWMWA_BORDER_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, DeleteObject, GetDC, GetMonitorInfoW, GetTextMetricsW, HBRUSH, HDC, HFONT,
    InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, ReleaseDC, SelectObject,
    SetBkColor, SetTextColor, TEXTMETRICW,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    EM_GETSEL, EM_REPLACESEL, EM_SETCUEBANNER, EM_SETLIMITTEXT, EM_SETMARGINS, EM_SETSEL,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_NEXT, VK_PRIOR,
    VK_RETURN, VK_SHIFT, VK_TAB, VK_UP,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use super::icons::{self, IconPixels};
use super::draw::BackBuffer;
use super::paint::{self, Fonts};
use super::settings;
use super::input;
use super::{WM_APP_APP_PICKED, WM_APP_CLEAR_HISTORY, WM_APP_CONFIG_CHANGED, WM_APP_PICK_APP, WM_APP_WINKEY};
use super::theme::{self, Theme, metrics as m};
use super::{hotkey, tray};
use super::{
    WINDOW_CLASS, WM_APP_HIDE, WM_APP_ICONS, WM_APP_INDEX, WM_APP_NAV, WM_APP_QUERY, WM_APP_QUIT,
    WM_APP_RELAYOUT, WM_APP_SHOW, WM_APP_TRAY, WM_APP_TRAYCMD,
};
use crate::config::ConfigStore;
use crate::fuzzy::Query;
use crate::history::History;
use crate::launch;
use crate::providers::apps::{IndexResult, spawn_indexer};
use crate::providers::{Action, Command, Row};
use crate::util::{SendHwnd, data_dir, wide};

const REINDEX_AFTER: Duration = Duration::from_secs(5 * 60);
const PULSE_TIMER: usize = 7;
const STATUS_TIMER: usize = 8;
const EDIT_ID: usize = 100;

thread_local! {
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
    /// Number of app hotkeys currently registered (ids from APP_HOTKEY_BASE).
    app_hotkeys_registered: usize,
    /// Calculator answer left in the search box after Enter (shown until edited).
    calc_hold: Option<crate::calc::Calc>,
    pulse_start: Option<Instant>,
}

impl Launcher {
    // ----- geometry -------------------------------------------------------

    /// Monitor DPI scaled by the "Launcher size" setting.
    fn eff_dpi(&self) -> u32 {
        self.dpi * self.zoom / 100
    }

    pub fn s(&self, v: i32) -> i32 {
        (v * self.eff_dpi() as i32 + 48) / 96
    }

    pub fn client_size(&self) -> (i32, i32) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        (rc.right - rc.left, rc.bottom - rc.top)
    }

    pub fn list_rect(&self) -> RECT {
        let (w, h) = self.client_size();
        RECT { left: 0, top: self.s(m::SEARCH_H) + 1, right: w, bottom: h - self.s(m::FOOTER_H) }
    }

    pub fn row_height(&self, row: &Row) -> i32 {
        match row {
            Row::Header(_) => self.s(m::HEADER_H),
            Row::Item(_) => self.s(m::ROW_H),
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
        Some(if self.pick_for.is_some() { "Choose App" } else { a.verb() })
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
        let theme = theme::resolve(config.config.theme, config.config.accent);
        let zoom = config.config.size.percent();
        let mut me = Box::new(Self {
            hwnd,
            edit: HWND::default(),
            dpi,
            zoom,
            theme,
            fonts: Fonts::create(dpi * zoom / 100),
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
            calc_hold: None,
            pulse_start: None,
        });
        me.create_edit();
        me.apply_dpi(dpi, false);
        me.apply_theme();
        tray::add(hwnd, "SIGMA5");
        me.apply_config(true);
        me.refresh_rows(false);
        me.start_index(false);
        me
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
        self.fonts = Fonts::create(self.eff_dpi());
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
        let w = if w <= 0 { self.s(m::WIDTH) } else { w };
        let th = text_height(self.fonts.search);
        let x = self.s(m::TEXT_X) - self.s(3);
        let y = (self.s(m::SEARCH_H) - th) / 2;
        unsafe {
            let _ = SetWindowPos(self.edit, None, x, y, w - 2 * x, th, SWP_NOZORDER | SWP_NOACTIVATE);
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
            let corner = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(self.hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const _, 4);
            let border = t.border;
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
        input::configure(self.hwnd, input::Rules { win_tap: cfg.windows_key, remaps: cfg.key_rules() });
        self.register_app_hotkeys(&cfg);
        self.registry.disabled = cfg.disabled_extensions.clone();

        let t = theme::resolve(cfg.theme, cfg.accent);
        if t != self.theme || first {
            self.theme = t;
            self.apply_theme();
        }
        let zoom = cfg.size.percent();
        if zoom != self.zoom {
            self.zoom = zoom;
            self.apply_dpi(self.dpi, true);
        }

        self.status = None;
        match hotkey::register(self.hwnd, &cfg.hotkey) {
            Ok(()) => tray::set_tip(self.hwnd, &format!("SIGMA5 ({})", cfg.hotkey)),
            Err(e) => {
                if first {
                    tray::notify(self.hwnd, "SIGMA5 hotkey unavailable", &e, true);
                }
                self.set_status(e, true, None);
            }
        }
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
            if hotkey::register_id(self.hwnd, hotkey::APP_HOTKEY_BASE + i as i32, &b.hotkey).is_err() {
                failed.push(format!("{} ({})", b.hotkey, b.name));
            }
        }
        self.app_hotkeys_registered = cfg.app_hotkeys.len();
        if !failed.is_empty() {
            self.set_status(format!("Hotkey in use by another app: {}", failed.join(", ")), true, None);
        }
    }

    fn on_app_hotkey(&mut self, index: usize) {
        let Some(b) = self.config.config.app_hotkeys.get(index) else { return };
        let exe = self
            .registry
            .apps
            .find(&b.app)
            .and_then(|a| a.exe.clone())
            .or_else(|| b.app.rsplit('\\').next().map(str::to_ascii_lowercase).filter(|e| e.ends_with(".exe")));
        crate::focus::focus_or_launch(&b.app, exe.as_deref());
    }

    /// Shows the launcher so the user can pick an app for the Settings window.
    fn start_pick(&mut self, settings: HWND) {
        self.reset_on_show = true;
        self.hidden_at = None;
        self.pick_for = Some(settings);
        self.show();
        self.set_status("Choose an app for the new hotkey", false, None);
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
        self.rows = self.registry.rows(&q, &self.history, self.config.config.recent_count);
        if self.pick_for.is_some() {
            // Pick mode: apps only, and drop headers left with no items.
            let rows = std::mem::take(&mut self.rows);
            let kept: Vec<Row> = rows
                .into_iter()
                .filter(|r| match r {
                    Row::Item(it) => matches!(it.action, Action::LaunchApp { .. }),
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
        } else if self.config.config.extension_enabled("calculator") {
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
        if text != self.query {
            if self.calc_hold.as_ref().is_some_and(|h| h.chain != text.trim()) {
                self.calc_hold = None;
            }
            self.query = text;
            self.refresh_rows(false);
        }
    }

    // ----- visibility ------------------------------------------------------

    fn show(&mut self) {
        if self.config.reload_if_changed() {
            self.apply_config(false);
        }
        // System theme/accent may have changed while hidden.
        let t = theme::resolve(self.config.config.theme, self.config.config.accent);
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
            let (w, h) = (self.s(m::WIDTH), self.s(m::HEIGHT));
            let x = wa.left + ((wa.right - wa.left) - w) / 2;
            let y = (wa.top + ((wa.bottom - wa.top) as f32 * m::TOP_FRACTION) as i32).min(wa.bottom - h).max(wa.top);
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
        }
        self.layout_edit();

        let expired = self
            .hidden_at
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(self.config.config.keep_query_secs));
        if (self.reset_on_show || expired) && !self.query.is_empty() {
            self.query.clear();
            unsafe {
                let _ = SetWindowTextW(self.edit, w!(""));
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
            ((lr.bottom - lr.top) / self.s(m::ROW_H)).max(1)
        };
        match vk {
            VK_DOWN => self.move_selection(1, true),
            VK_UP => self.move_selection(-1, true),
            VK_TAB => self.move_selection(if shift { -1 } else { 1 }, true),
            VK_NEXT => self.move_selection(page, false),
            VK_PRIOR => self.move_selection(-page, false),
            VK_RETURN => {
                if let Some(i) = self.selected {
                    self.activate(i);
                }
            }
            VK_ESCAPE => {
                if !self.query.is_empty() && !ctrl {
                    self.query.clear();
                    unsafe {
                        let _ = SetWindowTextW(self.edit, w!(""));
                    }
                    self.refresh_rows(false);
                } else {
                    self.hide();
                }
            }
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
        if id == PULSE_TIMER && self.pulse().is_none() {
            self.pulse_start = None;
            unsafe {
                let _ = KillTimer(Some(self.hwnd), PULSE_TIMER);
            }
        }
        if id == STATUS_TIMER {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), STATUS_TIMER);
            }
        }
        self.invalidate();
    }

    fn activate(&mut self, idx: usize) {
        if let Some(Row::Calc(c)) = self.rows.get(idx) {
            let c = (**c).clone();
            self.activate_calc(c);
            return;
        }
        let Some(Row::Item(it)) = self.rows.get(idx) else { return };
        let (key, action) = (it.key.clone(), it.action.clone());
        if let (Some(settings), Action::LaunchApp { id }) = (self.pick_for, &action) {
            // Pick mode: hand the app to Settings instead of launching it.
            let picked = Box::into_raw(Box::new((id.clone(), it.title.clone())));
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
                self.reset_on_show = true;
                self.hide();
            }
            Action::Command(c) => self.run_command(c),
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
        }
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
        let step = self.s(m::ROW_H) * 3 * delta / 120;
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
        }
        let t = theme::resolve(self.config.config.theme, self.config.config.accent);
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
        WM_MOUSEMOVE => {
            let (x, y) = point_from_lparam(lp);
            ui.on_mouse_move(x, y);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = point_from_lparam(lp);
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
            let (x, y) = point_from_lparam(lp);
            let hit = ui.hit_test(x, y);
            if hit.is_some() && hit == ui.press_row.take() {
                ui.activate(hit.unwrap());
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = hiword(wp.0) as u16 as i16 as i32;
            ui.on_wheel(delta);
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
        }
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
