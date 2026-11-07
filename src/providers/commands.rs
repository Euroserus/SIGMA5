//! SIGMA5's own commands (reload, settings, quit).

use super::{Action, Command, Icon, Item, Provider, SearchCtx};

struct Def {
    title: &'static str,
    keywords: &'static str,
    glyph: char,
    cmd: Command,
}

// Glyphs are Segoe Fluent Icons code points.
const COMMANDS: &[Def] = &[
    Def { title: "Reload Applications", keywords: "refresh reindex rescan apps", glyph: '\u{E72C}', cmd: Command::ReloadApps },
    Def { title: "SIGMA5 Settings", keywords: "config preferences options hotkey theme", glyph: '\u{E713}', cmd: Command::OpenSettings },
    Def { title: "Open SIGMA5 Data Folder", keywords: "appdata files history", glyph: '\u{E8B7}', cmd: Command::OpenDataFolder },
    Def { title: "Quit SIGMA5", keywords: "exit close stop", glyph: '\u{E7E8}', cmd: Command::Quit },
];

pub struct CommandsProvider;

fn key(cmd: Command) -> String {
    format!("cmd:{cmd:?}")
}

fn item(d: &Def, score: i32) -> Item {
    Item {
        key: key(d.cmd),
        title: d.title.into(),
        subtitle: String::new(),
        accessory: "Command",
        icon: Icon::Glyph(d.glyph),
        action: Action::Command(d.cmd),
        score,
    }
}

impl Provider for CommandsProvider {
    fn id(&self) -> &'static str {
        "commands"
    }

    fn section(&self) -> &'static str {
        "SIGMA5"
    }

    fn search(&self, ctx: &SearchCtx, out: &mut Vec<Item>) {
        for d in COMMANDS {
            let title = ctx.query.score(d.title);
            let kw = d
                .keywords
                .split(' ')
                .filter_map(|k| ctx.query.score(k))
                .max()
                .map(|s| s - 30);
            if let Some(s) = title.max(kw) {
                // Slightly below apps so an app with the same name wins.
                out.push(item(d, s - 5 + ctx.history.boost(&key(d.cmd))));
            }
        }
    }

    fn browse(&self, out: &mut Vec<Item>) {
        out.extend(COMMANDS.iter().map(|d| item(d, 0)));
    }

    fn lookup(&self, k: &str) -> Option<Item> {
        COMMANDS.iter().find(|d| key(d.cmd) == k).map(|d| item(d, 0))
    }
}
