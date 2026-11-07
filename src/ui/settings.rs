//! The Settings window.
//!
//! A normal top-level window (native title bar) whose client area is drawn
//! with the same GDI primitives as the launcher. It is created on demand and
//! destroyed when closed, so it costs nothing while not open. Every change is
//! written to config.toml immediately and the launcher is told to re-apply it.
//!
//! Key recording (hotkeys, remaps) goes through the keyboard hook's capture
//! mode (`input::start_capture`), so keys are seen before Windows processes
//! other apps' global shortcuts.

use std::cell::Cell;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VIRTUAL_KEY, VK_CONTROL,
    VK_ESCAPE, VK_SHIFT, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

use super::draw::{
    self, BackBuffer, DISPLAY_FACES, ICON_FACES, UI_FACES, bordered_round_rect, circle, contains, fill,
    round_rect, text, text_width,
};
use super::theme::{self, Theme, WHITE, accent_color};
use super::{
    WM_APP_APP_PICKED, WM_APP_CAPTURE_MODS, WM_APP_CAPTURED, WM_APP_CLEAR_HISTORY, WM_APP_CONFIG_CHANGED,
    WM_APP_PICK_APP, WM_APP_SETTINGS_MENU, hotkey, icons, input, tray,
};
use crate::config::{Accent, AppHotkey, CapsMode, Config, ConfigStore, Remap, ThemeMode, UiSize};
use crate::keys::{self, Target};
use crate::launch;
use crate::providers::EXTENSIONS;
use crate::util::{data_dir, wide};

const CLASS: &str = "SIGMA5.Settings";

// Layout, in logical pixels.
const CLIENT_W: i32 = 780;
const CLIENT_H: i32 = 560;
const SIDEBAR_W: i32 = 212;
const PAD: i32 = 32;
const ROW_H: i32 = 68;
const LIST_ROW_H: i32 = 60;
const ROW_GAP: i32 = 6;
const CARD_R: i32 = 8;
const NOTICE_TIMER: usize = 1;

const KEEP_STEPS: &[u64] = &[0, 5, 10, 15, 30, 60, 120, 300, 600];
const RECENT_MAX: usize = 12;

// Remap "To" menu.
const MENU_RECORD: usize = 1;
const MENU_HYPER: usize = 20;
const MENU_DISABLED: usize = 21;
const MENU_MEDIA: &[(usize, u32, &str)] = &[
    (10, 0xB3, "Play / Pause"),
    (11, 0xB0, "Next track"),
    (12, 0xB1, "Previous track"),
    (13, 0xB2, "Stop"),
    (14, 0xAF, "Volume up"),
    (15, 0xAE, "Volume down"),
    (16, 0xAD, "Mute"),
];

thread_local! {
    static OPEN: Cell<HWND> = const { Cell::new(HWND(std::ptr::null_mut())) };
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    General,
    Appearance,
    Hotkeys,
    Keyboard,
    Extensions,
    About,
}

impl Page {
    const ALL: [Page; 6] = [Page::General, Page::Appearance, Page::Hotkeys, Page::Keyboard, Page::Extensions, Page::About];
    fn title(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Appearance => "Appearance",
            Page::Hotkeys => "App Hotkeys",
            Page::Keyboard => "Keyboard",
            Page::Extensions => "Extensions",
            Page::About => "About",
        }
    }
    fn nav_label(self) -> &'static str {
        match self {
            Page::Hotkeys => "Hotkeys",
            p => p.title(),
        }
    }
    fn glyph(self) -> char {
        match self {
            Page::General => '\u{E713}',
            Page::Appearance => '\u{E790}',
            Page::Hotkeys => '\u{E945}',
            Page::Keyboard => '\u{E765}',
            Page::Extensions => '\u{EA86}',
            Page::About => '\u{E946}',
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stepper {
    KeepQuery,
    Recent,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Btn {
    OpenConfig,
    OpenData,
    ClearHistory,
    Reset,
}

/// What is being recorded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Rec {
    Launcher,
    App(usize),
    DraftApp,
    From(usize),
    DraftFrom,
    To(usize),
    DraftTo,
}

impl Rec {
    fn is_remap(self) -> bool {
        matches!(self, Rec::From(_) | Rec::DraftFrom | Rec::To(_) | Rec::DraftTo)
    }
}

/// Which remap a "To" menu is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MenuFor {
    Remap(usize),
    Draft,
}

/// Clickable things; zones are rebuilt on every paint.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ctl {
    Nav(Page),
    Rec(Rec),
    WindowsKey,
    StartWithWindows,
    Step(Stepper, i32),
    Theme(ThemeMode),
    Accent(Accent),
    Size(UiSize),
    Caps(CapsMode),
    Extension(usize),
    Button(Btn),
    AddApp,
    DelApp(usize),
    DelDraftApp,
    AddRemap,
    DelRemap(usize),
    DelDraftRemap,
    ToMenu(MenuFor),
}

struct Fonts {
    title: HFONT,
    label: HFONT,
    small: HFONT,
    kbd: HFONT,
    glyph: HFONT,
    glyph_small: HFONT,
    brand: HFONT,
}

impl Fonts {
    fn create(dpi: u32) -> Self {
        let s = |v: i32| (v * dpi as i32 + 48) / 96;
        Self {
            title: draw::font(s(24), 600, DISPLAY_FACES),
            label: draw::font(s(14), 400, UI_FACES),
            small: draw::font(s(12), 400, UI_FACES),
            kbd: draw::font(s(12), 600, UI_FACES),
            glyph: draw::font(s(16), 400, ICON_FACES),
            glyph_small: draw::font(s(12), 400, ICON_FACES),
            brand: draw::font(s(16), 600, DISPLAY_FACES),
        }
    }
    fn destroy(&self) {
        draw::delete_fonts(&[self.title, self.label, self.small, self.kbd, self.glyph, self.glyph_small, self.brand]);
    }
}

struct Settings {
    hwnd: HWND,
    launcher: HWND,
    dpi: u32,
    theme: Theme,
    fonts: Fonts,
    store: ConfigStore,
    page: Page,
    zones: Vec<(RECT, Ctl)>,
    hover: Option<Ctl>,
    pressed: Option<Ctl>,
    tracking: bool,
    /// Current recording target and the modifiers held so far.
    recording: Option<Rec>,
    rec_mods: u8,
    error: Option<(Rec, String)>,
    notice: Option<(Btn, String, Instant)>,
    /// App picked in the launcher, waiting for its hotkey.
    draft_app: Option<(String, String)>,
    /// New remap being built: Some(from key, if recorded yet).
    draft_remap: Option<Option<u32>>,
    pending_menu: Option<MenuFor>,
    scroll: i32,
    content_h: i32,
    backbuf: BackBuffer,
    logo: Option<HICON>,
    big_logo: Option<HICON>,
}

fn mix(a: COLORREF, b: COLORREF, t: f32) -> COLORREF {
    let ch = |s: u32| {
        let (x, y) = ((a.0 >> s) & 0xFF, (b.0 >> s) & 0xFF);
        ((x as f32 + (y as f32 - x as f32) * t).round() as u32) << s
    };
    COLORREF(ch(0) | ch(8) | ch(16))
}

fn key_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) < 0 }
}

fn keep_label(secs: u64) -> String {
    match secs {
        0 => "Off".into(),
        s if s < 60 => format!("{s} s"),
        s => format!("{} min", s / 60),
    }
}

fn accent_name(a: Accent) -> &'static str {
    match a {
        Accent::System => "Windows accent",
        Accent::Violet => "Violet",
        Accent::Blue => "Blue",
        Accent::Teal => "Teal",
        Accent::Green => "Green",
        Accent::Orange => "Orange",
        Accent::Red => "Red",
        Accent::Pink => "Pink",
    }
}

impl Settings {
    fn s(&self, v: i32) -> i32 {
        (v * self.dpi as i32 + 48) / 96
    }

    fn cfg(&self) -> &Config {
        &self.store.config
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn load_logos(&mut self) {
        for l in [self.logo.take(), self.big_logo.take()].into_iter().flatten() {
            unsafe {
                let _ = DestroyIcon(l);
            }
        }
        self.logo = tray::load_app_icon(self.s(28));
        self.big_logo = tray::load_app_icon(self.s(56));
    }

    fn apply_theme(&mut self) {
        self.theme = theme::resolve(self.cfg().theme, self.cfg().accent);
        let t = self.theme;
        unsafe {
            let dark: i32 = t.dark as i32;
            let _ = DwmSetWindowAttribute(self.hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const _, 4);
            let _ = DwmSetWindowAttribute(self.hwnd, DWMWA_CAPTION_COLOR, &t.bg as *const _ as *const _, 4);
            let _ = DwmSetWindowAttribute(self.hwnd, DWMWA_TEXT_COLOR, &t.text as *const _ as *const _, 4);
        }
        self.invalidate();
    }

    /// Saves settings and tells the launcher to re-apply them.
    fn commit(&mut self) {
        self.store.save();
        unsafe {
            let _ = PostMessageW(Some(self.launcher), WM_APP_CONFIG_CHANGED, WPARAM(0), LPARAM(0));
        }
        self.apply_theme();
    }

    fn set_notice(&mut self, btn: Btn, msg: &str) {
        self.notice = Some((btn, msg.into(), Instant::now() + Duration::from_secs(3)));
        unsafe {
            SetTimer(Some(self.hwnd), NOTICE_TIMER, 3100, None);
        }
    }

    fn error_for(&self, pred: impl Fn(Rec) -> bool) -> Option<String> {
        self.error.as_ref().filter(|(r, _)| pred(*r)).map(|(_, e)| e.clone())
    }

    // ----- recording ----------------------------------------------------------

    fn start_recording(&mut self, r: Rec) {
        self.recording = Some(r);
        self.rec_mods = 0;
        self.error = None;
        input::start_capture(self.hwnd);
        self.invalidate();
    }

    fn stop_recording(&mut self) {
        if self.recording.take().is_some() {
            input::stop_capture();
        }
        self.rec_mods = 0;
        self.invalidate();
    }

    /// Who in SIGMA5 already uses this hotkey (other than `except`).
    fn hotkey_owner(&self, spec: &str, except: Rec) -> Option<String> {
        let same = |h: &str| hotkey::normalize(h).as_deref() == Some(spec);
        if except != Rec::Launcher && same(&self.cfg().hotkey) {
            return Some("the launcher".into());
        }
        self.cfg()
            .app_hotkeys
            .iter()
            .enumerate()
            .find(|(i, b)| except != Rec::App(*i) && same(&b.hotkey))
            .map(|(_, b)| b.name.clone())
    }

    /// A key arrived from the hook while recording.
    fn on_captured(&mut self, code: u32, mods: u8) {
        let Some(r) = self.recording else { return };
        if code == keys::VK_ESCAPE && mods == 0 {
            self.stop_recording();
            return;
        }
        match r {
            Rec::Launcher | Rec::App(_) | Rec::DraftApp => {
                let spec = match hotkey::format(mods, code) {
                    Ok(s) => s,
                    Err(e) => {
                        self.error = Some((r, e)); // keep recording; try again
                        self.invalidate();
                        return;
                    }
                };
                if let Some(owner) = self.hotkey_owner(&spec, r) {
                    self.error = Some((r, format!("{spec} is already used for {owner}")));
                    self.invalidate();
                    return;
                }
                let current = match r {
                    Rec::Launcher => self.cfg().hotkey.clone(),
                    Rec::App(i) => self.cfg().app_hotkeys.get(i).map(|b| b.hotkey.clone()).unwrap_or_default(),
                    _ => String::new(),
                };
                self.stop_recording();
                if hotkey::normalize(&current).as_deref() == Some(spec.as_str()) {
                    return;
                }
                if !hotkey::is_free(self.launcher, &spec) {
                    self.error = Some((r, format!("{spec} is already used by another app")));
                    return;
                }
                let c = &mut self.store.config;
                match r {
                    Rec::Launcher => c.hotkey = spec,
                    Rec::App(i) => {
                        if let Some(b) = c.app_hotkeys.get_mut(i) {
                            b.hotkey = spec;
                        }
                    }
                    _ => {
                        if let Some((app, name)) = self.draft_app.take() {
                            c.app_hotkeys.push(AppHotkey { hotkey: spec, app, name });
                        }
                    }
                }
                self.commit();
            }
            Rec::From(_) | Rec::DraftFrom => {
                let Some(id) = keys::id(code) else {
                    self.error = Some((r, "That key can't be remapped".into()));
                    self.invalidate();
                    return;
                };
                let taken = self
                    .cfg()
                    .remaps
                    .iter()
                    .enumerate()
                    .any(|(i, m)| r != Rec::From(i) && keys::by_name(&m.from) == Some(code));
                if taken {
                    self.error = Some((r, format!("{} is already remapped", keys::label(code))));
                    self.invalidate();
                    return;
                }
                self.stop_recording();
                match r {
                    Rec::From(i) => {
                        if let Some(m) = self.store.config.remaps.get_mut(i) {
                            m.from = id.to_string();
                        }
                        self.commit();
                    }
                    _ => {
                        self.draft_remap = Some(Some(code));
                        self.request_menu(MenuFor::Draft); // straight on to choosing the target
                    }
                }
            }
            Rec::To(_) | Rec::DraftTo => {
                // A key that is itself remapped to modifiers (e.g. Caps → Hyper)
                // pressed alone means "those modifiers".
                let mods_only = keys::is_modifier(code)
                    || self.cfg().key_rules().iter().any(|(c, t)| *c == code && matches!(t, Target::Keys { key: None, .. }));
                let target = if mods_only { Target::Keys { mods, key: None } } else { Target::Keys { mods, key: Some(code) } };
                self.stop_recording();
                let which = if let Rec::To(i) = r { MenuFor::Remap(i) } else { MenuFor::Draft };
                self.set_remap_target(which, target);
            }
        }
    }

    fn set_remap_target(&mut self, which: MenuFor, target: Target) {
        let spec = target.to_spec();
        match which {
            MenuFor::Remap(i) => {
                if let Some(m) = self.store.config.remaps.get_mut(i) {
                    m.to = spec;
                }
            }
            MenuFor::Draft => {
                let Some(Some(from)) = self.draft_remap.take() else { return };
                let Some(id) = keys::id(from) else { return };
                self.store.config.remaps.push(Remap { from: id.to_string(), to: spec });
            }
        }
        self.error = None;
        self.commit();
    }

    /// Popup menus run a modal loop, so they are shown from a posted message
    /// where the state isn't borrowed.
    fn request_menu(&mut self, which: MenuFor) {
        self.pending_menu = Some(which);
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_APP_SETTINGS_MENU, WPARAM(0), LPARAM(0));
        }
    }

    fn on_menu(&mut self, which: MenuFor, cmd: usize) {
        match cmd {
            0 => {}
            MENU_RECORD => self.start_recording(if let MenuFor::Remap(i) = which { Rec::To(i) } else { Rec::DraftTo }),
            MENU_HYPER => self.set_remap_target(which, Target::HYPER),
            MENU_DISABLED => self.set_remap_target(which, Target::Disabled),
            c => {
                if let Some((_, vk, _)) = MENU_MEDIA.iter().find(|(id, _, _)| *id == c) {
                    self.set_remap_target(which, Target::Keys { mods: 0, key: Some(*vk) });
                }
            }
        }
        self.invalidate();
    }

    // ----- actions --------------------------------------------------------------

    fn act(&mut self, ctl: Ctl) {
        if let Some(r) = self.recording {
            if ctl != Ctl::Rec(r) {
                self.stop_recording();
            }
        }
        match ctl {
            Ctl::Nav(p) => {
                if self.page != p {
                    self.page = p;
                    self.scroll = 0;
                    self.error = None;
                }
            }
            Ctl::Rec(r) => {
                if self.recording == Some(r) {
                    self.stop_recording();
                } else {
                    self.start_recording(r);
                }
            }
            Ctl::WindowsKey => {
                self.store.config.windows_key ^= true;
                self.commit();
            }
            Ctl::StartWithWindows => {
                self.store.config.start_with_windows ^= true;
                self.commit();
            }
            Ctl::Step(which, dir) => {
                let c = &mut self.store.config;
                match which {
                    Stepper::KeepQuery => {
                        let i = KEEP_STEPS.iter().position(|&v| v >= c.keep_query_secs).unwrap_or(KEEP_STEPS.len() - 1);
                        let j = (i as i32 + dir).clamp(0, KEEP_STEPS.len() as i32 - 1) as usize;
                        c.keep_query_secs = KEEP_STEPS[j];
                    }
                    Stepper::Recent => {
                        c.recent_count = (c.recent_count as i32 + dir).clamp(0, RECENT_MAX as i32) as usize;
                    }
                }
                self.commit();
            }
            Ctl::Theme(m) => {
                self.store.config.theme = m;
                self.commit();
            }
            Ctl::Accent(a) => {
                self.store.config.accent = a;
                self.commit();
            }
            Ctl::Size(z) => {
                self.store.config.size = z;
                self.commit();
            }
            Ctl::Caps(m) => {
                self.store.config.caps_lock = m;
                self.commit();
            }
            Ctl::Extension(i) => {
                let id = EXTENSIONS[i].id.to_string();
                let d = &mut self.store.config.disabled_extensions;
                if let Some(pos) = d.iter().position(|x| *x == id) {
                    d.remove(pos);
                } else {
                    d.push(id);
                }
                self.commit();
            }
            Ctl::AddApp => unsafe {
                // The launcher becomes an app picker and sends WM_APP_APP_PICKED back.
                let _ = PostMessageW(Some(self.launcher), WM_APP_PICK_APP, WPARAM(self.hwnd.0 as usize), LPARAM(0));
            },
            Ctl::DelApp(i) => {
                if i < self.store.config.app_hotkeys.len() {
                    self.store.config.app_hotkeys.remove(i);
                    self.error = None;
                    self.commit();
                }
            }
            Ctl::DelDraftApp => self.draft_app = None,
            Ctl::AddRemap => {
                self.draft_remap = Some(None);
                self.start_recording(Rec::DraftFrom);
            }
            Ctl::DelRemap(i) => {
                if i < self.store.config.remaps.len() {
                    self.store.config.remaps.remove(i);
                    self.error = None;
                    self.commit();
                }
            }
            Ctl::DelDraftRemap => self.draft_remap = None,
            Ctl::ToMenu(which) => self.request_menu(which),
            Ctl::Button(Btn::OpenConfig) => launch::open_file(self.store.path()),
            Ctl::Button(Btn::OpenData) => launch::open_folder(&data_dir()),
            Ctl::Button(Btn::ClearHistory) => {
                unsafe {
                    let _ = PostMessageW(Some(self.launcher), WM_APP_CLEAR_HISTORY, WPARAM(0), LPARAM(0));
                }
                self.set_notice(Btn::ClearHistory, "History cleared");
            }
            Ctl::Button(Btn::Reset) => {
                self.store.config = Config::default();
                self.error = None;
                self.draft_app = None;
                self.draft_remap = None;
                self.commit();
                self.set_notice(Btn::Reset, "All settings restored to defaults");
            }
        }
        self.invalidate();
    }

    fn on_app_picked(&mut self, id: String, name: String) {
        self.page = Page::Hotkeys;
        self.draft_app = Some((id, name));
        self.start_recording(Rec::DraftApp);
        // Scroll so the new row is visible.
        self.scroll = i32::MAX / 2;
    }

    fn zone_at(&self, x: i32, y: i32) -> Option<Ctl> {
        self.zones.iter().rev().find(|(r, _)| contains(r, x, y)).map(|(_, c)| *c)
    }

    fn on_mouse_move(&mut self, x: i32, y: i32) {
        if !self.tracking {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut tme);
            }
            self.tracking = true;
        }
        let h = self.zone_at(x, y);
        if h != self.hover {
            self.hover = h;
            self.invalidate();
        }
    }

    fn on_wheel(&mut self, delta: i32) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        let max = (self.content_h - rc.bottom).max(0);
        let s = (self.scroll - delta * self.s(60) / 120).clamp(0, max);
        if s != self.scroll {
            self.scroll = s;
            self.invalidate();
        }
    }

    fn on_key(&mut self, vk: VIRTUAL_KEY) -> bool {
        match vk {
            VK_ESCAPE => {
                unsafe {
                    let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
                true
            }
            VK_TAB if key_down(VK_CONTROL) => {
                let i = Page::ALL.iter().position(|p| *p == self.page).unwrap_or(0) as i32;
                let d = if key_down(VK_SHIFT) { -1 } else { 1 };
                self.page = Page::ALL[(i + d).rem_euclid(Page::ALL.len() as i32) as usize];
                self.scroll = 0;
                self.invalidate();
                true
            }
            _ => false,
        }
    }

    fn on_dpi(&mut self, dpi: u32) {
        self.dpi = dpi;
        self.fonts.destroy();
        self.fonts = Fonts::create(dpi);
        self.load_logos();
        self.invalidate();
    }

    fn destroy(&mut self) {
        self.stop_recording();
        self.fonts.destroy();
        for l in [self.logo.take(), self.big_logo.take()].into_iter().flatten() {
            unsafe {
                let _ = DestroyIcon(l);
            }
        }
    }

    // ----- painting ---------------------------------------------------------------

    fn paint(&mut self) {
        let mut ps = PAINTSTRUCT::default();
        let hdc = unsafe { BeginPaint(self.hwnd, &mut ps) };
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        let (w, h) = (rc.right, rc.bottom);
        if w > 0 && h > 0 {
            let dc = self.backbuf.ensure(hdc, w, h);
            self.zones.clear();
            fill(dc, &rc, self.theme.bg);

            // Content first (scrolls), then the fixed sidebar on top.
            let x0 = self.s(SIDEBAR_W) + self.s(PAD);
            let x1 = w - self.s(PAD);
            // Clamp scroll against the previous frame's content height.
            self.scroll = self.scroll.clamp(0, (self.content_h - h).max(0));
            let off = -self.scroll;
            let title_rc = RECT { left: x0, top: self.s(22) + off, right: x1, bottom: self.s(62) + off };
            text(dc, self.page.title(), &title_rc, self.fonts.title, self.theme.text, DT_LEFT);
            let y = self.s(80) + off;
            let bottom = match self.page {
                Page::General => self.draw_general(dc, x0, x1, y),
                Page::Appearance => self.draw_appearance(dc, x0, x1, y),
                Page::Hotkeys => self.draw_hotkeys(dc, x0, x1, y),
                Page::Keyboard => self.draw_keyboard(dc, x0, x1, y),
                Page::Extensions => self.draw_extensions(dc, x0, x1, y),
                Page::About => self.draw_about(dc, x0, x1, y),
            };
            let new_h = bottom - off + self.s(24);
            if new_h != self.content_h {
                self.content_h = new_h;
                if self.scroll > (new_h - h).max(0) {
                    self.invalidate(); // re-clamp on next frame (e.g. after picking an app)
                }
            }

            // Sidebar (fixed). Zones drawn later win in hit-testing.
            let sw = self.s(SIDEBAR_W);
            fill(dc, &RECT { left: 0, top: 0, right: sw, bottom: h }, self.theme.bg);
            self.draw_sidebar(dc, h);

            // Scroll indicator.
            if self.content_h > h {
                let track = h - self.s(16);
                let thumb = (track * h / self.content_h).max(self.s(24));
                let y = self.s(8) + (track - thumb) * self.scroll / (self.content_h - h).max(1);
                let x = w - self.s(6);
                round_rect(dc, &RECT { left: x - self.s(3), top: y, right: x, bottom: y + thumb }, self.s(3), self.theme.scrollbar);
            }
            unsafe {
                let _ = BitBlt(hdc, 0, 0, w, h, Some(dc), 0, 0, SRCCOPY);
            }
        }
        unsafe {
            let _ = EndPaint(self.hwnd, &ps);
        }
    }

    fn draw_sidebar(&mut self, dc: HDC, h: i32) {
        let t = self.theme;
        let sw = self.s(SIDEBAR_W);
        fill(dc, &RECT { left: sw, top: 0, right: sw + 1, bottom: h }, t.divider);

        let lx = self.s(22);
        if let Some(logo) = self.logo {
            let sz = self.s(28);
            unsafe {
                let _ = DrawIconEx(dc, lx, self.s(24), logo, sz, sz, 0, None, DI_NORMAL);
            }
        }
        let bx = lx + self.s(40);
        text(dc, "SIGMA5", &RECT { left: bx, top: self.s(20), right: sw, bottom: self.s(40) }, self.fonts.brand, t.text, DT_LEFT);
        text(dc, "Settings", &RECT { left: bx, top: self.s(38), right: sw, bottom: self.s(56) }, self.fonts.small, t.subtext, DT_LEFT);

        let mut y = self.s(84);
        for p in Page::ALL {
            let rc = RECT { left: self.s(12), top: y, right: sw - self.s(12), bottom: y + self.s(38) };
            let ctl = Ctl::Nav(p);
            if self.page == p {
                round_rect(dc, &rc, self.s(6), t.selection);
                let bar = RECT { left: rc.left, top: rc.top + self.s(11), right: rc.left + self.s(3), bottom: rc.bottom - self.s(11) };
                round_rect(dc, &bar, self.s(2), t.accent);
            } else if self.hover == Some(ctl) {
                round_rect(dc, &rc, self.s(6), mix(t.bg, t.selection, 0.6));
            }
            let g = RECT { left: rc.left + self.s(14), right: rc.left + self.s(36), ..rc };
            text(dc, &p.glyph().to_string(), &g, self.fonts.glyph, t.text, DT_CENTER);
            let l = RECT { left: rc.left + self.s(46), ..rc };
            text(dc, p.nav_label(), &l, self.fonts.label, t.text, DT_LEFT);
            self.zones.push((rc, ctl));
            y += self.s(42);
        }

        let v = format!("Version {}", env!("CARGO_PKG_VERSION"));
        let vr = RECT { left: self.s(24), top: h - self.s(40), right: sw, bottom: h - self.s(16) };
        text(dc, &v, &vr, self.fonts.small, t.subtext, DT_LEFT);
    }

    /// Draws a setting card and returns its rect.
    fn card(&self, dc: HDC, x0: i32, x1: i32, y: i32, h: i32) -> RECT {
        let rc = RECT { left: x0, top: y, right: x1, bottom: y + h };
        bordered_round_rect(dc, &rc, self.s(CARD_R), 1, self.theme.card, self.theme.card_border);
        rc
    }

    /// Label (+ optional description) on the left of a card, clipped before `right`.
    fn labels(&self, dc: HDC, rc: &RECT, left: i32, right: i32, label: &str, desc: &str, desc_c: COLORREF) {
        if desc.is_empty() {
            let lr = RECT { left, right, ..*rc };
            text(dc, label, &lr, self.fonts.label, self.theme.text, DT_LEFT | DT_END_ELLIPSIS);
            return;
        }
        let mid = (rc.top + rc.bottom) / 2;
        let lr = RECT { left, top: mid - self.s(20), right, bottom: mid };
        text(dc, label, &lr, self.fonts.label, self.theme.text, DT_LEFT | DT_END_ELLIPSIS);
        let dr = RECT { left, top: mid + self.s(1), right, bottom: mid + self.s(19) };
        text(dc, desc, &dr, self.fonts.small, desc_c, DT_LEFT | DT_END_ELLIPSIS);
    }

    fn note(&self, dc: HDC, x0: i32, x1: i32, y: i32, msg: &str, color: COLORREF) -> i32 {
        let r = RECT { left: x0 + self.s(4), top: y, right: x1, bottom: y + self.s(20) };
        text(dc, msg, &r, self.fonts.small, color, DT_LEFT | DT_END_ELLIPSIS);
        y + self.s(20)
    }

    fn row_y(&self, y: i32, i: i32) -> i32 {
        y + i * (self.s(ROW_H) + self.s(ROW_GAP))
    }

    // ----- controls (each returns its left edge or rect) ---------------------------

    fn toggle(&mut self, dc: HDC, right: i32, cy: i32, on: bool, ctl: Ctl) -> i32 {
        let t = self.theme;
        let (tw, th) = (self.s(40), self.s(20));
        let track = RECT { left: right - tw, top: cy - th / 2, right, bottom: cy - th / 2 + th };
        let hovered = self.hover == Some(ctl);
        let col = if on {
            if hovered { mix(t.accent, WHITE, 0.12) } else { t.accent }
        } else if hovered {
            mix(t.toggle_off, t.text, 0.15)
        } else {
            t.toggle_off
        };
        round_rect(dc, &track, th, col);
        let d = self.s(14);
        let kx = if on { track.right - th / 2 } else { track.left + th / 2 };
        circle(dc, kx, cy, d, WHITE);
        let label = if on { "On" } else { "Off" };
        let lw = text_width(dc, label, self.fonts.label);
        let lr = RECT { left: track.left - self.s(10) - lw, top: track.top - self.s(6), right: track.left - self.s(10), bottom: track.bottom + self.s(6) };
        text(dc, label, &lr, self.fonts.label, t.text, DT_RIGHT);
        let zone = RECT { left: lr.left - self.s(6), top: lr.top, right: right + self.s(4), bottom: lr.bottom };
        self.zones.push((zone, ctl));
        zone.left
    }

    fn segmented(&mut self, dc: HDC, right: i32, cy: i32, opts: &[(&str, Ctl, bool)]) -> i32 {
        let t = self.theme;
        let widths: Vec<i32> = opts.iter().map(|o| text_width(dc, o.0, self.fonts.label) + self.s(26)).collect();
        let h = self.s(34);
        let p = self.s(3);
        let total = widths.iter().sum::<i32>() + 2 * p;
        let outer = RECT { left: right - total, top: cy - h / 2, right, bottom: cy - h / 2 + h };
        round_rect(dc, &outer, self.s(8), t.kbd_bg);
        let mut l = outer.left + p;
        for ((label, ctl, sel), w) in opts.iter().zip(widths) {
            let rc = RECT { left: l, top: outer.top + p, right: l + w, bottom: outer.bottom - p };
            if *sel {
                bordered_round_rect(dc, &rc, self.s(6), 1, t.seg_selected, if t.dark { t.seg_selected } else { t.card_border });
            } else if self.hover == Some(*ctl) {
                round_rect(dc, &rc, self.s(6), t.hover);
            }
            text(dc, label, &rc, self.fonts.label, if *sel { t.text } else { t.subtext }, DT_CENTER);
            self.zones.push((rc, *ctl));
            l += w;
        }
        outer.left
    }

    fn swatches(&mut self, dc: HDC, right: i32, cy: i32) -> i32 {
        let t = self.theme;
        let d = self.s(22);
        let gap = self.s(10);
        let n = Accent::ALL.len() as i32;
        let total = n * d + (n - 1) * gap;
        let mut cx = right - total + d / 2;
        for &a in Accent::ALL {
            let ctl = Ctl::Accent(a);
            let sel = self.cfg().accent == a;
            if sel {
                circle(dc, cx, cy, d + self.s(8), t.text);
                circle(dc, cx, cy, d + self.s(4), t.card);
            } else if self.hover == Some(ctl) {
                circle(dc, cx, cy, d + self.s(8), t.hover);
                circle(dc, cx, cy, d + self.s(4), t.card);
            }
            circle(dc, cx, cy, d, accent_color(a));
            if a == Accent::System {
                circle(dc, cx, cy, d - self.s(10), t.card); // ring = "follow Windows"
            }
            let hit = RECT { left: cx - d / 2 - gap / 2, top: cy - d / 2 - self.s(6), right: cx + d / 2 + gap / 2, bottom: cy + d / 2 + self.s(6) };
            self.zones.push((hit, ctl));
            cx += d + gap;
        }
        right - total - self.s(6)
    }

    fn stepper(&mut self, dc: HDC, right: i32, cy: i32, value: &str, which: Stepper, at_min: bool, at_max: bool) -> i32 {
        let t = self.theme;
        let b = self.s(30);
        let vw = self.s(72);
        let plus = RECT { left: right - b, top: cy - b / 2, right, bottom: cy - b / 2 + b };
        let minus = RECT { left: plus.left - vw - b, right: plus.left - vw, ..plus };
        for (rc, dir, glyph, disabled) in [(minus, -1, "\u{E738}", at_min), (plus, 1, "\u{E710}", at_max)] {
            let ctl = Ctl::Step(which, dir);
            let bg = if self.hover == Some(ctl) && !disabled { t.hover } else { t.kbd_bg };
            round_rect(dc, &rc, self.s(6), bg);
            text(dc, glyph, &rc, self.fonts.glyph_small, if disabled { t.subtext } else { t.text }, DT_CENTER);
            self.zones.push((rc, ctl));
        }
        let vr = RECT { left: minus.right, right: plus.left, ..plus };
        text(dc, value, &vr, self.fonts.label, t.text, DT_CENTER);
        minus.left
    }

    /// Keycaps of the modifiers held while recording.
    fn live_keys(&self) -> Vec<String> {
        let mut v = keys::modifier_labels(self.rec_mods);
        if !v.is_empty() {
            v.push("…".into());
        }
        v
    }

    /// A clickable box of keycaps. Anchored at `x` (its right edge if `anchor_right`).
    #[allow(clippy::too_many_arguments)]
    fn keys_box(
        &mut self, dc: HDC, x: i32, anchor_right: bool, cy: i32, labels: &[String], placeholder: &str,
        recording: bool, ctl: Ctl, min_w: i32,
    ) -> RECT {
        let t = self.theme;
        let kh = self.s(26);
        let gap = self.s(6);
        let widths: Vec<i32> = labels.iter().map(|k| (text_width(dc, k, self.fonts.kbd) + self.s(18)).max(kh)).collect();
        let inner_w = if labels.is_empty() {
            text_width(dc, placeholder, self.fonts.label)
        } else {
            widths.iter().sum::<i32>() + gap * (widths.len() as i32 - 1)
        };
        let h = self.s(40);
        let box_w = (inner_w + 2 * self.s(8)).max(min_w);
        let (l, r) = if anchor_right { (x - box_w, x) } else { (x, x + box_w) };
        let rc = RECT { left: l, top: cy - h / 2, right: r, bottom: cy - h / 2 + h };
        if recording {
            bordered_round_rect(dc, &rc, self.s(8), self.s(2), t.card, t.accent);
        } else if self.hover == Some(ctl) {
            round_rect(dc, &rc, self.s(8), t.hover);
        } else {
            bordered_round_rect(dc, &rc, self.s(8), 1, t.card, t.card_border);
        }
        if labels.is_empty() {
            text(dc, placeholder, &rc, self.fonts.label, if recording { t.accent } else { t.subtext }, DT_CENTER);
        } else {
            let mut kx = rc.left + (box_w - inner_w) / 2;
            for (k, kw) in labels.iter().zip(widths) {
                let kr = RECT { left: kx, top: cy - kh / 2, right: kx + kw, bottom: cy - kh / 2 + kh };
                round_rect(dc, &kr, self.s(5), t.kbd_bg);
                text(dc, k, &kr, self.fonts.kbd, t.kbd_text, DT_CENTER);
                kx += kw + gap;
            }
        }
        self.zones.push((rc, ctl));
        rc
    }

    fn button(&mut self, dc: HDC, right: i32, cy: i32, label: &str, btn: Btn, danger: bool) -> i32 {
        let t = self.theme;
        let h = self.s(32);
        let w = text_width(dc, label, self.fonts.label) + self.s(36);
        let rc = RECT { left: right - w, top: cy - h / 2, right, bottom: cy - h / 2 + h };
        let ctl = Ctl::Button(btn);
        let bg = if self.hover == Some(ctl) { t.hover } else { t.kbd_bg };
        round_rect(dc, &rc, self.s(6), bg);
        text(dc, label, &rc, self.fonts.label, if danger { t.warn } else { t.text }, DT_CENTER);
        self.zones.push((rc, ctl));
        rc.left
    }

    /// Small square glyph button (e.g. delete). Returns its left edge.
    fn icon_button(&mut self, dc: HDC, right: i32, cy: i32, glyph: char, ctl: Ctl) -> i32 {
        let t = self.theme;
        let b = self.s(30);
        let rc = RECT { left: right - b, top: cy - b / 2, right, bottom: cy - b / 2 + b };
        if self.hover == Some(ctl) {
            round_rect(dc, &rc, self.s(6), t.hover);
        }
        text(dc, &glyph.to_string(), &rc, self.fonts.glyph_small, t.subtext, DT_CENTER);
        self.zones.push((rc, ctl));
        rc.left
    }

    /// Full-width "+ Add …" button. Returns its bottom.
    fn add_button(&mut self, dc: HDC, x0: i32, x1: i32, y: i32, label: &str, ctl: Ctl) -> i32 {
        let t = self.theme;
        let rc = RECT { left: x0, top: y, right: x1, bottom: y + self.s(44) };
        let fill_c = if self.hover == Some(ctl) { t.hover } else { t.card };
        bordered_round_rect(dc, &rc, self.s(CARD_R), 1, fill_c, t.card_border);
        // Glyph and text use different fonts, so draw them separately.
        let gw = self.s(14);
        let tw = text_width(dc, label, self.fonts.label);
        let total = gw + self.s(10) + tw;
        let gx = (rc.left + rc.right - total) / 2;
        text(dc, "\u{E710}", &RECT { left: gx, right: gx + gw, ..rc }, self.fonts.glyph_small, t.accent, DT_CENTER);
        text(dc, label, &RECT { left: gx + gw + self.s(10), right: rc.right, ..rc }, self.fonts.label, t.accent, DT_LEFT);
        self.zones.push((rc, ctl));
        rc.bottom
    }

    /// App icon from the shared cache, or a letter tile.
    fn app_icon(&self, dc: HDC, rc: &RECT, id: &str, name: &str) {
        let size = rc.right - rc.left;
        if let Some(ic) = icons::get(id) {
            unsafe {
                let mem = CreateCompatibleDC(Some(dc));
                let old = SelectObject(mem, ic.bmp.into());
                let bf = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
                let _ = AlphaBlend(dc, rc.left, rc.top, size, size, mem, 0, 0, ic.w, ic.h, bf);
                SelectObject(mem, old);
                let _ = DeleteDC(mem);
            }
        } else {
            round_rect(dc, rc, self.s(6), self.theme.tile);
            let letter: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
            text(dc, &letter, rc, self.fonts.kbd, self.theme.tile_text, DT_CENTER);
        }
    }

    // ----- pages ------------------------------------------------------------------

    fn draw_general(&mut self, dc: HDC, x0: i32, x1: i32, y: i32) -> i32 {
        let t = self.theme;
        let ix = x0 + self.s(20);
        let right = x1 - self.s(18);
        let rh = self.s(ROW_H);
        let mut y = y;

        if let Some(err) = self.store.error.clone() {
            self.note(dc, x0, x1, y - self.s(10), &format!("{err} (changing a setting will rewrite the file)"), t.warn);
            y += self.s(20);
        }

        // Launcher hotkey
        let rc = self.card(dc, x0, x1, self.row_y(y, 0), rh);
        let rec = self.recording == Some(Rec::Launcher);
        let labels = if rec { self.live_keys() } else { hotkey::parts(&self.cfg().hotkey) };
        let b = self.keys_box(dc, right, true, (rc.top + rc.bottom) / 2, &labels, "Press keys…", rec, Ctl::Rec(Rec::Launcher), self.s(150));
        let (desc, col) = match (self.error_for(|r| r == Rec::Launcher), rec) {
            (Some(e), _) => (e, t.warn),
            (None, true) => ("Press the new shortcut, or Esc to cancel".to_string(), t.accent),
            (None, false) => ("Opens SIGMA5 from anywhere. Click to change".to_string(), t.subtext),
        };
        self.labels(dc, &rc, ix, b.left - self.s(16), "Launcher hotkey", &desc, col);

        let rc = self.card(dc, x0, x1, self.row_y(y, 1), rh);
        let on = self.cfg().windows_key;
        let left = self.toggle(dc, right, (rc.top + rc.bottom) / 2, on, Ctl::WindowsKey);
        self.labels(dc, &rc, ix, left - self.s(16), "Open with Windows key", "Replaces the Start menu. Ctrl+Esc still opens Start", t.subtext);

        let rc = self.card(dc, x0, x1, self.row_y(y, 2), rh);
        let on = self.cfg().start_with_windows;
        let left = self.toggle(dc, right, (rc.top + rc.bottom) / 2, on, Ctl::StartWithWindows);
        self.labels(dc, &rc, ix, left - self.s(16), "Launch at login", "Start SIGMA5 in the background when you sign in", t.subtext);

        let rc = self.card(dc, x0, x1, self.row_y(y, 3), rh);
        let k = self.cfg().keep_query_secs;
        let left = self.stepper(
            dc, right, (rc.top + rc.bottom) / 2, &keep_label(k), Stepper::KeepQuery,
            k <= KEEP_STEPS[0], k >= *KEEP_STEPS.last().unwrap(),
        );
        self.labels(dc, &rc, ix, left - self.s(16), "Keep last search", "Reopen within this time to pick up where you left off", t.subtext);

        let rc = self.card(dc, x0, x1, self.row_y(y, 4), rh);
        let r = self.cfg().recent_count;
        let label = if r == 0 { "Off".to_string() } else { r.to_string() };
        let left = self.stepper(dc, right, (rc.top + rc.bottom) / 2, &label, Stepper::Recent, r == 0, r >= RECENT_MAX);
        self.labels(dc, &rc, ix, left - self.s(16), "Recent items", "Recently used items shown when the search box is empty", t.subtext);

        let tip_y = self.row_y(y, 5) + self.s(6);
        self.note(dc, x0, x1, tip_y, "Tip: press Ctrl+, in the launcher to open settings. Ctrl+Tab switches pages here.", t.subtext)
    }

    fn draw_appearance(&mut self, dc: HDC, x0: i32, x1: i32, y: i32) -> i32 {
        let t = self.theme;
        let ix = x0 + self.s(20);
        let right = x1 - self.s(18);
        let rh = self.s(ROW_H);

        let rc = self.card(dc, x0, x1, self.row_y(y, 0), rh);
        let cur = self.cfg().theme;
        let opts: Vec<(&str, Ctl, bool)> = ThemeMode::ALL.iter().map(|m| (m.label(), Ctl::Theme(*m), *m == cur)).collect();
        let left = self.segmented(dc, right, (rc.top + rc.bottom) / 2, &opts);
        self.labels(dc, &rc, ix, left - self.s(16), "Theme", "Follow Windows or pick one", t.subtext);

        let rc = self.card(dc, x0, x1, self.row_y(y, 1), rh);
        let left = self.swatches(dc, right, (rc.top + rc.bottom) / 2);
        let shown = match self.hover {
            Some(Ctl::Accent(a)) => a,
            _ => self.cfg().accent,
        };
        self.labels(dc, &rc, ix, left - self.s(16), "Accent color", accent_name(shown), t.subtext);

        let rc = self.card(dc, x0, x1, self.row_y(y, 2), rh);
        let cur = self.cfg().size;
        let opts: Vec<(&str, Ctl, bool)> = UiSize::ALL.iter().map(|z| (z.label(), Ctl::Size(*z), *z == cur)).collect();
        let left = self.segmented(dc, right, (rc.top + rc.bottom) / 2, &opts);
        self.labels(dc, &rc, ix, left - self.s(16), "Launcher size", "Window and text scale", t.subtext);
        rc.bottom
    }

    fn draw_hotkeys(&mut self, dc: HDC, x0: i32, x1: i32, y: i32) -> i32 {
        let t = self.theme;
        let right = x1 - self.s(14);
        let mut y = self.note(
            dc, x0, x1, y - self.s(8),
            "Launch the app, bring it to the front, or minimise it if it's already in front.",
            t.subtext,
        ) + self.s(10);

        // (rec, app id, name, hotkey, delete control)
        let mut rows: Vec<(Rec, String, String, String, Ctl)> = self
            .cfg()
            .app_hotkeys
            .iter()
            .enumerate()
            .map(|(i, b)| (Rec::App(i), b.app.clone(), b.name.clone(), b.hotkey.clone(), Ctl::DelApp(i)))
            .collect();
        if let Some((id, name)) = &self.draft_app {
            rows.push((Rec::DraftApp, id.clone(), name.clone(), String::new(), Ctl::DelDraftApp));
        }

        let rh = self.s(LIST_ROW_H);
        for (rec, app, name, spec, del) in rows {
            let rc = self.card(dc, x0, x1, y, rh);
            let cy = (rc.top + rc.bottom) / 2;
            let isz = self.s(28);
            let irc = RECT { left: x0 + self.s(16), top: cy - isz / 2, right: x0 + self.s(16) + isz, bottom: cy - isz / 2 + isz };
            self.app_icon(dc, &irc, &app, &name);
            let dl = self.icon_button(dc, right, cy, '\u{E711}', del);
            let recording = self.recording == Some(rec);
            let labels = if recording { self.live_keys() } else { hotkey::parts(&spec) };
            let placeholder = if recording { "Press keys…" } else { "Set hotkey" };
            let b = self.keys_box(dc, dl - self.s(8), true, cy, &labels, placeholder, recording, Ctl::Rec(rec), self.s(140));
            let (desc, col) = match (self.error_for(|r| r == rec), recording) {
                (Some(e), _) => (e, t.warn),
                (None, true) => ("Press the shortcut (Caps + key if Caps is Hyper), or Esc".to_string(), t.accent),
                (None, false) => (String::new(), t.subtext),
            };
            self.labels(dc, &rc, irc.right + self.s(14), b.left - self.s(12), &name, &desc, col);
            y = rc.bottom + self.s(ROW_GAP);
        }

        let bottom = self.add_button(dc, x0, x1, y, "Add app hotkey", Ctl::AddApp);
        let tip = if self.cfg().caps_lock == CapsMode::Hyper {
            "Caps Lock is Hyper: hold Caps and press a letter to record Ctrl+Alt+Shift+letter."
        } else {
            "Tip: set Caps Lock to Hyper on the Keyboard page for one-hand app hotkeys."
        };
        self.note(dc, x0, x1, bottom + self.s(8), tip, t.subtext)
    }

    fn draw_keyboard(&mut self, dc: HDC, x0: i32, x1: i32, y: i32) -> i32 {
        let t = self.theme;
        let ix = x0 + self.s(20);
        let right = x1 - self.s(14);

        // Caps Lock
        let rc = self.card(dc, x0, x1, y, self.s(ROW_H));
        let cur = self.cfg().caps_lock;
        let opts: Vec<(&str, Ctl, bool)> = CapsMode::ALL.iter().map(|m| (m.label(), Ctl::Caps(*m), *m == cur)).collect();
        let left = self.segmented(dc, right - self.s(4), (rc.top + rc.bottom) / 2, &opts);
        let desc = match cur {
            CapsMode::Default => "Works as normal",
            CapsMode::Hyper => "Acts as Ctrl+Shift+Alt",
            CapsMode::Ctrl => "Acts as Ctrl",
            CapsMode::Escape => "Acts as Escape",
            CapsMode::Off => "Does nothing",
        };
        self.labels(dc, &rc, ix, left - self.s(12), "Caps Lock", desc, t.subtext);

        // Remaps
        let mut y = rc.bottom + self.s(22);
        let hr = RECT { left: x0 + self.s(4), top: y, right: x1, bottom: y + self.s(22) };
        text(dc, "Key remaps", &hr, self.fonts.label, t.text, DT_LEFT);
        y += self.s(22);
        let (msg, col) = match (self.error_for(Rec::is_remap), self.recording) {
            (Some(e), _) => (e, t.warn),
            (None, Some(Rec::DraftFrom | Rec::From(_))) => ("Press the key to remap, or Esc to cancel".to_string(), t.accent),
            (None, Some(Rec::DraftTo | Rec::To(_))) => ("Press the key or shortcut to send, or Esc to cancel".to_string(), t.accent),
            _ => ("Change what any key does, e.g. numpad keys to media controls.".to_string(), t.subtext),
        };
        y = self.note(dc, x0, x1, y, &msg, col) + self.s(10);

        // (from code, to target, rec-from, rec-to, menu target, delete)
        let mut rows: Vec<(Option<u32>, Option<Target>, Rec, Rec, MenuFor, Ctl)> = self
            .cfg()
            .remaps
            .iter()
            .enumerate()
            .map(|(i, m)| (keys::by_name(&m.from), Target::parse(&m.to).ok(), Rec::From(i), Rec::To(i), MenuFor::Remap(i), Ctl::DelRemap(i)))
            .collect();
        if let Some(from) = self.draft_remap {
            rows.push((from, None, Rec::DraftFrom, Rec::DraftTo, MenuFor::Draft, Ctl::DelDraftRemap));
        }

        let rh = self.s(LIST_ROW_H);
        for (from, to, rf, rt, menu, del) in rows {
            let rc = self.card(dc, x0, x1, y, rh);
            let cy = (rc.top + rc.bottom) / 2;
            let w = self.s(150);

            let rec_from = self.recording == Some(rf);
            let from_labels = if rec_from { Vec::new() } else { from.map(|c| vec![keys::label(c)]).unwrap_or_default() };
            let fb = self.keys_box(dc, x0 + self.s(16), false, cy, &from_labels, if rec_from { "Press a key…" } else { "Choose key" }, rec_from, Ctl::Rec(rf), w);

            let ar = RECT { left: fb.right, top: rc.top, right: fb.right + self.s(40), bottom: rc.bottom };
            text(dc, "\u{E72A}", &ar, self.fonts.glyph_small, t.subtext, DT_CENTER);

            let rec_to = self.recording == Some(rt);
            let to_labels = if rec_to { self.live_keys() } else { to.map(Target::labels).unwrap_or_default() };
            let placeholder = if rec_to { "Press keys…" } else { "Choose…" };
            // "To" opens a menu (media keys etc.); while recording, clicking it cancels.
            let to_ctl = if rec_to { Ctl::Rec(rt) } else { Ctl::ToMenu(menu) };
            self.keys_box(dc, ar.right, false, cy, &to_labels, placeholder, rec_to, to_ctl, w);

            self.icon_button(dc, right, cy, '\u{E711}', del);
            y = rc.bottom + self.s(ROW_GAP);
        }

        let bottom = self.add_button(dc, x0, x1, y, "Add remap", Ctl::AddRemap);
        self.note(dc, x0, x1, bottom + self.s(8), "Remaps don't apply in apps running as administrator (Windows blocks it).", t.subtext)
    }

    fn draw_extensions(&mut self, dc: HDC, x0: i32, x1: i32, y: i32) -> i32 {
        let t = self.theme;
        let right = x1 - self.s(18);
        let rh = self.s(ROW_H);
        for (i, ext) in EXTENSIONS.iter().enumerate() {
            let rc = self.card(dc, x0, x1, self.row_y(y, i as i32), rh);
            let cy = (rc.top + rc.bottom) / 2;
            let tile = self.s(32);
            let tr = RECT { left: x0 + self.s(18), top: cy - tile / 2, right: x0 + self.s(18) + tile, bottom: cy - tile / 2 + tile };
            let enabled = self.cfg().extension_enabled(ext.id);
            round_rect(dc, &tr, self.s(7), if enabled { t.accent } else { t.toggle_off });
            text(dc, &ext.glyph.to_string(), &tr, self.fonts.glyph, WHITE, DT_CENTER);
            let left = self.toggle(dc, right, cy, enabled, Ctl::Extension(i));
            self.labels(dc, &rc, tr.right + self.s(14), left - self.s(16), ext.name, ext.description, t.subtext);
        }
        let ny = self.row_y(y, EXTENSIONS.len() as i32) + self.s(6);
        self.note(dc, x0, x1, ny, "More extensions, such as Google Search and Ask AI, are on the way.", t.subtext)
    }

    fn draw_about(&mut self, dc: HDC, x0: i32, x1: i32, y: i32) -> i32 {
        let t = self.theme;
        let ix = x0 + self.s(20);
        let right = x1 - self.s(18);
        let rh = self.s(ROW_H);

        let hh = self.s(96);
        let rc = self.card(dc, x0, x1, y, hh);
        if let Some(logo) = self.big_logo {
            let sz = self.s(56);
            unsafe {
                let _ = DrawIconEx(dc, ix, rc.top + (hh - sz) / 2, logo, sz, sz, 0, None, DI_NORMAL);
            }
        }
        let tx = ix + self.s(74);
        let mid = (rc.top + rc.bottom) / 2;
        text(dc, "SIGMA5", &RECT { left: tx, top: mid - self.s(26), right: x1, bottom: mid }, self.fonts.title, t.text, DT_LEFT);
        let sub = format!("Version {} · A fast, lightweight launcher for Windows", env!("CARGO_PKG_VERSION"));
        text(dc, &sub, &RECT { left: tx, top: mid + self.s(2), right: x1 - self.s(16), bottom: mid + self.s(22) }, self.fonts.small, t.subtext, DT_LEFT | DT_END_ELLIPSIS);

        let y = y + hh + self.s(ROW_GAP);
        let path = self.store.path().display().to_string();
        let rows: [(&str, String, &str, Btn, bool); 4] = [
            ("Configuration file", path, "Open", Btn::OpenConfig, false),
            ("Data folder", "Settings, launch history and app cache".into(), "Open", Btn::OpenData, false),
            ("Launch history", "Used to rank the apps you open most".into(), "Clear", Btn::ClearHistory, true),
            ("Reset settings", "Restore every setting to its default".into(), "Reset", Btn::Reset, true),
        ];
        let mut bottom = y;
        for (i, (label, desc, blabel, btn, danger)) in rows.into_iter().enumerate() {
            let rc = self.card(dc, x0, x1, self.row_y(y, i as i32), rh);
            let left = self.button(dc, right, (rc.top + rc.bottom) / 2, blabel, btn, danger);
            let (d, c) = match &self.notice {
                Some((b, msg, until)) if *b == btn && Instant::now() < *until => (msg.clone(), t.accent),
                _ => (desc, t.subtext),
            };
            self.labels(dc, &rc, ix, left - self.s(16), label, &d, c);
            bottom = rc.bottom;
        }
        bottom
    }
}

// ---------------------------------------------------------------------------
// Popup menu for a remap's target (shown without holding the state).

fn show_to_menu(hwnd: HWND) -> usize {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return 0 };
        let add = |id: usize, label: &str| {
            let w = wide(label);
            let _ = AppendMenuW(menu, MF_STRING, id, PCWSTR(w.as_ptr()));
        };
        let sep = || {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        };
        add(MENU_RECORD, "Record a key or shortcut…");
        sep();
        for (id, _, label) in MENU_MEDIA {
            add(*id, label);
        }
        sep();
        add(MENU_HYPER, "Hyper (Ctrl+Shift+Alt)");
        add(MENU_DISABLED, "Disabled (do nothing)");
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None);
        let _ = DestroyMenu(menu);
        cmd.0 as usize
    }
}

// ---------------------------------------------------------------------------
// Window management.

/// Opens the settings window, or focuses it if already open.
pub fn open(launcher: HWND) {
    let existing = OPEN.with(|c| c.get());
    unsafe {
        if !existing.is_invalid() && IsWindow(Some(existing)).as_bool() {
            if IsIconic(existing).as_bool() {
                let _ = ShowWindow(existing, SW_RESTORE);
            }
            let _ = SetForegroundWindow(existing);
            return;
        }

        let hinst = GetModuleHandleW(None).expect("module handle");
        let class = wide(CLASS);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: tray::load_app_icon(32).unwrap_or_default(),
            hIconSm: tray::load_app_icon(16).unwrap_or_default(),
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        RegisterClassExW(&wc); // fails harmlessly if already registered

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let (mut dpi, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dpi, &mut dy);
        let sc = |v: i32| (v * dpi as i32 + 48) / 96;
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let mut wr = RECT { left: 0, top: 0, right: sc(CLIENT_W), bottom: sc(CLIENT_H) };
        let _ = AdjustWindowRectExForDpi(&mut wr, style, false, WINDOW_EX_STYLE(0), dpi);
        let (ww, wh) = (wr.right - wr.left, wr.bottom - wr.top);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let wa = mi.rcWork;
        let x = wa.left + ((wa.right - wa.left) - ww) / 2;
        let y = (wa.top + ((wa.bottom - wa.top) - wh) / 2).max(wa.top);

        let title = wide("SIGMA5 Settings");
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            style,
            x,
            y,
            ww,
            wh,
            None,
            None,
            Some(hinst.into()),
            None,
        ) else {
            return;
        };

        let store = ConfigStore::load();
        let theme = theme::resolve(store.config.theme, store.config.accent);
        let mut st = Box::new(Settings {
            hwnd,
            launcher,
            dpi,
            theme,
            fonts: Fonts::create(dpi),
            store,
            page: Page::General,
            zones: Vec::new(),
            hover: None,
            pressed: None,
            tracking: false,
            recording: None,
            rec_mods: 0,
            error: None,
            notice: None,
            draft_app: None,
            draft_remap: None,
            pending_menu: None,
            scroll: 0,
            content_h: 0,
            backbuf: BackBuffer::default(),
            logo: None,
            big_logo: None,
        });
        st.load_logos();
        st.apply_theme();
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(st) as isize);
        OPEN.with(|c| c.set(hwnd));

        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
}

fn point(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Settings;
    if ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
    }

    match msg {
        WM_DESTROY => {
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                let mut st = Box::from_raw(ptr);
                st.destroy();
            }
            OPEN.with(|c| c.set(HWND::default()));
            return LRESULT(0);
        }
        WM_DPICHANGED => {
            unsafe { (*ptr).on_dpi((wp.0 & 0xFFFF) as u32) };
            // SetWindowPos re-enters this proc; state is not borrowed here.
            let r = unsafe { &*(lp.0 as *const RECT) };
            unsafe {
                let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            }
            return LRESULT(0);
        }
        WM_APP_SETTINGS_MENU => {
            let which = unsafe { (*ptr).pending_menu.take() };
            if let Some(which) = which {
                let cmd = show_to_menu(hwnd); // modal loop: state not borrowed
                unsafe { (*ptr).on_menu(which, cmd) };
            }
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        _ => {}
    }

    let st = unsafe { &mut *ptr };
    match msg {
        WM_PAINT => {
            st.paint();
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = point(lp);
            st.on_mouse_move(x, y);
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            st.tracking = false;
            if st.hover.take().is_some() {
                st.invalidate();
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            st.on_wheel(((wp.0 >> 16) & 0xFFFF) as u16 as i16 as i32);
            LRESULT(0)
        }
        WM_SETCURSOR => {
            if (lp.0 & 0xFFFF) as u32 == HTCLIENT && st.hover.is_some() {
                unsafe {
                    if let Ok(c) = LoadCursorW(None, IDC_HAND) {
                        SetCursor(Some(c));
                    }
                }
                return LRESULT(1);
            }
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        WM_LBUTTONDOWN => {
            let (x, y) = point(lp);
            st.pressed = st.zone_at(x, y);
            if st.pressed.is_none() && st.recording.is_some() {
                st.stop_recording();
            }
            unsafe {
                SetCapture(hwnd);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            unsafe {
                let _ = ReleaseCapture();
            }
            let (x, y) = point(lp);
            let hit = st.zone_at(x, y);
            if let Some(c) = hit.filter(|_| hit == st.pressed.take()) {
                st.act(c);
            }
            LRESULT(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if st.on_key(VIRTUAL_KEY(wp.0 as u16)) {
                LRESULT(0)
            } else {
                unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
            }
        }
        WM_APP_CAPTURED => {
            st.on_captured(wp.0 as u32, lp.0 as u8);
            LRESULT(0)
        }
        WM_APP_CAPTURE_MODS => {
            st.rec_mods = wp.0 as u8;
            st.invalidate();
            LRESULT(0)
        }
        WM_APP_APP_PICKED => {
            let picked = unsafe { Box::from_raw(lp.0 as *mut (String, String)) };
            let (id, name) = *picked;
            st.on_app_picked(id, name);
            LRESULT(0)
        }
        WM_KILLFOCUS => {
            // Clicking away (or another window taking focus) cancels recording.
            st.stop_recording();
            LRESULT(0)
        }
        WM_TIMER => {
            if wp.0 == NOTICE_TIMER {
                unsafe {
                    let _ = KillTimer(Some(hwnd), NOTICE_TIMER);
                }
                st.notice = None;
                st.invalidate();
            }
            LRESULT(0)
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            st.apply_theme();
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}
