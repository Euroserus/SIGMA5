//! Providers are SIGMA5's "extensions": each one contributes result items for
//! a query and knows nothing about the UI. New features (web search, AI
//! answers, calculator, unit conversion, ...) are added by writing a new
//! provider, listing it in [`EXTENSIONS`], registering it in [`Registry`], and
//! adding an [`Action`] variant if they need new behaviour when activated.

pub mod apps;
pub mod commands;

use crate::fuzzy::Query;
use crate::history::History;

/// Metadata shown on the Settings > Extensions page.
pub struct ExtensionInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// Segoe Fluent Icons glyph.
    pub glyph: char,
}

pub const EXTENSIONS: &[ExtensionInfo] = &[
    ExtensionInfo {
        id: "apps",
        name: "Applications",
        description: "Search and open installed desktop and Store apps",
        glyph: '\u{E71D}',
    },
    ExtensionInfo {
        id: "calculator",
        name: "Calculator",
        description: "Maths and unit conversions as you type; Enter copies the answer",
        glyph: '\u{E8EF}',
    },
    ExtensionInfo {
        id: "clipboard",
        name: "Clipboard History",
        description: "Remembers what you copy; private content from password managers is skipped",
        glyph: '\u{E77F}',
    },
    ExtensionInfo {
        id: "winget",
        name: "WinGet",
        description: "Search, install, upgrade and remove packages",
        glyph: '\u{E7B8}',
    },
    ExtensionInfo {
        id: "commands",
        name: "SIGMA5 Commands",
        description: "Reload applications, open settings, quit",
        glyph: '\u{E945}',
    },
];

/// What a result row shows as its icon.
#[derive(Clone, Debug)]
pub enum Icon {
    /// Icon loaded from the shell for an installed app (keyed by app id).
    App(String),
    /// A Segoe Fluent Icons glyph drawn on an accent tile.
    Glyph(char),
}

/// Built-in commands handled by the launcher itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    ReloadApps,
    OpenSettings,
    OpenDataFolder,
    Quit,
    WingetSearch,
    WingetInstalled,
    WingetUpgradable,
    WingetUpgradeAll,
    WingetExport,
    WingetImport,
    ClipboardHistory,
    ClearClipboard,
    CheckUpdates,
}

/// What Enter does on a WinGet package.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WingetOp {
    Install,
    /// Installed package: Enter upgrades if an update exists, else uninstalls.
    Manage { has_update: bool },
    Upgrade,
    UpgradeAll,
}

/// What happens when a result is activated (Enter / click).
#[derive(Clone, Debug)]
pub enum Action {
    /// Launch an entry from the shell's Apps folder (Win32 or Store app).
    LaunchApp { id: String },
    Command(Command),
    Winget { op: WingetOp, id: String, name: String },
    /// A clipboard history entry (by id).
    Clip(u64),
    /// Download and install the available SIGMA5 update.
    Update,
}

impl Action {
    /// Label shown next to the Enter hint in the footer.
    pub fn verb(&self) -> &'static str {
        match self {
            Action::LaunchApp { .. } => "Open Application",
            Action::Command(_) => "Run Command",
            Action::Winget { op: WingetOp::Install, .. } => "Install",
            Action::Winget { op: WingetOp::Manage { has_update: false }, .. } => "Uninstall",
            Action::Winget { op: WingetOp::Upgrade | WingetOp::Manage { .. }, .. } => "Upgrade",
            Action::Winget { op: WingetOp::UpgradeAll, .. } => "Upgrade All",
            Action::Clip(_) => "Paste",
            Action::Update => "Update Now",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    /// Stable key used for history/ranking, e.g. "app:<id>".
    pub key: String,
    pub title: String,
    /// Dimmed text shown after the title (optional).
    pub subtitle: String,
    /// Right-aligned type label, e.g. "Application".
    pub accessory: String,
    pub icon: Icon,
    pub action: Action,
    pub score: i32,
}

pub struct SearchCtx<'a> {
    pub query: &'a Query,
    pub history: &'a History,
}

pub trait Provider {
    /// Matches an `ExtensionInfo::id`.
    fn id(&self) -> &'static str;
    /// Section title used when browsing with an empty query.
    fn section(&self) -> &'static str;
    /// Append items matching `ctx.query`, with `score` set (higher is better).
    fn search(&self, ctx: &SearchCtx, out: &mut Vec<Item>);
    /// Append every item, in display order, for browsing with an empty query.
    fn browse(&self, out: &mut Vec<Item>);
    /// Rebuild an item from its history key (for the "Recent" section).
    fn lookup(&self, key: &str) -> Option<Item>;
}

/// A row in the result list.
#[derive(Clone)]
pub enum Row {
    Header(&'static str),
    Item(Item),
    /// The calculator's result card.
    Calc(Box<crate::calc::Calc>),
}

impl Row {
    /// Rows that can be selected and activated.
    pub fn selectable(&self) -> bool {
        !matches!(self, Row::Header(_))
    }
}

/// All providers, in the order their sections appear when browsing.
pub struct Registry {
    pub apps: apps::AppsProvider,
    pub commands: commands::CommandsProvider,
    pub winget: commands::CommandsProvider,
    pub clipboard: commands::CommandsProvider,
    /// Extension ids turned off in settings.
    pub disabled: Vec<String>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            apps: apps::AppsProvider::new(),
            commands: commands::SIGMA5,
            winget: commands::WINGET,
            clipboard: commands::CLIPBOARD,
            disabled: Vec::new(),
        }
    }

    fn enabled(&self) -> Vec<&dyn Provider> {
        let all: [&dyn Provider; 4] = [&self.apps, &self.clipboard, &self.winget, &self.commands];
        all.into_iter().filter(|p| !self.disabled.iter().any(|d| d == p.id())).collect()
    }

    /// Builds the rows for a query. Empty query → Recent + browse sections.
    pub fn rows(&self, query: &Query, history: &History, recent_count: usize) -> Vec<Row> {
        let providers = self.enabled();
        let mut rows = Vec::new();
        if query.is_empty() {
            let mut recent_keys = Vec::new();
            let recent: Vec<Item> = history
                .top(recent_count * 2)
                .into_iter()
                .filter_map(|k| providers.iter().find_map(|p| p.lookup(&k)))
                .take(recent_count)
                .collect();
            if !recent.is_empty() {
                rows.push(Row::Header("Recent"));
                for it in recent {
                    recent_keys.push(it.key.clone());
                    rows.push(Row::Item(it));
                }
            }
            for p in &providers {
                let mut items = Vec::new();
                p.browse(&mut items);
                items.retain(|i| !recent_keys.contains(&i.key));
                if !items.is_empty() {
                    rows.push(Row::Header(p.section()));
                    rows.extend(items.into_iter().map(Row::Item));
                }
            }
        } else {
            let ctx = SearchCtx { query, history };
            let mut items = Vec::new();
            for p in &providers {
                p.search(&ctx, &mut items);
            }
            items.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.title.cmp(&b.title)));
            items.truncate(60);
            if !items.is_empty() {
                rows.push(Row::Header("Results"));
                rows.extend(items.into_iter().map(Row::Item));
            }
        }
        rows
    }
}
