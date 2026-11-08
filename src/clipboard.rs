//! Clipboard history.
//!
//! Captures text, file lists and images whenever the clipboard changes
//! (WM_CLIPBOARDUPDATE, no polling). Content that apps mark as private, as
//! password managers do, is never recorded. Text and file entries are saved
//! encrypted with Windows DPAPI (readable only by your Windows account);
//! images are kept in memory only.

use std::path::PathBuf;

use windows::Win32::Foundation::{HANDLE, HGLOBAL, HLOCAL, HWND, LocalFree};
use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptProtectData, CryptUnprotectData};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardOwner, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
use windows::core::w;

use crate::util::{data_dir, now_unix};

const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
const CF_DIB: u32 = 8;
const MAX_ENTRIES: usize = 250;
const MAX_IMAGES: usize = 20;
const MAX_TEXT: usize = 1 << 20; // 1 MiB of UTF-16 read
const MAX_PERSIST_TEXT: usize = 256 * 1024;
const MAX_IMAGE_BYTES: usize = 40 << 20;
const MAGIC: &[u8] = b"S5CB1";

#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Text(String),
    Files(Vec<String>),
    /// A packed DIB (BITMAPINFOHEADER + pixels), as on the clipboard.
    Image { width: i32, height: i32, dib: Vec<u8>, hash: u64 },
}

impl Content {
    fn same(&self, o: &Content) -> bool {
        match (self, o) {
            (Content::Image { hash: a, .. }, Content::Image { hash: b, .. }) => a == b,
            _ => self == o,
        }
    }

    /// One-line summary for the list.
    pub fn title(&self) -> String {
        match self {
            Content::Text(t) => {
                let line = t.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
                let mut s: String = line.chars().take(140).collect();
                if s.is_empty() {
                    s = "(whitespace)".into();
                }
                s
            }
            Content::Files(f) if f.len() == 1 => file_name(&f[0]),
            Content::Files(f) => format!("{} files: {}", f.len(), f.iter().map(|p| file_name(p)).take(3).collect::<Vec<_>>().join(", ")),
            Content::Image { width, height, .. } => format!("Image {width} × {height}"),
        }
    }

    /// Text used when searching.
    pub fn search_text(&self) -> String {
        match self {
            Content::Text(t) => t.chars().take(400).collect(),
            Content::Files(f) => f.join(" "),
            Content::Image { .. } => "image picture screenshot".into(),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Content::Text(_) => "Text",
            Content::Files(_) => "Files",
            Content::Image { .. } => "Image",
        }
    }
}

fn file_name(p: &str) -> String {
    p.rsplit(['\\', '/']).next().unwrap_or(p).to_string()
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: u64,
    pub content: Content,
    /// Unix time it was (last) copied.
    pub copied: u64,
    /// Exe name of the app that put it on the clipboard, e.g. "chrome.exe".
    pub source: String,
}

pub struct History {
    entries: Vec<Entry>,
    next_id: u64,
    path: PathBuf,
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

impl History {
    pub fn load() -> Self {
        let path = data_dir().join("clipboard.dat");
        let entries = std::fs::read(&path).ok().and_then(|raw| unprotect(&raw)).map(|b| deserialize(&b)).unwrap_or_default();
        let next_id = entries.iter().map(|e| e.id).max().unwrap_or(0) + 1;
        Self { entries, next_id, path }
    }

    #[cfg(test)]
    fn in_memory() -> Self {
        Self { entries: Vec::new(), next_id: 1, path: std::env::temp_dir().join("sigma5-test-clip.dat") }
    }

    /// Newest first.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn get(&self, id: u64) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Adds content (or moves an identical entry to the top).
    pub fn add(&mut self, content: Content, source: String) {
        let id = match self.entries.iter().position(|e| e.content.same(&content)) {
            Some(i) => self.entries.remove(i).id,
            None => {
                self.next_id += 1;
                self.next_id
            }
        };
        self.entries.insert(0, Entry { id, content, copied: now_unix(), source });
        // Limits: total entries, and images (memory).
        let mut images = 0;
        self.entries.retain(|e| {
            if matches!(e.content, Content::Image { .. }) {
                images += 1;
                images <= MAX_IMAGES
            } else {
                true
            }
        });
        self.entries.truncate(MAX_ENTRIES);
        self.save();
    }

    pub fn remove(&mut self, id: u64) {
        self.entries.retain(|e| e.id != id);
        self.save();
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.save();
    }

    fn save(&self) {
        let bytes = serialize(&self.entries);
        if let Some(enc) = protect(&bytes) {
            let _ = std::fs::write(&self.path, enc);
        }
    }

    /// Records the current clipboard. Returns true if history changed.
    pub fn capture(&mut self, hwnd: HWND) -> bool {
        match unsafe { read_clipboard(hwnd) } {
            Some((content, source)) => {
                self.add(content, source);
                true
            }
            None => false,
        }
    }

    /// Puts an entry back on the clipboard (it moves to the top when the
    /// resulting clipboard update is captured).
    pub fn restore(&self, id: u64, hwnd: HWND) -> bool {
        let Some(e) = self.get(id) else { return false };
        match &e.content {
            Content::Text(t) => crate::util::copy_to_clipboard(hwnd, t),
            Content::Image { dib, .. } => unsafe { set_clipboard(hwnd, CF_DIB, dib) },
            Content::Files(f) => unsafe { set_clipboard(hwnd, CF_HDROP, &dropfiles(f)) },
        }
    }
}

// ---------------------------------------------------------------------------
// Clipboard access

/// Formats apps set to say "don't record this" (password managers etc.).
unsafe fn is_private() -> bool {
    unsafe {
        for name in [w!("ExcludeClipboardContentFromMonitorProcessing"), w!("Clipboard Viewer Ignore")] {
            let f = RegisterClipboardFormatW(name);
            if f != 0 && IsClipboardFormatAvailable(f).is_ok() {
                return true;
            }
        }
        // "CanIncludeInClipboardHistory" = 0 also means private.
        let f = RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory"));
        if f != 0 && IsClipboardFormatAvailable(f).is_ok() {
            if let Ok(h) = GetClipboardData(f) {
                let g = HGLOBAL(h.0);
                let p = GlobalLock(g) as *const u32;
                let v = if p.is_null() { 1 } else { *p };
                let _ = GlobalUnlock(g);
                if v == 0 {
                    return true;
                }
            }
        }
        false
    }
}

unsafe fn global_bytes(h: HANDLE, max: usize) -> Option<Vec<u8>> {
    unsafe {
        let g = HGLOBAL(h.0);
        let size = GlobalSize(g);
        if size == 0 || size > max {
            return None;
        }
        let p = GlobalLock(g) as *const u8;
        if p.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(p, size).to_vec();
        let _ = GlobalUnlock(g);
        Some(v)
    }
}

unsafe fn read_clipboard(hwnd: HWND) -> Option<(Content, String)> {
    unsafe {
        let source = source_app();
        // Another app may hold the clipboard briefly.
        let mut opened = false;
        for _ in 0..8 {
            if OpenClipboard(Some(hwnd)).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        if !opened {
            return None;
        }
        let content = (|| {
            if is_private() {
                return None;
            }
            if IsClipboardFormatAvailable(CF_HDROP).is_ok() {
                let h = GetClipboardData(CF_HDROP).ok()?;
                let drop = HDROP(h.0);
                let n = DragQueryFileW(drop, u32::MAX, None);
                let files: Vec<String> = (0..n)
                    .filter_map(|i| {
                        let len = DragQueryFileW(drop, i, None) as usize;
                        let mut buf = vec![0u16; len + 1];
                        let got = DragQueryFileW(drop, i, Some(&mut buf)) as usize;
                        (got > 0).then(|| String::from_utf16_lossy(&buf[..got]))
                    })
                    .collect();
                if !files.is_empty() {
                    return Some(Content::Files(files));
                }
            }
            if IsClipboardFormatAvailable(CF_UNICODETEXT).is_ok() {
                let h = GetClipboardData(CF_UNICODETEXT).ok()?;
                let bytes = global_bytes(h, MAX_TEXT * 2)?;
                let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
                let text = String::from_utf16_lossy(&units[..end]);
                if text.trim().is_empty() {
                    return None;
                }
                return Some(Content::Text(text));
            }
            if IsClipboardFormatAvailable(CF_DIB).is_ok() {
                let h = GetClipboardData(CF_DIB).ok()?;
                let dib = global_bytes(h, MAX_IMAGE_BYTES)?;
                if dib.len() < 40 {
                    return None;
                }
                let width = i32::from_le_bytes(dib[4..8].try_into().ok()?);
                let height = i32::from_le_bytes(dib[8..12].try_into().ok()?).abs();
                let hash = fnv(&dib);
                return Some(Content::Image { width, height, dib, hash });
            }
            None
        })();
        let _ = CloseClipboard();
        content.map(|c| (c, source))
    }
}

unsafe fn source_app() -> String {
    unsafe {
        let Ok(owner) = GetClipboardOwner() else { return String::new() };
        let mut pid = 0u32;
        GetWindowThreadProcessId(owner, Some(&mut pid));
        crate::util::process_exe_name(pid).unwrap_or_default()
    }
}

/// DROPFILES header + double-NUL-terminated wide paths.
fn dropfiles(files: &[String]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&20u32.to_le_bytes()); // pFiles
    v.extend_from_slice(&[0u8; 12]); // pt + fNC
    v.extend_from_slice(&1u32.to_le_bytes()); // fWide
    for f in files {
        for u in f.encode_utf16().chain([0]) {
            v.extend_from_slice(&u.to_le_bytes());
        }
    }
    v.extend_from_slice(&[0, 0]);
    v
}

unsafe fn set_clipboard(hwnd: HWND, format: u32, bytes: &[u8]) -> bool {
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return false;
        }
        let mut ok = false;
        if EmptyClipboard().is_ok() {
            if let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, bytes.len()) {
                let p = GlobalLock(mem) as *mut u8;
                if !p.is_null() {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
                    let _ = GlobalUnlock(mem);
                    ok = SetClipboardData(format, Some(HANDLE(mem.0))).is_ok();
                }
                if !ok {
                    let _ = windows::Win32::Foundation::GlobalFree(Some(mem));
                }
            }
        }
        let _ = CloseClipboard();
        ok
    }
}

// ---------------------------------------------------------------------------
// Persistence (text and files only), encrypted with DPAPI.

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u32).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn serialize(entries: &[Entry]) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    for e in entries {
        let (kind, payload) = match &e.content {
            Content::Text(t) if t.len() <= MAX_PERSIST_TEXT => (0u8, t.clone()),
            Content::Files(f) => (1u8, f.join("\n")),
            _ => continue,
        };
        out.push(kind);
        out.extend_from_slice(&e.id.to_le_bytes());
        out.extend_from_slice(&e.copied.to_le_bytes());
        put_str(&mut out, &e.source);
        put_str(&mut out, &payload);
    }
    out
}

fn deserialize(b: &[u8]) -> Vec<Entry> {
    let mut out = Vec::new();
    if !b.starts_with(MAGIC) {
        return out;
    }
    let mut i = MAGIC.len();
    let take = |i: &mut usize, n: usize| -> Option<&[u8]> {
        let s = b.get(*i..*i + n)?;
        *i += n;
        Some(s)
    };
    let take_str = |i: &mut usize| -> Option<String> {
        let len = u32::from_le_bytes(take(i, 4)?.try_into().ok()?) as usize;
        Some(String::from_utf8_lossy(take(i, len)?).into_owned())
    };
    while i < b.len() {
        let Some(entry) = (|| {
            let kind = *take(&mut i, 1)?.first()?;
            let id = u64::from_le_bytes(take(&mut i, 8)?.try_into().ok()?);
            let copied = u64::from_le_bytes(take(&mut i, 8)?.try_into().ok()?);
            let source = take_str(&mut i)?;
            let payload = take_str(&mut i)?;
            let content = match kind {
                0 => Content::Text(payload),
                1 => Content::Files(payload.split('\n').map(String::from).collect()),
                _ => return None,
            };
            Some(Entry { id, content, copied, source })
        })() else {
            break;
        };
        out.push(entry);
    }
    out
}

fn blob(b: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB { cbData: b.len() as u32, pbData: b.as_ptr() as *mut u8 }
}

fn take_blob(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    unsafe {
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut _)));
        v
    }
}

fn protect(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe { CryptProtectData(&blob(data), w!("SIGMA5 clipboard"), None, None, None, 0, &mut out).ok()? };
    Some(take_blob(out))
}

fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe { CryptUnprotectData(&blob(data), None, None, None, None, 0, &mut out).ok()? };
    Some(take_blob(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupe_moves_to_top() {
        let mut h = History::in_memory();
        h.add(Content::Text("a".into()), "x.exe".into());
        h.add(Content::Text("b".into()), "y.exe".into());
        h.add(Content::Text("a".into()), "z.exe".into());
        let titles: Vec<String> = h.entries().iter().map(|e| e.content.title()).collect();
        assert_eq!(titles, vec!["a", "b"]);
        assert_eq!(h.entries()[0].source, "z.exe");
    }

    #[test]
    fn round_trip_encrypted() {
        let entries = vec![
            Entry { id: 3, content: Content::Text("héllo\nworld".into()), copied: 100, source: "code.exe".into() },
            Entry { id: 2, content: Content::Files(vec!["C:\\a.txt".into(), "C:\\b c.png".into()]), copied: 50, source: String::new() },
            Entry { id: 1, content: Content::Image { width: 1, height: 1, dib: vec![0; 44], hash: 9 }, copied: 10, source: String::new() },
        ];
        let enc = protect(&serialize(&entries)).unwrap();
        assert!(!enc.windows(5).any(|w| w == b"world"), "must not be plaintext");
        let back = deserialize(&unprotect(&enc).unwrap());
        assert_eq!(back.len(), 2); // images aren't persisted
        assert_eq!(back[0].content, entries[0].content);
        assert_eq!(back[1].content, entries[1].content);
        assert_eq!(back[0].source, "code.exe");
    }

    #[test]
    fn titles() {
        assert_eq!(Content::Text("\n\n  first line  \nsecond".into()).title(), "first line");
        assert_eq!(Content::Files(vec!["C:\\x\\a.txt".into()]).title(), "a.txt");
        assert_eq!(Content::Image { width: 1920, height: 1080, dib: vec![], hash: 0 }.title(), "Image 1920 × 1080");
    }

    #[test]
    fn dropfiles_layout() {
        let d = dropfiles(&["C:\\a".into()]);
        assert_eq!(&d[0..4], &20u32.to_le_bytes());
        assert_eq!(&d[16..20], &1u32.to_le_bytes());
        assert!(d.ends_with(&[0, 0, 0, 0]));
    }
}
