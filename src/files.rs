//! File search: an in-memory index of the user's folders, plus Windows'
//! Recent Files list.
//!
//! Built for speed and a small footprint:
//! - The index is built on a background thread (`std::fs::read_dir`, which
//!   uses FindFirstFile and gets attributes and times without extra calls).
//! - Each entry is 24 bytes and points at its parent folder, so full paths are
//!   never stored; names live in one shared string.
//! - Every name carries a 32-bit mask of the letters/digits it contains, so a
//!   search rejects most entries with one AND before doing any real work. The
//!   survivors get a cheap in-order character check, and only real candidates
//!   reach the full fuzzy scorer.

use std::path::{Path, PathBuf};

use crate::fuzzy::Query;

/// Hard cap on indexed entries (about 6 MB of memory at most).
pub const MAX_ENTRIES: usize = 250_000;
/// Folders deeper than this below a root are not indexed.
const MAX_DEPTH: usize = 14;
const NO_PARENT: u32 = u32::MAX;

/// Folder names never indexed (build outputs, caches, VCS internals).
const SKIP_DIRS: &[&str] = &["node_modules", "__pycache__", "$recycle.bin", "system volume information"];

#[derive(Clone, Copy, Debug)]
struct Entry {
    /// Index of the parent folder entry, or NO_PARENT for a root.
    parent: u32,
    name_off: u32,
    name_len: u16,
    is_dir: bool,
    /// Letters/digits present in the name (see `mask_of`).
    mask: u32,
    /// Last modified, minutes since the Unix epoch.
    mtime: u32,
}

#[derive(Default)]
pub struct FileIndex {
    entries: Vec<Entry>,
    names: String,
    /// The folders that were indexed, in order (their entries have no parent).
    pub roots: Vec<PathBuf>,
    /// True if the cap was reached, so some files are missing.
    pub truncated: bool,
}

/// One search result.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub score: i32,
}

/// Bit per letter a–z, one for digits, one for anything else.
fn mask_of(s: &str) -> u32 {
    let mut m = 0u32;
    for c in s.chars() {
        let c = c.to_ascii_lowercase();
        m |= match c {
            'a'..='z' => 1 << (c as u32 - 'a' as u32),
            '0'..='9' => 1 << 26,
            c if c.is_whitespace() => 0,
            _ => 1 << 27,
        };
    }
    m
}

/// Is `q` (lowercase chars) an in-order subsequence of `name`? No allocation.
fn subsequence(q: &[char], name: &str) -> bool {
    let mut i = 0;
    for c in name.chars() {
        let c = if c.is_ascii() { c.to_ascii_lowercase() } else { c.to_lowercase().next().unwrap_or(c) };
        if c == q[i] {
            i += 1;
            if i == q.len() {
                return true;
            }
        }
    }
    false
}

/// Cheap first-pass score with no allocation: whole-query matches at the
/// start of the name or of a word beat matches elsewhere, which beat a
/// scattered subsequence. None if `q` isn't in `name` at all.
fn quick_score(q: &[char], qb: Option<&[u8]>, name: &str) -> Option<i32> {
    if let (Some(qb), true) = (qb, name.is_ascii()) {
        let nb = name.as_bytes();
        let pos = nb.windows(qb.len()).position(|w| w.eq_ignore_ascii_case(qb));
        let base = match pos {
            Some(0) => 300,
            Some(p) if !nb[p - 1].is_ascii_alphanumeric() => 200,
            Some(_) => 100,
            None if subsequence(q, name) => 0,
            None => return None,
        };
        return Some(base - (nb.len() as i32 / 4).min(30));
    }
    subsequence(q, name).then_some(0)
}

/// Every query word must appear compactly in the name: as a subsequence
/// spanning at most about twice its length. "brkt" fits "bracket", but
/// letters scattered across a long name don't count.
fn compact(words: &[Vec<char>], name: &str) -> bool {
    let n: Vec<char> = name.chars().map(|c| if c.is_ascii() { c.to_ascii_lowercase() } else { c.to_lowercase().next().unwrap_or(c) }).collect();
    words.iter().all(|w| {
        let limit = w.len() * 2 + 2;
        (0..n.len()).filter(|&s| n[s] == w[0]).any(|s| {
            let mut i = 0;
            for (j, &c) in n[s..].iter().enumerate() {
                if j >= limit {
                    return false;
                }
                if c == w[i] {
                    i += 1;
                    if i == w.len() {
                        return true;
                    }
                }
            }
            false
        })
    })
}

fn minutes(t: std::time::SystemTime) -> u32 {
    t.duration_since(std::time::UNIX_EPOCH).map(|d| (d.as_secs() / 60) as u32).unwrap_or(0)
}

fn now_minutes() -> u32 {
    minutes(std::time::SystemTime::now())
}

impl FileIndex {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    fn name(&self, e: &Entry) -> &str {
        &self.names[e.name_off as usize..e.name_off as usize + e.name_len as usize]
    }

    fn push(&mut self, parent: u32, name: &str, is_dir: bool, mtime: u32) -> u32 {
        let off = self.names.len() as u32;
        self.names.push_str(name);
        self.entries.push(Entry { parent, name_off: off, name_len: name.len() as u16, is_dir, mask: mask_of(name), mtime });
        (self.entries.len() - 1) as u32
    }

    /// Full path of entry `i`.
    pub fn path(&self, i: usize) -> PathBuf {
        let mut parts = Vec::new();
        let mut cur = i as u32;
        while cur != NO_PARENT {
            let e = &self.entries[cur as usize];
            parts.push(self.name(e));
            cur = e.parent;
        }
        let mut p = PathBuf::new();
        for part in parts.iter().rev() {
            p.push(part);
        }
        p
    }

    /// Walks `roots` (breadth-first, so shallow files are kept if the cap is hit).
    pub fn build(roots: &[PathBuf]) -> Self {
        let mut idx = FileIndex::default();
        let mut queue = std::collections::VecDeque::new();
        for r in roots {
            if !r.is_dir() || idx.roots.iter().any(|x| r.starts_with(x)) {
                continue; // missing, or already covered by an earlier root
            }
            let id = idx.push(NO_PARENT, &r.to_string_lossy(), true, 0);
            idx.roots.push(r.clone());
            queue.push_back((id, r.clone(), 0usize));
        }
        while let Some((dir_id, dir, depth)) = queue.pop_front() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for de in rd.flatten() {
                if idx.entries.len() >= MAX_ENTRIES {
                    idx.truncated = true;
                    return idx;
                }
                let Ok(md) = de.metadata() else { continue };
                let name = de.file_name();
                let name = name.to_string_lossy();
                if !keep(&name, &md, depth == 0) {
                    continue;
                }
                let is_dir = md.is_dir();
                let mtime = md.modified().map(minutes).unwrap_or(0);
                let id = idx.push(dir_id, &name, is_dir, mtime);
                if is_dir && depth + 1 < MAX_DEPTH {
                    queue.push_back((id, de.path(), depth + 1));
                }
            }
        }
        idx.entries.shrink_to_fit();
        idx.names.shrink_to_fit();
        idx
    }

    /// Best `limit` matches for `q`, best first. Prefers names over folders,
    /// recently changed files, and shallower paths.
    pub fn search(&self, q: &Query, limit: usize) -> Vec<Hit> {
        let qc = q.chars();
        if qc.is_empty() || limit == 0 {
            return Vec::new();
        }
        let qstr: String = qc.iter().collect();
        let qmask = mask_of(&qstr);
        let qb = qstr.is_ascii().then(|| qstr.as_bytes());
        let now = now_minutes();
        let recency = |e: &Entry| match now.saturating_sub(e.mtime) / 1440 {
            0..=1 => 12,
            2..=7 => 8,
            8..=30 => 4,
            _ => 0,
        };

        // Pass 1: cheap filter and ranking over everything, keeping a shortlist.
        let keep_n = (limit * 6).max(120);
        let mut short: Vec<(i32, u32)> = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            if e.parent == NO_PARENT || e.mask & qmask != qmask || (e.name_len as usize) < qc.len() {
                continue;
            }
            let Some(s) = quick_score(qc, qb, self.name(e)) else { continue };
            push_top(&mut short, keep_n, s + recency(e), i as u32);
        }

        // Pass 2: the full fuzzy score on the shortlist only. Letters
        // scattered thinly through a long name are noise, not a match.
        let words: Vec<Vec<char>> = q.words().map(|w| w.chars().collect()).collect();
        let mut best: Vec<(i32, u32)> = Vec::new();
        for (_, i) in short {
            let e = &self.entries[i as usize];
            let name = self.name(e);
            if !compact(&words, name) {
                continue;
            }
            if let Some(s) = q.score(name) {
                push_top(&mut best, limit, s + recency(e), i);
            }
        }
        best.into_iter()
            .map(|(score, i)| {
                let e = &self.entries[i as usize];
                Hit { path: self.path(i as usize), name: self.name(e).to_string(), is_dir: e.is_dir, score }
            })
            .collect()
    }
}

/// Inserts into a best-first list capped at `n`.
fn push_top(list: &mut Vec<(i32, u32)>, n: usize, s: i32, i: u32) {
    if list.len() == n && list.last().is_some_and(|l| l.0 >= s) {
        return;
    }
    let pos = list.partition_point(|x| x.0 >= s);
    list.insert(pos, (s, i));
    list.truncate(n);
}

/// Whether to index an entry: no hidden/system items, dot-folders, caches,
/// or the profile's AppData.
fn keep(name: &str, md: &std::fs::Metadata, at_root: bool) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    const TEMPORARY: u32 = 0x100;
    let a = md.file_attributes();
    if a & (HIDDEN | SYSTEM | TEMPORARY) != 0 || name.starts_with('.') || name.starts_with('~') {
        return false;
    }
    if md.is_dir() {
        let lower = name.to_ascii_lowercase();
        if SKIP_DIRS.contains(&lower.as_str()) || (at_root && lower == "appdata") {
            return false;
        }
    }
    true
}

/// Folders indexed by default: the user profile (Desktop, Documents,
/// Downloads, OneDrive, …), plus any extra folders from settings.
pub fn default_roots(extra: &[String]) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE") {
        roots.push(PathBuf::from(home));
    }
    for e in extra {
        let p = PathBuf::from(e.trim());
        if !e.trim().is_empty() && !roots.contains(&p) {
            roots.push(p);
        }
    }
    roots
}

/// Shortens a folder for display: the profile becomes "~".
pub fn display_dir(path: &Path) -> String {
    let dir = path.parent().unwrap_or(path).to_string_lossy().to_string();
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let home = home.to_string_lossy().to_string();
        if let Some(rest) = dir.strip_prefix(&home) {
            return format!("~{rest}");
        }
    }
    dir
}

/// Fluent icon glyph for a file, by extension.
pub fn glyph_for(path: &Path, is_dir: bool) -> char {
    if is_dir {
        return '\u{E8B7}';
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "pdf" => '\u{EA90}',
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "heic" | "tif" | "tiff" | "ico" => '\u{EB9F}',
        "mp4" | "mov" | "mkv" | "avi" | "webm" => '\u{E714}',
        "mp3" | "wav" | "flac" | "m4a" | "ogg" => '\u{E8D6}',
        "zip" | "7z" | "rar" | "tar" | "gz" => '\u{E7B8}',
        "xlsx" | "xls" | "csv" => '\u{E80A}',
        "exe" | "msi" | "bat" | "cmd" | "ps1" => '\u{E756}',
        "rs" | "py" | "js" | "ts" | "c" | "cpp" | "h" | "cs" | "java" | "m" | "json" | "toml" | "html" | "css" => '\u{E943}',
        // CAD, 3D printing and engineering formats.
        "sldprt" | "sldasm" | "slddrw" | "step" | "stp" | "iges" | "igs" | "stl" | "3mf" | "obj" | "dwg" | "dxf"
        | "ipt" | "iam" | "idw" | "f3d" | "gcode" => '\u{F158}',
        _ => '\u{E8A5}',
    }
}

// ---------------------------------------------------------------------------
// Windows' Recent Files list.

/// A recently opened file, newest first.
#[derive(Clone, Debug)]
pub struct Recent {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
}

/// Reads `%APPDATA%\Microsoft\Windows\Recent` (shortcuts Windows makes as
/// files are opened) and resolves the newest `max` that still exist. COM must
/// be initialised on the calling thread.
pub fn recent_files(max: usize) -> Vec<Recent> {
    let Some(appdata) = std::env::var_os("APPDATA") else { return Vec::new() };
    let dir = PathBuf::from(appdata).join("Microsoft\\Windows\\Recent");
    let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut links: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    links.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out = Vec::new();
    for (_, lnk) in links {
        if out.len() >= max {
            break;
        }
        let Some(target) = resolve_link(&lnk) else { continue };
        let Ok(md) = std::fs::metadata(&target) else { continue };
        let name = target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if name.is_empty() || out.iter().any(|r: &Recent| r.path == target) {
            continue;
        }
        out.push(Recent { is_dir: md.is_dir(), path: target, name });
    }
    out
}

/// Target path of a .lnk shortcut (no UI, no searching for moved targets).
fn resolve_link(lnk: &Path) -> Option<PathBuf> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{Interface, PCWSTR};
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let pf: IPersistFile = link.cast().ok()?;
        let w = crate::util::wide(&lnk.to_string_lossy());
        pf.Load(PCWSTR(w.as_ptr()), STGM_READ).ok()?;
        let mut buf = [0u16; 1024];
        link.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
        let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
        (n > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> PathBuf {
        let root = std::env::temp_dir().join(format!("sigma5-files-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["Projects/Bracket v2", "Projects/node_modules/junk", "Uni/Thermo", ".git/objects"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in [
            "Projects/Bracket v2/bracket_mount.SLDPRT",
            "Projects/Bracket v2/Bracket Drawing.pdf",
            "Projects/node_modules/junk/index.js",
            "Uni/Thermo/Lab Report 3.docx",
            "Uni/Thermo/steam_tables.xlsx",
            ".git/objects/abc",
            "notes.txt",
        ] {
            std::fs::write(root.join(f), b"x").unwrap();
        }
        root
    }

    #[test]
    fn indexes_and_finds_files() {
        let root = tree();
        let idx = FileIndex::build(&[root.clone()]);
        // Root + Projects, Bracket v2, 2 files, Uni, Thermo, 2 files, notes.txt.
        assert_eq!(idx.len(), 10, "skips node_modules and dot-folders");
        let hits = idx.search(&Query::new("brkt mount"), 5);
        assert_eq!(hits[0].name, "bracket_mount.SLDPRT");
        assert_eq!(hits[0].path, root.join("Projects/Bracket v2/bracket_mount.SLDPRT"));
        let hits = idx.search(&Query::new("lab rep"), 5);
        assert_eq!(hits[0].name, "Lab Report 3.docx");
        assert!(idx.search(&Query::new("index.js"), 5).is_empty());
        assert!(idx.search(&Query::new("thermo"), 5).iter().any(|h| h.is_dir && h.name == "Thermo"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn masks_and_subsequence() {
        assert_eq!(mask_of("ab"), 0b11);
        assert_eq!(mask_of("A1-"), 1 | 1 << 26 | 1 << 27);
        let q: Vec<char> = "brm".chars().collect();
        assert!(subsequence(&q, "Bracket_Mount"));
        assert!(!subsequence(&q, "mount bracket"));
        let w = |s: &str| s.split(' ').map(|x| x.chars().collect()).collect::<Vec<Vec<char>>>();
        assert!(compact(&w("brkt mount"), "bracket_mount.SLDPRT"));
        assert!(compact(&w("eng draw"), "EngDraw2016.pdf"));
        assert!(!compact(&w("cargo toml"), "Chapter-22---Geometrical-Tolerancing-and-Dat_2012_Manual-of-Engineering-Draw.pdf"));
    }

    #[test]
    fn glyphs_and_display() {
        assert_eq!(glyph_for(Path::new("a/part.SLDPRT"), false), '\u{F158}');
        assert_eq!(glyph_for(Path::new("a/b"), true), '\u{E8B7}');
        if let Some(home) = std::env::var_os("USERPROFILE") {
            let p = PathBuf::from(home).join("Documents").join("x.pdf");
            assert_eq!(display_dir(&p), "~\\Documents");
        }
    }

    /// Indexes the real user folder (read-only) and times it:
    /// `cargo test --release files_real -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn files_real() {
        let t = std::time::Instant::now();
        let idx = FileIndex::build(&default_roots(&[]));
        println!(
            "indexed {} entries in {:.2} s (truncated: {}), ~{} KB",
            idx.len(),
            t.elapsed().as_secs_f64(),
            idx.truncated,
            (idx.entries.capacity() * std::mem::size_of::<Entry>() + idx.names.capacity()) / 1024
        );
        for q in ["sigma", "pdf", "readme", "cargo toml", "eng draw", "lab"] {
            let t = std::time::Instant::now();
            let hits = idx.search(&Query::new(q), 5);
            let top: Vec<String> = hits.iter().map(|h| h.name.clone()).collect();
            println!("{q:>12}: {:.2} ms {:?}", t.elapsed().as_secs_f64() * 1000.0, top);
        }
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
        }
        let t = std::time::Instant::now();
        let r = recent_files(40);
        println!("{} recent files in {:.0} ms, newest: {:?}", r.len(), t.elapsed().as_secs_f64() * 1000.0, r.first().map(|x| &x.name));
    }

    /// Search speed over a synthetic 200k-entry index:
    /// `cargo test --release files_bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn files_bench() {
        let mut idx = FileIndex::default();
        let root = idx.push(NO_PARENT, "C:\\Users\\x", true, 0);
        let words = ["report", "bracket", "final", "draft", "thermo", "lab", "assembly", "photo", "invoice", "notes", "cad", "v2", "mount", "data"];
        let exts = ["pdf", "docx", "SLDPRT", "png", "xlsx", "txt", "step"];
        let mut seed = 1u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as usize
        };
        for _ in 0..200_000 {
            let name = format!("{} {} {}.{}", words[rnd() % words.len()], words[rnd() % words.len()], rnd() % 1000, exts[rnd() % exts.len()]);
            idx.push(root, &name, false, 0);
        }
        for q in ["b", "br", "brk", "lab rep", "thermo final pdf", "zzz"] {
            let t = std::time::Instant::now();
            let hits = idx.search(&Query::new(q), 50);
            println!("{q:>18}: {:>6.2} ms, {} hits", t.elapsed().as_secs_f64() * 1000.0, hits.len());
        }
        println!("memory: {} KB entries + {} KB names", idx.entries.capacity() * std::mem::size_of::<Entry>() / 1024, idx.names.capacity() / 1024);
    }
}
