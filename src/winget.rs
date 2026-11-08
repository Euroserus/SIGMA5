//! WinGet integration: search, list installed, list upgradable, and run
//! install / upgrade / uninstall / export / import.
//!
//! Queries run `winget` hidden on a worker thread and parse its fixed-width
//! table output. Actions open a console window so you can watch progress
//! (and answer any UAC prompt), exactly as if you'd typed the command.

use std::os::windows::process::CommandExt;
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub id: String,
    pub version: String,
    /// Newer version available (list/upgrade), or empty.
    pub available: String,
    pub source: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Query {
    Search,
    Installed,
    Upgradable,
}

/// Parses winget's table output (header, dashed rule, rows).
pub fn parse_table(output: &str) -> Vec<Package> {
    // Progress spinners overwrite the line with '\r'; keep what's left visible.
    let lines: Vec<Vec<char>> = output
        .split('\n')
        .map(|l| {
            let l = l.trim_end_matches('\r');
            l.rsplit('\r').next().unwrap_or(l).chars().collect()
        })
        .collect();
    let Some(rule) = lines.iter().position(|l| l.len() > 10 && l.iter().all(|&c| c == '-')) else {
        return Vec::new();
    };
    let Some(header) = rule.checked_sub(1).map(|i| &lines[i]) else { return Vec::new() };

    // Column starts = where each header word begins.
    let mut cols: Vec<(String, usize)> = Vec::new();
    let mut i = 0;
    while i < header.len() {
        if header[i] != ' ' && (i == 0 || header[i - 1] == ' ') {
            let start = i;
            while i < header.len() && header[i] != ' ' {
                i += 1;
            }
            cols.push((header[start..i].iter().collect::<String>().to_lowercase(), start));
        } else {
            i += 1;
        }
    }
    if cols.len() < 2 {
        return Vec::new();
    }
    // Localised headers: fall back to positions (Name, Id, Version, …).
    let find = |names: &[&str], pos: usize| -> Option<usize> {
        cols.iter().position(|(n, _)| names.contains(&n.as_str())).or((pos < cols.len()).then_some(pos))
    };
    let (ci_name, ci_id, ci_ver) = (find(&["name"], 0), find(&["id"], 1), find(&["version"], 2));
    let ci_avail = cols.iter().position(|(n, _)| n == "available");
    let ci_src = cols.iter().position(|(n, _)| n == "source");

    let field = |row: &[char], ci: Option<usize>| -> String {
        let Some(ci) = ci else { return String::new() };
        let start = cols[ci].1;
        let end = cols.get(ci + 1).map_or(row.len(), |c| c.1).min(row.len());
        if start >= end {
            return String::new();
        }
        row[start..end].iter().collect::<String>().trim().to_string()
    };

    let mut out = Vec::new();
    for row in &lines[rule + 1..] {
        // Rows reach at least the Id column; summaries ("18 upgrades available.") don't.
        if row.iter().all(|c| c.is_whitespace()) || row.len() <= cols[1].1 {
            break;
        }
        let p = Package {
            name: field(row, ci_name),
            id: field(row, ci_id),
            version: field(row, ci_ver),
            available: field(row, ci_avail),
            source: field(row, ci_src),
        };
        if !p.id.is_empty() {
            out.push(p);
        }
    }
    out
}

/// Runs a query (blocking; call from a worker thread).
pub fn query(kind: Query, text: &str) -> Result<Vec<Package>, String> {
    let mut args: Vec<String> = match kind {
        Query::Search => vec!["search".into(), "--query".into(), text.into(), "--count".into(), "50".into()],
        Query::Installed => vec!["list".into()],
        Query::Upgradable => vec!["upgrade".into()],
    };
    args.extend(["--accept-source-agreements".into(), "--disable-interactivity".into()]);
    let out = Command::new("winget")
        .args(&args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|_| "WinGet isn't available. Install \"App Installer\" from the Microsoft Store.".to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let pkgs = parse_table(&text);
    if pkgs.is_empty() && !out.status.success() && kind != Query::Search {
        let msg = text.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or("WinGet failed").to_string();
        return Err(msg);
    }
    Ok(pkgs)
}

/// Ids go into a command line, so only allow characters winget ids use.
pub fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_alphanumeric() || "._+-\\{}~ ".contains(c))
}

fn safe_label(s: &str) -> String {
    s.chars().filter(|c| !"\"&|<>^%!()".contains(*c)).collect()
}

/// Runs a script in a new console window that stays open at the end.
fn console(title: &str, script: &str) {
    let full = format!("title {} & {} & echo. & pause", safe_label(title), script);
    let _ = Command::new("cmd").args(["/c", &full]).creation_flags(CREATE_NEW_CONSOLE).spawn();
}

const AGREE: &str = "--accept-package-agreements --accept-source-agreements";

pub fn install(id: &str, name: &str) {
    if safe_id(id) {
        console(&format!("Installing {name}"), &format!("winget install --id \"{id}\" --exact {AGREE}"));
    }
}

pub fn upgrade(id: &str, name: &str) {
    if safe_id(id) {
        console(&format!("Upgrading {name}"), &format!("winget upgrade --id \"{id}\" --exact {AGREE}"));
    }
}

pub fn upgrade_all() {
    console("Upgrading all packages", &format!("winget upgrade --all {AGREE}"));
}

/// Asks in the console before uninstalling.
pub fn uninstall(id: &str, name: &str) {
    if safe_id(id) {
        let label = safe_label(name);
        console(
            &format!("Uninstall {label}"),
            &format!("choice /c YN /m \"Uninstall {label}\" & if errorlevel 2 (exit) else (winget uninstall --id \"{id}\" --exact)"),
        );
    }
}

/// Exports installed packages to Documents and selects the file in Explorer.
pub fn export() {
    let docs = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from).unwrap_or_default().join("Documents");
    let file = docs.join(format!("winget-packages-{}.json", crate::util::today()));
    let f = file.display();
    console("Exporting packages", &format!("winget export -o \"{f}\" --accept-source-agreements & explorer /select,\"{f}\""));
}

/// Picks a winget export file, then imports it (dialog runs on its own thread).
pub fn import() {
    let _ = std::thread::Builder::new().name("sigma5-import".into()).spawn(|| unsafe {
        use windows::Win32::System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
        };
        use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
        use windows::Win32::UI::Shell::{FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH};
        use windows::core::w;
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let picked = (|| -> Option<String> {
            let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            let filters = [COMDLG_FILTERSPEC { pszName: w!("WinGet export (*.json)"), pszSpec: w!("*.json") }];
            dlg.SetFileTypes(&filters).ok()?;
            dlg.SetTitle(w!("Import WinGet packages")).ok()?;
            dlg.Show(None).ok()?;
            let item = dlg.GetResult().ok()?;
            Some(crate::util::take_pwstr(item.GetDisplayName(SIGDN_FILESYSPATH).ok()?))
        })();
        if let Some(path) = picked.filter(|p| !p.contains('"')) {
            console("Importing packages", &format!("winget import -i \"{path}\" {AGREE}"));
        }
        CoUninitialize();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = "Name                         Id                               Version      Match           Source\r\n\
-------------------------------------------------------------------------------------------------\r\n\
Microsoft Visual Studio Code Microsoft.VisualStudioCode       1.143.1      Moniker: vscode winget\r\n\
Codium                       Alex313031.Codium                1.93.1.24277 Tag: vscode     winget\r\n";

    const UPGRADE: &str = "   - \r   \\ \r\
Name                                             Id                                 Version              Available           Source\r\n\
-----------------------------------------------------------------------------------------------------------------------------------\r\n\
1Password CLI                                    AgileBits.1Password.CLI            2.41.1               2.42.0              winget\r\n\
Windows Terminal                                 Microsoft.WindowsTerminal          1.24.12741.0         1.25.2733.0         winget\r\n\
18 upgrades available.\r\n";

    /// A `winget list` table with real fixed-width columns.
    fn list_sample() -> String {
        let row = |n: &str, i: &str, v: &str, a: &str, s: &str| format!("{n:<25}{i:<62}{v:<16}{a:<10}{s}\r\n");
        let mut t = row("Name", "Id", "Version", "Available", "Source");
        t.push_str(&format!("{}\r\n", "-".repeat(120)));
        t.push_str(&row("1Password", "AgileBits.1Password", "> 8.12.42.36", "", "winget"));
        // ARP entries have no source, so the line ends early.
        t.push_str(row("Ahwoo", "ARP\\Machine\\X64\\{54AFD767-69AF-4220-9BAD-6C546413C6AB}_is1", "2026.10.17.1348", "", "").trim_end_matches([' ', '\r', '\n']));
        t.push_str("\r\n");
        t.push_str(&row("Zen Browser (x64 en-US)", "Zen-Team.Zen-Browser", "1.24.1b", "", "winget"));
        t
    }

    #[test]
    fn parses_search() {
        let p = parse_table(SEARCH);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].name, "Microsoft Visual Studio Code");
        assert_eq!(p[0].id, "Microsoft.VisualStudioCode");
        assert_eq!(p[0].version, "1.143.1");
        assert_eq!(p[0].source, "winget");
        assert_eq!(p[1].version, "1.93.1.24277");
    }

    #[test]
    fn parses_upgrade_with_spinner_and_summary() {
        let p = parse_table(UPGRADE);
        assert_eq!(p.len(), 2, "{p:?}");
        assert_eq!(p[0].available, "2.42.0");
        assert_eq!(p[1].name, "Windows Terminal");
    }

    #[test]
    fn parses_list_with_arp_ids_and_short_rows() {
        let p = parse_table(&list_sample());
        assert_eq!(p.len(), 3, "{p:?}");
        assert_eq!(p[0].version, "> 8.12.42.36");
        assert_eq!(p[1].id, "ARP\\Machine\\X64\\{54AFD767-69AF-4220-9BAD-6C546413C6AB}_is1");
        assert_eq!(p[1].source, "");
        assert_eq!(p[2].name, "Zen Browser (x64 en-US)");
    }

    #[test]
    fn ids_are_validated() {
        assert!(safe_id("Microsoft.VisualStudioCode"));
        assert!(safe_id("ARP\\Machine\\X64\\{54AFD767-69AF}_is1"));
        assert!(!safe_id("evil\" & del *"));
        assert!(!safe_id(""));
    }

    /// Parses real captured winget output: `SIGMA5_WG_DIR` holding
    /// wg_search.txt / wg_upgrade.txt / wg_list.txt.
    #[test]
    #[ignore]
    fn diag_parse_real_output() {
        let Ok(dir) = std::env::var("SIGMA5_WG_DIR") else { return };
        for (file, min) in [("wg_search.txt", 3), ("wg_upgrade.txt", 5), ("wg_list.txt", 50)] {
            let text = std::fs::read_to_string(std::path::Path::new(&dir).join(file)).unwrap();
            let p = parse_table(&text);
            println!("{file}: {} packages; first: {:?}; last: {:?}", p.len(), p.first(), p.last());
            assert!(p.len() >= min, "{file}");
            assert!(p.iter().all(|x| !x.id.is_empty() && !x.name.is_empty()), "{file}");
        }
    }

    #[test]
    fn empty_or_garbage() {
        assert!(parse_table("").is_empty());
        assert!(parse_table("No package found matching input criteria.").is_empty());
    }
}
