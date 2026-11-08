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
