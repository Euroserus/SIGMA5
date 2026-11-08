//! Fixed command lists: SIGMA5's own commands, WinGet, and Clipboard.

use super::{Action, Command, Icon, Item, Provider, SearchCtx};

pub struct Def {
    title: &'static str,
    /// Dimmed text after the title, e.g. "WinGet".
    subtitle: &'static str,
    keywords: &'static str,
    glyph: char,
    cmd: Command,
}

/// A static list of commands forming one extension.
pub struct CommandsProvider {
    id: &'static str,
    section: &'static str,
    defs: &'static [Def],
}

// Glyphs are Segoe Fluent Icons code points.
pub const SIGMA5: CommandsProvider = CommandsProvider {
    id: "commands",
    section: "SIGMA5",
    defs: &[
        Def { title: "Reload Applications", subtitle: "", keywords: "refresh reindex rescan apps", glyph: '\u{E72C}', cmd: Command::ReloadApps },
        Def { title: "SIGMA5 Settings", subtitle: "", keywords: "config preferences options hotkey theme", glyph: '\u{E713}', cmd: Command::OpenSettings },
        Def { title: "Check for Updates", subtitle: "", keywords: "update upgrade new version", glyph: '\u{E895}', cmd: Command::CheckUpdates },
        Def { title: "Open SIGMA5 Data Folder", subtitle: "", keywords: "appdata files history", glyph: '\u{E8B7}', cmd: Command::OpenDataFolder },
        Def { title: "Quit SIGMA5", subtitle: "", keywords: "exit close stop", glyph: '\u{E7E8}', cmd: Command::Quit },
    ],
};

pub const WINGET: CommandsProvider = CommandsProvider {
    id: "winget",
    section: "WinGet",
    defs: &[
        Def { title: "Search Packages", subtitle: "WinGet", keywords: "winget install find download app", glyph: '\u{E7B8}', cmd: Command::WingetSearch },
        Def { title: "Upgrade All Packages", subtitle: "WinGet", keywords: "winget update everything", glyph: '\u{E7B8}', cmd: Command::WingetUpgradeAll },
        Def { title: "Show Installed", subtitle: "WinGet", keywords: "winget list uninstall manage", glyph: '\u{E7B8}', cmd: Command::WingetInstalled },
        Def { title: "Export Packages", subtitle: "WinGet", keywords: "winget backup save json", glyph: '\u{E7B8}', cmd: Command::WingetExport },
        Def { title: "Import Packages", subtitle: "WinGet", keywords: "winget restore json", glyph: '\u{E7B8}', cmd: Command::WingetImport },
        Def { title: "Show Upgradable", subtitle: "WinGet", keywords: "winget updates outdated", glyph: '\u{E7B8}', cmd: Command::WingetUpgradable },
    ],
};

pub const CLIPBOARD: CommandsProvider = CommandsProvider {
    id: "clipboard",
    section: "Clipboard",
    defs: &[
        Def { title: "Clipboard History", subtitle: "Clipboard", keywords: "paste copied history", glyph: '\u{E77F}', cmd: Command::ClipboardHistory },
        Def { title: "Clear Clipboard History", subtitle: "Clipboard", keywords: "delete wipe forget", glyph: '\u{E74D}', cmd: Command::ClearClipboard },
    ],
};

fn key(cmd: Command) -> String {
    format!("cmd:{cmd:?}")
}

fn item(d: &Def, score: i32) -> Item {
    Item {
        key: key(d.cmd),
        title: d.title.into(),
        subtitle: d.subtitle.into(),
        accessory: "Command".into(),
        icon: Icon::Glyph(d.glyph),
        action: Action::Command(d.cmd),
        score,
    }
}

impl Provider for CommandsProvider {
    fn id(&self) -> &'static str {
        self.id
    }

    fn section(&self) -> &'static str {
        self.section
    }

    fn search(&self, ctx: &SearchCtx, out: &mut Vec<Item>) {
        for d in self.defs {
            let title = ctx.query.score(d.title);
            // "winget search" style queries match via the subtitle too.
            let full = (!d.subtitle.is_empty()).then(|| ctx.query.score(&format!("{} {}", d.subtitle, d.title))).flatten();
            let kw = d.keywords.split(' ').filter_map(|k| ctx.query.score(k)).max().map(|s| s - 30);
            if let Some(s) = title.max(full).max(kw) {
                // Slightly below apps so an app with the same name wins.
                out.push(item(d, s - 5 + ctx.history.boost(&key(d.cmd))));
            }
        }
    }

    fn browse(&self, out: &mut Vec<Item>) {
        out.extend(self.defs.iter().map(|d| item(d, 0)));
    }

    fn lookup(&self, k: &str) -> Option<Item> {
        self.defs.iter().find(|d| key(d.cmd) == k).map(|d| item(d, 0))
    }
}
