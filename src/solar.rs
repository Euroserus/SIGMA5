//! Solar System: a small, orderly planet-building simulation fed by the
//! Typing Test. Each correctly typed word becomes one particle of a material
//! that depends on the kind of word; the first test each hour is harvested.
//!
//! The physics is deliberately calm rather than a full n-body simulation:
//! - Planets keep to circular orbits at the distance they formed (or were
//!   dragged to). They only feel each other up close (inside the larger one's
//!   Hill sphere), where a small body is captured as a moon and similar ones
//!   merge. Anything that strays too close falls into the star.
//! - Released particles form a cloud that pulls itself together and becomes a
//!   planet within a few seconds, or, if released near a planet, joins it.
//!   No particle stays loose for long.
//!
//! Pure logic and persistence only; `ui::solar` draws it.

use serde::{Deserialize, Serialize};

/// Number of materials.
pub const N: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Material {
    Hydrogen,
    Rock,
    Water,
    Iron,
    Carbon,
}

impl Material {
    pub const ALL: [Material; N] = [Material::Hydrogen, Material::Rock, Material::Water, Material::Iron, Material::Carbon];

    pub fn name(self) -> &'static str {
        match self {
            Material::Hydrogen => "Hydrogen",
            Material::Rock => "Rock",
            Material::Water => "Water",
            Material::Iron => "Iron",
            Material::Carbon => "Carbon",
        }
    }

    /// Which words make it.
    pub fn source(self) -> &'static str {
        match self {
            Material::Hydrogen => "little words: the, of, and",
            Material::Rock => "nouns",
            Material::Water => "verbs",
            Material::Iron => "describing words",
            Material::Carbon => "long words (8+ letters)",
        }
    }

    /// 0xRRGGBB.
    pub fn rgb(self) -> u32 {
        match self {
            Material::Hydrogen => 0xF2D49B,
            Material::Rock => 0xB08A68,
            Material::Water => 0x4FA6FF,
            Material::Iron => 0xD0573E,
            Material::Carbon => 0x5EDB8C,
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

// ---------------------------------------------------------------------------
// Words → materials

const FUNCTION_WORDS: &[&str] = &[
    "the", "of", "and", "a", "an", "to", "in", "he", "it", "that", "for", "they", "with", "as", "not", "on", "she",
    "at", "by", "this", "we", "you", "but", "from", "or", "which", "one", "would", "all", "will", "there", "who",
    "when", "can", "more", "if", "no", "out", "other", "so", "what", "up", "about", "than", "into", "could", "only",
    "some", "these", "then", "any", "now", "may", "such", "over", "most", "even", "also", "after", "many", "must",
    "before", "back", "through", "where", "much", "should", "well", "down", "own", "just", "because", "each",
    "those", "how", "too", "very", "still", "here", "both", "between", "under", "while", "might", "off", "few",
    "since", "against", "again", "around", "however", "once", "upon", "every", "let", "until", "far", "always",
    "away", "toward", "though", "less", "enough", "almost", "yet", "why", "nothing", "another", "never", "same",
    "during", "without", "i", "me", "my", "him", "his", "her", "our", "us", "them", "their", "its", "your", "is",
    "are", "was", "were", "has", "had",
];

const VERBS: &[&str] = &[
    "be", "have", "do", "say", "make", "go", "take", "come", "know", "see", "use", "get", "like", "give", "think",
    "find", "look", "feel", "seem", "tell", "write", "become", "show", "need", "mean", "call", "develop", "move",
    "begin", "leave", "want", "ask", "hold", "follow", "lead", "set", "run", "keep", "play", "stand", "change",
    "help", "put", "close", "meet", "build", "hear", "unite", "live", "bring", "try", "provide", "pay", "include",
    "expect", "act", "read", "start", "turn", "govern", "open", "work",
];

const DESCRIBING: &[&str] = &[
    "new", "good", "great", "high", "little", "old", "long", "small", "large", "real", "right", "general", "public",
    "certain", "social", "young", "big", "clear", "better", "late", "early", "first", "second", "last", "whole",
];

/// The material a typed word becomes.
pub fn classify(word: &str) -> Material {
    let w = word.trim().to_ascii_lowercase();
    if w == "water" {
        return Material::Water;
    }
    if w.chars().count() >= 8 {
        return Material::Carbon;
    }
    if FUNCTION_WORDS.contains(&w.as_str()) {
        Material::Hydrogen
    } else if VERBS.contains(&w.as_str()) {
        Material::Water
    } else if DESCRIBING.contains(&w.as_str()) || w.ends_with("ly") || w.ends_with("ful") || w.ends_with("ous") {
        Material::Iron
    } else if w.ends_with("ing") || w.ends_with("ed") || w.ends_with("ize") {
        Material::Water
    } else {
        Material::Rock
    }
}

// ---------------------------------------------------------------------------
// Saved state

/// Seconds between harvests.
pub const HARVEST_EVERY: u64 = 3600;
/// Material given once, so a new solar system has something to play with.
pub const STARTER: [u32; N] = [10, 20, 10, 6, 2];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Moon {
    /// Empty in older saves: see [`Moon::display_name`].
    #[serde(default)]
    pub name: String,
    pub comp: [f32; N],
    /// Distance from the planet, angle (rad) and angular speed (rad/s).
    pub dist: f32,
    pub angle: f32,
    pub speed: f32,
    pub seed: u32,
}

impl Moon {
    pub fn display_name(&self) -> String {
        if self.name.is_empty() { planet_name(self.seed) } else { self.name.clone() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Planet {
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    /// Mass of each material, in particles.
    pub comp: [f32; N],
    /// Unique id and texture seed.
    pub seed: u32,
    pub moons: Vec<Moon>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Universe {
    /// Material the star has swallowed, in particles.
    pub star: f32,
    pub planets: Vec<Planet>,
    /// Material waiting to be released.
    pub inventory: [u32; N],
    /// When the last test was harvested (unix seconds).
    pub last_harvest: u64,
    /// Particles harvested, all time.
    pub harvested: u32,
    pub next_seed: u32,
    /// Seconds the simulation has run.
    pub time: f64,
}

impl Default for Universe {
    fn default() -> Self {
        Self { star: 0.0, planets: Vec::new(), inventory: STARTER, last_harvest: 0, harvested: 0, next_seed: 1, time: 0.0 }
    }
}

/// "35 MIN" / "1 H 5 MIN" for the pixel interface.
pub fn wait_short(secs: u64) -> String {
    match secs {
        0..60 => "1 MIN".into(),
        60..3600 => format!("{} MIN", secs.div_ceil(60)),
        _ => format!("{} H {} MIN", secs / 3600, secs % 3600 / 60),
    }
}

pub fn mass(comp: &[f32; N]) -> f32 {
    comp.iter().sum()
}

/// Share of each material, 0..1.
pub fn fractions(comp: &[f32; N]) -> [f32; N] {
    let m = mass(comp).max(1e-6);
    comp.map(|c| c / m)
}

/// Body radius (world units, about a screen pixel each): a clump of its
/// particles, one pixel apiece, with hydrogen taking twice the room.
pub fn radius(comp: &[f32; N]) -> f32 {
    let area = mass(comp) + comp[Material::Hydrogen.index()];
    (area / std::f32::consts::PI).sqrt() * 1.15 + 0.8
}

/// Inside-out order of a body's layers: densest at the core.
pub const LAYERS: [Material; N] = [Material::Iron, Material::Rock, Material::Carbon, Material::Water, Material::Hydrogen];

/// What kind of world a composition makes.
pub fn kind(comp: &[f32; N]) -> &'static str {
    let f = fractions(comp);
    let m = mass(comp);
    let [h, rock, water, iron, carbon] = f;
    if h > 0.5 {
        if m > 150.0 { "Gas giant" } else { "Gas dwarf" }
    } else if water > 0.45 {
        "Ocean world"
    } else if water > 0.12 && carbon > 0.04 && rock + iron > 0.3 {
        "Temperate world"
    } else if iron > 0.35 {
        "Iron world"
    } else if water > 0.25 {
        "Ice world"
    } else if m < 12.0 && rock > 0.0 {
        "Planetesimal"
    } else {
        "Rocky world"
    }
}

const SYLLABLES: &[&str] = &[
    "ka", "ri", "to", "ve", "lu", "xo", "ne", "sa", "dra", "mi", "or", "eth", "qua", "zan", "bel", "ty", "ar", "on",
    "is", "ul", "pho", "ce", "ga", "nym", "thy", "ro", "dia", "sel", "vo", "ix",
];

pub fn planet_name(seed: u32) -> String {
    let mut h = seed.wrapping_mul(0x9E37_79B1) ^ 0x5bd1_e995;
    let mut next = || {
        h ^= h << 13;
        h ^= h >> 17;
        h ^= h << 5;
        h
    };
    let n = 2 + (next() % 3 == 0) as u32;
    let mut s = String::new();
    for _ in 0..n {
        s.push_str(SYLLABLES[(next() % SYLLABLES.len() as u32) as usize]);
    }
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

impl Universe {
    fn path() -> std::path::PathBuf {
        crate::util::data_dir().join("solar.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(t) = serde_json::to_string(self) {
            let tmp = Self::path().with_extension("json.tmp");
            if std::fs::write(&tmp, t).is_ok() {
                let _ = std::fs::rename(&tmp, Self::path());
            }
        }
    }

    /// Seconds until the next test counts (0 = now).
    pub fn harvest_wait(&self, now: u64) -> u64 {
        (self.last_harvest + HARVEST_EVERY).saturating_sub(now)
    }

    /// Turns typed words into material if a harvest is due. Returns what was
    /// gained, or the seconds left to wait.
    pub fn harvest<'a>(&mut self, words: impl IntoIterator<Item = &'a str>, now: u64) -> Result<[u32; N], u64> {
        let wait = self.harvest_wait(now);
        if wait > 0 {
            return Err(wait);
        }
        let mut got = [0u32; N];
        for w in words {
            got[classify(w).index()] += 1;
        }
        if got.iter().sum::<u32>() == 0 {
            return Ok(got); // nothing typed right: the hour isn't used up
        }
        for (inv, g) in self.inventory.iter_mut().zip(got) {
            *inv += g;
        }
        self.harvested += got.iter().sum::<u32>();
        self.last_harvest = now;
        Ok(got)
    }
}

// ---------------------------------------------------------------------------
// Simulation

/// Star's gravity (world units³/s²): an orbit at radius 100 takes ~21 s.
pub const GM: f32 = 90_000.0;
/// Star mass in particles, for Hill-sphere sizes.
const STAR_MASS: f32 = 30_000.0;
const MAX_MOONS: usize = 4;
/// A cloud becomes a planet this long after its last particle was released...
const CLOUD_SETTLE: f32 = 1.6;
/// ...or sooner once it's this tight (world units, RMS).
const CLOUD_TIGHT: f32 = 2.0;
/// Clouds with fewer particles join the nearest body instead.
const MIN_PLANET: usize = 4;
/// Nothing stays loose longer than this.
const MAX_DUST_AGE: f32 = 10.0;
/// Fixed physics step.
const STEP: f32 = 1.0 / 120.0;

pub fn star_radius(star: f32) -> f32 {
    (13.0 + (star / 40.0).sqrt()).min(34.0)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    /// Pulls together into a new planet.
    Free,
    /// Falls onto this planet (by seed).
    Planet(u32),
    Star,
}

#[derive(Clone, Debug)]
pub struct Dust {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub mat: Material,
    pub cloud: u32,
    pub age: f32,
}

struct Cloud {
    id: u32,
    target: Target,
    /// Seconds since a particle was last added.
    quiet: f32,
}

/// Something worth showing: (x, y, colour, kind).
#[derive(Clone, Debug)]
pub struct Flash {
    pub x: f32,
    pub y: f32,
    pub rgb: u32,
    pub age: f32,
    pub size: f32,
}

pub struct Sim {
    pub u: Universe,
    pub dust: Vec<Dust>,
    pub flashes: Vec<Flash>,
    /// Messages for the user ("Kerath captured a moon").
    pub notes: Vec<String>,
    /// Cursor in world space, for a light wobble of loose dust.
    pub mouse: Option<(f32, f32)>,
    /// A planet held by the mouse, and where it's being pulled to.
    pub held: Option<u32>,
    grab: (f32, f32),
    /// Planets moving freely (thrown, or just let go) until they settle
    /// back into a circular orbit: (seed, seconds spent nearly circular).
    free: Vec<(u32, f32)>,
    clouds: Vec<Cloud>,
    next_cloud: u32,
    rng: u64,
    acc: f32,
}

fn tangent(x: f32, y: f32) -> (f32, f32) {
    let d = (x * x + y * y).sqrt().max(1e-3);
    (-y / d, x / d)
}

/// Circular orbital velocity (anticlockwise on screen) at a point.
pub fn orbit_velocity(x: f32, y: f32) -> (f32, f32) {
    // Matches the softened gravity used for motion (GM / (d² + 25)).
    let d = (x * x + y * y).sqrt().max(1.0);
    let v = (GM * d / (d * d + 25.0)).sqrt();
    let (tx, ty) = tangent(x, y);
    (tx * v, ty * v)
}

impl Sim {
    pub fn new(u: Universe) -> Self {
        let rng = 0x2545_F491_4F6C_DD1D ^ (u.time.to_bits()) ^ u.next_seed as u64;
        Sim { u, dust: Vec::new(), flashes: Vec::new(), notes: Vec::new(), mouse: None, held: None, grab: (0.0, 0.0), free: Vec::new(), clouds: Vec::new(), next_cloud: 1, rng: rng | 1, acc: 0.0 }
    }

    fn rand(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        (x >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn star_radius(&self) -> f32 {
        star_radius(self.u.star)
    }

    fn planet_index(&self, seed: u32) -> Option<usize> {
        self.u.planets.iter().position(|p| p.seed == seed)
    }

    /// Distance from (x, y) within which released material joins a planet.
    pub fn capture_radius(p: &Planet) -> f32 {
        radius(&p.comp) * 3.0 + 26.0
    }

    /// The planet whose reach covers (x, y), nearest first (by seed).
    fn planet_reaching(&self, x: f32, y: f32) -> Option<u32> {
        self.u
            .planets
            .iter()
            .map(|p| (p.seed, ((p.x - x).powi(2) + (p.y - y).powi(2)).sqrt() / Self::capture_radius(p)))
            .filter(|(_, r)| *r < 1.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(s, _)| s)
    }

    /// Takes hold of a planet and pulls it towards (x, y). It follows on a
    /// spring that is softer the heavier the planet is.
    pub fn grab(&mut self, seed: u32, x: f32, y: f32) {
        if self.held != Some(seed) {
            self.held = Some(seed);
            self.free.retain(|(s, _)| *s != seed);
        }
        self.grab = (x, y);
    }

    /// Lets go: the planet keeps its momentum (on top of the orbital speed
    /// where it is), then eases back into a circular orbit.
    pub fn let_go(&mut self) {
        let Some(seed) = self.held.take() else { return };
        if let Some(p) = self.u.planets.iter_mut().find(|p| p.seed == seed) {
            let (cx, cy) = orbit_velocity(p.x, p.y);
            p.vx = cx + p.vx * 0.6;
            p.vy = cy + p.vy * 0.6;
            self.free.push((seed, 0.0));
        }
    }

    #[cfg(test)]
    pub fn is_free(&self, seed: u32) -> bool {
        self.held == Some(seed) || self.free.iter().any(|(s, _)| *s == seed)
    }

    /// The moon (planet index, moon index) whose disc is at (x, y).
    pub fn moon_at(&self, x: f32, y: f32, slack: f32) -> Option<(usize, usize)> {
        let mut best: Option<(f32, usize, usize)> = None;
        for (pi, p) in self.u.planets.iter().enumerate() {
            for (mi, m) in p.moons.iter().enumerate() {
                let (mx, my) = (p.x + m.angle.cos() * m.dist, p.y + m.angle.sin() * m.dist);
                let d = ((mx - x).powi(2) + (my - y).powi(2)).sqrt() - radius(&m.comp);
                if d <= slack && best.is_none_or(|b| d < b.0) {
                    best = Some((d, pi, mi));
                }
            }
        }
        best.map(|(_, p, m)| (p, m))
    }

    /// The planet (index) whose disc is at (x, y), within `slack` world units.
    pub fn planet_at(&self, x: f32, y: f32, slack: f32) -> Option<usize> {
        self.u
            .planets
            .iter()
            .enumerate()
            .map(|(i, p)| (i, ((p.x - x).powi(2) + (p.y - y).powi(2)).sqrt() - radius(&p.comp)))
            .filter(|(_, d)| *d <= slack)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// Starts a new release at (x, y): near a planet it feeds that planet,
    /// near the star it feeds the star, otherwise it forms a new planet.
    pub fn new_cloud(&mut self, x: f32, y: f32) -> u32 {
        let id = self.next_cloud;
        self.next_cloud += 1;
        let d_star = (x * x + y * y).sqrt();
        let target = if d_star < self.star_radius() * 2.2 {
            Target::Star
        } else {
            self.planet_reaching(x, y).map_or(Target::Free, Target::Planet)
        };
        self.clouds.push(Cloud { id, target, quiet: 0.0 });
        id
    }

    /// Releases one particle of `mat` at (x, y) into `cloud`. False when none are left.
    pub fn release(&mut self, mat: Material, x: f32, y: f32, cloud: u32) -> bool {
        let inv = &mut self.u.inventory[mat.index()];
        if *inv == 0 {
            return false;
        }
        *inv -= 1;
        let target = self.clouds.iter().find(|c| c.id == cloud).map_or(Target::Free, |c| c.target);
        if let Some(c) = self.clouds.iter_mut().find(|c| c.id == cloud) {
            c.quiet = 0.0;
        }
        let (mut vx, mut vy) = match target {
            Target::Planet(s) => self.planet_index(s).map_or(orbit_velocity(x, y), |i| (self.u.planets[i].vx, self.u.planets[i].vy)),
            _ => orbit_velocity(x, y),
        };
        let (jx, jy) = (self.rand() - 0.5, self.rand() - 0.5);
        vx += jx * 6.0;
        vy += jy * 6.0;
        let (ox, oy) = (self.rand() - 0.5, self.rand() - 0.5);
        self.dust.push(Dust { x: x + ox * 3.0, y: y + oy * 3.0, vx, vy, mat, cloud, age: 0.0 });
        true
    }

    /// Advances by `dt` seconds (fixed internal steps; long gaps are capped).
    pub fn step(&mut self, dt: f32) {
        self.acc = (self.acc + dt.min(0.25)).min(0.25);
        while self.acc >= STEP {
            self.acc -= STEP;
            self.substep(STEP);
        }
    }

    fn substep(&mut self, h: f32) {
        self.u.time += h as f64;
        self.move_planets(h);
        self.planet_encounters();
        self.move_dust(h);
        self.settle_clouds(h);
        for f in &mut self.flashes {
            f.age += h;
        }
        self.flashes.retain(|f| f.age < 1.2);
    }

    fn move_planets(&mut self, h: f32) {
        let sr = self.star_radius();
        let (held, grab) = (self.held, self.grab);
        for p in &mut self.u.planets {
            let d = (p.x * p.x + p.y * p.y).sqrt().max(1.0);
            let free = self.free.iter_mut().find(|(s, _)| *s == p.seed);
            if Some(p.seed) == held {
                // On a spring to the mouse; heavier planets lag more.
                let k = 70.0 / (1.0 + mass(&p.comp) / 120.0);
                let c = 2.0 * k.sqrt() * 0.85;
                p.vx += ((grab.0 - p.x) * k - p.vx * c) * h;
                p.vy += ((grab.1 - p.y) * k - p.vy * c) * h;
                p.x += p.vx * h;
                p.y += p.vy * h;
            } else if let Some((_, calm)) = free {
                // Free flight under the star's pull, easing towards a circle.
                let d2 = p.x * p.x + p.y * p.y;
                let g = GM / (d2 + 25.0);
                p.vx -= g * p.x / d * h;
                p.vy -= g * p.y / d * h;
                let (cx, cy) = orbit_velocity(p.x, p.y);
                let dir = if p.x * p.vy - p.y * p.vx >= 0.0 { 1.0 } else { -1.0 };
                p.vx += (cx * dir - p.vx) * 0.9 * h;
                p.vy += (cy * dir - p.vy) * 0.9 * h;
                p.x += p.vx * h;
                p.y += p.vy * h;
                let off = ((p.vx - cx * dir).powi(2) + (p.vy - cy * dir).powi(2)).sqrt() / (cx * cx + cy * cy).sqrt().max(1e-3);
                *calm = if off < 0.03 { *calm + h } else { 0.0 };
            } else {
                // A circular orbit at its distance: steady and predictable.
                let w = (GM * d / (d * d + 25.0)).sqrt() / d;
                let a = p.y.atan2(p.x) + w * h;
                (p.x, p.y) = (d * a.cos(), d * a.sin());
                (p.vx, p.vy) = orbit_velocity(p.x, p.y);
            }
            for m in &mut p.moons {
                m.angle = (m.angle + m.speed * h) % std::f32::consts::TAU;
            }
        }
        // Settled: back on a steady circular orbit.
        self.free.retain(|(s, calm)| *calm < 0.6 && self.u.planets.iter().any(|p| p.seed == *s));
        // Too close to the star: swallowed.
        let mut i = 0;
        while i < self.u.planets.len() {
            let p = &self.u.planets[i];
            let d = (p.x * p.x + p.y * p.y).sqrt();
            if d < sr + radius(&p.comp) * 0.5 {
                let p = self.u.planets.remove(i);
                self.u.star += mass(&p.comp) + p.moons.iter().map(|m| mass(&m.comp)).sum::<f32>();
                self.flashes.push(Flash { x: p.x, y: p.y, rgb: 0xFFD27A, age: 0.0, size: sr });
                self.notes.push(format!("{} fell into the star", p.name));
            } else {
                i += 1;
            }
        }
    }

    fn planet_encounters(&mut self) {
        let n = self.u.planets.len();
        let mut event: Option<(usize, usize, bool)> = None; // (big, small, as moon)
        'outer: for i in 0..n {
            for j in i + 1..n {
                let (a, b) = (&self.u.planets[i], &self.u.planets[j]);
                let (ma, mb) = (mass(&a.comp), mass(&b.comp));
                let (big, small) = if ma >= mb { (i, j) } else { (j, i) };
                let (pb, ps) = (&self.u.planets[big], &self.u.planets[small]);
                let (mbig, msmall) = (ma.max(mb), ma.min(mb));
                let (dx, dy) = (pb.x - ps.x, pb.y - ps.y);
                let d = (dx * dx + dy * dy).sqrt().max(1e-3);
                let rb = radius(&pb.comp);
                let rs = radius(&ps.comp);
                let star_d = (pb.x * pb.x + pb.y * pb.y).sqrt();
                let hill = (star_d * (mbig / (3.0 * STAR_MASS)).cbrt()).max(rb * 2.5);
                if d >= hill {
                    continue;
                }
                let moonish = msmall / mbig < 0.25 && pb.moons.len() < MAX_MOONS;
                let held = self.held.is_some_and(|h| h == pb.seed || h == ps.seed);
                if !held && (d < rb + rs || d < hill * if moonish { 0.45 } else { 0.3 }) {
                    event = Some((big, small, moonish));
                    break 'outer;
                }
            }
        }
        let Some((big, small, as_moon)) = event else { return };
        let s = self.u.planets[small].clone();
        let b = &mut self.u.planets[big];
        if as_moon {
            let rb = radius(&b.comp);
            let dist = ((s.x - b.x).powi(2) + (s.y - b.y).powi(2)).sqrt().clamp(rb * 2.2, rb * 2.2 + 6.0 + 3.0 * b.moons.len() as f32);
            let angle = (s.y - b.y).atan2(s.x - b.x);
            let speed = 6.0 / dist.sqrt();
            b.moons.push(Moon { name: s.name.clone(), comp: s.comp, dist, angle, speed, seed: s.seed });
            // Its own moons become this planet's.
            for m in s.moons.iter().take(MAX_MOONS.saturating_sub(b.moons.len())) {
                b.moons.push(Moon { dist: dist + 4.0 + m.dist * 0.3, ..m.clone() });
            }
            self.notes.push(format!("{} captured {} as a moon", b.name, s.name));
            self.flashes.push(Flash { x: s.x, y: s.y, rgb: 0xBFD4FF, age: 0.0, size: radius(&s.comp) * 2.0 });
        } else {
            let (mb, ms) = (mass(&b.comp), mass(&s.comp));
            let t = ms / (mb + ms);
            b.x += (s.x - b.x) * t;
            b.y += (s.y - b.y) * t;
            b.vx += (s.vx - b.vx) * t;
            b.vy += (s.vy - b.vy) * t;
            for k in 0..N {
                b.comp[k] += s.comp[k];
            }
            for m in s.moons {
                if b.moons.len() < MAX_MOONS {
                    b.moons.push(m);
                }
            }
            let rb = radius(&b.comp);
            for (i, m) in b.moons.iter_mut().enumerate() {
                m.dist = m.dist.max(rb * 2.2 + 3.0 * i as f32);
            }
            self.notes.push(format!("{} and {} collided and merged", b.name, s.name));
            self.flashes.push(Flash { x: b.x, y: b.y, rgb: 0xFFB070, age: 0.0, size: rb * 2.5 });
        }
        self.u.planets.remove(small);
    }

    fn move_dust(&mut self, h: f32) {
        // Cloud centres and mean velocities.
        let mut centres: Vec<(u32, f32, f32, f32, f32, f32)> = Vec::new(); // id, n, x, y, vx, vy
        for d in &self.dust {
            match centres.iter_mut().find(|c| c.0 == d.cloud) {
                Some(c) => {
                    c.1 += 1.0;
                    c.2 += d.x;
                    c.3 += d.y;
                    c.4 += d.vx;
                    c.5 += d.vy;
                }
                None => centres.push((d.cloud, 1.0, d.x, d.y, d.vx, d.vy)),
            }
        }
        for c in &mut centres {
            let n = c.1;
            c.2 /= n;
            c.3 /= n;
            c.4 /= n;
            c.5 /= n;
        }
        let sr = self.star_radius();
        let mut absorbed: Vec<(usize, Option<u32>)> = Vec::new(); // dust index, planet seed (None = star)
        for (i, d) in self.dust.iter_mut().enumerate() {
            d.age += h;
            let r2 = d.x * d.x + d.y * d.y;
            let r = r2.sqrt().max(1.0);
            let g = GM / (r2 + 25.0);
            let (mut ax, mut ay) = (-g * d.x / r, -g * d.y / r);
            let target = self.clouds.iter().find(|c| c.id == d.cloud).map_or(Target::Free, |c| c.target);
            let homing = match target {
                Target::Planet(s) => self.u.planets.iter().find(|p| p.seed == s).map(|p| (p.x, p.y, p.vx, p.vy, radius(&p.comp), Some(s))),
                Target::Star => Some((0.0, 0.0, 0.0, 0.0, sr, None)),
                Target::Free => None,
            };
            match homing {
                Some((px, py, pvx, pvy, pr, seed)) => {
                    let (dx, dy) = (px - d.x, py - d.y);
                    let dist = (dx * dx + dy * dy).sqrt().max(1e-3);
                    if dist < pr + 0.8 {
                        absorbed.push((i, seed));
                        continue;
                    }
                    // A firm, damped pull: it spirals in within a second or two.
                    let pull = (dist * 18.0).min(500.0);
                    ax = dx / dist * pull + (pvx - d.vx) * 3.0 + if seed.is_some() { ax } else { 0.0 };
                    ay = dy / dist * pull + (pvy - d.vy) * 3.0 + if seed.is_some() { ay } else { 0.0 };
                }
                None => {
                    if let Some(c) = centres.iter().find(|c| c.0 == d.cloud) {
                        ax += (c.2 - d.x) * 5.0 + (c.4 - d.vx) * 2.5;
                        ay += (c.3 - d.y) * 5.0 + (c.5 - d.vy) * 2.5;
                    }
                    if r < sr {
                        absorbed.push((i, None));
                        continue;
                    }
                }
            }
            // The cursor stirs loose dust a little.
            if let Some((mx, my)) = self.mouse {
                let (dx, dy) = (d.x - mx, d.y - my);
                let md = (dx * dx + dy * dy).sqrt();
                if md < 24.0 && md > 0.1 {
                    let s = 35.0 * (1.0 - md / 24.0);
                    ax += -dy / md * s;
                    ay += dx / md * s;
                }
            }
            d.vx += ax * h;
            d.vy += ay * h;
            d.x += d.vx * h;
            d.y += d.vy * h;
        }
        // Stragglers join the nearest body.
        for (i, d) in self.dust.iter().enumerate() {
            if d.age > MAX_DUST_AGE && !absorbed.iter().any(|(j, _)| *j == i) {
                let near = self
                    .u
                    .planets
                    .iter()
                    .min_by(|a, b| ((a.x - d.x).powi(2) + (a.y - d.y).powi(2)).total_cmp(&((b.x - d.x).powi(2) + (b.y - d.y).powi(2))))
                    .map(|p| p.seed);
                absorbed.push((i, near));
            }
        }
        absorbed.sort_by_key(|(i, _)| std::cmp::Reverse(*i));
        for (i, seed) in absorbed {
            let d = self.dust.swap_remove(i);
            match seed.and_then(|s| self.planet_index(s)) {
                Some(pi) => {
                    let p = &mut self.u.planets[pi];
                    // Momentum: a light nudge only, so orbits stay calm.
                    let m = mass(&p.comp);
                    p.vx += (d.vx - p.vx) / (m + 1.0) * 0.3;
                    p.vy += (d.vy - p.vy) / (m + 1.0) * 0.3;
                    p.comp[d.mat.index()] += 1.0;
                }
                None => self.u.star += 1.0,
            }
        }
    }

    fn settle_clouds(&mut self, h: f32) {
        let mut done = Vec::new();
        for c in &mut self.clouds {
            c.quiet += h;
            let members: Vec<&Dust> = self.dust.iter().filter(|d| d.cloud == c.id).collect();
            if members.is_empty() {
                if c.quiet > 0.5 {
                    done.push(c.id);
                }
                continue;
            }
            if let Target::Planet(s) = c.target {
                if !self.u.planets.iter().any(|p| p.seed == s) {
                    c.target = Target::Free; // that planet is gone: form a new one
                }
            }
            if c.target != Target::Free || c.quiet < 0.4 {
                continue;
            }
            let n = members.len() as f32;
            let (cx, cy) = (members.iter().map(|d| d.x).sum::<f32>() / n, members.iter().map(|d| d.y).sum::<f32>() / n);
            let spread = (members.iter().map(|d| (d.x - cx).powi(2) + (d.y - cy).powi(2)).sum::<f32>() / n).sqrt();
            if c.quiet >= CLOUD_SETTLE || spread < CLOUD_TIGHT && c.quiet > 0.8 {
                done.push(c.id);
            }
        }
        for id in done {
            self.collapse(id);
        }
    }

    /// A free cloud becomes a planet, or (if too small) heads for the nearest body.
    fn collapse(&mut self, id: u32) {
        let members: Vec<Dust> = self.dust.iter().filter(|d| d.cloud == id).cloned().collect();
        let target = self.clouds.iter().find(|c| c.id == id).map(|c| c.target);
        if members.is_empty() || target != Some(Target::Free) {
            if members.is_empty() {
                self.clouds.retain(|c| c.id != id);
            }
            return;
        }
        let n = members.len() as f32;
        let (cx, cy) = (members.iter().map(|d| d.x).sum::<f32>() / n, members.iter().map(|d| d.y).sum::<f32>() / n);
        // Ended up within a planet's reach: join it rather than start a new one.
        if let Some(s) = self.planet_reaching(cx, cy) {
            if let Some(c) = self.clouds.iter_mut().find(|c| c.id == id) {
                c.target = Target::Planet(s);
            }
            return;
        }
        if members.len() < MIN_PLANET {
            let near = self
                .u
                .planets
                .iter()
                .min_by(|a, b| ((a.x - cx).powi(2) + (a.y - cy).powi(2)).total_cmp(&((b.x - cx).powi(2) + (b.y - cy).powi(2))))
                .map(|p| p.seed);
            if let Some(c) = self.clouds.iter_mut().find(|c| c.id == id) {
                c.target = near.map_or(Target::Star, Target::Planet);
            }
            return;
        }
        let mut comp = [0.0; N];
        for d in &members {
            comp[d.mat.index()] += 1.0;
        }
        let (vx, vy) = (members.iter().map(|d| d.vx).sum::<f32>() / n, members.iter().map(|d| d.vy).sum::<f32>() / n);
        let seed = self.u.next_seed;
        self.u.next_seed += 1;
        let name = planet_name(seed);
        self.notes.push(format!("A new world formed: {name}"));
        let _ = (vx, vy);
        let (vx, vy) = orbit_velocity(cx, cy);
        self.u.planets.push(Planet { name, x: cx, y: cy, vx, vy, comp, seed, moons: Vec::new() });
        self.dust.retain(|d| d.cloud != id);
        self.clouds.retain(|c| c.id != id);
    }

    /// The universe as it should be saved: loose dust goes back to the inventory.
    pub fn snapshot(&self) -> Universe {
        let mut u = self.u.clone();
        for d in &self.dust {
            u.inventory[d.mat.index()] += 1;
        }
        u
    }

    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(sim: &mut Sim, secs: f32) {
        for _ in 0..(secs * 60.0) as usize {
            sim.step(1.0 / 60.0);
        }
    }

    fn planet(seed: u32, x: f32, y: f32, comp: [f32; N]) -> Planet {
        let (vx, vy) = orbit_velocity(x, y);
        Planet { name: planet_name(seed), x, y, vx, vy, comp, seed, moons: Vec::new() }
    }

    #[test]
    fn words_become_materials() {
        assert_eq!(classify("the"), Material::Hydrogen);
        assert_eq!(classify("house"), Material::Rock);
        assert_eq!(classify("write"), Material::Water);
        assert_eq!(classify("great"), Material::Iron);
        assert_eq!(classify("president"), Material::Carbon);
        assert_eq!(classify("quickly"), Material::Iron);
        assert_eq!(classify("water"), Material::Water);
        // Every test word gets some material, and all five occur.
        let mut seen = [0; N];
        for w in crate::typing::WORDS {
            seen[classify(w).index()] += 1;
        }
        assert!(seen.iter().all(|&n| n > 0), "{seen:?}");
        assert!(seen[Material::Carbon.index()] < seen[Material::Rock.index()], "carbon is rare");
    }

    #[test]
    fn one_harvest_an_hour() {
        let mut u = Universe { inventory: [0; N], ..Universe::default() };
        let t0 = 1_800_000_000;
        assert_eq!(u.harvest(["the", "house", "write"], t0), Ok([1, 1, 1, 0, 0]));
        assert_eq!(u.inventory, [1, 1, 1, 0, 0]);
        assert_eq!(u.harvest(["the"], t0 + 600), Err(3000));
        assert_eq!(u.harvest_wait(t0 + 3599), 1);
        assert_eq!(u.harvest(["the"], t0 + 3600), Ok([1, 0, 0, 0, 0]));
        // A run with nothing typed right doesn't use up the hour.
        let mut v = Universe::default();
        assert_eq!(v.harvest(std::iter::empty(), t0), Ok([0; N]));
        assert_eq!(v.harvest_wait(t0), 0);
    }

    #[test]
    fn released_cloud_becomes_one_planet() {
        let mut sim = Sim::new(Universe { inventory: [0, 30, 0, 0, 0], ..Universe::default() });
        let c = sim.new_cloud(120.0, 0.0);
        for i in 0..20 {
            assert!(sim.release(Material::Rock, 120.0 + i as f32 * 0.3, 0.0, c));
            run(&mut sim, 0.05);
        }
        run(&mut sim, 4.0);
        assert!(sim.dust.is_empty(), "no stray dust");
        assert_eq!(sim.u.planets.len(), 1);
        let p = &sim.u.planets[0];
        assert_eq!(mass(&p.comp), 20.0);
        let d = (p.x * p.x + p.y * p.y).sqrt();
        assert!((100.0..140.0).contains(&d), "stays roughly where released: {d}");
        assert_eq!(sim.u.inventory[Material::Rock.index()], 10);
    }

    #[test]
    fn release_near_a_planet_joins_it() {
        let mut sim = Sim::new(Universe { inventory: [0, 0, 10, 0, 0], ..Universe::default() });
        sim.u.planets.push(planet(7, 150.0, 0.0, [0.0, 50.0, 0.0, 0.0, 0.0]));
        let c = sim.new_cloud(156.0, 8.0);
        for _ in 0..10 {
            sim.release(Material::Water, 156.0, 8.0, c);
        }
        run(&mut sim, 3.0);
        assert!(sim.dust.is_empty());
        assert_eq!(sim.u.planets.len(), 1);
        assert_eq!(sim.u.planets[0].comp[Material::Water.index()], 10.0);
    }

    #[test]
    fn tiny_cloud_joins_nearest_body_and_star_feeding() {
        let mut sim = Sim::new(Universe { inventory: [5, 2, 0, 0, 0], ..Universe::default() });
        sim.u.planets.push(planet(3, -200.0, 0.0, [0.0, 40.0, 0.0, 0.0, 0.0]));
        let c = sim.new_cloud(100.0, 60.0);
        sim.release(Material::Rock, 100.0, 60.0, c);
        sim.release(Material::Rock, 100.0, 60.0, c);
        let s = sim.new_cloud(5.0, 5.0);
        for _ in 0..5 {
            sim.release(Material::Hydrogen, 5.0, 5.0, s);
        }
        run(&mut sim, 8.0);
        assert!(sim.dust.is_empty());
        assert_eq!(sim.u.planets.len(), 1, "too small to be its own planet");
        assert_eq!(mass(&sim.u.planets[0].comp), 42.0);
        assert_eq!(sim.u.star, 5.0);
    }

    #[test]
    fn close_small_planet_becomes_a_moon() {
        let mut sim = Sim::new(Universe::default());
        sim.u.planets.push(planet(1, 160.0, 0.0, [0.0, 400.0, 0.0, 0.0, 0.0]));
        sim.u.planets.push(planet(2, 166.0, 9.0, [0.0, 10.0, 0.0, 0.0, 0.0]));
        run(&mut sim, 3.0);
        assert_eq!(sim.u.planets.len(), 1);
        assert_eq!(sim.u.planets[0].moons.len(), 1);
        assert!(sim.take_notes().iter().any(|n| n.contains("moon")));
    }

    #[test]
    fn similar_planets_merge_and_star_swallows() {
        let mut sim = Sim::new(Universe::default());
        sim.u.planets.push(planet(1, 160.0, 0.0, [0.0, 100.0, 0.0, 0.0, 0.0]));
        sim.u.planets.push(planet(2, 162.0, 3.0, [0.0, 80.0, 20.0, 0.0, 0.0]));
        sim.u.planets.push(planet(3, 14.0, 0.0, [30.0, 0.0, 0.0, 0.0, 0.0]));
        run(&mut sim, 2.0);
        assert_eq!(sim.u.planets.len(), 1);
        assert_eq!(mass(&sim.u.planets[0].comp), 200.0);
        assert_eq!(sim.u.star, 30.0);
    }

    #[test]
    fn released_beside_a_planet_joins_it() {
        let mut sim = Sim::new(Universe { inventory: [0, 0, 30, 0, 0], ..Universe::default() });
        sim.u.planets.push(planet(4, 150.0, 0.0, [0.0, 60.0, 0.0, 0.0, 0.0]));
        // Well outside the planet itself, but clearly beside it.
        let c = sim.new_cloud(150.0, 32.0);
        for _ in 0..20 {
            sim.release(Material::Water, 150.0, 32.0, c);
        }
        run(&mut sim, 4.0);
        assert_eq!(sim.u.planets.len(), 1, "joined, no new planet");
        assert_eq!(sim.u.planets[0].comp[Material::Water.index()], 20.0);
    }

    #[test]
    fn dragged_planet_has_weight_and_settles_where_dropped() {
        let mut sim = Sim::new(Universe::default());
        sim.u.planets.push(planet(5, 100.0, 0.0, [0.0, 40.0, 0.0, 0.0, 0.0]));
        sim.u.planets.push(planet(6, -100.0, 0.0, [0.0, 900.0, 0.0, 0.0, 0.0]));
        sim.grab(5, 0.0, -180.0);
        run(&mut sim, 0.1);
        let light = sim.u.planets[0].clone();
        assert!(light.y > -180.0 + 20.0, "it doesn't snap: it's on its way");
        run(&mut sim, 2.0);
        let p = &sim.u.planets[0];
        assert!((p.x).abs() < 3.0 && (p.y + 180.0).abs() < 3.0, "then arrives: {:?}", (p.x, p.y));
        sim.let_go();
        // A heavy planet lags further behind on the same pull.
        sim.grab(6, 0.0, 180.0);
        run(&mut sim, 0.1);
        let heavy = &sim.u.planets[1];
        assert!(heavy.x < -60.0, "heavy one barely moved yet: {}", heavy.x);
        sim.let_go();
        run(&mut sim, 30.0);
        let p = &sim.u.planets[0];
        let d = (p.x * p.x + p.y * p.y).sqrt();
        assert!((d - 180.0).abs() < 25.0, "settles near where it was dropped: {d}");
        assert!(!sim.is_free(5) && !sim.is_free(6), "back on circular orbits");
    }

    #[test]
    fn moons_can_be_found_and_named() {
        let mut sim = Sim::new(Universe::default());
        let mut p = planet(1, 150.0, 0.0, [0.0, 400.0, 0.0, 0.0, 0.0]);
        p.moons.push(Moon { name: String::new(), comp: [0.0, 10.0, 0.0, 0.0, 0.0], dist: 30.0, angle: 0.0, speed: 0.5, seed: 42 });
        sim.u.planets.push(p);
        assert_eq!(sim.moon_at(180.0, 0.0, 1.0), Some((0, 0)));
        assert_eq!(sim.moon_at(150.0, 40.0, 1.0), None);
        assert_eq!(sim.u.planets[0].moons[0].display_name(), planet_name(42));
        let old: Moon = serde_json::from_str(r#"{"comp":[0,1,0,0,0],"dist":5,"angle":0,"speed":1,"seed":3}"#).unwrap();
        assert_eq!(old.name, "", "older saves still load");
    }

    #[test]
    fn orbits_stay_calm_for_minutes() {
        let mut sim = Sim::new(Universe::default());
        for (i, r) in [70.0f32, 110.0, 170.0, 240.0].iter().enumerate() {
            let a = i as f32 * 1.7;
            sim.u.planets.push(planet(i as u32 + 1, r * a.cos(), r * a.sin(), [5.0, 60.0, 20.0, 10.0, 1.0]));
        }
        run(&mut sim, 180.0);
        assert_eq!(sim.u.planets.len(), 4);
        for (p, r) in sim.u.planets.iter().zip([70.0f32, 110.0, 170.0, 240.0]) {
            let d = (p.x * p.x + p.y * p.y).sqrt();
            assert!((d - r).abs() < 0.5, "{}: {d} vs {r}", p.name);
            assert!(p.x.is_finite() && p.vx.is_finite());
        }
    }

    #[test]
    fn saving_returns_loose_dust_and_round_trips() {
        let mut sim = Sim::new(Universe { inventory: [0, 5, 0, 0, 0], ..Universe::default() });
        sim.u.planets.push(planet(9, 90.0, 0.0, [1.0, 2.0, 3.0, 4.0, 5.0]));
        let c = sim.new_cloud(200.0, 0.0);
        sim.release(Material::Rock, 200.0, 0.0, c);
        let u = sim.snapshot();
        assert_eq!(u.inventory[Material::Rock.index()], 5);
        let back: Universe = serde_json::from_str(&serde_json::to_string(&u).unwrap()).unwrap();
        assert_eq!(back, u);
        let fresh: Universe = serde_json::from_str("{}").unwrap();
        assert_eq!(fresh.inventory, STARTER);
    }

    #[test]
    fn names_and_kinds() {
        assert_ne!(planet_name(1), planet_name(2));
        assert!(planet_name(5).chars().next().unwrap().is_uppercase());
        assert_eq!(kind(&[80.0, 10.0, 5.0, 5.0, 0.0]), "Gas dwarf");
        assert_eq!(kind(&[400.0, 10.0, 5.0, 5.0, 0.0]), "Gas giant");
        assert_eq!(kind(&[0.0, 30.0, 60.0, 5.0, 0.0]), "Ocean world");
        assert_eq!(kind(&[0.0, 50.0, 20.0, 20.0, 8.0]), "Temperate world");
        assert_eq!(kind(&[0.0, 50.0, 0.0, 60.0, 0.0]), "Iron world");
        assert!(radius(&[100.0, 0.0, 0.0, 0.0, 0.0]) > radius(&[0.0, 100.0, 0.0, 0.0, 0.0]));
    }
}
