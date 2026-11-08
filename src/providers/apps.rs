//! Installed applications.
//!
//! Enumerates the shell's Apps folder (`shell:AppsFolder`), the same list the
//! Start menu's "All apps" uses. That covers classic Win32 shortcuts and
//! Microsoft Store/UWP apps alike, and each entry can be launched with
//! `shell:AppsFolder\<id>`. Indexing and icon extraction run on a worker
//! thread and post their results back to the UI window.

use std::collections::HashSet;

use windows::Win32::Foundation::{LPARAM, SIZE, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize};
use windows::Win32::Storage::EnhancedStorage::PKEY_Link_TargetParsingPath;
use windows::Win32::UI::Shell::{
    BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, IShellItem2, IShellItemImageFactory,
    KF_FLAG_DEFAULT, SHGetKnownFolderItem, SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING,
    SIIGBF_ICONONLY,
};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::Interface;

use super::{Action, Icon, Item, Provider, SearchCtx};
use crate::ui::icons::{IconPixels, hbitmap_to_pixels};
use crate::ui::{WM_APP_ICONS, WM_APP_INDEX};
use crate::util::{SendHwnd, data_dir, take_pwstr};

#[derive(Clone, Debug)]
pub struct AppEntry {
    pub name: String,
    /// Parsing name inside the Apps folder (AppUserModelID or path-like id).
    pub id: String,
    /// Extra searchable word, e.g. the exe name ("code" for VS Code).
    pub alias: Option<String>,
    /// Lowercase exe file name of a desktop app (used to find its windows).
    pub exe: Option<String>,
}

impl AppEntry {
    fn new(name: String, id: String, target: Option<&str>) -> Self {
        let alias = alias_for(&id, &name);
        let exe = exe_name(target.unwrap_or(&id));
        Self { name, id, alias, exe }
    }
}

/// "C:\...\Code.exe" → "code.exe".
fn exe_name(path: &str) -> Option<String> {
    let last = path.rsplit(['\\', '/']).next()?.to_ascii_lowercase();
    last.ends_with(".exe").then_some(last)
}

fn alias_for(id: &str, name: &str) -> Option<String> {
    let last = id.rsplit(['\\', '/']).next()?;
    let lower = last.to_ascii_lowercase();
    let stem = lower.strip_suffix(".exe")?;
    if stem.len() < 2 || name.to_lowercase().contains(stem) {
        return None;
    }
    Some(stem.to_string())
}

/// Entries that clutter results without being apps people launch.
fn is_noise(name: &str, id: &str) -> bool {
    let n = name.to_lowercase();
    let i = id.to_lowercase();
    if n.starts_with("uninstall") || n.contains(" uninstall") || n.starts_with("remove ") {
        return true;
    }
    if i.starts_with("http:") || i.starts_with("https:") {
        return true;
    }
    const DOC_EXT: &[&str] = &[".url", ".txt", ".htm", ".html", ".chm", ".pdf", ".rtf", ".md", ".ini", ".log", ".xml", ".hlp"];
    DOC_EXT.iter().any(|e| i.ends_with(e))
}

pub struct AppsProvider {
    apps: Vec<AppEntry>,
}

fn key(id: &str) -> String {
    format!("app:{id}")
}

fn item(a: &AppEntry, score: i32) -> Item {
    Item {
        key: key(&a.id),
        title: a.name.clone(),
        subtitle: String::new(),
        accessory: "Application".into(),
        icon: Icon::App(a.id.clone()),
        action: Action::LaunchApp { id: a.id.clone() },
        score,
    }
}

impl AppsProvider {
    pub fn new() -> Self {
        Self { apps: load_cache() }
    }

    pub fn len(&self) -> usize {
        self.apps.len()
    }

    pub fn find(&self, id: &str) -> Option<&AppEntry> {
        self.apps.iter().find(|a| a.id == id)
    }

    /// Start-menu name of the app with this exe ("code.exe" → "Visual Studio Code").
    pub fn name_for_exe(&self, exe: &str) -> Option<&str> {
        self.apps.iter().find(|a| a.exe.as_deref() == Some(exe)).map(|a| a.name.as_str())
    }

    pub fn set_entries(&mut self, apps: Vec<AppEntry>) {
        save_cache(&apps);
        self.apps = apps;
    }
}

impl Provider for AppsProvider {
    fn id(&self) -> &'static str {
        "apps"
    }

    fn section(&self) -> &'static str {
        "Applications"
    }

    fn search(&self, ctx: &SearchCtx, out: &mut Vec<Item>) {
        for a in &self.apps {
            let by_name = ctx.query.score(&a.name);
            let by_alias = a.alias.as_deref().and_then(|s| ctx.query.score(s)).map(|s| s - 20);
            if let Some(s) = by_name.max(by_alias) {
                out.push(item(a, s + ctx.history.boost(&key(&a.id))));
            }
        }
    }

    fn browse(&self, out: &mut Vec<Item>) {
        out.extend(self.apps.iter().map(|a| item(a, 0)));
    }

    fn lookup(&self, k: &str) -> Option<Item> {
        let id = k.strip_prefix("app:")?;
        self.apps.iter().find(|a| a.id == id).map(|a| item(a, 0))
    }
}

// ---------------------------------------------------------------------------
// Disk cache so the list is populated instantly at startup.

fn cache_path() -> std::path::PathBuf {
    data_dir().join("apps.cache")
}

fn load_cache() -> Vec<AppEntry> {
    let Ok(text) = std::fs::read_to_string(cache_path()) else { return Vec::new() };
    // name \t id [\t exe]
    text.lines()
        .filter_map(|l| {
            let mut it = l.split('\t');
            let (n, i) = (it.next()?, it.next()?);
            let mut e = AppEntry::new(n.to_string(), i.to_string(), None);
            if let Some(exe) = it.next().filter(|x| !x.is_empty()) {
                e.exe = Some(exe.to_string());
            }
            Some(e)
        })
        .collect()
}

fn save_cache(apps: &[AppEntry]) {
    let mut s = String::with_capacity(apps.len() * 80);
    for a in apps {
        s.push_str(&a.name);
        s.push('\t');
        s.push_str(&a.id);
        s.push('\t');
        s.push_str(a.exe.as_deref().unwrap_or(""));
        s.push('\n');
    }
    let _ = std::fs::write(cache_path(), s);
}

// ---------------------------------------------------------------------------
// Background indexer.

/// Index result posted to the UI as a boxed pointer in LPARAM.
pub struct IndexResult {
    pub apps: Vec<AppEntry>,
}

/// Spawns the indexer. Icons for ids in `have_icons` are not re-extracted.
pub fn spawn_indexer(hwnd: SendHwnd, icon_px: i32, have_icons: HashSet<String>) {
    std::thread::Builder::new()
        .name("sigma5-indexer".into())
        .spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
            let found = enumerate_apps();
            let (apps, items): (Vec<AppEntry>, Vec<IShellItem>) = found.into_iter().unzip();
            post_box(hwnd, WM_APP_INDEX, IndexResult { apps: apps.clone() });

            // Icons, sent in small batches so the list fills in progressively.
            let mut batch: Vec<IconPixels> = Vec::new();
            for (app, si) in apps.iter().zip(items.iter()) {
                if have_icons.contains(&app.id) {
                    continue;
                }
                if let Some(px) = extract_icon(si, icon_px) {
                    batch.push(IconPixels { id: app.id.clone(), ..px });
                }
                if batch.len() >= 24 {
                    post_box(hwnd, WM_APP_ICONS, std::mem::take(&mut batch));
                }
            }
            if !batch.is_empty() {
                post_box(hwnd, WM_APP_ICONS, batch);
            }
            drop(items);
            CoUninitialize();
        })
        .ok();
}

fn post_box<T>(hwnd: SendHwnd, msg: u32, value: T) {
    let raw = Box::into_raw(Box::new(value));
    unsafe {
        if PostMessageW(Some(hwnd.get()), msg, WPARAM(0), LPARAM(raw as isize)).is_err() {
            drop(Box::from_raw(raw));
        }
    }
}

unsafe fn enumerate_apps() -> Vec<(AppEntry, IShellItem)> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let Ok(folder) = (unsafe { SHGetKnownFolderItem::<IShellItem>(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None) }) else {
        return out;
    };
    let Ok(en) = (unsafe { folder.BindToHandler::<_, IEnumShellItems>(None, &BHID_EnumItems) }) else {
        return out;
    };
    loop {
        let mut slot = [None];
        let mut fetched = 0u32;
        if unsafe { en.Next(&mut slot, Some(&mut fetched)) }.is_err() || fetched == 0 {
            break;
        }
        let Some(si) = slot[0].take() else { break };
        let name = unsafe { si.GetDisplayName(SIGDN_NORMALDISPLAY) }.map(|p| unsafe { take_pwstr(p) }).unwrap_or_default();
        let id = unsafe { si.GetDisplayName(SIGDN_PARENTRELATIVEPARSING) }.map(|p| unsafe { take_pwstr(p) }).unwrap_or_default();
        let name = name.trim().to_string();
        if name.is_empty() || id.is_empty() || is_noise(&name, &id) {
            continue;
        }
        if !seen.insert(name.to_lowercase()) {
            continue;
        }
        // Where a desktop app's shortcut points, used to find its windows later.
        let target = si
            .cast::<IShellItem2>()
            .ok()
            .and_then(|s2| unsafe { s2.GetString(&PKEY_Link_TargetParsingPath) }.ok())
            .map(|p| unsafe { take_pwstr(p) });
        out.push((AppEntry::new(name, id, target.as_deref()), si));
    }
    out.sort_by(|a, b| a.0.name.to_lowercase().cmp(&b.0.name.to_lowercase()));
    out
}

unsafe fn extract_icon(si: &IShellItem, px: i32) -> Option<IconPixels> {
    let factory: IShellItemImageFactory = si.cast().ok()?;
    let hbmp = unsafe { factory.GetImage(SIZE { cx: px, cy: px }, SIIGBF_ICONONLY) }.ok()?;
    let res = unsafe { hbitmap_to_pixels(hbmp) };
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::DeleteObject(hbmp.into());
    }
    res
}
