//! Fidget mode: drag the launcher and shake it hard, and the letters in the
//! search box dissolve into pixels that swirl round the mouse cursor (the
//! "sun") and clump together like planets. Anything typed while it runs
//! dissolves straight away.
//!
//! Kept lightweight on purpose:
//! - The simulation is O(n). Particles attract each other through a coarse grid
//!   of buckets (mass, centre of mass and mean velocity per bucket), never
//!   pairwise.
//! - Pixels are drawn in "cells" a few physical px wide (scaled with DPI so
//!   they look the same on every screen) into a small DIB: the screen size
//!   divided by the cell size. Only tiles that changed since the last frame
//!   are stretched onto a click-through, colour-keyed overlay window, so a
//!   frame touches a fraction of the screen.
//! - Nothing exists until fidget mode starts, and all of it is freed when it ends.

use std::cell::Cell;
use std::time::Instant;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use super::draw::mix;
use crate::util::utf16;

// ---------------------------------------------------------------------------
// Shake detection (pure; fed with cursor positions while dragging).

/// Minimum distance one swing must cover, in logical px.
const SHAKE_MIN_AMP: f32 = 28.0;
/// Movement back past this distance counts as turning round (filters jitter).
const SHAKE_HYST: f32 = 10.0;
/// Minimum average speed of a swing, logical px/s.
const SHAKE_MIN_SPEED: f32 = 450.0;
/// Longest a swing may take, in seconds.
const SHAKE_MAX_SWING: f32 = 0.35;
/// Fast swings in a row needed to count as a shake.
const SHAKE_SWINGS: u32 = 6;

#[derive(Clone, Copy, Default)]
struct Axis {
    /// Current direction: -1, 0 (not moving yet) or 1.
    dir: f32,
    /// Where the current swing started.
    anchor: f32,
    /// Furthest point of the current swing, and when it was reached.
    extreme: f32,
    extreme_t: f32,
    /// When the current swing started.
    start_t: f32,
    swings: u32,
}

impl Axis {
    fn restart(&mut self, t: f32, v: f32) {
        *self = Axis { dir: 0.0, anchor: v, extreme: v, extreme_t: t, start_t: t, swings: 0 };
    }

    /// Returns true when this sample completes a vigorous shake.
    fn feed(&mut self, t: f32, v: f32, unit: f32) -> bool {
        let hyst = SHAKE_HYST * unit;
        if self.dir == 0.0 {
            if (v - self.anchor).abs() > hyst {
                self.dir = (v - self.anchor).signum();
                self.extreme = v;
                self.extreme_t = t;
            }
            return false;
        }
        if (v - self.extreme) * self.dir > 0.0 {
            self.extreme = v;
            self.extreme_t = t;
            return false;
        }
        if (self.extreme - v) * self.dir <= hyst {
            return false;
        }
        // Turned round at `extreme`: judge the swing that just ended.
        let amp = (self.extreme - self.anchor).abs();
        let dt = (self.extreme_t - self.start_t).max(1e-3);
        let fast = amp >= SHAKE_MIN_AMP * unit && amp / dt >= SHAKE_MIN_SPEED * unit && dt <= SHAKE_MAX_SWING;
        self.swings = if fast { self.swings + 1 } else { 0 };
        self.anchor = self.extreme;
        self.start_t = self.extreme_t;
        self.dir = -self.dir;
        self.extreme = v;
        self.extreme_t = t;
        if self.swings >= SHAKE_SWINGS {
            self.swings = 0;
            return true;
        }
        false
    }
}

/// Detects a deliberate, vigorous back-and-forth shake of the dragged window.
pub struct Shake {
    /// Physical px per logical px.
    unit: f32,
    axes: [Axis; 2],
    last: Option<(f32, f32, f32)>,
    /// Smoothed velocity, physical px/s.
    vel: (f32, f32),
}

impl Shake {
    pub fn new(unit: f32) -> Self {
        Self { unit: unit.max(0.5), axes: [Axis::default(); 2], last: None, vel: (0.0, 0.0) }
    }

    /// Feeds a cursor sample (seconds, physical screen px). Returns true when
    /// the movement so far is an unmistakable shake.
    pub fn sample(&mut self, t: f32, x: f32, y: f32) -> bool {
        match self.last {
            // First sample, or the mouse rested: start counting again.
            None => {
                self.axes[0].restart(t, x);
                self.axes[1].restart(t, y);
            }
            Some((lt, _, _)) if t - lt > 0.25 => {
                self.axes[0].restart(t, x);
                self.axes[1].restart(t, y);
                self.vel = (0.0, 0.0);
            }
            Some((lt, lx, ly)) => {
                let dt = t - lt;
                if dt > 1e-4 {
                    let k = 0.4;
                    self.vel.0 += ((x - lx) / dt - self.vel.0) * k;
                    self.vel.1 += ((y - ly) / dt - self.vel.1) * k;
                }
            }
        }
        self.last = Some((t, x, y));
        let a = self.axes[0].feed(t, x, self.unit);
        let b = self.axes[1].feed(t, y, self.unit);
        if a || b {
            // Don't fire again from the same swings.
            self.axes[0].swings = 0;
            self.axes[1].swings = 0;
        }
        a || b
    }

    /// Recent velocity of the shake, physical px/s.
    pub fn velocity(&self) -> (f32, f32) {
        self.vel
    }
}

// ---------------------------------------------------------------------------
// Particle simulation (pure; logical px and seconds).

/// Size of one pixel cell in logical px (rounded to whole physical px).
pub const CELL: f32 = 2.0;
/// Oldest pixels are dropped past this, which bounds the cost of a frame.
const MAX_PARTICLES: usize = 3000;

/// Gravity of the cursor ("sun").
const SUN_G: f32 = 3.0e7;
/// How far the cursor's pull is spread out; larger means a wider area of effect.
const SUN_SOFT: f32 = 60.0;
/// Particles closer than this are pushed out, so the cursor stays visible.
const SUN_CORE: f32 = 14.0;
const SUN_CORE_PUSH: f32 = 60_000.0;
/// Particles are steered into circular orbits round the cursor, all going
/// the same way. The steering is measured per radian of orbit, so near the
/// cursor (fast orbits) it acts quickly and far away particles fly freely.
const ORBIT_STEER: f32 = 0.4;
const ORBIT_RADIAL_DRAG: f32 = 0.25;
/// Inside this radius orbits are kept a touch faster than circular and
/// outside it a touch slower, so the swirl settles into a belt this far from
/// the cursor instead of slowly spiralling in (or out).
const ORBIT_BELT: f32 = 300.0;
const ORBIT_BOOST: f32 = 0.05;
/// General air drag, per second.
const DRAG: f32 = 0.05;

/// Size of the buckets particles attract each other through.
const BUCKET: f32 = 40.0;
const CLUMP_G: f32 = 10000.0;
/// Pushes particles apart inside a clump, so planets keep their size.
const CLUMP_PUSH: f32 = 4000.0;
/// How strongly a particle matches its neighbours' velocity (keeps clumps together).
const CLUMP_VISC: f32 = 0.4;

const MAX_SPEED: f32 = 4000.0;
const BOUNCE: f32 = 0.5;
/// Speed at which a particle is drawn in the brightest colour.
const HOT_SPEED: f32 = 800.0;
/// The whole simulation runs at this fraction of real time, for a calmer swirl.
const TIME_SCALE: f32 = 0.7;

#[derive(Clone, Copy, Debug)]
pub struct Particle {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    age: f32,
    /// Index of the colour it started as (the text it came from).
    src: u8,
}

pub struct Sim {
    pub w: f32,
    pub h: f32,
    pub particles: Vec<Particle>,
    rng: u32,
    bw: usize,
    bh: usize,
    /// Per bucket: mass, Σx, Σy, Σvx, Σvy.
    buckets: Vec<[f32; 5]>,
    /// Where the sun was last step, and its smoothed velocity.
    sun_last: Option<(f32, f32)>,
    sun_v: (f32, f32),
}

impl Sim {
    pub fn new(w: f32, h: f32) -> Self {
        let bw = ((w / BUCKET).ceil() as usize).max(1);
        let bh = ((h / BUCKET).ceil() as usize).max(1);
        Self {
            w,
            h,
            particles: Vec::new(),
            rng: 0x9E37_79B9,
            bw,
            bh,
            buckets: vec![[0.0; 5]; bw * bh],
            sun_last: None,
            sun_v: (0.0, 0.0),
        }
    }

    fn rand(&mut self) -> f32 {
        // xorshift32, plenty for jitter.
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Adds a particle moving at (`vx`, `vy`) plus a little scatter, already
    /// on its way round `sun` so it peels off into an orbit at its own distance.
    pub fn spawn(&mut self, x: f32, y: f32, vx: f32, vy: f32, src: u8, sun: (f32, f32)) {
        let a = self.rand() * std::f32::consts::TAU;
        let s = 20.0 + self.rand() * 60.0;
        let (dx, dy) = (sun.0 - x, sun.1 - y);
        let r2 = dx * dx + dy * dy;
        let r = r2.sqrt().max(1e-3);
        let d2 = r2 + SUN_SOFT * SUN_SOFT;
        let v_orbit = (SUN_G * r2 / (d2 * d2.sqrt())).sqrt() * (0.8 + 0.2 * self.rand());
        self.particles.push(Particle {
            x: x.clamp(0.0, self.w - 0.01),
            y: y.clamp(0.0, self.h - 0.01),
            vx: vx + a.cos() * s - dy / r * v_orbit,
            vy: vy + a.sin() * s + dx / r * v_orbit,
            age: 0.0,
            src,
        });
        if self.particles.len() > MAX_PARTICLES {
            let extra = self.particles.len() - MAX_PARTICLES;
            self.particles.drain(..extra);
        }
    }

    /// Flings every particle away from (`cx`, `cy`).
    pub fn burst(&mut self, cx: f32, cy: f32) {
        for i in 0..self.particles.len() {
            let k = 500.0 + self.rand() * 700.0;
            let p = &mut self.particles[i];
            let (dx, dy) = (p.x - cx, p.y - cy);
            let r = (dx * dx + dy * dy).sqrt().max(1.0);
            p.vx += dx / r * k;
            p.vy += dy / r * k;
        }
    }

    pub fn step(&mut self, dt: f32, sun: (f32, f32)) {
        let dt = dt.clamp(0.0, 1.0 / 20.0) * TIME_SCALE;
        if dt <= 0.0 {
            return;
        }
        // Orbits are steered relative to the sun, so the swirl travels with it.
        if let Some((lx, ly)) = self.sun_last {
            let k = 0.3;
            self.sun_v.0 += ((sun.0 - lx) / dt - self.sun_v.0) * k;
            self.sun_v.1 += ((sun.1 - ly) / dt - self.sun_v.1) * k;
        }
        self.sun_last = Some(sun);
        let n = ((dt * 120.0).ceil() as usize).max(1);
        let h = dt / n as f32;
        for _ in 0..n {
            self.substep(h, sun);
        }
    }

    fn bucket_of(bw: usize, bh: usize, x: f32, y: f32) -> (usize, usize) {
        (((x / BUCKET) as usize).min(bw - 1), ((y / BUCKET) as usize).min(bh - 1))
    }

    fn substep(&mut self, h: f32, (sx, sy): (f32, f32)) {
        let Self { w, h: height, particles, bw, bh, buckets, sun_v, .. } = self;
        let (w, height, bw, bh, (svx, svy)) = (*w, *height, *bw, *bh, *sun_v);
        buckets.iter_mut().for_each(|b| *b = [0.0; 5]);
        for p in particles.iter() {
            let (bx, by) = Self::bucket_of(bw, bh, p.x, p.y);
            let b = &mut buckets[by * bw + bx];
            b[0] += 1.0;
            b[1] += p.x;
            b[2] += p.y;
            b[3] += p.vx;
            b[4] += p.vy;
        }
        let damp = (-DRAG * h).exp();
        let soft2 = SUN_SOFT * SUN_SOFT;
        for p in particles.iter_mut() {
            let (mut ax, mut ay) = (0.0f32, 0.0f32);

            // The sun: softened gravity, with a push-out core.
            let (dx, dy) = (sx - p.x, sy - p.y);
            let r2 = dx * dx + dy * dy;
            let r = r2.sqrt().max(1e-3);
            let (ux, uy) = (dx / r, dy / r);
            let d2 = r2 + soft2;
            let g = SUN_G / (d2 * d2.sqrt());
            ax += dx * g;
            ay += dy * g;
            if r < SUN_CORE {
                let k = SUN_CORE_PUSH * (1.0 - r / SUN_CORE);
                ax -= ux * k;
                ay -= uy * k;
            }

            // Neighbouring buckets: attraction, packing pressure, viscosity.
            let (bx, by) = Self::bucket_of(bw, bh, p.x, p.y);
            for ny in by.saturating_sub(1)..=(by + 1).min(bh - 1) {
                for nx in bx.saturating_sub(1)..=(bx + 1).min(bw - 1) {
                    let mut b = buckets[ny * bw + nx];
                    let own = nx == bx && ny == by;
                    if own {
                        b[0] -= 1.0;
                        b[1] -= p.x;
                        b[2] -= p.y;
                        b[3] -= p.vx;
                        b[4] -= p.vy;
                    }
                    let m = b[0];
                    if m < 0.5 {
                        continue;
                    }
                    let (dx, dy) = (b[1] / m - p.x, b[2] / m - p.y);
                    let r2 = dx * dx + dy * dy;
                    // Radius of a tightly packed clump of m cells.
                    let rad = 0.56 * CELL * m.sqrt() + 0.5 * CELL;
                    let d2 = r2 + rad * rad;
                    let g = CLUMP_G * m / (d2 * d2.sqrt());
                    ax += dx * g;
                    ay += dy * g;
                    if r2 < rad * rad {
                        let r = r2.sqrt().max(1e-3);
                        let k = CLUMP_PUSH * (1.0 - r / rad) / r;
                        ax -= dx * k;
                        ay -= dy * k;
                    }
                    if own {
                        ax += (b[3] / m - p.vx) * CLUMP_VISC;
                        ay += (b[4] / m - p.vy) * CLUMP_VISC;
                    }
                }
            }

            // Orbit steering: the speed a circular orbit needs given the
            // total inward pull (at least the sun's own, so particles that
            // fall into the core get spun back out), against how fast this
            // particle actually goes round (relative to the moving sun).
            let inward = (ax * ux + ay * uy).max(g * r);
            let v_target = (inward * r).sqrt();
            let omega = v_target / r.max(SUN_SOFT);
            let (rvx, rvy) = (p.vx - svx, p.vy - svy);
            let (tx, ty) = (-uy, ux);
            let v_t = rvx * tx + rvy * ty;
            let v_r = rvx * ux + rvy * uy;
            let boost = 1.0 + (ORBIT_BOOST * (1.0 - r / ORBIT_BELT)).clamp(-0.03, ORBIT_BOOST);
            let steer = (v_target * boost - v_t) * omega * ORBIT_STEER;
            let radial = v_r * omega * ORBIT_RADIAL_DRAG;
            ax += tx * steer - ux * radial;
            ay += ty * steer - uy * radial;

            p.vx = (p.vx + ax * h) * damp;
            p.vy = (p.vy + ay * h) * damp;
            let s2 = p.vx * p.vx + p.vy * p.vy;
            if s2 > MAX_SPEED * MAX_SPEED {
                let k = MAX_SPEED / s2.sqrt();
                p.vx *= k;
                p.vy *= k;
            }
            p.x += p.vx * h;
            p.y += p.vy * h;
            if p.x < 0.0 {
                p.x = 0.0;
                p.vx = p.vx.abs() * BOUNCE;
            } else if p.x >= w {
                p.x = w - 0.01;
                p.vx = -p.vx.abs() * BOUNCE;
            }
            if p.y < 0.0 {
                p.y = 0.0;
                p.vy = p.vy.abs() * BOUNCE;
            } else if p.y >= height {
                p.y = height - 0.01;
                p.vy = -p.vy.abs() * BOUNCE;
            }
            p.age += h;
        }
    }

    /// Draws every particle as one cell. `scale` is cells per logical px.
    pub fn render(&self, grid: &mut [u32], gw: usize, gh: usize, scale: f32, pal: &Palette) {
        for p in &self.particles {
            let (gx, gy) = ((p.x * scale) as usize, (p.y * scale) as usize);
            if gx < gw && gy < gh {
                grid[gy * gw + gx] = pal.color(p);
            }
        }
    }
}

/// Particle colours (DIB pixel format, 0x00RRGGBB), by source, age and speed.
pub struct Palette {
    colors: [[[u32; 16]; 4]; 2],
}

impl Palette {
    /// `sources`: the colours particles start as (search text, placeholder).
    pub fn new(sources: [COLORREF; 2], accent: COLORREF) -> Self {
        let dim = mix(accent, COLORREF(0), 0.3);
        let hot = mix(accent, COLORREF(0x00FF_FFFF), 0.85);
        let mut colors = [[[0u32; 16]; 4]; 2];
        for (s, src) in sources.iter().enumerate() {
            for (a, age) in [0.0, 0.4, 0.75, 1.0].iter().enumerate() {
                for i in 0..16 {
                    let t = i as f32 / 15.0;
                    let speed_c = if t < 0.3 { mix(dim, accent, t / 0.3) } else { mix(accent, hot, (t - 0.3) / 0.7) };
                    colors[s][a][i] = to_dib(mix(*src, speed_c, *age));
                }
            }
        }
        Self { colors }
    }

    fn color(&self, p: &Particle) -> u32 {
        let age = match p.age {
            a if a < 0.12 => 0,
            a if a < 0.3 => 1,
            a if a < 0.55 => 2,
            _ => 3,
        };
        let speed = (p.vx * p.vx + p.vy * p.vy).sqrt();
        let i = ((speed / HOT_SPEED * 15.0) as usize).min(15);
        self.colors[(p.src as usize).min(1)][age][i]
    }
}

/// Transparent colour of the overlay (identical in COLORREF and DIB order).
const KEY: u32 = 0x0001_0001;

fn to_dib(c: COLORREF) -> u32 {
    let (r, g, b) = (c.0 & 0xFF, (c.0 >> 8) & 0xFF, (c.0 >> 16) & 0xFF);
    let v = r << 16 | g << 8 | b;
    if v == KEY { v ^ 0x100 } else { v }
}

/// Cells (in grid coordinates) covered by text. `cov` is a `tw`×`th`
/// coverage bitmap whose top-left sits at (`x0`, `y0`) physical px in grid
/// space; a cell counts when ink covers a quarter of it.
pub fn sample_cells(cov: &[u8], tw: i32, th: i32, x0: i32, y0: i32, cell: i32) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    if tw <= 0 || th <= 0 || cell <= 0 {
        return out;
    }
    let (gx0, gy0) = (x0.div_euclid(cell), y0.div_euclid(cell));
    let (gx1, gy1) = ((x0 + tw - 1).div_euclid(cell), (y0 + th - 1).div_euclid(cell));
    let need = (cell * cell) as u32 * 255 / 4;
    for gy in gy0..=gy1 {
        for gx in gx0..=gx1 {
            let mut sum = 0u32;
            for py in (gy * cell - y0).max(0)..((gy + 1) * cell - y0).min(th) {
                for px in (gx * cell - x0).max(0)..((gx + 1) * cell - x0).min(tw) {
                    sum += cov[(py * tw + px) as usize] as u32;
                }
            }
            if sum >= need {
                out.push((gx, gy));
            }
        }
    }
    out
}

/// Renders `text` white-on-black and returns its coverage (0–255) and size.
pub fn rasterize(text: &str, font: HFONT) -> Option<(Vec<u8>, i32, i32)> {
    let buf = utf16(text);
    if buf.is_empty() {
        return None;
    }
    unsafe {
        let dc = CreateCompatibleDC(None);
        let old_font = SelectObject(dc, font.into());
        let mut sz = SIZE::default();
        let _ = GetTextExtentPoint32W(dc, &buf, &mut sz);
        let (tw, th) = (sz.cx, sz.cy);
        let mut result = None;
        if tw > 0 && th > 0 && tw < 16_384 {
            let info = dib_info(tw, th);
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            if let Ok(bmp) = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                if !bits.is_null() {
                    let old_bmp = SelectObject(dc, bmp.into());
                    SetBkMode(dc, OPAQUE);
                    SetBkColor(dc, COLORREF(0));
                    SetTextColor(dc, COLORREF(0x00FF_FFFF));
                    let _ = TextOutW(dc, 0, 0, &buf);
                    let _ = GdiFlush();
                    let px = std::slice::from_raw_parts(bits as *const u32, (tw * th) as usize);
                    let cov = px
                        .iter()
                        .map(|&c| ((c >> 16) & 0xFF).max((c >> 8) & 0xFF).max(c & 0xFF) as u8)
                        .collect();
                    result = Some((cov, tw, th));
                    SelectObject(dc, old_bmp);
                }
                let _ = DeleteObject(bmp.into());
            }
        }
        SelectObject(dc, old_font);
        let _ = DeleteDC(dc);
        result
    }
}

fn dib_info(w: i32, h: i32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Frame pacing: one frame per refresh of the monitor the pixels are on.

/// Waits for the vertical blank of one monitor, so frames match its refresh
/// rate (60, 144, 165 Hz…). DXGI is loaded only when fidget mode starts, so
/// it costs the launcher nothing otherwise.
pub struct VBlank {
    output: Option<windows::Win32::Graphics::Dxgi::IDXGIOutput>,
}

impl VBlank {
    pub fn new(monitor: HMONITOR) -> Self {
        Self { output: unsafe { dxgi_output(monitor) } }
    }

    /// Blocks until the next refresh. Falls back to DWM's composition clock,
    /// and then to a plain ~60 Hz sleep.
    pub fn wait(&self) {
        let t = Instant::now();
        let ok = match &self.output {
            Some(o) => unsafe { o.WaitForVBlank().is_ok() },
            None => unsafe { windows::Win32::Graphics::Dwm::DwmFlush().is_ok() },
        };
        // A display that's asleep can return at once; don't spin.
        if !ok || t.elapsed() < std::time::Duration::from_micros(500) {
            std::thread::sleep(std::time::Duration::from_millis(if ok { 4 } else { 16 }));
        }
    }
}

unsafe fn dxgi_output(monitor: HMONITOR) -> Option<windows::Win32::Graphics::Dxgi::IDXGIOutput> {
    use windows::Win32::Graphics::Dxgi::IDXGIFactory1;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::core::{GUID, HRESULT, Interface, s};
    type Create = unsafe extern "system" fn(*const GUID, *mut *mut core::ffi::c_void) -> HRESULT;
    unsafe {
        let dll = LoadLibraryW(w!("dxgi.dll")).ok()?;
        let create: Create = std::mem::transmute(GetProcAddress(dll, s!("CreateDXGIFactory1"))?);
        let mut raw = std::ptr::null_mut();
        create(&IDXGIFactory1::IID, &mut raw).ok().ok()?;
        let factory = IDXGIFactory1::from_raw(raw);
        for a in 0.. {
            let adapter = factory.EnumAdapters1(a).ok()?;
            for o in 0.. {
                let Ok(output) = adapter.EnumOutputs(o) else { break };
                if output.GetDesc().is_ok_and(|d| d.Monitor == monitor) {
                    return Some(output);
                }
            }
        }
        None
    }
}

/// Background thread that posts `msg` to `hwnd` once per refresh. A frame
/// is only posted once the previous one has been drawn, so a slow frame
/// never queues up a backlog. Stops when dropped.
pub struct Pacer {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Pacer {
    pub fn start(hwnd: HWND, msg: u32, monitor: HMONITOR) -> Self {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let stop = Arc::new(AtomicBool::new(false));
        let pending = Arc::new(AtomicBool::new(false));
        let (s, p) = (stop.clone(), pending.clone());
        let target = hwnd.0 as isize;
        let mon = monitor.0 as isize;
        let _ = std::thread::Builder::new().name("sigma5-vsync".into()).spawn(move || {
            let vblank = VBlank::new(HMONITOR(mon as *mut _));
            while !s.load(Ordering::Relaxed) {
                vblank.wait();
                if !p.swap(true, Ordering::AcqRel) {
                    unsafe {
                        let _ = PostMessageW(Some(HWND(target as *mut _)), msg, WPARAM(0), LPARAM(0));
                    }
                }
            }
        });
        Self { stop, pending }
    }

    /// Call when a frame has been drawn, to accept the next one.
    pub fn frame_done(&self) {
        self.pending.store(false, std::sync::atomic::Ordering::Release);
    }
}

impl Drop for Pacer {
    fn drop(&mut self) {
        // The thread notices within one refresh; no need to wait for it.
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// The overlay window.

/// Cells per tile when finding what changed between frames.
const TILE: usize = 16;

thread_local! {
    /// The overlay was repainted by the system; redraw all of it next frame.
    static FULL_REDRAW: Cell<bool> = const { Cell::new(false) };
}

pub struct Fidget {
    hwnd: HWND,
    /// Monitor origin (physical screen px).
    left: i32,
    top: i32,
    unit: f32,
    cell: i32,
    gw: usize,
    gh: usize,
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    prev: Vec<u32>,
    first: bool,
    pub sim: Sim,
    palette: Palette,
    last_tick: Instant,
}

impl Fidget {
    /// Opens the overlay over `monitor` (physical px). `unit` is physical px
    /// per logical px; `sources` are the colours of search text and placeholder.
    pub fn start(owner: HWND, monitor: RECT, unit: f32, sources: [COLORREF; 2], accent: COLORREF) -> Option<Box<Self>> {
        let (mw, mh) = (monitor.right - monitor.left, monitor.bottom - monitor.top);
        if mw <= 0 || mh <= 0 {
            return None;
        }
        // One px short of the full height, so Windows doesn't treat the
        // overlay as a full-screen app (that would hide the taskbar).
        let mh = mh - 1;
        let cell = ((CELL * unit).round() as i32).max(2);
        let (gw, gh) = ((mw + cell - 1) / cell, (mh + cell - 1) / cell);
        unsafe {
            register_class();
            let hinst = GetModuleHandleW(None).ok()?;
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                CLASS,
                PCWSTR::null(),
                WS_POPUP,
                monitor.left,
                monitor.top,
                mw,
                mh,
                Some(owner),
                None,
                Some(hinst.into()),
                None,
            )
            .ok()?;
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(KEY), 0, LWA_COLORKEY);

            let dc = CreateCompatibleDC(None);
            let info = dib_info(gw, gh);
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let bmp = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) if !bits.is_null() => b,
                _ => {
                    let _ = DeleteDC(dc);
                    let _ = DestroyWindow(hwnd);
                    return None;
                }
            };
            let old = SelectObject(dc, bmp.into());
            let n = (gw * gh) as usize;
            std::slice::from_raw_parts_mut(bits as *mut u32, n).fill(KEY);
            let me = Box::new(Self {
                hwnd,
                left: monitor.left,
                top: monitor.top,
                unit,
                cell,
                gw: gw as usize,
                gh: gh as usize,
                dc,
                bmp,
                old,
                bits: bits as *mut u32,
                prev: vec![KEY; n],
                first: true,
                sim: Sim::new(mw as f32 / unit, mh as f32 / unit),
                palette: Palette::new(sources, accent),
                last_tick: Instant::now(),
            });
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            Some(me)
        }
    }

    /// Screen point (physical px) → simulation space (logical px).
    pub fn to_sim(&self, x: i32, y: i32) -> (f32, f32) {
        ((x - self.left) as f32 / self.unit, (y - self.top) as f32 / self.unit)
    }

    /// Turns `text`, drawn with `font` with its top-left at `at` (physical
    /// screen px), into particles moving at `vel` (logical px/s). Returns the
    /// text's width in physical px.
    pub fn dissolve(&mut self, text: &str, font: HFONT, at: POINT, src: u8, vel: (f32, f32)) -> i32 {
        let Some((cov, tw, th)) = rasterize(text, font) else { return 0 };
        let (x0, y0) = (at.x - self.left, at.y - self.top);
        let mut cursor = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut cursor);
        }
        let sun = self.to_sim(cursor.x, cursor.y);
        let c = self.cell as f32;
        for (gx, gy) in sample_cells(&cov, tw, th, x0, y0, self.cell) {
            let (x, y) = ((gx as f32 + 0.5) * c / self.unit, (gy as f32 + 0.5) * c / self.unit);
            self.sim.spawn(x, y, vel.0, vel.1, src, sun);
        }
        tw
    }

    /// Advances the simulation to now and draws the frame.
    pub fn tick(&mut self, cursor: POINT) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f32();
        self.last_tick = now;
        let sun = self.to_sim(cursor.x, cursor.y);
        self.sim.step(dt, sun);
        unsafe {
            // The previous frame's blits must finish reading the DIB first.
            let _ = GdiFlush();
        }
        let n = self.gw * self.gh;
        let grid = unsafe { std::slice::from_raw_parts_mut(self.bits, n) };
        grid.fill(KEY);
        self.sim.render(grid, self.gw, self.gh, self.unit / self.cell as f32, &self.palette);
        self.present();
    }

    /// Copies the tiles that changed (all of them after a system repaint)
    /// onto the overlay.
    fn present(&mut self) {
        let full = FULL_REDRAW.with(|f| f.replace(false)) || std::mem::take(&mut self.first);
        let (gw, gh) = (self.gw, self.gh);
        let grid = unsafe { std::slice::from_raw_parts(self.bits, gw * gh) };
        let (tx_n, ty_n) = (gw.div_ceil(TILE), gh.div_ceil(TILE));
        let dirty = |tx: usize, ty: usize| {
            if full {
                return true;
            }
            let (x0, x1) = (tx * TILE, ((tx + 1) * TILE).min(gw));
            (ty * TILE..((ty + 1) * TILE).min(gh)).any(|y| grid[y * gw + x0..y * gw + x1] != self.prev[y * gw + x0..y * gw + x1])
        };
        unsafe {
            let hdc = GetDC(Some(self.hwnd));
            SetStretchBltMode(hdc, COLORONCOLOR);
            let c = self.cell;
            for ty in 0..ty_n {
                let (y0, y1) = (ty * TILE, ((ty + 1) * TILE).min(gh));
                let mut run: Option<usize> = None;
                for tx in 0..=tx_n {
                    let d = tx < tx_n && dirty(tx, ty);
                    match (d, run) {
                        (true, None) => run = Some(tx),
                        (false, Some(start)) => {
                            let (x0, x1) = (start * TILE, (tx * TILE).min(gw));
                            let (sw, sh) = ((x1 - x0) as i32, (y1 - y0) as i32);
                            let _ = StretchBlt(
                                hdc,
                                x0 as i32 * c,
                                y0 as i32 * c,
                                sw * c,
                                sh * c,
                                Some(self.dc),
                                x0 as i32,
                                y0 as i32,
                                sw,
                                sh,
                                SRCCOPY,
                            );
                            run = None;
                        }
                        _ => {}
                    }
                }
            }
            ReleaseDC(Some(self.hwnd), hdc);
        }
        self.prev.copy_from_slice(grid);
    }
}

impl Drop for Fidget {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

const CLASS: PCWSTR = w!("SIGMA5.Fidget");

fn register_class() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(overlay_proc),
            hInstance: GetModuleHandleW(None).unwrap_or_default().into(),
            // Erasing paints the transparent colour.
            hbrBackground: CreateSolidBrush(COLORREF(KEY)),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    });
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_PAINT => unsafe {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(hwnd, &mut ps);
            let _ = EndPaint(hwnd, &ps);
            FULL_REDRAW.with(|f| f.set(true));
            LRESULT(0)
        },
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Samples a path at 125 Hz (a typical mouse) and reports whether it shook.
    fn shakes(unit: f32, secs: f32, path: impl Fn(f32) -> (f32, f32)) -> bool {
        let mut s = Shake::new(unit);
        let mut t = 0.0;
        while t < secs {
            let (x, y) = path(t);
            if s.sample(t, x, y) {
                return true;
            }
            t += 0.008;
        }
        false
    }

    fn sine(amp: f32, hz: f32) -> impl Fn(f32) -> (f32, f32) {
        move |t| (500.0 + amp * (t * hz * std::f32::consts::TAU).sin(), 300.0)
    }

    #[test]
    fn vigorous_shake_triggers() {
        assert!(shakes(1.0, 2.0, sine(90.0, 4.0)));
        // Diagonal shake.
        assert!(shakes(1.0, 2.0, |t| {
            let s = (t * 5.0 * std::f32::consts::TAU).sin();
            (500.0 + 70.0 * s, 400.0 + 50.0 * s)
        }));
    }

    #[test]
    fn scales_with_dpi() {
        // The same physical-size shake on a 200% screen covers twice the px.
        assert!(shakes(2.0, 2.0, sine(180.0, 4.0)));
        // A shake that's small in physical terms on a 200% screen does not count.
        assert!(!shakes(2.0, 2.0, sine(20.0, 4.0)));
    }

    #[test]
    fn normal_dragging_does_not_trigger() {
        // Straight drag.
        assert!(!shakes(1.0, 2.0, |t| (100.0 + 800.0 * t, 300.0 + 200.0 * t)));
        // Slow back-and-forth while positioning the window.
        assert!(!shakes(1.0, 4.0, sine(150.0, 1.0)));
        // Hand tremor: fast but tiny.
        assert!(!shakes(1.0, 2.0, sine(8.0, 9.0)));
        // A couple of quick wiggles then stop.
        assert!(!shakes(1.0, 2.0, |t| if t < 0.4 { sine(90.0, 4.0)(t) } else { (500.0, 300.0) }));
    }

    #[test]
    fn shake_reports_velocity() {
        let mut s = Shake::new(1.0);
        for i in 0..20 {
            s.sample(i as f32 * 0.01, i as f32 * 10.0, 0.0);
        }
        let (vx, vy) = s.velocity();
        assert!((vx - 1000.0).abs() < 50.0 && vy.abs() < 1.0, "{vx} {vy}");
    }

    #[test]
    fn sampling_finds_ink() {
        // A 6x4 bitmap with a solid 4x4 block on the right, drawn at x=2.
        let (tw, th) = (6, 4);
        let cov: Vec<u8> = (0..tw * th).map(|i| if i % tw >= 2 { 255 } else { 0 }).collect();
        let cells = sample_cells(&cov, tw, th, 2, 0, 2);
        // Text spans px 2..8 → cells 1..3; ink at px 4..8 → cells 2 and 3.
        assert_eq!(cells, vec![(2, 0), (3, 0), (2, 1), (3, 1)]);
        assert!(sample_cells(&cov, 0, 0, 0, 0, 2).is_empty());
    }

    #[test]
    fn simulation_stays_finite_and_on_screen() {
        let mut sim = Sim::new(1920.0, 1080.0);
        for i in 0..2000 {
            sim.spawn(700.0 + (i % 200) as f32 * 2.5, 300.0 + (i / 200) as f32 * 2.5, 600.0, -200.0, 0, (960.0, 540.0));
        }
        // The cursor wanders, sits still, then whips across the screen.
        for f in 0..1200 {
            let t = f as f32 / 60.0;
            let sun = if f < 600 { (960.0 + 300.0 * t.cos(), 540.0 + 200.0 * t.sin()) } else { ((f % 120) as f32 * 16.0, 540.0) };
            sim.step(1.0 / 60.0, sun);
        }
        sim.step(5.0, (0.0, 0.0)); // a long stall is clamped
        for p in &sim.particles {
            assert!(p.x.is_finite() && p.y.is_finite() && p.vx.is_finite() && p.vy.is_finite());
            assert!((0.0..1920.0).contains(&p.x) && (0.0..1080.0).contains(&p.y));
        }
    }

    #[test]
    fn particles_orbit_rather_than_collapse() {
        let mut sim = Sim::new(1920.0, 1080.0);
        for i in 0..800 {
            sim.spawn(800.0 + (i % 80) as f32 * 2.5, 450.0 + (i / 80) as f32 * 2.5, 0.0, 0.0, 0, (960.0, 540.0));
        }
        let sun = (960.0, 540.0);
        for _ in 0..1200 {
            sim.step(1.0 / 60.0, sun);
        }
        // After 20 s most particles are still moving round the sun, not
        // sitting in a heap on it.
        let moving = sim
            .particles
            .iter()
            .filter(|p| {
                let (dx, dy) = (p.x - sun.0, p.y - sun.1);
                let r = (dx * dx + dy * dy).sqrt();
                let tangential = (-dy * p.vx + dx * p.vy).abs() / r.max(1.0);
                r > SUN_CORE && tangential > 30.0
            })
            .count();
        assert!(moving > sim.particles.len() / 2, "{moving} of {} orbiting", sim.particles.len());
    }

    /// Runs the real overlay on a private desktop (never shown, no input
    /// sent) and measures a frame: `cargo test overlay_offscreen -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn overlay_offscreen() {
        use crate::ui::draw;
        #[link(name = "user32")]
        unsafe extern "system" {
            fn CreateDesktopW(name: PCWSTR, dev: PCWSTR, mode: *const core::ffi::c_void, flags: u32, access: u32, sa: *const core::ffi::c_void) -> isize;
            fn SetThreadDesktop(desk: isize) -> i32;
            fn CloseDesktop(desk: isize) -> i32;
        }
        std::thread::spawn(|| unsafe {
            let desk = CreateDesktopW(w!("SIGMA5.FidgetTest"), PCWSTR::null(), std::ptr::null(), 0, 0x1000_0000, std::ptr::null());
            assert!(desk != 0, "CreateDesktop failed");
            assert!(SetThreadDesktop(desk) != 0, "SetThreadDesktop failed");
            let owner = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                PCWSTR::null(),
                WS_POPUP,
                0,
                0,
                10,
                10,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let hd = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
            let uhd = RECT { left: 0, top: 0, right: 3840, bottom: 2160 };
            for (label, rc, unit, worst) in [
                ("1080p 100%, typical", hd, 1.0, false),
                ("1080p 100%, worst", hd, 1.0, true),
                ("4K 150%, typical", uhd, 1.5, false),
                ("4K 150%, worst", uhd, 1.5, true),
            ] {
                let mut f = Fidget::start(owner, rc, unit, [COLORREF(0xEEEEEE), COLORREF(0x888888)], COLORREF(0xFF72AB)).unwrap();
                let font = draw::font((19.0 * unit) as i32, 400, draw::DISPLAY_FACES);
                f.dissolve("Search for apps and commands…", font, POINT { x: 400, y: 200 }, 1, (0.0, 0.0));
                let letters = f.sim.particles.len();
                assert!(letters > 300, "{letters}");
                if worst {
                    // The particle cap, spread over the whole screen.
                    for i in 0..MAX_PARTICLES {
                        let (x, y) = ((i * 37 % 1900) as f32, (i * 53 % 1000) as f32);
                        f.sim.spawn(x, y, 0.0, 0.0, 0, (900.0, 500.0));
                    }
                } else {
                    // About a sentence of typing, then let it settle into orbit.
                    for i in 0..1000 {
                        let (x, y) = (500.0 + (i % 100) as f32 * 2.0, 200.0 + (i / 100) as f32 * 2.0);
                        f.sim.spawn(x, y, 0.0, 0.0, 0, (600.0, 400.0));
                    }
                    for _ in 0..180 {
                        f.sim.step(1.0 / 60.0, (600.0, 400.0));
                    }
                }
                let frames = 240;
                let t0 = Instant::now();
                for i in 0..frames {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    // The cursor drifts slowly, as when idly moving the mouse.
                    f.tick(POINT { x: ((600.0 + i as f32) * unit) as i32, y: (400.0 * unit) as i32 });
                }
                let per = t0.elapsed().as_secs_f64() * 1000.0 / frames as f64 - 1.0;
                let drawn = std::slice::from_raw_parts(f.bits, f.gw * f.gh).iter().filter(|&&c| c != KEY).count();
                println!(
                    "{label}: {letters} letter pixels, cell {} px, grid {}x{} ({} KB), {drawn} cells lit, {per:.2} ms/frame with {} particles",
                    f.cell,
                    f.gw,
                    f.gh,
                    f.gw * f.gh * 8 / 1024,
                    f.sim.particles.len()
                );
                assert!(drawn > 500);
                let _ = DeleteObject(font.into());
            }
            let _ = DestroyWindow(owner);
            CloseDesktop(desk);
        })
        .join()
        .unwrap();
    }

    /// Counts refreshes of each monitor for a second (no windows, no input):
    /// `cargo test vblank_rate -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn vblank_rate() {
        unsafe extern "system" fn each(mon: HMONITOR, _: HDC, _: *mut RECT, list: LPARAM) -> windows::core::BOOL {
            unsafe { (*(list.0 as *mut Vec<isize>)).push(mon.0 as isize) };
            true.into()
        }
        let mut mons: Vec<isize> = Vec::new();
        unsafe {
            let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut mons as *mut _ as isize));
        }
        for m in mons {
            let v = VBlank::new(HMONITOR(m as *mut _));
            let dxgi = v.output.is_some();
            v.wait();
            let t0 = Instant::now();
            let mut n = 0;
            while t0.elapsed().as_secs_f32() < 1.0 {
                v.wait();
                n += 1;
            }
            println!("monitor {m:#x}: {n} frames/s (dxgi: {dxgi})");
            assert!(n >= 30);
        }
    }

    /// Offscreen preview for tuning: `SIGMA5_FIDGET_DUMP=<dir> cargo test
    /// fidget_dump -- --ignored` writes BMP frames of a scripted session.
    #[test]
    #[ignore]
    fn fidget_dump() {
        let Ok(dir) = std::env::var("SIGMA5_FIDGET_DUMP") else { return };
        let (w, h, cell) = (1280usize, 720usize, 2usize);
        let mut sim = Sim::new(w as f32, h as f32);
        let pal = Palette::new([COLORREF(0x00F1EEEE), COLORREF(0x00948C8C)], COLORREF(0x00FF72AB));
        // The placeholder text, dissolved from where the search box would be.
        let font = crate::ui::draw::font(19, 400, crate::ui::draw::DISPLAY_FACES);
        let (cov, tw, th) = rasterize("Search for apps and commands…", font).unwrap();
        for (gx, gy) in sample_cells(&cov, tw, th, 300, 160, cell as i32) {
            sim.spawn((gx as f32 + 0.5) * cell as f32, (gy as f32 + 0.5) * cell as f32, 300.0, 0.0, 1, (420.0, 175.0));
        }
        println!("{} particles", sim.particles.len());
        let (gw, gh) = (w / cell, h / cell);
        let mut grid = vec![0u32; gw * gh];
        let shots = [0.0f32, 0.15, 0.5, 1.5, 3.0, 6.0, 10.0, 15.0, 20.0, 40.0];
        let mut t = 0.0f32;
        for (i, &at) in shots.iter().enumerate() {
            while t < at {
                // The cursor rests by the search box, then drifts round.
                let sun = dump_sun(t);
                sim.step(1.0 / 60.0, sun);
                t += 1.0 / 60.0;
            }
            grid.fill(0x101014);
            sim.render(&mut grid, gw, gh, 1.0 / cell as f32, &pal);
            write_png(&format!("{dir}/frame{i}_{at}s.png"), &grid, gw, gh, cell);
            let sun = dump_sun(t);
            let mut rs: Vec<f32> = sim.particles.iter().map(|p| ((p.x - sun.0).powi(2) + (p.y - sun.1).powi(2)).sqrt()).collect();
            rs.sort_by(f32::total_cmp);
            let sp: f32 = sim.particles.iter().map(|p| (p.vx * p.vx + p.vy * p.vy).sqrt()).sum::<f32>() / sim.particles.len() as f32;
            println!("t={at}: radius p10={:.0} p50={:.0} p90={:.0}, mean speed {sp:.0}", rs[rs.len() / 10], rs[rs.len() / 2], rs[rs.len() * 9 / 10]);
        }
    }

    /// Cursor path for the dump: rests by the search box, circles, rests again.
    fn dump_sun(t: f32) -> (f32, f32) {
        let a = t.min(14.0) - 8.0;
        if t < 8.0 { (420.0, 175.0) } else { (640.0 + 200.0 * a.cos(), 360.0 + 120.0 * a.sin()) }
    }

    fn write_png(path: &str, px: &[u32], w: usize, h: usize, zoom: usize) {
        crate::ui::draw::write_png(path, px, w, h, zoom);
    }

    #[test]
    fn particle_cap_drops_oldest() {
        let mut sim = Sim::new(500.0, 500.0);
        for i in 0..MAX_PARTICLES + 10 {
            sim.spawn(10.0, 10.0, 0.0, 0.0, (i >= 10) as u8, (100.0, 100.0));
        }
        assert_eq!(sim.particles.len(), MAX_PARTICLES);
        assert!(sim.particles.iter().all(|p| p.src == 1));
    }

    #[test]
    fn palette_never_uses_the_transparent_colour() {
        let p = Palette::new([COLORREF(KEY), COLORREF(0)], COLORREF(KEY));
        assert!(p.colors.iter().flatten().flatten().all(|&c| c != KEY));
    }
}

