//! Test-only: runs the real launcher off-screen and renders it to PNG.
//!
//! The window lives on a private desktop (never shown, no real input), with
//! `%APPDATA%`/`%LOCALAPPDATA%` pointed at a temporary folder so the user's
//! settings, history and clipboard are untouched, and in headless mode so
//! nothing system-wide is changed (startup entry, tray, hotkeys, hooks).
//!
//! `SIGMA5_SNAP_DIR=<dir> cargo test --release launcher_snapshots -- --ignored --nocapture`

use super::*;

#[link(name = "user32")]
unsafe extern "system" {
    fn CreateDesktopW(name: PCWSTR, dev: PCWSTR, mode: *const core::ffi::c_void, flags: u32, access: u32, sa: *const core::ffi::c_void) -> isize;
    fn SetThreadDesktop(desk: isize) -> i32;
    fn CloseDesktop(desk: isize) -> i32;
}

/// Processes window messages for `ms` milliseconds.
pub fn pump(ms: u64) {
    let until = Instant::now() + Duration::from_millis(ms);
    let mut msg = MSG::default();
    while Instant::now() < until {
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Runs `f` against a live launcher created with `config` (TOML).
pub fn run(name: &str, config: &str, f: impl FnOnce(&mut Launcher) + Send + 'static) {
    crate::ui::set_headless();
    let dir = std::env::temp_dir().join(format!("sigma5-harness-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("SIGMA5")).unwrap();
    std::fs::write(dir.join("SIGMA5").join("config.toml"), config).unwrap();
    unsafe {
        std::env::set_var("APPDATA", &dir);
        std::env::set_var("LOCALAPPDATA", &dir);
    }
    let desk_name = crate::util::wide(&format!("SIGMA5.Harness.{name}"));
    std::thread::spawn(move || unsafe {
        let desk = CreateDesktopW(PCWSTR(desk_name.as_ptr()), PCWSTR::null(), std::ptr::null(), 0, 0x1000_0000, std::ptr::null());
        assert!(desk != 0 && SetThreadDesktop(desk) != 0, "private desktop");
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
        let hwnd = create();
        pump(300);
        let ui = &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Launcher);
        f(ui);
        let _ = DestroyWindow(hwnd);
        pump(50);
        CloseDesktop(desk);
    })
    .join()
    .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

impl Launcher {
    /// Types into the search box as a user would.
    pub fn type_query(&mut self, text: &str) {
        let w = wide(text);
        unsafe {
            let _ = SetWindowTextW(self.edit, PCWSTR(w.as_ptr()));
        }
        self.on_query_changed();
        pump(30);
    }

    pub fn test_show(&mut self) {
        self.show();
        pump(50);
    }

    pub fn test_mode(&mut self, mode: Mode) {
        self.enter_mode(mode);
        pump(30);
    }

    pub fn test_add_color(&mut self, hex: &str) {
        self.colors.add(crate::colors::parse(hex).unwrap());
    }

    pub fn test_pulse(&mut self, row: Option<usize>, t: f32) {
        self.pulse_row = row;
        self.pulse_start = Some(Instant::now() - Duration::from_millis((t * m::PULSE_MS as f32) as u64));
    }

    /// Renders the window (including the search box) to a PNG.
    pub fn snapshot(&mut self, path: &std::path::Path) {
        let (w, h) = self.client_size();
        unsafe {
            let screen = GetDC(None);
            let dc = windows::Win32::Graphics::Gdi::CreateCompatibleDC(Some(screen));
            let info = windows::Win32::Graphics::Gdi::BITMAPINFO {
                bmiHeader: windows::Win32::Graphics::Gdi::BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<windows::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bmp = windows::Win32::Graphics::Gdi::CreateDIBSection(Some(dc), &info, windows::Win32::Graphics::Gdi::DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
            let old = SelectObject(dc, bmp.into());
            windows::Win32::Graphics::Gdi::SetBkMode(dc, windows::Win32::Graphics::Gdi::TRANSPARENT);
            paint::draw(self, dc, w, h);
            // The search box is a child control: print it in place.
            let mut rc = RECT::default();
            let _ = GetWindowRect(self.edit, &mut rc);
            let mut pt = POINT { x: rc.left, y: rc.top };
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(self.hwnd, &mut pt);
            let mut prev = POINT::default();
            let _ = windows::Win32::Graphics::Gdi::SetViewportOrgEx(dc, pt.x, pt.y, Some(&mut prev));
            SendMessageW(self.edit, WM_PRINT, Some(WPARAM(dc.0 as usize)), Some(LPARAM((PRF_CLIENT | PRF_ERASEBKGND) as isize)));
            let _ = windows::Win32::Graphics::Gdi::SetViewportOrgEx(dc, prev.x, prev.y, None);
            let _ = windows::Win32::Graphics::Gdi::GdiFlush();
            let px = std::slice::from_raw_parts(bits as *const u32, (w * h) as usize);
            crate::ui::draw::write_png(&path.to_string_lossy(), px, w as usize, h as usize, 1);
            SelectObject(dc, old);
            let _ = DeleteObject(bmp.into());
            let _ = windows::Win32::Graphics::Gdi::DeleteDC(dc);
            ReleaseDC(None, screen);
        }
    }
}

const BASE_CONFIG: &str = r#"
hotkey = "Ctrl+Alt+Shift+F24"
theme = "dark"
accent = "violet"
disabled_modules = ["clipboard"]
"#;

#[test]
#[ignore]
fn launcher_snapshots() {
    let Ok(dir) = std::env::var("SIGMA5_SNAP_DIR") else { return };
    let dir = std::path::PathBuf::from(dir);
    for (theme, cfg) in [("dark", BASE_CONFIG.to_string()), ("light", BASE_CONFIG.replace("\"dark\"", "\"light\""))] {
        let dir = dir.clone();
        run(theme, &cfg, move |ui| {
            ui.test_show();
            pump(2500); // app index, icons and file index arrive
            ui.snapshot(&dir.join(format!("{theme}-1-root.png")));
            ui.type_query("lab");
            ui.snapshot(&dir.join(format!("{theme}-2-search.png")));
            ui.type_query("#ab72ff");
            ui.snapshot(&dir.join(format!("{theme}-3-colour.png")));
            ui.type_query("3 kN * 0.5 m");
            ui.snapshot(&dir.join(format!("{theme}-4-calc.png")));
            ui.test_pulse(Some(1), 0.25);
            ui.snapshot(&dir.join(format!("{theme}-5-calc-pulse.png")));
            ui.type_query("");
            for c in ["#AB72FF", "#12AA9C", "#F58220", "#1C1C1F"] {
                ui.test_add_color(c);
            }
            ui.test_mode(Mode::Colors);
            ui.test_pulse(Some(3), 0.3);
            ui.snapshot(&dir.join(format!("{theme}-6-colours.png")));
            ui.test_mode(Mode::Files);
            ui.type_query("eng draw");
            ui.snapshot(&dir.join(format!("{theme}-7-files.png")));
            ui.test_mode(Mode::Root);
            ui.type_query("");
            ui.test_mode(Mode::Root);
            ui.start_pick(HWND::default());
            ui.type_query("col");
            ui.snapshot(&dir.join(format!("{theme}-8-pick.png")));
            ui.hide();
            ui.test_show();
            ui.test_mode(Mode::Suggest);
            ui.type_query("Spotify controls in the top bar");
            ui.snapshot(&dir.join(format!("{theme}-9-suggest.png")));
            println!("{theme}: top bar {:?}", ui.sysinfo);
        });
    }
    // A customised look: rounded corners, another font, custom colours.
    let custom = format!(
        "{BASE_CONFIG}
[appearance]
corner_radius = 8
font = \"Bahnschrift\"
search_font = \"Bahnschrift\"
font_scale = 110
         width = 820
row_height = 50
background = \"#0E1A24\"
accent_color = \"#FF8800\"
"
    );
    let d = dir.clone();
    run("custom", &custom, move |ui| {
        ui.test_show();
        pump(2500);
        ui.snapshot(&d.join("custom-1-root.png"));
        ui.type_query("3 kN * 0.5 m");
        ui.snapshot(&d.join("custom-2-calc.png"));
    });
}

impl Launcher {
    /// Pretends a track is playing (with gradient "album art" and some level history).
    pub fn test_media(&mut self, playing: bool) {
        let n = 256;
        let px = (0..n * n)
            .map(|i| {
                let (x, y) = (i % n, i / n);
                (40 + x * 160 / n) << 16 | (30 + y * 90 / n) << 8 | (150 + (x + y) * 90 / (2 * n))
            })
            .collect();
        let art = std::sync::Arc::new(crate::media::Art { size: n as i32, px });
        self.on_media(Some(crate::media::NowPlaying {
            title: "Midnight City".into(),
            artist: "M83".into(),
            album: "Hurry Up, We're Dreaming".into(),
            app: "Spotify.exe".into(),
            playing,
            position_ms: Some(83_000),
            duration_ms: Some(243_000),
            read_at: Instant::now(),
            art: Some(art),
        }));
        for i in 0..60 {
            let v = ((i as f32 * 0.7).sin() * 0.5 + 0.5) * 0.6 + ((i * 37 % 11) as f32 / 11.0) * 0.4;
            self.levels.push(v * v);
        }
    }

    /// Types into the Typing Test as a user would.
    pub fn test_type(&mut self, s: &str) {
        for c in s.encode_utf16() {
            self.on_type_char(c);
        }
    }
}

#[test]
#[ignore]
fn new_pages_snapshots() {
    let Ok(dir) = std::env::var("SIGMA5_SNAP_DIR") else { return };
    let dir = std::path::PathBuf::from(dir);
    let cfg = format!("{BASE_CONFIG}\n[top_bar]\nitems = [\"clock\", \"date\", \"media\", \"cpu\", \"ram\"]\n");
    for (theme, cfg) in [("dark", cfg.clone()), ("light", cfg.replace("\"dark\"", "\"light\""))] {
        let dir = dir.clone();
        run(&format!("pages-{theme}"), &cfg, move |ui| {
            ui.test_show();
            ui.snapshot(&dir.join(format!("{theme}-a-nothing-playing.png")));
            ui.test_media(true);
            ui.snapshot(&dir.join(format!("{theme}-b-topbar-media.png")));
            ui.test_mode(Mode::Media);
            ui.snapshot(&dir.join(format!("{theme}-c-now-playing.png")));
            ui.test_media(false);
            ui.snapshot(&dir.join(format!("{theme}-d-paused.png")));
            ui.go_back();
            ui.test_mode(Mode::Typing);
            ui.snapshot(&dir.join(format!("{theme}-e-typing-start.png")));
            let words: Vec<&str> = ui.typing.as_ref().unwrap().words[..10].to_vec();
            let mut s = String::new();
            for (i, w) in words[..9].iter().enumerate() {
                // A couple of mistakes: a wrong letter, then an extra one.
                let w = match i {
                    2 => w.replacen(|_| true, "x", 1),
                    4 => format!("{w}s"),
                    _ => w.to_string(),
                };
                s.push_str(&w);
                s.push(' ');
            }
            ui.test_type(&s);
            ui.test_type(&words[9][..2]);
            ui.snapshot(&dir.join(format!("{theme}-f-typing-run.png")));
            // Finish a 10-word run for the results screen.
            ui.set_typing_kind(crate::typing::Kind::Words(10));
            let words: Vec<&str> = ui.typing.as_ref().unwrap().words.clone();
            ui.typing.as_mut().unwrap().started = Some(Instant::now() - Duration::from_secs(9));
            ui.test_type(&words.join(" "));
            ui.snapshot(&dir.join(format!("{theme}-g-typing-results.png")));
            ui.go_back();
            ui.test_mode(Mode::Greek);
            ui.snapshot(&dir.join(format!("{theme}-h-greek.png")));
            ui.go_back();
            ui.type_query("theta");
            ui.snapshot(&dir.join(format!("{theme}-i-root-theta.png")));
        });
    }
}

#[test]
#[ignore]
fn harvest_snapshots() {
    let Ok(dir) = std::env::var("SIGMA5_SNAP_DIR") else { return };
    let dir = std::path::PathBuf::from(dir);
    run("harvest", BASE_CONFIG, move |ui| {
        ui.test_show();
        ui.test_mode(Mode::Typing);
        ui.snapshot(&dir.join("harvest-0-start.png"));
        ui.set_typing_kind(crate::typing::Kind::Words(25));
        pump(30);
        let words: Vec<&str> = ui.typing.as_ref().unwrap().words.clone();
        ui.typing.as_mut().unwrap().started = Some(Instant::now() - Duration::from_secs(20));
        let mut s = String::new();
        for (i, w) in words.iter().enumerate() {
            if i == 3 { s.push_str("xx") } else { s.push_str(w) }
            s.push(' ');
        }
        // Type all but the last word, draw a frame (records word positions), then finish.
        let cut = s.trim_end().rfind(' ').unwrap();
        ui.test_type(&s[..cut + 1]);
        ui.snapshot(&dir.join("harvest-1-before.png"));
        ui.test_type(words[24]);
        ui.snapshot(&dir.join("harvest-2-result.png"));
        println!("harvest: {:?}", ui.harvest);
        // The fly-off layer (built directly: headless runs skip the animation).
        let mut fx = ui.make_fly().expect("words to fly");
        println!("fly-off: {} pixels on a {}x{} layer", fx.parts.len(), fx.ov.gw, fx.ov.gh);
        assert!(fx.parts.len() > 300);
        fx.start = Instant::now() - Duration::from_millis(500);
        ui.fly = Some(fx);
        ui.on_fly_timer();
        let lit = ui.fly.as_ref().unwrap().ov.lit();
        println!("fly-off frame: {lit} pixels lit");
        {
            let ov = &mut ui.fly.as_mut().unwrap().ov;
            let (gw, gh) = (ov.gw, ov.gh);
            let px: Vec<u32> = ov.grid().iter().enumerate().map(|(i, &c)| if c == crate::ui::fidget::KEY { if (i % gw / 40 + i / gw / 40) % 2 == 0 { 0x2A2D38 } else { 0x23252E } } else { c }).collect();
            crate::ui::draw::write_png(&dir.join("harvest-3-flyoff.png").to_string_lossy(), &px, gw, gh, 2);
        }
        assert!(lit > 100);
        ui.fly.as_mut().unwrap().start = Instant::now() - Duration::from_secs(3);
        ui.on_fly_timer();
        assert!(ui.fly.is_none(), "it ends on its own");
        // A second test in the same hour is practice.
        ui.typing_restart();
        ui.snapshot(&dir.join("harvest-7-practice.png"));
    });
}
