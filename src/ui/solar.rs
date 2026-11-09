//! The Solar System, out on the desktop: your star in the middle of the
//! screen and its planets orbiting over everything, all drawn as pixels,
//! interface included (a tiny built-in pixel font, `pixfont`).
//!
//! While it's open it takes over the mouse and keyboard: an invisible
//! full-screen layer catches every click (the desktop shows through but
//! can't be clicked), so nothing slips through to the apps behind. Hold the
//! mouse to release the material picked in the toolbar, drag a planet to
//! give it a new orbit, click one to see it. Esc puts it all away at once,
//! as does switching to another app.
//!
//! The launcher shows the star as a tiny badge circling its window
//! ([`StarBadge`]); clicking it opens the system with a whoosh.
//!
//! Costs: nothing until opened. While open, frames follow the monitor at
//! 30 fps (60 while you're doing something), only changed tiles reach the
//! screen, and the simulation itself is tiny.

use std::cell::Cell;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_BACK, VK_ESCAPE, VK_F2, VK_RETURN};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use super::fidget::{KEY, Overlay, Pacer};
use super::pixfont;
use super::{WM_APP_OPEN_SOLAR, WM_APP_OPEN_TYPING};
use crate::solar::{self, LAYERS, Material, N, Sim, Universe};

/// Posted by the frame pacer to the input layer.
const WM_FRAME: u32 = WM_APP + 1;
const SAVE_EVERY: Duration = Duration::from_secs(30);
/// Particles released per second while the mouse is held.
const RELEASE_RATE: f32 = 14.0;
/// World radius that fits the screen.
const VIEW_R: f32 = 250.0;
/// Opening animation length.
const INTRO_SECS: f32 = 0.75;

// Interface colours (0xRRGGBB).
const PANEL: u32 = 0x0B0C13;
const EDGE: u32 = 0x3A3F58;
const INK: u32 = 0xE8E8F0;
const DIM: u32 = 0x8B90A8;
const SHADOW: u32 = 0x05050A;

thread_local! {
    /// The input layer while the Solar System is open (it owns the state).
    static OPEN: Cell<HWND> = const { Cell::new(HWND(std::ptr::null_mut())) };
}

// ---------------------------------------------------------------------------
// Pixels (0x00RRGGBB; KEY is see-through).

fn rgb(c: u32) -> (f32, f32, f32) {
    (((c >> 16) & 0xFF) as f32, ((c >> 8) & 0xFF) as f32, (c & 0xFF) as f32)
}

fn pack(r: f32, g: f32, b: f32) -> u32 {
    let q = |v: f32| v.clamp(0.0, 255.0) as u32;
    let c = q(r) << 16 | q(g) << 8 | q(b);
    if c == KEY { c ^ 0x0100 } else { c }
}

fn lerp_c(a: u32, b: u32, t: f32) -> u32 {
    let (ar, ag, ab) = rgb(a);
    let (br, bg, bb) = rgb(b);
    let t = t.clamp(0.0, 1.0);
    pack(ar + (br - ar) * t, ag + (bg - ag) * t, ab + (bb - ab) * t)
}

fn scale_c(c: u32, k: f32) -> u32 {
    let (r, g, b) = rgb(c);
    pack(r * k, g * k, b * k)
}

/// Ordered-dither threshold: fades become patterns of whole pixels, which
/// is all a colour-keyed layer can show (and suits the pixel look).
fn bayer(x: i32, y: i32) -> f32 {
    const M: [u8; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
    (M[((y & 3) * 4 + (x & 3)) as usize] as f32 + 0.5) / 16.0
}

pub struct Canvas<'a> {
    pub px: &'a mut [u32],
    pub w: i32,
    pub h: i32,
}

impl Canvas<'_> {
    fn set(&mut self, x: i32, y: i32, c: u32) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.px[(y * self.w + x) as usize] = if c == KEY { c ^ 0x0100 } else { c };
        }
    }
    /// Sets the pixel with probability `a` (dithered transparency).
    fn dither(&mut self, x: i32, y: i32, c: u32, a: f32) {
        if a > bayer(x, y) {
            self.set(x, y, c);
        }
    }
    fn rect(&mut self, r: Rc, c: u32) {
        for y in r.y..r.y + r.h {
            for x in r.x..r.x + r.w {
                self.set(x, y, c);
            }
        }
    }
    /// A panel: dark fill, one-pixel edge, accent line on top.
    fn panel(&mut self, r: Rc, accent: u32) {
        self.rect(r, PANEL);
        for x in r.x..r.x + r.w {
            self.set(x, r.y, accent);
            self.set(x, r.y + r.h - 1, EDGE);
        }
        for y in r.y..r.y + r.h {
            self.set(r.x, y, EDGE);
            self.set(r.x + r.w - 1, y, EDGE);
        }
    }
    fn text(&mut self, s: &str, x: i32, y: i32, scale: i32, c: u32) {
        pixfont::draw(s, x, y, scale, |px, py| self.set(px, py, c));
    }
    /// Text with a dark outline, readable over anything.
    fn label(&mut self, s: &str, x: i32, y: i32, c: u32) {
        for (ox, oy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            pixfont::draw(s, x + ox, y + oy, 1, |px, py| self.set(px, py, SHADOW));
        }
        self.text(s, x, y, 1, c);
    }
}

/// A rectangle in layer pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rc {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rc {
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

// ---------------------------------------------------------------------------
// Bodies.

fn hash3(x: i32, y: i32, z: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8DA6_B343)
        ^ (y as u32).wrapping_mul(0xD816_3841)
        ^ (z as u32).wrapping_mul(0xCB1A_B31F)
        ^ seed.wrapping_mul(0x9E37_79B1);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5BD1_E995);
    h ^= h >> 15;
    (h & 0xFF_FFFF) as f32 / 16_777_216.0
}

/// A planet or moon as what it is: its particles packed together, in
/// layers from the core out (iron, rock, carbon, water, then a thin
/// hydrogen haze), each grain its own pixel. The grains turn slowly, and
/// the side away from the star is in shadow.
#[allow(clippy::too_many_arguments)]
fn clump(cv: &mut Canvas, cx: f32, cy: f32, r: f32, comp: &[f32; N], seed: u32, t: f32, light: (f32, f32), alpha: f32) {
    let total = solar::mass(comp) + comp[Material::Hydrogen.index()];
    if total <= 0.0 {
        return;
    }
    // Outer radius of each layer, by the room its particles take.
    let mut edges = [0.0f32; N];
    let mut cum = 0.0;
    for (i, m) in LAYERS.iter().enumerate() {
        let n = comp[m.index()] * if *m == Material::Hydrogen { 2.0 } else { 1.0 };
        cum += n;
        edges[i] = r * (cum / total).sqrt();
    }
    let spin = t * (0.18 + (seed % 5) as f32 * 0.05);
    let (sa, ca) = spin.sin_cos();
    let (lx, ly) = light;
    for y in (cy - r - 1.0) as i32..=(cy + r + 1.0) as i32 {
        for x in (cx - r - 1.0) as i32..=(cx + r + 1.0) as i32 {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let d = (dx * dx + dy * dy).sqrt();
            if d > r.max(0.75) {
                continue;
            }
            // Grains turn with the body: hash them in its rotating frame.
            let (gx, gy) = ((dx * ca - dy * sa).round() as i32, (dx * sa + dy * ca).round() as i32);
            let ragged = (hash3(gx, gy, 1, seed) - 0.5) * 1.4;
            let dd = d + ragged;
            let layer = edges.iter().position(|e| dd <= *e).unwrap_or(N - 1);
            let mut li = layer;
            while comp[LAYERS[li].index()] <= 0.0 && li > 0 {
                li -= 1;
            }
            let mat = LAYERS[li];
            let grain = 0.8 + 0.32 * hash3(gx, gy, 2 + li as i32, seed);
            let nz = (1.0 - (d / r).powi(2)).max(0.0).sqrt();
            let lit = ((dx / r) * lx + (dy / r) * ly + nz * 0.55).max(0.0) / 1.15;
            let shade = 0.32 + 0.78 * lit;
            let c = scale_c(mat.rgb(), grain * shade);
            let a = if mat == Material::Hydrogen { 0.7 * alpha } else { alpha };
            cv.dither(x, y, c, a);
        }
    }
}

/// Where space sits on the pixel grid.
#[derive(Clone, Copy)]
pub struct Frame {
    pub cx: f32,
    pub cy: f32,
    /// Grid pixels per world unit.
    pub k: f32,
}

impl Frame {
    pub fn fit(gw: usize, gh: usize, reserve_bottom: f32) -> Self {
        let h = gh as f32 - reserve_bottom;
        Frame { cx: gw as f32 / 2.0, cy: h / 2.0, k: ((gw as f32).min(h) * 0.5 - 6.0).max(10.0) / VIEW_R }
    }
    pub fn to_grid(self, x: f32, y: f32) -> (f32, f32) {
        (self.cx + x * self.k, self.cy + y * self.k)
    }
    fn to_world(self, gx: f32, gy: f32) -> (f32, f32) {
        ((gx - self.cx) / self.k, (gy - self.cy) / self.k)
    }
}

/// Recent positions of each planet (world units, newest last), for trails.
pub type Trails = Vec<(u32, std::collections::VecDeque<(f32, f32)>)>;

/// How many recent positions make a trail.
const TRAIL_LEN: usize = 64;

/// Draws the star, planets, moons, dust and flashes. `open` runs 0 → 1 while
/// the system unfolds out of `from` (grid px).
#[allow(clippy::too_many_arguments)]
pub fn paint_space(cv: &mut Canvas, sim: &Sim, fr: Frame, marks: (Option<u32>, Option<u32>), accent: u32, open: f32, from: (f32, f32), trails: &Trails) {
    let (selected, hover) = marks;
    let t = sim.u.time as f32;
    let e = 1.0 - (1.0 - open).powi(3);
    let fr = Frame { cx: from.0 + (fr.cx - from.0) * e, cy: from.1 + (fr.cy - from.1) * e, k: fr.k * (0.08 + 0.92 * e) };
    let (cx, cy, k) = (fr.cx, fr.cy, fr.k);

    // A short fading trail behind each planet.
    if open >= 1.0 {
        for (seed, pts) in trails {
            let col = if Some(*seed) == selected { accent } else { 0x9096B4 };
            let n = pts.len();
            for (i, w) in pts.iter().zip(pts.iter().skip(1)).enumerate() {
                let a = (i as f32 / n as f32).powf(1.2) * if Some(*seed) == selected { 1.0 } else { 0.7 };
                let (x0, y0) = fr.to_grid(w.0.0, w.0.1);
                let (x1, y1) = fr.to_grid(w.1.0, w.1.1);
                let steps = ((x1 - x0).abs().max((y1 - y0).abs()) as i32).clamp(1, 40);
                for s in 0..steps {
                    let f = s as f32 / steps as f32;
                    cv.dither((x0 + (x1 - x0) * f) as i32, (y0 + (y1 - y0) * f) as i32, col, a);
                }
            }
        }
    }

    // The star: a granulated disc in a dithered corona.
    let sr = (sim.star_radius() * k).max(2.0);
    let glow = sr * 3.2 + 4.0;
    for y in (cy - glow) as i32..=(cy + glow) as i32 {
        for x in (cx - glow) as i32..=(cx + glow) as i32 {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let d = (dx * dx + dy * dy).sqrt() / sr;
            if d < 1.0 {
                let gran = hash3(x, y, (t * 4.0) as i32, 3) * 0.14;
                let c = lerp_c(0xFFF8E6, 0xFFC25A, d * d);
                cv.set(x, y, scale_c(c, 0.92 + gran - 0.22 * d * d));
            } else {
                let a = dy.atan2(dx);
                let flick = 1.0 + 0.12 * (a * 7.0 + t * 1.3).sin() * (a * 3.0 - t * 0.7).cos();
                let g = ((1.0 - (d - 1.0) / (glow / sr - 1.0)).max(0.0)).powf(2.2) * 0.9 * flick;
                cv.dither(x, y, lerp_c(0xE0702A, 0xFFD27A, g), g);
            }
        }
    }

    // Planets and their moons.
    for p in &sim.u.planets {
        let (px, py) = fr.to_grid(p.x, p.y);
        let r = solar::radius(&p.comp) * k;
        let dl = (p.x * p.x + p.y * p.y).sqrt().max(1.0);
        let light = (-p.x / dl, -p.y / dl);
        clump(cv, px, py, r, &p.comp, p.seed, t, light, open.max(0.15));
        let ring = |cv: &mut Canvas, x: f32, y: f32, r: f32, seed: u32| {
            let mark = if Some(seed) == selected { Some(accent) } else if Some(seed) == hover { Some(0xD8DCEE) } else { None };
            if let Some(c) = mark {
                let rr = r + 3.0;
                let n = (std::f32::consts::TAU * rr) as i32;
                for i in (0..n).step_by(2) {
                    let a = i as f32 / rr + t * 0.6;
                    cv.set((x + a.cos() * rr) as i32, (y + a.sin() * rr) as i32, c);
                }
            }
        };
        for m in &p.moons {
            let (mx, my) = (px + m.angle.cos() * m.dist * k, py + m.angle.sin() * m.dist * k);
            let mr = solar::radius(&m.comp) * k;
            clump(cv, mx, my, mr, &m.comp, m.seed, t, light, open.max(0.15));
            ring(cv, mx, my, mr, m.seed);
        }
        ring(cv, px, py, r, p.seed);
    }

    // Loose particles, with a short dithered trail.
    for d in &sim.dust {
        let (x, y) = fr.to_grid(d.x, d.y);
        let c = d.mat.rgb();
        cv.dither((x - d.vx * k * 0.03) as i32, (y - d.vy * k * 0.03) as i32, scale_c(c, 0.7), 0.5);
        cv.set(x as i32, y as i32, c);
    }

    // Flashes: expanding rings that thin out.
    for f in &sim.flashes {
        let (x, y) = fr.to_grid(f.x, f.y);
        let rr = f.size * k * (0.6 + f.age * 2.2) + 1.0;
        let a = (1.0 - f.age / 1.2).max(0.0);
        let n = (std::f32::consts::TAU * rr) as i32 + 4;
        for i in 0..n {
            let ang = i as f32 / rr;
            cv.dither((x + ang.cos() * rr) as i32, (y + ang.sin() * rr) as i32, f.rgb, a);
        }
    }
}

// ---------------------------------------------------------------------------
// The state and its pixel interface.

#[derive(Clone, Copy, PartialEq, Debug)]
enum Ctl {
    Mat(Material),
    Rename,
    Typing,
    Close,
}

/// A mouse press in space: on a planet (maybe becoming a drag) or releasing.
struct Press {
    planet: Option<u32>,
    at: (i32, i32),
    dragging: bool,
}

struct Desk {
    catcher: HWND,
    launcher: HWND,
    ov: Overlay,
    accent: u32,
    sim: Sim,
    pacer: Option<Pacer>,
    last_frame: Instant,
    saved: Instant,
    /// Material released by holding the mouse.
    mat: Material,
    /// Cloud being fed while the mouse is held, and particles owed to it.
    releasing: Option<u32>,
    owed: f32,
    press: Option<Press>,
    /// Cursor in layer pixels.
    cursor: (i32, i32),
    selected: Option<u32>,
    hover: Option<u32>,
    hot: Option<Ctl>,
    renaming: Option<String>,
    note: Option<(String, Instant)>,
    zones: Vec<(Rc, Ctl)>,
    /// Opening animation: when, and where it unfolds from (layer px).
    intro: (Instant, (f32, f32)),
    trails: Trails,
}

/// A selected or hovered body: a planet, or a moon and the planet it circles.
enum Body<'a> {
    Planet(&'a solar::Planet),
    Moon(&'a solar::Planet, &'a solar::Moon),
}

impl Body<'_> {
    fn name(&self) -> String {
        match self {
            Body::Planet(p) => p.name.clone(),
            Body::Moon(_, m) => m.display_name(),
        }
    }
    fn comp(&self) -> &[f32; N] {
        match self {
            Body::Planet(p) => &p.comp,
            Body::Moon(_, m) => &m.comp,
        }
    }
    /// World position.
    fn pos(&self) -> (f32, f32) {
        match self {
            Body::Planet(p) => (p.x, p.y),
            Body::Moon(p, m) => (p.x + m.angle.cos() * m.dist, p.y + m.angle.sin() * m.dist),
        }
    }
}

/// Toolbar height in layer pixels (and the info panel above it).
const BAR_H: i32 = 31;
const INFO_H: i32 = 32;

impl Desk {
    fn frame_geom(&self) -> Frame {
        Frame::fit(self.ov.gw, self.ov.gh, (BAR_H + 8) as f32)
    }

    fn to_layer(&self, sx: i32, sy: i32) -> (i32, i32) {
        ((sx - self.ov.left).div_euclid(self.ov.cell), (sy - self.ov.top).div_euclid(self.ov.cell))
    }

    fn layer_to_world(&self, gx: i32, gy: i32) -> (f32, f32) {
        self.frame_geom().to_world(gx as f32 + 0.5, gy as f32 + 0.5)
    }

    /// The planet or moon under a layer pixel: (seed, is a moon).
    fn body_at(&self, gx: i32, gy: i32) -> Option<(u32, bool)> {
        let (wx, wy) = self.layer_to_world(gx, gy);
        let slack = 3.0 / self.frame_geom().k;
        if let Some(i) = self.sim.planet_at(wx, wy, slack) {
            return Some((self.sim.u.planets[i].seed, false));
        }
        self.sim.moon_at(wx, wy, slack).map(|(p, m)| (self.sim.u.planets[p].moons[m].seed, true))
    }

    fn body(&self, seed: u32) -> Option<Body<'_>> {
        for p in &self.sim.u.planets {
            if p.seed == seed {
                return Some(Body::Planet(p));
            }
            if let Some(m) = p.moons.iter().find(|m| m.seed == seed) {
                return Some(Body::Moon(p, m));
            }
        }
        None
    }

    fn selected_body(&self) -> Option<Body<'_>> {
        self.body(self.selected?)
    }

    /// Records where each planet is, for the trails.
    fn sample_trails(&mut self) {
        self.trails.retain(|(s, _)| self.sim.u.planets.iter().any(|p| p.seed == *s));
        for p in &self.sim.u.planets {
            let pts = match self.trails.iter_mut().find(|(s, _)| *s == p.seed) {
                Some((_, pts)) => pts,
                None => {
                    self.trails.push((p.seed, Default::default()));
                    &mut self.trails.last_mut().unwrap().1
                }
            };
            if pts.len() == TRAIL_LEN {
                pts.pop_front();
            }
            pts.push_back((p.x, p.y));
        }
    }

    fn bar_rect(&self) -> Rc {
        let gw = self.ov.gw as i32;
        let w = (gw - 16).min(420);
        Rc { x: (gw - w) / 2, y: self.ov.gh as i32 - BAR_H - 6, w, h: BAR_H }
    }

    fn note(&mut self, msg: impl Into<String>) {
        self.note = Some((msg.into(), Instant::now()));
    }

    fn save(&mut self) {
        self.sim.snapshot().save();
        self.saved = Instant::now();
    }

    fn busy(&self) -> bool {
        self.press.is_some() || self.intro.0.elapsed().as_secs_f32() < INTRO_SECS || !self.sim.dust.is_empty()
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let dt = now - self.last_frame;
        let min = Duration::from_millis(if self.busy() { 15 } else { 32 });
        if dt < min {
            return;
        }
        self.last_frame = now;
        let dt = dt.as_secs_f32().min(0.1);
        let (wx, wy) = self.layer_to_world(self.cursor.0, self.cursor.1);
        if let Some(cloud) = self.releasing {
            self.owed += dt * RELEASE_RATE;
            while self.owed >= 1.0 {
                self.owed -= 1.0;
                if !self.sim.release(self.mat, wx, wy, cloud) {
                    self.releasing = None;
                    self.note(format!("That's all your {}", self.mat.name().to_lowercase()));
                    break;
                }
            }
        }
        self.sim.mouse = Some((wx, wy));
        self.sim.step(dt);
        self.sample_trails();
        if let Some(n) = self.sim.take_notes().pop() {
            self.note(n);
        }
        if self.selected.is_some() && self.selected_body().is_none() {
            self.selected = None;
            self.renaming = None;
        }
        self.draw();
        if self.saved.elapsed() > SAVE_EVERY {
            self.save();
        }
    }

    fn draw(&mut self) {
        let fr = self.frame_geom();
        let (gw, gh) = (self.ov.gw as i32, self.ov.gh as i32);
        let open = (self.intro.0.elapsed().as_secs_f32() / INTRO_SECS).min(1.0);
        let accent = self.accent;
        let marks = (self.selected, self.hover);
        let from = self.intro.1;
        let mut zones = Vec::new();
        {
            // The DIB belongs to the overlay and nothing else touches it while
            // drawing, so it can be borrowed apart from `self`.
            let (ptr, len) = {
                let g = self.ov.grid();
                (g.as_mut_ptr(), g.len())
            };
            let px = unsafe { std::slice::from_raw_parts_mut(ptr, len) };
            px.fill(KEY);
            let mut cv = Canvas { px, w: gw, h: gh };
            paint_space(&mut cv, &self.sim, fr, marks, accent, open, from, &self.trails);
            if open > 0.5 {
                self.paint_ui(&mut cv, &mut zones, ((1.0 - open) * 2.0 * 40.0) as i32);
            }
        }
        self.zones = zones;
        self.ov.present();
    }

    /// The pixel interface: hover label, planet info, toolbar.
    fn paint_ui(&self, cv: &mut Canvas, zones: &mut Vec<(Rc, Ctl)>, drop: i32) {
        let accent = self.accent;
        let fr = self.frame_geom();
        let k = fr.k;

        // Name above the planet or moon under the cursor.
        if let Some(b) = self.hover.filter(|h| Some(*h) != self.selected).and_then(|s| self.body(s)) {
            let (wx, wy) = b.pos();
            let (x, y) = fr.to_grid(wx, wy);
            let r = solar::radius(b.comp()) * k;
            let name = b.name().to_uppercase();
            cv.label(&name, x as i32 - pixfont::width(&name, 1) / 2, (y - r) as i32 - 13, INK);
        }

        let bar = Rc { y: self.bar_rect().y + drop, ..self.bar_rect() };

        // Selected planet or moon.
        if let Some(b) = self.selected_body() {
            let comp = *b.comp();
            let r = Rc { x: bar.x, y: bar.y - INFO_H - 3, w: bar.w, h: INFO_H };
            cv.panel(r, accent);
            let name_rc = Rc { x: r.x + 6, y: r.y + 5, w: r.w / 2 - 6, h: 14 };
            match &self.renaming {
                Some(buf) => {
                    let s = format!("{}_", buf.to_uppercase());
                    cv.text(&pixfont::fit(&s, name_rc.w, 2), name_rc.x, name_rc.y, 2, accent);
                }
                None => {
                    cv.text(&pixfont::fit(&b.name().to_uppercase(), name_rc.w, 2), name_rc.x, name_rc.y, 2, INK);
                    zones.push((name_rc, Ctl::Rename));
                }
            }
            let m = solar::mass(&comp);
            let line = match &b {
                Body::Planet(p) => {
                    let d = (p.x * p.x + p.y * p.y).sqrt().max(1.0);
                    let year = std::f32::consts::TAU / ((solar::GM * d / (d * d + 25.0)).sqrt() / d);
                    let moons = match p.moons.len() {
                        0 => String::new(),
                        1 => " · 1 MOON".into(),
                        n => format!(" · {n} MOONS"),
                    };
                    format!("{} · {m:.0} PARTICLES{moons} · YEAR {year:.0}S", solar::kind(&comp).to_uppercase())
                }
                Body::Moon(p, _) => format!("MOON OF {} · {m:.0} PARTICLES", p.name.to_uppercase()),
            };
            let p = (comp,);
            cv.text(&pixfont::fit(&line, r.w / 2 - 6, 1), r.x + 6, r.y + 22, 1, DIM);
            // Make-up: a stacked bar and the shares.
            let bx = r.x + r.w / 2 + 6;
            let bw = r.w / 2 - 12;
            let f = solar::fractions(&p.0);
            let mut x = bx;
            for mat in LAYERS {
                let seg = (bw as f32 * f[mat.index()]).round() as i32;
                cv.rect(Rc { x, y: r.y + 6, w: seg.min(bx + bw - x), h: 5 }, mat.rgb());
                x += seg;
            }
            let mut lx = bx;
            for mat in LAYERS {
                if p.0[mat.index()] <= 0.0 {
                    continue;
                }
                let s = format!("{:.0}%", f[mat.index()] * 100.0);
                let w = 5 + 2 + pixfont::width(&s, 1);
                if lx + w > bx + bw {
                    break;
                }
                cv.rect(Rc { x: lx, y: r.y + 16, w: 4, h: 4 }, mat.rgb());
                cv.text(&s, lx + 6, r.y + 15, 1, DIM);
                lx += w + 6;
            }
        }

        // Toolbar: materials, then a status line.
        cv.panel(bar, accent);
        let cy = bar.y + 10;
        let mut x = bar.x + 4;
        for mat in Material::ALL {
            let count = self.sim.u.inventory[mat.index()];
            let name = mat.name().to_uppercase();
            let cnt = count.to_string();
            let w = 3 + 7 + 4 + pixfont::width(&name, 1) + 4 + pixfont::width(&cnt, 1) + 4;
            let chip = Rc { x, y: bar.y + 3, w, h: 15 };
            let on = mat == self.mat;
            if on {
                cv.rect(chip, 0x1E2131);
                cv.rect(Rc { y: chip.y + chip.h - 1, h: 1, ..chip }, accent);
            } else if self.hot == Some(Ctl::Mat(mat)) {
                cv.rect(chip, 0x151724);
            }
            cv.rect(Rc { x: x + 3, y: cy - 3, w: 7, h: 7 }, mat.rgb());
            let ink = if count == 0 { DIM } else { INK };
            cv.text(&name, x + 14, cy - 3, 1, ink);
            cv.text(&cnt, x + 14 + pixfont::width(&name, 1) + 4, cy - 3, 1, if on { accent } else { DIM });
            zones.push((chip, Ctl::Mat(mat)));
            x += w + 2;
        }
        // Close: a pixel X.
        let close = Rc { x: bar.x + bar.w - 14, y: bar.y + 4, w: 11, h: 11 };
        let xc = if self.hot == Some(Ctl::Close) { INK } else { DIM };
        for i in 0..7 {
            cv.set(close.x + 2 + i, close.y + 2 + i, xc);
            cv.set(close.x + 8 - i, close.y + 2 + i, xc);
        }
        zones.push((close, Ctl::Close));

        // Status line: news, else what to do; harvest on the right.
        let wait = self.sim.u.harvest_wait(crate::util::now_unix());
        let (harvest, hc) = if wait == 0 {
            ("HARVEST READY: DO A TYPING TEST".to_string(), accent)
        } else {
            (format!("NEXT HARVEST {}", solar::wait_short(wait)), DIM)
        };
        let hw = pixfont::width(&harvest, 1);
        let hx = bar.x + bar.w - 5 - hw;
        let ly = bar.y + 21;
        cv.text(&harvest, hx, ly, 1, hc);
        if wait == 0 {
            zones.push((Rc { x: hx, y: ly - 2, w: hw, h: 11 }, Ctl::Typing));
        }
        let fresh = self.note.as_ref().filter(|(_, at)| at.elapsed() < Duration::from_secs(5)).map(|(n, _)| n.to_uppercase());
        let (msg, mc) = match fresh {
            Some(n) => (n, INK),
            None if self.renaming.is_some() => ("TYPE A NAME · ENTER TO SAVE · ESC TO CANCEL".into(), DIM),
            None if self.selected.is_some() => ("DRAG A PLANET TO THROW IT · F2 TO RENAME · RIGHT-CLICK TO DESELECT".into(), DIM),
            None if self.sim.u.inventory[self.mat.index()] == 0 => (format!("NO {} LEFT: IT COMES FROM {}", self.mat.name().to_uppercase(), self.mat.source().to_uppercase()), DIM),
            None => (format!("HOLD TO RELEASE {} · DRAG PLANETS · ESC CLOSES", self.mat.name().to_uppercase()), DIM),
        };
        cv.text(&pixfont::fit(&msg, hx - bar.x - 14, 1), bar.x + 5, ly, 1, mc);
    }

    fn zone_at(&self, gx: i32, gy: i32) -> Option<Ctl> {
        self.zones.iter().rev().find(|(r, _)| r.contains(gx, gy)).map(|(_, c)| *c)
    }

    // ----- input ----------------------------------------------------------------

    fn on_down(&mut self, gx: i32, gy: i32) {
        self.cursor = (gx, gy);
        if let Some(c) = self.zone_at(gx, gy) {
            match c {
                Ctl::Mat(m) => self.mat = m,
                Ctl::Rename => self.start_rename(),
                Ctl::Typing => {
                    self.close();
                    unsafe {
                        let _ = PostMessageW(Some(self.launcher), WM_APP_OPEN_TYPING, WPARAM(0), LPARAM(0));
                    }
                }
                Ctl::Close => self.close(),
            }
            return;
        }
        if self.renaming.is_some() {
            self.finish_rename(true);
        }
        let hit = self.body_at(gx, gy);
        let planet = hit.filter(|(_, moon)| !moon).map(|(s, _)| s);
        if let Some((seed, true)) = hit {
            // Moons are selected, not dragged.
            self.selected = if self.selected == Some(seed) { None } else { Some(seed) };
            self.renaming = None;
            return;
        }
        if planet.is_none() {
            if self.sim.u.inventory[self.mat.index()] == 0 {
                self.note(format!("No {} left: it comes from {}", self.mat.name().to_lowercase(), self.mat.source()));
            } else {
                let (wx, wy) = self.layer_to_world(gx, gy);
                self.releasing = Some(self.sim.new_cloud(wx, wy));
                self.owed = 1.0;
            }
        }
        self.press = Some(Press { planet, at: (gx, gy), dragging: false });
        unsafe {
            SetCapture(self.catcher);
        }
    }

    fn on_move(&mut self, gx: i32, gy: i32) {
        self.cursor = (gx, gy);
        self.hot = self.zone_at(gx, gy);
        self.hover = if self.hot.is_some() { None } else { self.body_at(gx, gy).map(|(s, _)| s) };
        let (wx, wy) = self.layer_to_world(gx, gy);
        if let Some(p) = &mut self.press {
            if let Some(seed) = p.planet {
                if !p.dragging && ((gx - p.at.0).pow(2) + (gy - p.at.1).pow(2)) > 9 {
                    p.dragging = true;
                    self.selected = Some(seed);
                    self.renaming = None;
                }
                if p.dragging {
                    self.sim.grab(seed, wx, wy);
                }
            }
        }
    }

    fn on_up(&mut self) {
        self.releasing = None;
        if let Some(p) = self.press.take() {
            match (p.planet, p.dragging) {
                (Some(seed), false) => {
                    self.selected = if self.selected == Some(seed) { None } else { Some(seed) };
                    self.renaming = None;
                }
                (Some(_), true) => self.sim.let_go(),
                _ => {}
            }
        }
        unsafe {
            let _ = ReleaseCapture();
        }
    }

    fn start_rename(&mut self) {
        if let Some(b) = self.selected_body() {
            self.renaming = Some(b.name());
        }
    }

    fn finish_rename(&mut self, keep: bool) {
        let Some(name) = self.renaming.take() else { return };
        let name = name.trim().to_string();
        if keep && !name.is_empty() {
            if let Some(s) = self.selected {
                for p in &mut self.sim.u.planets {
                    if p.seed == s {
                        p.name = name.clone();
                    }
                    for m in &mut p.moons {
                        if m.seed == s {
                            m.name = name.clone();
                        }
                    }
                }
                self.save();
            }
        }
    }

    fn on_key(&mut self, vk: u16) {
        if self.renaming.is_some() {
            match vk {
                v if v == VK_RETURN.0 => self.finish_rename(true),
                v if v == VK_ESCAPE.0 => self.finish_rename(false),
                v if v == VK_BACK.0 => {
                    if let Some(b) = &mut self.renaming {
                        b.pop();
                    }
                }
                _ => {}
            }
            return;
        }
        match vk {
            v if v == VK_ESCAPE.0 => self.close(),
            v if v == VK_F2.0 => self.start_rename(),
            v @ 0x31..=0x35 => self.mat = Material::ALL[(v - 0x31) as usize],
            _ => {}
        }
    }

    fn on_char(&mut self, c: u32) {
        if let Some(buf) = &mut self.renaming {
            if let Some(c) = char::from_u32(c).filter(|c| !c.is_control()) {
                if buf.chars().count() < 20 {
                    buf.push(c);
                }
            }
        }
    }

    /// Puts it all away (saved) at once.
    fn close(&self) {
        unsafe {
            let _ = PostMessageW(Some(self.catcher), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

impl Drop for Desk {
    fn drop(&mut self) {
        self.pacer = None;
    }
}

pub fn wait_label(secs: u64) -> String {
    match secs {
        0..60 => "under a minute".into(),
        60..3600 => format!("{} min", secs.div_ceil(60)),
        _ => format!("{} h {} min", secs / 3600, secs % 3600 / 60),
    }
}

// ---------------------------------------------------------------------------
// Opening, and the API the launcher uses.

const CATCHER_CLASS: PCWSTR = w!("SIGMA5.SolarSystem");
const BADGE_CLASS: PCWSTR = w!("SIGMA5.SolarBadge");

fn register_classes() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let catcher = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(catcher_proc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
            hbrBackground: CreateSolidBrush(COLORREF(0)),
            lpszClassName: CATCHER_CLASS,
            ..Default::default()
        };
        RegisterClassExW(&catcher);
        let badge = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(badge_proc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_HAND).unwrap_or_default(),
            hbrBackground: CreateSolidBrush(COLORREF(KEY)),
            lpszClassName: BADGE_CLASS,
            ..Default::default()
        };
        RegisterClassExW(&badge);
    });
}

/// Opens the Solar System on the monitor under the cursor, unfolding from
/// `from` (screen px; the middle of the screen if None). If it's already
/// open, brings it forward.
pub fn open(launcher: HWND, from: Option<POINT>, accent: COLORREF) {
    if let Some(c) = with_open(|d| d.catcher) {
        unsafe {
            let _ = SetForegroundWindow(c);
        }
        return;
    }
    unsafe {
        register_classes();
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(from.unwrap_or(pt), MONITOR_DEFAULTTONEAREST);
        let (mut dpi, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dpi, &mut dy);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let work = mi.rcWork;
        let cell = ((2.0 * dpi as f32 / 96.0).round() as i32).max(2);
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        // The input layer: alpha 1, so it's invisible but takes every click.
        let Ok(catcher) = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            CATCHER_CLASS,
            w!("SIGMA5 Solar System"),
            WS_POPUP,
            work.left,
            work.top,
            work.right - work.left,
            work.bottom - work.top - 1,
            None,
            None,
            Some(hinst.into()),
            None,
        ) else {
            return;
        };
        let _ = SetLayeredWindowAttributes(catcher, COLORREF(0), 1, LWA_ALPHA);
        let Some(ov) = Overlay::new(None, work, cell) else {
            let _ = DestroyWindow(catcher);
            return;
        };
        let a = accent.0;
        let mut desk = Box::new(Desk {
            catcher,
            launcher,
            accent: (a & 0xFF) << 16 | (a & 0xFF00) | ((a >> 16) & 0xFF),
            sim: Sim::new(Universe::load()),
            pacer: Some(Pacer::start(catcher, WM_FRAME, mon)),
            last_frame: Instant::now(),
            saved: Instant::now(),
            mat: Material::Rock,
            releasing: None,
            owed: 0.0,
            press: None,
            cursor: (-100, -100),
            selected: None,
            hover: None,
            hot: None,
            renaming: None,
            note: None,
            zones: Vec::new(),
            intro: (Instant::now(), (0.0, 0.0)),
            trails: Vec::new(),
            ov,
        });
        desk.mat = Material::ALL.iter().copied().find(|m| desk.sim.u.inventory[m.index()] > 0).unwrap_or(Material::Rock);
        let fr = desk.frame_geom();
        desk.intro.1 = from.map_or((fr.cx, fr.cy), |p| {
            let (x, y) = desk.to_layer(p.x, p.y);
            (x as f32, y as f32)
        });
        SetWindowLongPtrW(catcher, GWLP_USERDATA, Box::into_raw(desk) as isize);
        OPEN.with(|c| c.set(catcher));
        let _ = ShowWindow(catcher, SW_SHOW);
        let _ = SetForegroundWindow(catcher);
    }
}

fn with_open<R>(f: impl FnOnce(&mut Desk) -> R) -> Option<R> {
    let hwnd = OPEN.with(|c| c.get());
    if hwnd.is_invalid() {
        return None;
    }
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Desk;
    if ptr.is_null() {
        return None;
    }
    Some(f(unsafe { &mut *ptr }))
}

/// The toolbar's screen rectangle while the Solar System is open.
pub fn toolbar_rect() -> Option<RECT> {
    with_open(|d| {
        let b = d.bar_rect();
        let c = d.ov.cell;
        RECT { left: d.ov.left + b.x * c, top: d.ov.top + b.y * c, right: d.ov.left + (b.x + b.w) * c, bottom: d.ov.top + (b.y + b.h) * c }
    })
}

/// Harvests a finished typing test's correct words, if one is due.
/// Returns the particles gained, or the seconds left to wait.
pub fn harvest(words: &[&str]) -> Result<[u32; N], u64> {
    let now = crate::util::now_unix();
    if let Some(r) = with_open(|d| {
        let r = d.sim.u.harvest(words.iter().copied(), now);
        if let Ok(got) = r {
            let n: u32 = got.iter().sum();
            if n > 0 {
                d.note(format!("+{n} particles from your typing test"));
                d.save();
            }
        }
        r
    }) {
        return r;
    }
    let mut u = Universe::load();
    let r = u.harvest(words.iter().copied(), now);
    if r.is_ok() {
        u.save();
    }
    r
}

/// Seconds until the next typing test is harvested (0 = the next one counts).
pub fn harvest_wait() -> u64 {
    let now = crate::util::now_unix();
    with_open(|d| d.sim.u.harvest_wait(now)).unwrap_or_else(|| Universe::load().harvest_wait(now))
}

fn point(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32)
}

unsafe extern "system" fn catcher_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Desk;
    if ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
    }
    if msg == WM_DESTROY {
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            let mut d = Box::from_raw(ptr);
            d.save();
        }
        OPEN.with(|c| c.set(HWND::default()));
        return LRESULT(0);
    }
    let d = unsafe { &mut *ptr };
    let layer = |lp: LPARAM, d: &Desk| {
        let (x, y) = point(lp);
        let mut pt = POINT { x, y };
        unsafe {
            let _ = ClientToScreen(hwnd, &mut pt);
        }
        d.to_layer(pt.x, pt.y)
    };
    match msg {
        WM_FRAME => {
            d.frame();
            if let Some(p) = &d.pacer {
                p.frame_done();
            }
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let (gx, gy) = layer(lp, d);
            d.on_down(gx, gy);
        }
        WM_MOUSEMOVE => {
            let (gx, gy) = layer(lp, d);
            d.on_move(gx, gy);
        }
        WM_LBUTTONUP => d.on_up(),
        WM_CAPTURECHANGED => {
            d.releasing = None;
            if d.press.as_ref().is_some_and(|p| p.dragging) {
                d.sim.let_go();
            }
            d.press = None;
        }
        WM_RBUTTONDOWN => {
            d.selected = None;
            d.renaming = None;
        }
        WM_KEYDOWN => d.on_key(wp.0 as u16),
        WM_CHAR => d.on_char(wp.0 as u32),
        WM_ACTIVATE => {
            // Switching to another app puts it away (but not to SIGMA5's launcher).
            let other = HWND(lp.0 as *mut _);
            if wp.0 & 0xFFFF == WA_INACTIVE as usize && other != d.launcher && !other.is_invalid() {
                d.close();
            }
            return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
        }
        WM_CLOSE => unsafe {
            let _ = DestroyWindow(hwnd);
        },
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
    LRESULT(0)
}

// ---------------------------------------------------------------------------
// The launcher's star: a tiny pixel star circling the launcher window. A
// ring round it fills as the next harvest comes due; clicking it opens the
// Solar System.

pub struct StarBadge {
    hwnd: HWND,
    n: i32,
    cell: i32,
    /// The sprite in pixels (KEY = empty).
    sprite: Vec<u32>,
    /// The window's image at screen size: premultiplied BGRA.
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
}

/// Sprite size in pixels. Empty pixels are nearly (not fully) transparent,
/// so the whole square takes the click, not just the star.
const BADGE_N: i32 = 21;

impl StarBadge {
    pub fn new(owner: HWND, cell: i32) -> Option<Self> {
        register_classes();
        let n = BADGE_N;
        let size = n * cell;
        unsafe {
            let hinst = GetModuleHandleW(None).ok()?;
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                BADGE_CLASS,
                PCWSTR::null(),
                WS_POPUP,
                -10_000,
                -10_000,
                size,
                size,
                Some(owner),
                None,
                Some(hinst.into()),
                None,
            )
            .ok()?;
            let dc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: size,
                    biHeight: -size,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let Ok(bmp) = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
                let _ = DeleteDC(dc);
                let _ = DestroyWindow(hwnd);
                return None;
            };
            let old = SelectObject(dc, bmp.into());
            Some(Self { hwnd, n, cell, sprite: vec![KEY; (n * n) as usize], dc, bmp, old, bits: bits as *mut u32 })
        }
    }

    /// Size on screen (physical px).
    pub fn size(&self) -> i32 {
        self.n * self.cell
    }

    /// Draws the star at `t` seconds with the harvest ring (`progress` 0..1,
    /// full when a harvest is ready) and moves it to centre on (x, y).
    pub fn update(&mut self, x: i32, y: i32, t: f32, progress: f32, accent: COLORREF) {
        let a = accent.0;
        let accent = (a & 0xFF) << 16 | (a & 0xFF00) | ((a >> 16) & 0xFF);
        let (n, cell) = (self.n, self.cell);
        self.sprite.fill(KEY);
        paint_badge(&mut Canvas { px: &mut self.sprite, w: n, h: n }, t, progress, accent);
        let size = self.size();
        unsafe {
            let _ = GdiFlush();
            let out = std::slice::from_raw_parts_mut(self.bits, (size * size) as usize);
            for (i, o) in out.iter_mut().enumerate() {
                let (px, py) = (i as i32 % size / cell, i as i32 / size / cell);
                let c = self.sprite[(py * n + px) as usize];
                // Alpha 1 where empty: invisible, but still clickable.
                *o = if c == KEY { 0x0100_0000 } else { 0xFF00_0000 | c };
            }
            let pos = POINT { x: x - size / 2, y: y - size / 2 };
            let sz = windows::Win32::Foundation::SIZE { cx: size, cy: size };
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            let _ = UpdateLayeredWindow(self.hwnd, None, Some(&pos), Some(&sz), Some(self.dc), Some(&POINT::default()), COLORREF(0), Some(&blend), ULW_ALPHA);
            if !IsWindowVisible(self.hwnd).as_bool() {
                let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
            }
        }
    }
}

impl Drop for StarBadge {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// The badge sprite: a twinkling star in a dithered glow, and a ring that
/// fills clockwise towards the next harvest (solid accent when it's ready).
pub fn paint_badge(cv: &mut Canvas, t: f32, progress: f32, accent: u32) {
    let c = (cv.w as f32) / 2.0;
    let ready = progress >= 1.0;
    // Ring.
    let rr = c - 1.5;
    let steps = (std::f32::consts::TAU * rr) as i32 + 6;
    for i in 0..steps {
        let f = i as f32 / steps as f32;
        let a = f * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let (x, y) = ((c + a.cos() * rr) as i32, (c + a.sin() * rr) as i32);
        if ready {
            // A bright spot runs round the ring.
            let run = ((f - t * 0.35).rem_euclid(1.0) * 6.0).min(1.0);
            cv.set(x, y, lerp_c(0xFFFFFF, accent, run));
        } else if f <= progress {
            cv.set(x, y, scale_c(accent, 0.75));
        } else if i % 2 == 0 {
            cv.set(x, y, 0x50556C);
        }
    }
    // Star.
    let tw = 1.0 + 0.25 * (t * 2.3).sin();
    for y in 0..cv.h {
        for x in 0..cv.w {
            let (dx, dy) = (x as f32 + 0.5 - c, y as f32 + 0.5 - c);
            let d = (dx * dx + dy * dy).sqrt();
            if d < 2.2 {
                cv.set(x, y, lerp_c(0xFFFBEA, 0xFFC860, d / 2.2));
            } else if d < 5.5 * tw {
                let g = (1.0 - (d - 2.2) / (5.5 * tw - 2.2)).powf(1.8);
                // Twinkle spikes along the axes.
                let spike = if dx.abs() < 0.6 || dy.abs() < 0.6 { 0.35 } else { 0.0 };
                cv.dither(x, y, lerp_c(0xE07A30, 0xFFD27A, g), g * 0.85 + spike * g.sqrt());
            }
        }
    }
}

unsafe extern "system" fn badge_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_LBUTTONDOWN => unsafe {
            let mut rc = RECT::default();
            let _ = GetWindowRect(hwnd, &mut rc);
            let (x, y) = ((rc.left + rc.right) / 2, (rc.top + rc.bottom) / 2);
            let lp = ((y as i16 as u16 as isize) << 16) | x as i16 as u16 as isize;
            if let Ok(owner) = GetWindow(hwnd, GW_OWNER) {
                let _ = PostMessageW(Some(owner), WM_APP_OPEN_SOLAR, WPARAM(0), LPARAM(lp));
            }
            LRESULT(0)
        },
        WM_PAINT => unsafe {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(hwnd, &mut ps);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        },
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Where the badge sits on its slow lap round a window (screen px): a
/// rounded-rectangle path just outside it, one lap a minute.
pub fn badge_path(win: RECT, margin: i32, t: f32) -> (i32, i32) {
    let (cx, cy) = ((win.left + win.right) as f32 / 2.0, (win.top + win.bottom) as f32 / 2.0);
    let (a, b) = ((win.right - win.left) as f32 / 2.0 + margin as f32, (win.bottom - win.top) as f32 / 2.0 + margin as f32);
    let th = t / 60.0 * std::f32::consts::TAU;
    let squash = |v: f32| v.signum() * v.abs().powf(0.2);
    ((cx + a * squash(th.cos())) as i32, (cy + b * squash(th.sin())) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solar::{Planet, orbit_velocity};

    fn planet(seed: u32, r: f32, a: f32, comp: [f32; N]) -> Planet {
        let (x, y) = (r * a.cos(), r * a.sin());
        let (vx, vy) = orbit_velocity(x, y);
        Planet { name: solar::planet_name(seed), x, y, vx, vy, comp, seed, moons: Vec::new() }
    }

    #[test]
    fn badge_stays_outside_the_window() {
        let win = RECT { left: 100, top: 100, right: 850, bottom: 574 };
        for i in 0..240 {
            let (x, y) = badge_path(win, 26, i as f32 * 0.25);
            let inside = x > win.left && x < win.right && y > win.top && y < win.bottom;
            assert!(!inside, "({x}, {y}) at step {i}");
        }
    }

    /// The launcher's star on a private desktop: it shows where it's put and
    /// a click asks its owner to open the Solar System.
    /// `cargo test star_badge_live -- --ignored`
    #[test]
    #[ignore]
    fn star_badge_live() {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn CreateDesktopW(name: PCWSTR, dev: PCWSTR, mode: *const core::ffi::c_void, flags: u32, access: u32, sa: *const core::ffi::c_void) -> isize;
            fn SetThreadDesktop(desk: isize) -> i32;
            fn CloseDesktop(desk: isize) -> i32;
        }
        std::thread::spawn(|| unsafe {
            let desk = CreateDesktopW(w!("SIGMA5.BadgeTest"), PCWSTR::null(), std::ptr::null(), 0, 0x1000_0000, std::ptr::null());
            assert!(desk != 0 && SetThreadDesktop(desk) != 0);
            let owner = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_POPUP, 100, 100, 750, 474, None, None, None, None).unwrap();
            let mut b = StarBadge::new(owner, 3).expect("badge");
            let win = RECT { left: 100, top: 100, right: 850, bottom: 574 };
            let (x, y) = badge_path(win, 10 + b.size() / 2, 12.0);
            b.update(x, y, 12.0, 0.4, COLORREF(0x00FF72AB));
            let mut rc = RECT::default();
            let _ = GetWindowRect(b.hwnd, &mut rc);
            assert!(IsWindowVisible(b.hwnd).as_bool());
            assert_eq!(((rc.left + rc.right) / 2, (rc.top + rc.bottom) / 2), (x, y));
            SendMessageW(b.hwnd, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(LPARAM(0)));
            let mut msg = MSG::default();
            let got = PeekMessageW(&mut msg, Some(owner), WM_APP_OPEN_SOLAR, WM_APP_OPEN_SOLAR, PM_REMOVE).as_bool();
            assert!(got, "the click reaches the launcher");
            let (px, py) = ((msg.lParam.0 & 0xFFFF) as i16 as i32, ((msg.lParam.0 >> 16) & 0xFFFF) as i16 as i32);
            assert_eq!((px, py), (x, y), "with where the star is");
            drop(b);
            let _ = DestroyWindow(owner);
            CloseDesktop(desk);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn clumps_are_about_one_pixel_per_particle() {
        let comp = [0.0, 200.0, 0.0, 0.0, 0.0];
        let mut px = vec![KEY; 60 * 60];
        let mut cv = Canvas { px: &mut px, w: 60, h: 60 };
        clump(&mut cv, 30.0, 30.0, solar::radius(&comp), &comp, 1, 0.0, (1.0, 0.0), 1.0);
        let lit = px.iter().filter(|&&c| c != KEY).count();
        assert!((200..330).contains(&lit), "{lit} pixels for 200 particles");
    }

    /// Renders the space layer and pixel interface over a fake desktop to PNG.
    /// `SIGMA5_SNAP_DIR=<dir> cargo test solar_snapshot -- --ignored`
    #[test]
    #[ignore]
    fn solar_snapshot() {
        let Ok(dir) = std::env::var("SIGMA5_SNAP_DIR") else { return };
        let mut u = Universe { star: 120.0, last_harvest: crate::util::now_unix() - 1500, harvested: 280, ..Universe::default() };
        u.planets = vec![
            planet(11, 60.0, 0.4, [0.0, 30.0, 0.0, 22.0, 0.0]),
            planet(12, 105.0, 2.4, [4.0, 120.0, 90.0, 30.0, 14.0]),
            planet(13, 160.0, -0.9, [6.0, 60.0, 200.0, 10.0, 2.0]),
            planet(14, 225.0, 3.6, [500.0, 60.0, 40.0, 30.0, 0.0]),
            planet(16, 195.0, 1.2, [0.0, 25.0, 0.0, 0.0, 0.0]),
        ];
        u.planets[3].moons.push(solar::Moon { name: String::new(), comp: [0.0, 20.0, 10.0, 0.0, 0.0], dist: 30.0, angle: 0.6, speed: 0.6, seed: 21 });
        u.planets[1].moons.push(solar::Moon { name: String::new(), comp: [0.0, 9.0, 0.0, 3.0, 0.0], dist: 20.0, angle: 2.0, speed: 0.9, seed: 23 });
        let mut sim = Sim::new(u);
        let c = sim.new_cloud(-60.0, -150.0);
        for _ in 0..14 {
            sim.release(Material::Rock, -60.0, -150.0, c);
        }
        sim.step(0.4);
        let (gw, gh) = (854usize, 533usize);
        let mut px = vec![KEY; gw * gh];
        let fr = Frame::fit(gw, gh, (BAR_H + 8) as f32);
        let t = Instant::now();
        let mut cv = Canvas { px: &mut px, w: gw as i32, h: gh as i32 };
        // Trails: where each planet was over the last second and a half.
        let trails: Trails = sim
            .u
            .planets
            .iter()
            .map(|p| {
                let d = (p.x * p.x + p.y * p.y).sqrt();
                let a0 = p.y.atan2(p.x);
                let w = (solar::GM * d / (d * d + 25.0)).sqrt() / d;
                (p.seed, (0..TRAIL_LEN).map(|i| { let a = a0 - w * (TRAIL_LEN - 1 - i) as f32 / 30.0; (d * a.cos(), d * a.sin()) }).collect())
            })
            .collect();
        paint_space(&mut cv, &sim, fr, (Some(12), Some(14)), 0xAB72FF, 1.0, (0.0, 0.0), &trails);
        println!("space layer: {:?}", t.elapsed());
        // The interface, through a stand-in Desk-less copy of its drawing.
        let bar = Rc { x: (gw as i32 - 420) / 2, y: gh as i32 - BAR_H - 6, w: 420, h: BAR_H };
        cv.panel(Rc { x: bar.x, y: bar.y - INFO_H - 3, w: bar.w, h: INFO_H }, 0xAB72FF);
        cv.text("SAETH", bar.x + 6, bar.y - INFO_H + 2, 2, INK);
        cv.text("TEMPERATE WORLD · 258 PARTICLES · 1 MOON · YEAR 23S", bar.x + 6, bar.y - INFO_H + 19, 1, DIM);
        cv.panel(bar, 0xAB72FF);
        let mut x = bar.x + 4;
        for (i, mat) in Material::ALL.iter().enumerate() {
            let name = mat.name().to_uppercase();
            let cnt = (10 + i * 3).to_string();
            let w = 3 + 7 + 4 + pixfont::width(&name, 1) + 4 + pixfont::width(&cnt, 1) + 4;
            if i == 1 {
                cv.rect(Rc { x, y: bar.y + 3, w, h: 15 }, 0x1E2131);
                cv.rect(Rc { x, y: bar.y + 17, w, h: 1 }, 0xAB72FF);
            }
            cv.rect(Rc { x: x + 3, y: bar.y + 7, w: 7, h: 7 }, mat.rgb());
            cv.text(&name, x + 14, bar.y + 7, 1, INK);
            cv.text(&cnt, x + 14 + pixfont::width(&name, 1) + 4, bar.y + 7, 1, DIM);
            x += w + 2;
        }
        cv.text("HOLD TO RELEASE ROCK · DRAG PLANETS · ESC CLOSES", bar.x + 5, bar.y + 21, 1, DIM);
        let h = "NEXT HARVEST 35 MIN";
        cv.text(h, bar.x + bar.w - 5 - pixfont::width(h, 1), bar.y + 21, 1, DIM);
        cv.label("KATHY", 300, 120, INK);
        // The launcher badge, beside it.
        let mut bpx = vec![KEY; (BADGE_N * BADGE_N) as usize];
        paint_badge(&mut Canvas { px: &mut bpx, w: BADGE_N, h: BADGE_N }, 1.0, 0.6, 0xAB72FF);
        for (i, c) in bpx.iter().enumerate() {
            if *c != KEY {
                let (bx, by) = (i as i32 % BADGE_N, i as i32 / BADGE_N);
                px[((20 + by) as usize) * gw + (20 + bx) as usize] = *c;
            }
        }
        // See-through pixels as a light "desktop" so edges are visible.
        for (i, p) in px.iter_mut().enumerate() {
            if *p == KEY {
                let (x, y) = (i % gw, i / gw);
                *p = if (x / 40 + y / 40) % 2 == 0 { 0xDCE3EC } else { 0xC9D2DD };
            }
        }
        crate::ui::draw::write_png(&format!("{dir}/solar-pixel.png"), &px, gw, gh, 2);
    }

    /// Opens the real thing on a private desktop (never on screen) and drives
    /// it with window messages: release, select, drag, rename, Esc.
    /// `cargo test solar_desktop_live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn solar_desktop_live() {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn CreateDesktopW(name: PCWSTR, dev: PCWSTR, mode: *const core::ffi::c_void, flags: u32, access: u32, sa: *const core::ffi::c_void) -> isize;
            fn SetThreadDesktop(desk: isize) -> i32;
            fn CloseDesktop(desk: isize) -> i32;
        }
        fn pump(ms: u64) {
            let until = Instant::now() + Duration::from_millis(ms);
            let mut msg = MSG::default();
            while Instant::now() < until {
                unsafe {
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        let tmp = std::env::temp_dir().join(format!("sigma5-solar-desk-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("SIGMA5")).unwrap();
        unsafe {
            std::env::set_var("APPDATA", &tmp);
        }
        let saved = tmp.join("SIGMA5").join("solar.json");
        std::thread::spawn(move || unsafe {
            let desk = CreateDesktopW(w!("SIGMA5.SolarDesk2"), PCWSTR::null(), std::ptr::null(), 0, 0x1000_0000, std::ptr::null());
            assert!(desk != 0 && SetThreadDesktop(desk) != 0);
            open(HWND::default(), Some(POINT { x: 40, y: 40 }), COLORREF(0x00FF72AB));
            let catcher = OPEN.with(|c| c.get());
            assert!(!catcher.is_invalid(), "opened");
            assert!(IsWindowVisible(catcher).as_bool(), "it takes the mouse while open");
            pump(1000);
            let (left, top, cell, gw, gh) = with_open(|d| (d.ov.left, d.ov.top, d.ov.cell, d.ov.gw as i32, d.ov.gh as i32)).unwrap();
            println!("layer {gw}x{gh} of {cell} px");
            // Client coordinates of the catcher == screen minus its origin.
            let lp = |gx: i32, gy: i32| {
                let (x, y) = (gx * cell + cell / 2, gy * cell + cell / 2);
                LPARAM(((y as isize) << 16) | (x as isize & 0xFFFF))
            };
            let _ = (left, top);
            // "2" picks rock; hold the mouse out in space.
            SendMessageW(catcher, WM_KEYDOWN, Some(WPARAM(0x32)), Some(LPARAM(0)));
            let (gx, gy) = (gw * 7 / 10, gh / 4);
            SendMessageW(catcher, WM_MOUSEMOVE, Some(WPARAM(0)), Some(lp(gx, gy)));
            SendMessageW(catcher, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(lp(gx, gy)));
            pump(1200);
            SendMessageW(catcher, WM_LBUTTONUP, Some(WPARAM(0)), Some(lp(gx, gy)));
            pump(3500);
            let (planets, dust, lit) = with_open(|d| (d.sim.u.planets.clone(), d.sim.dust.len(), d.ov.lit())).unwrap();
            println!("{} planet(s), loose {dust}, {lit} pixels lit", planets.len());
            assert_eq!(planets.len(), 1);
            assert_eq!(dust, 0);
            // Click the planet: selected. Then drag it to a new orbit.
            let p = planets[0].clone();
            let (px, py) = with_open(|d| d.frame_geom().to_grid(p.x, p.y)).unwrap();
            let (px, py) = (px as i32, py as i32);
            SendMessageW(catcher, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(lp(px, py)));
            SendMessageW(catcher, WM_LBUTTONUP, Some(WPARAM(0)), Some(lp(px, py)));
            assert_eq!(with_open(|d| d.selected).unwrap(), Some(p.seed), "click selects");
            let (cx, cy) = with_open(|d| { let f = d.frame_geom(); (f.cx as i32, f.cy as i32) }).unwrap();
            let target = (cx - gh / 3, cy);
            let (px, py) = with_open(|d| d.sim.u.planets.iter().find(|q| q.seed == p.seed).map(|q| d.frame_geom().to_grid(q.x, q.y))).unwrap().unwrap();
            SendMessageW(catcher, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(lp(px as i32, py as i32)));
            for i in 1..=10 {
                let (x, y) = (px as i32 + (target.0 - px as i32) * i / 10, py as i32 + (target.1 - py as i32) * i / 10);
                SendMessageW(catcher, WM_MOUSEMOVE, Some(WPARAM(1)), Some(lp(x, y)));
            }
            let radius = || with_open(|d| { let q = d.sim.u.planets.iter().find(|q| q.seed == p.seed).unwrap(); (q.x * q.x + q.y * q.y).sqrt() * d.frame_geom().k }).unwrap();
            let r0 = radius();
            pump(1500); // it has weight: it takes a moment to follow
            let r1 = radius();
            SendMessageW(catcher, WM_LBUTTONUP, Some(WPARAM(0)), Some(lp(target.0, target.1)));
            pump(5000);
            let r2 = radius();
            println!("drag: orbit {r0:.0} px at the start, {r1:.0} px after following, {r2:.0} px once settled (wanted {})", gh / 3);
            assert!((r1 - (gh / 3) as f32).abs() < 6.0, "followed the mouse");
            assert!((r2 - (gh / 3) as f32).abs() < (gh / 3) as f32 * 0.2, "settled near where it was let go");
            // Rename: F2, type, Enter.
            SendMessageW(catcher, WM_KEYDOWN, Some(WPARAM(VK_F2.0 as usize)), Some(LPARAM(0)));
            for _ in 0..20 {
                SendMessageW(catcher, WM_KEYDOWN, Some(WPARAM(VK_BACK.0 as usize)), Some(LPARAM(0)));
            }
            for c in "Gaia".chars() {
                SendMessageW(catcher, WM_CHAR, Some(WPARAM(c as usize)), Some(LPARAM(0)));
            }
            SendMessageW(catcher, WM_KEYDOWN, Some(WPARAM(VK_RETURN.0 as usize)), Some(LPARAM(0)));
            pump(200);
            if let Ok(dir) = std::env::var("SIGMA5_SNAP_DIR") {
                with_open(|d| {
                    let (gw, gh) = (d.ov.gw, d.ov.gh);
                    let px: Vec<u32> = d.ov.grid().iter().enumerate().map(|(i, &c)| if c == KEY { if (i % gw / 40 + i / gw / 40) % 2 == 0 { 0x2A2D38 } else { 0x23252E } } else { c }).collect();
                    crate::ui::draw::write_png(&format!("{dir}/solar-live.png"), &px, gw, gh, 2);
                });
            }
            // Esc puts it all away at once.
            SendMessageW(catcher, WM_KEYDOWN, Some(WPARAM(VK_ESCAPE.0 as usize)), Some(LPARAM(0)));
            pump(100);
            assert!(OPEN.with(|c| c.get()).is_invalid(), "Esc closes");
            assert!(!IsWindow(Some(catcher)).as_bool(), "everything is freed");
            CloseDesktop(desk);
        })
        .join()
        .unwrap();
        let u: Universe = serde_json::from_str(&std::fs::read_to_string(&saved).unwrap()).unwrap();
        assert_eq!(u.planets[0].name, "Gaia");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
