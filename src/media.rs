//! Now playing: what Spotify (or any app that shows up in Windows' own media
//! flyout) is playing, with play/pause/skip, read through Windows' System
//! Media Transport Controls. No web API or sign-in is involved.
//!
//! A worker thread reads the sessions about once a second, but only while
//! the launcher is open; otherwise it sleeps on its channel. Album art is
//! decoded with WIC once per track. The little waveform is driven by the
//! real output level (`IAudioMeterInformation`), read on the UI thread only
//! while it is on screen.
//!
//! Windows doesn't expose a player's queue, so there is no queue view.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::util::SendHwnd;

/// Album art, cropped square: opaque BGRX pixels, top-down.
#[derive(Clone, PartialEq)]
pub struct Art {
    pub size: i32,
    pub px: Vec<u32>,
}

impl std::fmt::Debug for Art {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Art({}px)", self.size)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// The player's app id: "Spotify.exe", a Store app id, a browser, ...
    pub app: String,
    pub playing: bool,
    /// Position and length in ms, when the player reports them.
    pub position_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    /// When `position_ms` was read (the UI extrapolates from here).
    pub read_at: Instant,
    pub art: Option<std::sync::Arc<Art>>,
}

impl NowPlaying {
    /// Position now, extrapolated while playing, capped at the length.
    pub fn position_now(&self) -> Option<u64> {
        let p = self.position_ms?;
        let p = if self.playing { p + self.read_at.elapsed().as_millis() as u64 } else { p };
        Some(self.duration_ms.map_or(p, |d| p.min(d)))
    }

    /// Same track and state, ignoring when it was read (so a steady track
    /// doesn't repaint every poll).
    fn same_as(&self, o: &NowPlaying) -> bool {
        let drift = match (self.position_now(), o.position_now()) {
            (Some(a), Some(b)) => a.abs_diff(b) < 1500,
            (a, b) => a == b,
        };
        self.title == o.title
            && self.artist == o.artist
            && self.album == o.album
            && self.app == o.app
            && self.playing == o.playing
            && self.duration_ms == o.duration_ms
            && self.art.is_some() == o.art.is_some()
            && drift
    }

    /// "Title · Artist" (or whichever is known).
    pub fn line(&self) -> String {
        match (self.title.is_empty(), self.artist.is_empty()) {
            (false, false) => format!("{} · {}", self.title, self.artist),
            (false, true) => self.title.clone(),
            (true, false) => self.artist.clone(),
            (true, true) => "Unknown track".into(),
        }
    }

    /// The player's exe ("spotify.exe") when its app id is one.
    pub fn exe(&self) -> Option<String> {
        let a = self.app.to_ascii_lowercase();
        if a.ends_with(".exe") {
            return Some(a.rsplit('\\').next().unwrap_or(&a).to_string());
        }
        // The Microsoft Store Spotify ("SpotifyAB.SpotifyMusic_…!Spotify") runs Spotify.exe too.
        a.starts_with("spotifyab.").then(|| "spotify.exe".to_string())
    }
}

/// "3:07" / "1:02:33".
pub fn clock(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cmd {
    PlayPause,
    Next,
    Previous,
    /// Jump to this position (ms).
    Seek(u64),
}

enum Msg {
    Cmd(Cmd),
    /// Launcher opened (true) or closed (false).
    Active(bool),
}

/// Handle to the worker; dropping it stops the thread.
pub struct Media {
    tx: Sender<Msg>,
}

impl Media {
    /// Starts the worker; it posts `msg` with LPARAM = Box<Option<NowPlaying>>.
    pub fn start(hwnd: HWND, msg: u32) -> Self {
        let (tx, rx) = channel();
        let target = SendHwnd::new(hwnd);
        let _ = std::thread::Builder::new().name("sigma5-media".into()).spawn(move || worker(rx, target, msg));
        Self { tx }
    }

    pub fn set_active(&self, on: bool) {
        let _ = self.tx.send(Msg::Active(on));
    }

    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(Msg::Cmd(c));
    }
}

fn post(hwnd: SendHwnd, msg: u32, np: Option<NowPlaying>) {
    let raw = Box::into_raw(Box::new(np));
    unsafe {
        if PostMessageW(Some(hwnd.get()), msg, WPARAM(0), LPARAM(raw as isize)).is_err() {
            drop(Box::from_raw(raw));
        }
    }
}

struct Reader {
    mgr: Option<Manager>,
    /// Art of the current track: (track key, art).
    art: Option<(String, Option<std::sync::Arc<Art>>)>,
}

fn worker(rx: Receiver<Msg>, hwnd: SendHwnd, msg: u32) {
    unsafe {
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mut r = Reader { mgr: None, art: None };
    let mut active = false;
    let mut last: Option<Option<NowPlaying>> = None;
    loop {
        let m = if active {
            match rx.recv_timeout(Duration::from_millis(1000)) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => break,
            }
        };
        match m {
            Some(Msg::Active(on)) => {
                active = on;
                if !on {
                    continue;
                }
                last = None; // report afresh on every open
            }
            Some(Msg::Cmd(c)) => {
                r.command(c);
                // Give the player a moment to react before reading it back.
                std::thread::sleep(Duration::from_millis(120));
            }
            None => {}
        }
        if !active {
            continue;
        }
        let now = r.read();
        let changed = match (&last, &now) {
            (Some(Some(a)), Some(b)) => !a.same_as(b),
            (Some(None), None) => false,
            _ => true,
        };
        if changed {
            last = Some(now.clone());
            post(hwnd, msg, now);
        }
    }
}

impl Reader {
    fn manager(&mut self) -> Option<&Manager> {
        if self.mgr.is_none() {
            self.mgr = Manager::RequestAsync().and_then(|op| op.get()).ok();
        }
        self.mgr.as_ref()
    }

    /// The session worth showing: whatever is playing (Spotify first), else
    /// Windows' current session.
    fn session(&mut self) -> Option<Session> {
        let mgr = self.manager()?;
        let playing = |s: &Session| {
            s.GetPlaybackInfo().and_then(|i| i.PlaybackStatus()).map(|st| st == Status::Playing).unwrap_or(false)
        };
        let current = mgr.GetCurrentSession().ok();
        if let Some(c) = &current {
            if playing(c) {
                return current;
            }
        }
        let mut best: Option<(i32, Session)> = None;
        if let Ok(all) = mgr.GetSessions() {
            for i in 0..all.Size().unwrap_or(0) {
                let Ok(s) = all.GetAt(i) else { continue };
                if !playing(&s) {
                    continue;
                }
                let spotify = s.SourceAppUserModelId().map(|a| a.to_string().to_ascii_lowercase().contains("spotify")).unwrap_or(false);
                let rank = if spotify { 2 } else { 1 };
                if best.as_ref().is_none_or(|(b, _)| rank > *b) {
                    best = Some((rank, s));
                }
            }
        }
        best.map(|(_, s)| s).or(current)
    }

    fn command(&mut self, c: Cmd) {
        let Some(s) = self.session() else { return };
        let op = match c {
            Cmd::PlayPause => s.TryTogglePlayPauseAsync(),
            Cmd::Next => s.TrySkipNextAsync(),
            Cmd::Previous => s.TrySkipPreviousAsync(),
            Cmd::Seek(ms) => s.TryChangePlaybackPositionAsync(ms as i64 * 10_000),
        };
        if let Ok(op) = op {
            let _ = op.get();
        }
    }

    fn read(&mut self) -> Option<NowPlaying> {
        let s = self.session()?;
        let app = s.SourceAppUserModelId().map(|a| a.to_string()).unwrap_or_default();
        let playing = s.GetPlaybackInfo().and_then(|i| i.PlaybackStatus()).map(|st| st == Status::Playing).unwrap_or(false);
        let props = s.TryGetMediaPropertiesAsync().and_then(|op| op.get()).ok();
        let get = |f: &dyn Fn() -> windows::core::Result<windows::core::HSTRING>| f().map(|h| h.to_string()).unwrap_or_default();
        let (title, artist, album) = match &props {
            Some(p) => (get(&|| p.Title()), get(&|| p.Artist()), get(&|| p.AlbumTitle())),
            None => Default::default(),
        };
        if title.is_empty() && artist.is_empty() {
            return None;
        }

        // Timeline (in 100 ns units). The position is as of LastUpdatedTime.
        let (mut position_ms, mut duration_ms) = (None, None);
        if let Ok(t) = s.GetTimelineProperties() {
            let start = t.StartTime().map(|v| v.Duration).unwrap_or(0);
            let end = t.EndTime().map(|v| v.Duration).unwrap_or(0);
            if end > start {
                duration_ms = Some(((end - start) / 10_000) as u64);
                let pos = t.Position().map(|v| v.Duration).unwrap_or(0) - start;
                let mut pos_ms = (pos.max(0) / 10_000) as u64;
                if playing {
                    if let Ok(upd) = t.LastUpdatedTime() {
                        let ago = (now_filetime() - upd.UniversalTime) / 10_000;
                        if (0..6 * 3_600_000).contains(&ago) {
                            pos_ms += ago as u64;
                        }
                    }
                }
                position_ms = Some(pos_ms.min(duration_ms.unwrap_or(u64::MAX)));
            }
        }

        // Art, once per track.
        let key = format!("{app}\n{title}\n{artist}\n{album}");
        if self.art.as_ref().is_none_or(|(k, _)| *k != key) {
            let art = props
                .as_ref()
                .and_then(|p| p.Thumbnail().ok())
                .and_then(|t| read_stream(&t))
                .and_then(|bytes| decode_art(&bytes, 256))
                .map(std::sync::Arc::new);
            self.art = Some((key, art));
        }
        let art = self.art.as_ref().and_then(|(_, a)| a.clone());
        Some(NowPlaying { title, artist, album, app, playing, position_ms, duration_ms, read_at: Instant::now(), art })
    }
}

fn now_filetime() -> i64 {
    let ft = unsafe { windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime() };
    ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64
}

fn read_stream(r: &windows::Storage::Streams::IRandomAccessStreamReference) -> Option<Vec<u8>> {
    use windows::Storage::Streams::DataReader;
    let stream = r.OpenReadAsync().ok()?.get().ok()?;
    let size = stream.Size().ok()?;
    if size == 0 || size > 16 << 20 {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    let got = reader.LoadAsync(size as u32).ok()?.get().ok()?;
    let mut buf = vec![0u8; got as usize];
    reader.ReadBytes(&mut buf).ok()?;
    Some(buf)
}

/// Decodes an image (JPEG/PNG/...) with WIC, centre-cropped to a square of `size` px.
pub fn decode_art(bytes: &[u8], size: u32) -> Option<Art> {
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    unsafe {
        let f: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let stream = f.CreateStream().ok()?;
        stream.InitializeFromMemory(bytes).ok()?;
        let dec = f.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
        let frame = dec.GetFrame(0).ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).ok()?;
        if w == 0 || h == 0 {
            return None;
        }
        let side = w.min(h);
        let rc = WICRect { X: ((w - side) / 2) as i32, Y: ((h - side) / 2) as i32, Width: side as i32, Height: side as i32 };
        let clip = f.CreateBitmapClipper().ok()?;
        clip.Initialize(&frame, &rc).ok()?;
        let out = size.min(side).max(1);
        let scaler = f.CreateBitmapScaler().ok()?;
        scaler.Initialize(&clip, out, out, WICBitmapInterpolationModeFant).ok()?;
        let conv = f.CreateFormatConverter().ok()?;
        conv.Initialize(&scaler, &GUID_WICPixelFormat32bppBGR, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let mut px = vec![0u32; (out * out) as usize];
        let bytes = std::slice::from_raw_parts_mut(px.as_mut_ptr() as *mut u8, px.len() * 4);
        conv.CopyPixels(std::ptr::null(), out * 4, bytes).ok()?;
        Some(Art { size: out as i32, px })
    }
}

// ---------------------------------------------------------------------------
// Output level for the waveform (UI thread).

/// Peak meter on the player's own audio session if it can be found,
/// otherwise on the default output device.
pub struct Meter {
    meter: windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation,
    /// The exe it is tied to ("" = whole device).
    pub exe: String,
}

impl Meter {
    pub fn open(exe: Option<&str>) -> Option<Self> {
        use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
        use windows::Win32::Media::Audio::{
            IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender,
        };
        use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
        use windows::core::Interface;
        unsafe {
            let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let dev = en.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
            if let Some(exe) = exe {
                let found = (|| {
                    let mgr: IAudioSessionManager2 = dev.Activate(CLSCTX_ALL, None).ok()?;
                    let list = mgr.GetSessionEnumerator().ok()?;
                    for i in 0..list.GetCount().ok()? {
                        let Ok(ctl) = list.GetSession(i) else { continue };
                        let Ok(c2) = ctl.cast::<IAudioSessionControl2>() else { continue };
                        let pid = c2.GetProcessId().unwrap_or(0);
                        if pid != 0 && crate::util::process_exe_name(pid).as_deref() == Some(exe) {
                            if let Ok(m) = ctl.cast::<IAudioMeterInformation>() {
                                return Some(m);
                            }
                        }
                    }
                    None
                })();
                if let Some(meter) = found {
                    return Some(Self { meter, exe: exe.to_string() });
                }
            }
            let meter: IAudioMeterInformation = dev.Activate(CLSCTX_ALL, None).ok()?;
            Some(Self { meter, exe: String::new() })
        }
    }

    /// Peak level 0..1 since the last read.
    pub fn peak(&self) -> f32 {
        unsafe { self.meter.GetPeakValue().unwrap_or(0.0) }
    }
}

/// Recent output levels, oldest first, shaped for drawing.
#[derive(Default)]
pub struct Levels {
    v: std::collections::VecDeque<f32>,
}

pub const LEVELS: usize = 160;

impl Levels {
    pub fn push(&mut self, peak: f32) {
        // A square-root curve makes quiet passages still move.
        let v = peak.clamp(0.0, 1.0).sqrt();
        if self.v.len() == LEVELS {
            self.v.pop_front();
        }
        self.v.push_back(v);
    }

    pub fn clear(&mut self) {
        self.v.clear();
    }

    /// The newest `n` levels, oldest first, padded with zeros at the start.
    pub fn last(&self, n: usize) -> Vec<f32> {
        let have = self.v.len().min(n);
        let mut out = vec![0.0; n - have];
        out.extend(self.v.iter().skip(self.v.len() - have));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn np(playing: bool, pos: Option<u64>) -> NowPlaying {
        NowPlaying {
            title: "Song".into(),
            artist: "Band".into(),
            album: String::new(),
            app: "Spotify.exe".into(),
            playing,
            position_ms: pos,
            duration_ms: Some(200_000),
            read_at: Instant::now(),
            art: None,
        }
    }

    #[test]
    fn clock_format() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(61_500), "1:01");
        assert_eq!(clock(3_725_000), "1:02:05");
    }

    #[test]
    fn line_and_exe() {
        let n = np(true, Some(0));
        assert_eq!(n.line(), "Song · Band");
        assert_eq!(n.exe().as_deref(), Some("spotify.exe"));
        let mut n2 = n.clone();
        n2.app = "Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic".into();
        assert_eq!(n2.exe(), None);
        n2.app = "SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify".into();
        assert_eq!(n2.exe().as_deref(), Some("spotify.exe"));
    }

    #[test]
    fn position_extrapolates_only_while_playing() {
        let mut n = np(false, Some(1000));
        n.read_at = Instant::now() - Duration::from_millis(500);
        assert_eq!(n.position_now(), Some(1000));
        n.playing = true;
        assert!(n.position_now().unwrap() >= 1500);
        n.position_ms = Some(199_900);
        assert_eq!(n.position_now(), Some(200_000)); // capped at the length
    }

    #[test]
    fn steady_track_counts_as_unchanged() {
        let a = np(true, Some(10_000));
        let mut b = np(true, Some(10_400));
        assert!(a.same_as(&b));
        b.playing = false;
        assert!(!a.same_as(&b));
        let mut c = np(true, Some(60_000));
        assert!(!a.same_as(&c)); // seeked
        c.position_ms = Some(10_000);
        c.title = "Other".into();
        assert!(!a.same_as(&c));
    }

    /// Reads the real media session and level meter (read-only, no windows).
    /// `cargo test read_live_media -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn read_live_media() {
        unsafe {
            use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let mut r = Reader { mgr: None, art: None };
        let np = r.read();
        println!("manager: {}", r.mgr.is_some());
        match &np {
            Some(n) => println!(
                "now playing: {:?} by {:?} ({:?}) on {:?}, playing={}, pos={:?}/{:?}, art={:?}",
                n.title, n.artist, n.album, n.app, n.playing, n.position_ms, n.duration_ms, n.art
            ),
            None => println!("now playing: nothing"),
        }
        let exe = np.as_ref().and_then(|n| n.exe());
        let m = Meter::open(exe.as_deref());
        println!("meter: {:?}", m.as_ref().map(|m| if m.exe.is_empty() { "device" } else { m.exe.as_str() }));
        if let Some(m) = m {
            let peaks: Vec<f32> = (0..10).map(|_| {
                std::thread::sleep(Duration::from_millis(33));
                m.peak()
            }).collect();
            println!("peaks: {peaks:.2?}");
        }
    }

    #[test]
    fn levels_window() {
        let mut l = Levels::default();
        assert_eq!(l.last(3), vec![0.0, 0.0, 0.0]);
        l.push(0.25);
        l.push(1.0);
        assert_eq!(l.last(3), vec![0.0, 0.5, 1.0]);
        for _ in 0..100 {
            l.push(0.0);
        }
        assert_eq!(l.last(LEVELS + 2).len(), LEVELS + 2);
    }
}
