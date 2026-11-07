//! Low-level keyboard hook: key remapping, Windows-key tap, and key capture.
//!
//! One `WH_KEYBOARD_LL` hook on a dedicated thread handles, per key event:
//!
//! 1. **Capture**: while the Settings window is recording a key, every
//!    physical key is reported to it and swallowed. Because this happens
//!    before Windows processes hotkeys, even combos owned by other apps can be
//!    recorded (and are then reported as conflicts).
//! 2. **Remaps**: a source key is swallowed and its target (key, shortcut,
//!    media key, modifier set like Hyper, or nothing) is injected instead.
//! 3. **Windows-key tap**: a lone Win tap opens SIGMA5 instead of Start (see
//!    [`TapDetector`]).
//!
//! The hook thread only exists while one of these is needed. Our own injected
//! events are ignored by the hook, so remaps never loop.
//!
//! Safety: when the focused window belongs to an elevated (admin) process,
//! Windows silently drops injected input. Swallowing the Win key-up there
//! could leave Win "stuck", so the Win tap stands aside in that case.

use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    GetCurrentThreadId, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC, MapVirtualKeyW, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, HHOOK,
    KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, MSG, PM_NOREMOVE, PeekMessageW, PostMessageW,
    PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN,
    WM_QUIT, WM_SYSKEYDOWN, WM_USER,
};

use super::{WM_APP_CAPTURE_MODS, WM_APP_CAPTURED, WM_APP_WINKEY};
use crate::keys::{self, ALT, CTRL, SHIFT, Target, WIN};

/// Unassigned virtual key used to "mask" a lone Win/Alt release (as AutoHotkey does).
const MASK_VK: u16 = 0xE8;
/// Holding Win longer than this is not a tap.
const MAX_TAP_MS: u32 = 1500;
/// Tags our injected events.
const SIGNATURE: usize = 0x5349_474D; // "SIGM"
/// Thread message: release any keys we are holding down.
const MSG_RELEASE_ALL: u32 = WM_USER + 1;

/// What the hook should do. Built from the config by the launcher.
#[derive(Default, Clone, Debug, PartialEq)]
pub struct Rules {
    pub win_tap: bool,
    pub remaps: Vec<(u32, Target)>,
}

impl Rules {
    fn lookup(&self, code: u32) -> Option<Target> {
        self.remaps.iter().find(|(c, _)| *c == code).map(|(_, t)| *t)
    }
    fn is_empty(&self) -> bool {
        !self.win_tap && self.remaps.is_empty()
    }
}

static HOOK_THREAD: AtomicU32 = AtomicU32::new(0);
/// Launcher window (receives Win taps).
static TARGET: AtomicIsize = AtomicIsize::new(0);
/// Settings window while it is capturing a key, else 0.
static CAPTURE: AtomicIsize = AtomicIsize::new(0);

fn rules_cell() -> &'static Mutex<Arc<Rules>> {
    static R: OnceLock<Mutex<Arc<Rules>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Arc::new(Rules::default())))
}

fn current_rules() -> Arc<Rules> {
    rules_cell().lock().map(|r| r.clone()).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Public API (called from the UI thread).

/// Applies new rules, starting or stopping the hook as needed.
pub fn configure(target: HWND, rules: Rules) {
    TARGET.store(target.0 as isize, Ordering::Relaxed);
    let caps_remapped = rules.remaps.iter().any(|(c, _)| *c == keys::VK_CAPITAL);
    let changed = {
        let mut cur = rules_cell().lock().unwrap();
        let changed = **cur != rules;
        *cur = Arc::new(rules);
        changed
    };
    if changed {
        let tid = HOOK_THREAD.load(Ordering::Acquire);
        if tid != 0 {
            unsafe {
                let _ = PostThreadMessageW(tid, MSG_RELEASE_ALL, WPARAM(0), LPARAM(0));
            }
        }
    }
    update_thread();
    // Once Caps Lock is remapped it can no longer be toggled, so turn it off.
    if caps_remapped && unsafe { GetKeyState(keys::VK_CAPITAL as i32) } & 1 != 0 {
        send(&[key_input(keys::VK_CAPITAL as u16, false, false), key_input(keys::VK_CAPITAL as u16, true, false)]);
    }
}

/// Routes the next keys to `hwnd` as `WM_APP_CAPTURED` / `WM_APP_CAPTURE_MODS`.
pub fn start_capture(hwnd: HWND) {
    CAPTURE.store(hwnd.0 as isize, Ordering::Release);
    update_thread();
}

pub fn stop_capture() {
    CAPTURE.store(0, Ordering::Release);
    update_thread();
}

/// Stops the hook (on exit).
pub fn shutdown() {
    CAPTURE.store(0, Ordering::Release);
    *rules_cell().lock().unwrap() = Arc::new(Rules::default());
    update_thread();
}

fn update_thread() {
    let needed = CAPTURE.load(Ordering::Acquire) != 0 || !current_rules().is_empty();
    let running = HOOK_THREAD.load(Ordering::Acquire);
    if needed && running == 0 {
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("sigma5-input".into()).spawn(move || unsafe {
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE); // create the queue
            let hinst = GetModuleHandleW(None).ok().map(Into::into);
            let hook: Option<HHOOK> = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), hinst, 0).ok();
            let _ = tx.send(if hook.is_some() { GetCurrentThreadId() } else { 0 });
            let Some(hook) = hook else { return };
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                if msg.hwnd.is_invalid() && msg.message == MSG_RELEASE_ALL {
                    STATE.with(|s| s.borrow_mut().release_all());
                    continue;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            STATE.with(|s| s.borrow_mut().release_all());
            let _ = UnhookWindowsHookEx(hook);
        });
        if spawned.is_ok() {
            if let Ok(tid) = rx.recv() {
                HOOK_THREAD.store(tid, Ordering::Release);
            }
        }
    } else if !needed && running != 0 {
        HOOK_THREAD.store(0, Ordering::Release);
        unsafe {
            let _ = PostThreadMessageW(running, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

// ---------------------------------------------------------------------------
// Windows-key tap detection (pure, unit-tested).

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Tap,
}

#[derive(Default)]
pub struct TapDetector {
    /// Win key currently held: (code, time it went down).
    down: Option<(u32, u32)>,
    /// Another key was involved, so this is a shortcut, not a tap.
    spoiled: bool,
}

pub fn is_win(code: u32) -> bool {
    code == keys::VK_LWIN || code == keys::VK_RWIN
}

impl TapDetector {
    /// Feeds one physical key event. `mods_held` = Ctrl/Alt/Shift held when it happened.
    pub fn key(&mut self, code: u32, is_down: bool, time_ms: u32, mods_held: bool) -> Verdict {
        if is_win(code) {
            if is_down {
                match self.down {
                    None => {
                        self.down = Some((code, time_ms));
                        self.spoiled = mods_held;
                    }
                    Some((held, _)) if held != code => self.spoiled = true,
                    Some(_) => {} // auto-repeat
                }
                return Verdict::Pass;
            }
            let Some((_, t0)) = self.down.take() else { return Verdict::Pass };
            let quick = time_ms.wrapping_sub(t0) <= MAX_TAP_MS;
            return if !self.spoiled && quick { Verdict::Tap } else { Verdict::Pass };
        }
        if self.down.is_some() && is_down {
            self.spoiled = true;
        }
        Verdict::Pass
    }
}

// ---------------------------------------------------------------------------
// Remap engine (pure state machine producing key events; unit-tested).

/// A key event to inject: (code, is_up).
pub type Out = (u32, bool);

struct Active {
    src: u32,
    mods: u8,
    key: Option<u32>,
    /// Another key was pressed while this (modifier) remap was held.
    used: bool,
}

#[derive(Default)]
pub struct Remapper {
    active: Vec<Active>,
}

const MOD_KEYS: [(u8, u32); 4] = [(CTRL, keys::VK_LCONTROL), (SHIFT, keys::VK_LSHIFT), (ALT, keys::VK_LMENU), (WIN, keys::VK_LWIN)];

impl Remapper {
    /// Any key going down counts as "using" held modifier remaps.
    pub fn note_other_key(&mut self, code: u32) {
        for a in self.active.iter_mut().filter(|a| a.src != code) {
            a.used = true;
        }
    }

    /// Handles an event for a remapped source key; returns events to inject.
    pub fn event(&mut self, src: u32, target: Target, is_down: bool) -> Vec<Out> {
        let mut out = Vec::new();
        let Target::Keys { mods, key } = target else { return out };
        if is_down {
            if self.active.iter().any(|a| a.src == src) {
                // Auto-repeat: repeat the key, not the modifiers.
                if let Some(k) = key {
                    out.push((k, false));
                }
                return out;
            }
            self.note_other_key(src);
            for (bit, vk) in MOD_KEYS {
                if mods & bit != 0 {
                    out.push((vk, false));
                }
            }
            if let Some(k) = key {
                out.push((k, false));
            }
            self.active.push(Active { src, mods, key, used: false });
        } else if let Some(pos) = self.active.iter().position(|a| a.src == src) {
            let a = self.active.remove(pos);
            out.extend(Self::release(&a));
        }
        out
    }

    fn release(a: &Active) -> Vec<Out> {
        let mut out = Vec::new();
        if let Some(k) = a.key {
            out.push((k, true));
        }
        // A lone tap of Alt/Win would open a menu / Start; mask it.
        if a.key.is_none() && !a.used && a.mods & (ALT | WIN) != 0 {
            out.push((MASK_VK as u32, false));
            out.push((MASK_VK as u32, true));
        }
        for (bit, vk) in MOD_KEYS.iter().rev() {
            if a.mods & bit != 0 {
                out.push((*vk, true));
            }
        }
        out
    }

    /// Releases everything currently held (rules changed / shutting down).
    pub fn release_all(&mut self) -> Vec<Out> {
        let all: Vec<Active> = self.active.drain(..).collect();
        all.iter().flat_map(Self::release).collect()
    }
}

// ---------------------------------------------------------------------------
// Key capture for the settings recorder (pure, unit-tested).

#[derive(Default)]
pub struct Capture {
    held: Vec<(u32, u8)>,
    /// The last key pressed was this modifier (so releasing it records it alone).
    lone: Option<u32>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CaptureEvent {
    /// Modifiers currently held changed (for live display).
    Mods(u8),
    /// A key was recorded: (code, modifiers held with it, including itself if a modifier).
    Key(u32, u8),
}

impl Capture {
    pub fn mods(&self) -> u8 {
        self.held.iter().fold(0, |m, (_, b)| m | b)
    }

    /// `mod_bits` is the modifier set this key acts as (0 for normal keys).
    pub fn event(&mut self, code: u32, mod_bits: u8, is_down: bool) -> Option<CaptureEvent> {
        if mod_bits != 0 {
            if is_down {
                if self.held.iter().any(|(c, _)| *c == code) {
                    return None; // auto-repeat
                }
                self.held.push((code, mod_bits));
                self.lone = Some(code);
                return Some(CaptureEvent::Mods(self.mods()));
            }
            let all = self.mods();
            self.held.retain(|(c, _)| *c != code);
            if self.lone.take() == Some(code) {
                return Some(CaptureEvent::Key(code, all));
            }
            return Some(CaptureEvent::Mods(self.mods()));
        }
        if is_down {
            self.lone = None;
            return Some(CaptureEvent::Key(code, self.mods()));
        }
        None
    }
}

// ---------------------------------------------------------------------------
// The hook itself.

#[derive(Default)]
struct HookState {
    tap: TapDetector,
    remap: Remapper,
    capture: Capture,
    capture_owner: isize,
}

impl HookState {
    fn release_all(&mut self) {
        let out = self.remap.release_all();
        inject(&out);
    }
}

thread_local! {
    static STATE: RefCell<HookState> = RefCell::new(HookState::default());
}

fn key_held(vk: u32) -> bool {
    unsafe { GetAsyncKeyState(vk as i32) < 0 }
}

/// True if the focused window's process is elevated (or we can't tell).
fn foreground_is_elevated() -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, Some(&mut pid));
        if pid == std::process::id() {
            return false;
        }
        let Ok(proc_) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return true };
        let mut token = HANDLE::default();
        let ok = OpenProcessToken(proc_, TOKEN_QUERY, &mut token).is_ok();
        let _ = CloseHandle(proc_);
        if !ok {
            return true;
        }
        let mut elev = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let got = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elev as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        !got || elev.TokenIsElevated != 0
    }
}

fn key_input(vk: u16, up: bool, extended: bool) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: SIGNATURE },
        },
    }
}

fn send(inputs: &[INPUT]) -> bool {
    if inputs.is_empty() {
        return true;
    }
    unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) == inputs.len() as u32 }
}

fn inject(events: &[Out]) {
    let inputs: Vec<INPUT> = events
        .iter()
        .map(|&(code, up)| key_input(keys::vk(code), up, keys::is_extended(code)))
        .collect();
    send(&inputs);
}

fn post(hwnd: isize, msg: u32, wp: usize, lp: isize) {
    unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as *mut _)), msg, WPARAM(wp), LPARAM(lp));
    }
}

unsafe extern "system" fn hook_proc(ncode: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if ncode >= 0 {
        let k = unsafe { &*(lp.0 as *const KBDLLHOOKSTRUCT) };
        if !k.flags.contains(LLKHF_INJECTED) && handle(k, matches!(wp.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN)) {
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, ncode, wp, lp) }
}

/// Returns true to swallow the event.
fn handle(k: &KBDLLHOOKSTRUCT, is_down: bool) -> bool {
    let code = keys::canonical(k.vkCode, k.flags.contains(LLKHF_EXTENDED));
    let rules = current_rules();
    STATE.with(|s| {
        let mut s = s.borrow_mut();

        // 1. Capture for the settings recorder.
        let owner = CAPTURE.load(Ordering::Acquire);
        if owner != s.capture_owner {
            s.capture = Capture::default();
            s.capture_owner = owner;
        }
        if owner != 0 {
            let as_mod = match rules.lookup(code) {
                Some(Target::Keys { mods, key: None }) => mods,
                _ => keys::modifier_of(code),
            };
            match s.capture.event(code, as_mod, is_down) {
                Some(CaptureEvent::Mods(m)) => post(owner, WM_APP_CAPTURE_MODS, m as usize, 0),
                Some(CaptureEvent::Key(c, m)) => post(owner, WM_APP_CAPTURED, c as usize, m as isize),
                None => {}
            }
            return true;
        }

        // 2. Windows-key tap bookkeeping sees every physical key.
        let mods_held = is_down && is_win(code) && [0x10, 0x11, 0x12].iter().any(|&v| key_held(v));
        let verdict = s.tap.key(code, is_down, k.time, mods_held);

        // 3. Remaps.
        if let Some(target) = rules.lookup(code) {
            let out = s.remap.event(code, target, is_down);
            drop(s);
            inject(&out);
            return true;
        }
        if is_down {
            s.remap.note_other_key(code);
        }
        drop(s);

        // 4. Windows key → SIGMA5.
        if verdict == Verdict::Tap && rules.win_tap && !foreground_is_elevated() {
            let inputs = [
                key_input(MASK_VK, false, false),
                key_input(MASK_VK, true, false),
                key_input(keys::vk(code), true, true),
            ];
            if send(&inputs) {
                post(TARGET.load(Ordering::Relaxed), WM_APP_WINKEY, 0, 0);
                return true; // the real Win key-up was replaced by ours
            }
        }
        false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LWIN: u32 = 0x5B;
    const RWIN: u32 = 0x5C;
    const E: u32 = 0x45;
    const KEY_SHIFT: u32 = 0x10;
    const CAPS: u32 = 0x14;
    const NUM5: u32 = 0x65;
    const PLAY: u32 = 0xB3;

    #[test]
    fn lone_tap_is_detected() {
        let mut d = TapDetector::default();
        assert_eq!(d.key(LWIN, true, 1000, false), Verdict::Pass);
        assert_eq!(d.key(LWIN, false, 1100, false), Verdict::Tap);
    }

    #[test]
    fn auto_repeat_still_a_tap() {
        let mut d = TapDetector::default();
        d.key(LWIN, true, 0, false);
        d.key(LWIN, true, 300, false);
        assert_eq!(d.key(LWIN, false, 400, false), Verdict::Tap);
    }

    #[test]
    fn win_shortcuts_are_not_taps() {
        let mut d = TapDetector::default();
        d.key(LWIN, true, 0, false);
        d.key(E, true, 50, false);
        assert_eq!(d.key(LWIN, false, 120, false), Verdict::Pass);
        d.key(LWIN, true, 200, true); // Shift held before Win
        assert_eq!(d.key(LWIN, false, 260, true), Verdict::Pass);
        d.key(LWIN, true, 300, false);
        d.key(KEY_SHIFT, true, 310, false);
        assert_eq!(d.key(LWIN, false, 360, false), Verdict::Pass);
        d.key(LWIN, true, 400, false);
        d.key(RWIN, true, 410, false);
        assert_eq!(d.key(RWIN, false, 420, false), Verdict::Pass);
    }

    #[test]
    fn long_hold_and_wraparound() {
        let mut d = TapDetector::default();
        d.key(LWIN, true, 0, false);
        assert_eq!(d.key(LWIN, false, 2000, false), Verdict::Pass);
        d.key(LWIN, true, u32::MAX - 50, false);
        assert_eq!(d.key(LWIN, false, 49, false), Verdict::Tap);
    }

    #[test]
    fn remap_simple_key_with_repeat() {
        let mut r = Remapper::default();
        let t = Target::Keys { mods: 0, key: Some(PLAY) };
        assert_eq!(r.event(NUM5, t, true), vec![(PLAY, false)]);
        assert_eq!(r.event(NUM5, t, true), vec![(PLAY, false)]); // auto-repeat
        assert_eq!(r.event(NUM5, t, false), vec![(PLAY, true)]);
        assert!(r.event(NUM5, t, false).is_empty()); // stray key-up
    }

    #[test]
    fn hyper_used_with_a_key() {
        let mut r = Remapper::default();
        let down = r.event(CAPS, Target::HYPER, true);
        assert_eq!(down, vec![(keys::VK_LCONTROL, false), (keys::VK_LSHIFT, false), (keys::VK_LMENU, false)]);
        assert!(r.event(CAPS, Target::HYPER, true).is_empty()); // repeat: nothing
        r.note_other_key(0x43); // C pressed while held
        let up = r.event(CAPS, Target::HYPER, false);
        assert_eq!(up, vec![(keys::VK_LMENU, true), (keys::VK_LSHIFT, true), (keys::VK_LCONTROL, true)]);
    }

    #[test]
    fn hyper_tapped_alone_is_masked() {
        let mut r = Remapper::default();
        r.event(CAPS, Target::HYPER, true);
        let up = r.event(CAPS, Target::HYPER, false);
        assert_eq!(up[0], (MASK_VK as u32, false));
        assert_eq!(up[1], (MASK_VK as u32, true));
        assert_eq!(up.len(), 5);
    }

    #[test]
    fn shortcut_target_and_disabled() {
        let mut r = Remapper::default();
        let ctrl_c = Target::Keys { mods: CTRL, key: Some(0x43) };
        assert_eq!(r.event(NUM5, ctrl_c, true), vec![(keys::VK_LCONTROL, false), (0x43, false)]);
        assert_eq!(r.event(NUM5, ctrl_c, false), vec![(0x43, true), (keys::VK_LCONTROL, true)]);
        assert!(r.event(NUM5, Target::Disabled, true).is_empty());
    }

    #[test]
    fn release_all_lets_go() {
        let mut r = Remapper::default();
        r.event(CAPS, Target::Keys { mods: CTRL, key: None }, true);
        r.note_other_key(0x41);
        assert_eq!(r.release_all(), vec![(keys::VK_LCONTROL, true)]);
        assert!(r.release_all().is_empty());
    }

    #[test]
    fn capture_combo_with_hyper_caps() {
        let mut c = Capture::default();
        assert_eq!(c.event(CAPS, keys::HYPER, true), Some(CaptureEvent::Mods(keys::HYPER)));
        assert_eq!(c.event(0x43, 0, true), Some(CaptureEvent::Key(0x43, keys::HYPER)));
        assert_eq!(c.event(0x43, 0, false), None);
        assert_eq!(c.event(CAPS, keys::HYPER, false), Some(CaptureEvent::Mods(0)));
    }

    #[test]
    fn capture_lone_key_and_lone_modifier() {
        let mut c = Capture::default();
        assert_eq!(c.event(NUM5, 0, true), Some(CaptureEvent::Key(NUM5, 0)));
        let mut c = Capture::default();
        c.event(keys::VK_RMENU, ALT, true);
        assert_eq!(c.event(keys::VK_RMENU, ALT, false), Some(CaptureEvent::Key(keys::VK_RMENU, ALT)));
        // Ctrl then Alt, released Alt alone → Ctrl+Alt
        let mut c = Capture::default();
        c.event(keys::VK_LCONTROL, CTRL, true);
        c.event(keys::VK_LMENU, ALT, true);
        assert_eq!(c.event(keys::VK_LMENU, ALT, false), Some(CaptureEvent::Key(keys::VK_LMENU, CTRL | ALT)));
    }
}
