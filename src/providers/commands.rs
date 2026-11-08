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

/// A static list of commands forming one module.
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
        Def { title: "Suggest a Feature", subtitle: "", keywords: "recommend idea request feedback suggestion wish improve", glyph: '\u{E82F}', cmd: Command::SuggestFeature },
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

pub const FILES: CommandsProvider = CommandsProvider {
    id: "files",
    section: "Files",
    defs: &[
        Def { title: "Search Files", subtitle: "Files", keywords: "find file folder document recent explorer open", glyph: '\u{E8B7}', cmd: Command::SearchFiles },
    ],
};

pub const COLORS: CommandsProvider = CommandsProvider {
    id: "colors",
    section: "Colours",
    defs: &[
        Def { title: "Pick Colour", subtitle: "Colours", keywords: "color picker eyedropper dropper hex rgb screen sample", glyph: '\u{EF3C}', cmd: Command::PickColor },
        Def { title: "Colour History", subtitle: "Colours", keywords: "color picked palette saved hex", glyph: '\u{E790}', cmd: Command::ColorHistory },
        Def { title: "Clear Colour History", subtitle: "Colours", keywords: "color delete wipe forget", glyph: '\u{E74D}', cmd: Command::ClearColors },
    ],
};

/// Every command list, for finding a command by its key (used by hotkeys).
const ALL: [&CommandsProvider; 5] = [&SIGMA5, &WINGET, &CLIPBOARD, &FILES, &COLORS];

/// A command from its history/hotkey key ("cmd:PickColor"): its module id,
/// title, glyph and the command itself.
pub fn find(k: &str) -> Option<(&'static str, &'static str, char, Command)> {
    ALL.iter().find_map(|p| p.defs.iter().find(|d| key(d.cmd) == k).map(|d| (p.id, d.title, d.glyph, d.cmd)))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_found_by_key() {
        let (module, title, _, cmd) = find("cmd:PickColor").unwrap();
        assert_eq!((module, title, cmd), ("colors", "Pick Colour", Command::PickColor));
        assert_eq!(find("cmd:SearchFiles").unwrap().3, Command::SearchFiles);
        assert!(find("cmd:Nope").is_none());
        assert!(find("Microsoft.WindowsCalculator").is_none());
    }
}
