<p align="center"><img src="assets/sigma5.png" width="96" alt="SIGMA5 logo"></p>

# SIGMA5 (S5)

**A fast launcher for engineers on Windows.** Open CAD, simulation and coding tools in a keystroke, do unit-aware maths without reaching for a calculator, and keep your machine's software up to date, all from one small window.

Built by a Mechanical Engineering student, for engineering students. Raycast-style, but written in Rust on raw Win32: no web view, no runtime, a ~1 MB exe, ~13 MB of RAM, and it opens in ~10 ms.

### Why engineers use it

- **Unit-aware calculator.** Type the maths, get the answer *with dimensions*: `3 kN * 0.5 m` → `1,500 J`, `200 psi to MPa`, `9.81 m/s^2 * 75 kg` → `735.75 N`, `100 rpm to rad/s`, `5 ft 10 in to cm`, `100 C to F`. It knows ~110 units across force, pressure/stress, energy/torque, power, temperature, rotation and more, and refuses physically meaningless maths (metres + seconds). Enter copies the answer and keeps it ready for the next step of the calculation.
- **Your tools, instantly.** Fuzzy search finds SOLIDWORKS (`sw`), Inventor, Fusion, MATLAB, ANSYS, Excel or VS Code from a couple of letters, ranking the ones you use most first.
- **One-key app switching.** Turn Caps Lock into a *Hyper* key and bind Caps+letter to any app: it launches it, brings it to the front, or hides it if it's already in front. It's great for flipping between CAD, a spreadsheet and your notes.
- **Software maintenance without the hassle.** Search, install, upgrade and remove packages through WinGet.
- **Clipboard history** for the numbers, part numbers and code snippets you keep re-copying, with private content from password managers skipped.
- **Key remaps** (numpad → media keys, etc.) and an optional Start-menu replacement on the Windows key.
- **File search** across your user folder (Desktop, Documents, OneDrive…) with typo-tolerant fuzzy matching (`brkt mount` finds `bracket_mount.SLDPRT`), plus your recent files.
- **Colour picker** with a magnifying loupe, colour history, and `#hex` / `rgb()` / `hsl()` conversion as you type.
- **Top bar** with a clock and live CPU, RAM, GPU, temperatures, battery, network and disk readings: pick and order them yourself.
- **Now Playing:** what Spotify (or any player) is playing, with album art and a live waveform in the top bar; click it for play, pause, skip and seek.
- **Greek letters anywhere, SMath style:** type `q`, press your Greek hotkey, get `θ`.
- **Typing Test** with timed and word-count runs, WPM, accuracy and personal bests, which also feeds a **Solar System** you build a planet at a time.
- **Make it yours:** square or rounded corners, fonts, text size, window and row size, and every colour.

### Releases

Each release is named after a famous engineer or researcher. The current release is **0.4 “Newton”**, after Isaac Newton, whose laws of motion and gravity keep the new Solar System's planets in orbit. Before it came 0.3 “Charpy”, after Georges Charpy (whose impact test still measures a material's toughness), and 0.2 “Euler”, after Leonhard Euler.

SIGMA5 updates itself: installed copies check GitHub every 12 hours and offer new versions in the launcher. One Enter downloads it, verifies its SHA-256 checksum and swaps it in, keeping your settings.

## Install

1. Download `sigma5.exe` from the [latest release](../../releases/latest) and run it.
2. Click **Install**. SIGMA5 copies itself to `%LOCALAPPDATA%\Programs\SIGMA5` (no admin rights needed), adds a Start menu shortcut, sets up launch at login, and appears in **Settings › Apps** so you can uninstall it normally. Or click **Just run it** to use it without installing.
3. Press `Alt+Space`. Open Settings with `Ctrl+,`.

**Updating:** automatic. When a new version is out, an *Update Available* banner appears at the top of the launcher; press Enter (Shift+Enter shows the release notes). You can also run **Check for Updates**, or use Settings › About. Turn off background checks there if you prefer.
**Uninstalling:** Settings › Apps › SIGMA5 › Uninstall (your settings in `%APPDATA%\SIGMA5` are kept).
**Command line:** `sigma5.exe --install` installs quietly, `--uninstall` removes it, `--portable` skips the installer.

The exe isn't code-signed, so Windows SmartScreen may say "Windows protected your PC" the first time. Click **More info → Run anyway**. Requires Windows 10 or 11 (64-bit).

## Usage

| Key | Action |
| --- | --- |
| `Alt+Space` | Open / close SIGMA5 (configurable) |
| type | Fuzzy-search apps and commands |
| `↑` `↓` / `Tab` / `Ctrl+N` `Ctrl+P` | Move selection |
| `PgUp` `PgDn` | Jump a page |
| `Enter` / click | Open the selected item |
| `Esc` | Clear the search, or close if it is empty |
| `Ctrl+Backspace` | Delete the previous word |
| `Ctrl+,` | Open Settings |
| `Win` (optional) | Tap the Windows key to open SIGMA5 instead of Start (Settings › General) |

Have an idea? Run *Suggest a Feature*, type it, and press Enter: WhatsApp opens a chat with SIGMA5's developer with your idea (and SIGMA5 version) typed in, ready to send. Without the WhatsApp app it opens WhatsApp Web instead.

An empty search shows **Recent** items, then every application, then SIGMA5's own commands (Reload Applications, Settings, Data Folder, Quit). There is also a tray icon with the same options.

You can move the launcher: drag it by the search bar (anywhere past the text) or the footer. It opens centred again next time.

### File search

Type a few letters of a file or folder name and the best matches appear under your results; *Search Files* opens a page with up to 60 matches, and shows your recent files when the search is empty.

- **Enter** opens it, **Shift+Enter** shows it in Explorer.
- Matching is fuzzy but strict about each word: `brkt mount`, `eng draw` and `lab rep` all work, while letters scattered through a long name don't count. Recently changed files rank higher.
- It indexes your user folder (skipping hidden and system items, AppData, `node_modules` and dot-folders) in the background and keeps it in memory: about 40 bytes per file, so 35,000 files is ~1.5 MB, and a search takes about a millisecond. Add more folders with `file_folders` in config.toml.

### Colour picker

*Pick Colour* shows a magnifying loupe that follows your cursor. Click to pick, arrow keys nudge by a pixel (Shift: ten), Esc cancels. The colour is copied as HEX and added to *Colour History*, where Enter copies HEX and Shift+Enter copies RGB. Typing `#ff8800`, `rgb(255, 136, 0)` or `hsl(32, 100%, 50%)` in the launcher shows the colour in all three formats.

### Top bar

A strip above the search box with a clock, the date and live readings: CPU, RAM and GPU use (with meters), CPU and GPU temperature, battery, network speed and free disk space. Choose and order them under Settings › Top Bar. Readings are taken once a second only while the launcher is open (about 2 ms each), so it costs nothing in the background. GPU temperature needs an NVIDIA driver; CPU temperature needs a PC that exposes an ACPI thermal zone, which many laptops don't. Unavailable readings show "—".

### Now Playing

Shows what's playing in Spotify, a browser, or any app that appears in Windows' media controls, read through Windows itself (no sign-in or web API). Add *Now playing* to the top bar under Settings › Top Bar: it shows the album art, a little waveform driven by the player's actual audio output, and the track. Click it (or search *Now Playing*) for the full card: art, track and album, progress (click to seek), previous / play-pause / next, and a button to bring the player forward. **Space** plays or pauses and **← →** skip. *Play/Pause*, *Next Track* and *Previous Track* are commands too, so they can have hotkeys. Windows doesn't expose a player's queue, so there is no queue view. Nothing runs while the launcher is closed.

### Greek letters

Set a hotkey under Settings › Keyboard › *Greek letter hotkey* (with Caps Lock as Hyper, Caps+G works well). Then, in any app, type a letter and press it: the letter becomes its Greek twin, using the same keys as SMath Studio and Mathcad (the classic Symbol font): `a b g d e z h q i k l m n x o p r s t u f c y w` → `α β γ δ ε ζ η θ ι κ λ μ ν ξ ο π ρ σ τ υ φ χ ψ ω`, capitals likewise, plus `j` → `ϕ` and `v` → `ϖ`. Press it again to turn it back. A mouse click or any other key in between cancels it, so it never deletes the wrong character. Search *Greek Letters* (or a name like `theta`) to copy one: Enter copies the small letter, Shift+Enter the capital.

### Typing Test

Search *Typing Test* and start typing: the clock starts on your first key. The length (15, 30, 60 or 120 seconds, or 10, 25, 50 or 100 words) is in the small menu above the words. Mistakes show in red as you go, and you can only go back into a word you got wrong. When it ends you get your WPM (from correctly typed words), accuracy and raw speed, and whether it's a personal best (runs under 75% accuracy don't count). **Tab** or **Enter** goes again, **Esc** goes back.

### Solar System

Every word you type correctly in a typing test becomes a particle of material, depending on the kind of word: little words (*the, of, and*) make **hydrogen**, nouns make **rock**, verbs make **water**, describing words make **iron**, and long words (8+ letters) make rare **carbon**. Only the first test each hour is harvested. When it ends, the words break into coloured pixels and fly off the sides of your screen (or into the Solar System's toolbar if it's open).

While the launcher is open, a tiny pixel star circles its window. The ring round it fills as the next harvest comes due, and glows when a typing test will count. Click it (or search *Solar System*) and your system unfolds over the whole screen. Your star sits in the middle, its planets orbit over your desktop, and everything, interface included, is drawn as pixels. While it's open it takes every click, so nothing reaches the apps behind. **Esc** (or switching to another app) puts it away at once. You can turn the orbiting star off under Settings › Modules.

- **Release material:** pick one in the toolbar (or press 1–5) and hold the mouse anywhere. The particles stay particles: they pull together into a clump at that spot, and that clump is your new world, about one pixel per particle. Release near a planet and they settle onto it in layers: iron sinks to the core, then rock, carbon and water, with hydrogen as a hazy shell. Adding water to a rocky world really does give it a blue layer.
- **Move a planet:** drag it. It keeps a steady circular orbit at whatever distance you leave it.
- **Look at a planet:** click it to see its make-up and its orbit; click the name (or press F2) to rename it, and right-click to deselect.
- **What happens on its own:** a small planet that comes close to a big one is captured as a moon, similar-sized ones merge, and anything that strays too close falls into your star.

It's saved in `%APPDATA%\SIGMA5\solar.json`, and a new system starts with a little material to try out. Everything is drawn in software on the same click-through layer as fidget mode (only what changed reaches the screen), at about 1 ms a frame: 30 fps when idle, 60 while you're doing something. Put away, it costs nothing.

### Appearance

Settings › Appearance covers theme and accent (presets or any colour), square or rounded corners (square by default), animations, the interface and search fonts, text size, launcher width, height and row height, and every colour: background, text, secondary text, selection and borders. Set a colour by picking it from the screen or pasting a hex code; a custom background re-derives the other surfaces so contrast holds. A coloured border runs round the launcher (2 px in the accent by default; set its thickness, 0–8 px, and colour). Small animations (the calculator's copy pulse on any copied row, a flash on launch, sliding toggles) can be turned off, and follow Windows' own animation setting.

### Fidget mode

Pick the launcher up and shake it hard. The letters in the search box dissolve into pixels that swirl round your mouse like planets round a sun, clumping together as they orbit. Anything you type then dissolves straight away. **Enter** (or another shake) scatters them, and **Esc** ends it.

It costs nothing until you start it. While it runs, the physics is O(n) (particles attract through a coarse grid, not pairwise) and only the parts of the screen that changed are redrawn, at about 1 ms per frame in normal use. Pixels are sized by DPI, so they look the same on any screen.

## Build & run

Requires Rust (MSVC toolchain) and the Visual Studio C++ Build Tools.

```bash
cargo build --release
```

```bash
./target/release/sigma5.exe
```

**Publishing a release:** bump `version` in `Cargo.toml` and `CODENAME` in `src/install.rs`, write release notes to a Markdown file, commit, then:

```bash
powershell -ExecutionPolicy Bypass -File scripts/release.ps1 -Notes notes.md
```

It runs the tests, builds, and publishes `SIGMA5 <version> “<Codename>”` with `sigma5.exe` attached. GitHub records the file's SHA-256, which the updater checks before installing.

The logo source is `SIGMA5_LOGO.svg`. `assets/make-icon.ps1` turns it into the embedded `sigma5.ico` (exe, tray, windows) and `sigma5.png`. Rerun the script if the logo changes.

Running the exe again while SIGMA5 is already running just opens the existing window. `--background` starts it hidden (this is what the start-at-login entry uses).

## Settings

Open Settings with `Ctrl+,` in the launcher, the **SIGMA5 Settings** command, or the tray menu. Changes apply instantly; there is no Save button.

| Page | Options |
| --- | --- |
| General | Launcher hotkey (click, then press your combo), open with Windows key, launch at login, keep last search, recent items |
| Appearance | Theme (System / Dark / Light), accent colour (Windows accent + 7 presets), launcher size (Compact / Default / Large) |
| Hotkeys | Bind any app to any shortcut: launch it, bring it to the front, or minimise it if it's already in front |
| Keyboard | Caps Lock → Default / Hyper (Ctrl+Shift+Alt) / Ctrl / Esc / Off, plus a list of key remaps |
| Modules | Turn each module on or off |
| About | Open config file / data folder, clear launch history, reset all settings |

The Settings window is created only when opened and destroyed when closed, so it uses no memory the rest of the time. If a shortcut belongs to another app (e.g. 1Password's `Ctrl+Shift+Space`), that app grabs the keys and Settings tells you to pick another.

### Calculator & unit converter

Type maths and the answer appears in a card at the top of the results:

- **Maths:** `+ − × ÷ ^ % !`, parentheses, implicit multiplication (`2pi r^2`, `3(4+5)`), `sqrt cbrt abs ln log log2 exp sin cos tan asin acos atan sinh cosh tanh round floor ceil min max atan2 hypot`, constants `pi e tau phi g0`.
- **Units with dimensional analysis:** `3 kN * 0.5 m` → `1,500 J`, `5 kg * 9.81 m/s^2` → `49.05 N`, `10 m / 2 s` → `5 m/s`, `5 ft 10 in to cm`, `200 psi to MPa`, `100 C to F`, `100 rpm to rad/s`, `1 GiB to MB`, `m2`/`ft3` shorthands. Convert with `to`, `in`, `as` or `->`. A bare quantity (`5 km`) shows its metric/imperial counterpart.
- **Covers:** length, area, volume, mass, time, temperature (absolute for single readings), speed, acceleration, force, pressure/stress, energy/torque, power, angle/rotation (1 rev = 2π rad is handled, so rpm ↔ rad/s and rpm ↔ Hz both work), frequency, electrical, and data.
- **Enter** copies the plain number to the clipboard *and* leaves the answer (with its unit) in the search box, so you can keep going (`* 2`, ` to lbf ft`). The card pulses to confirm.
- It stays out of the way of app searches: `7zip`, `1password`, `office 365` and lone words or numbers never trigger it. It can be turned off under Settings › Modules.

### WinGet

Type `winget` (or a command name) in the launcher:

| Command | What it does |
| --- | --- |
| **Search Packages** | Live search of the WinGet catalogue; Enter installs |
| **Show Installed** | Your installed packages (filter as you type); Enter upgrades if an update exists, otherwise uninstalls; Shift+Enter uninstalls |
| **Show Upgradable** | Packages with updates, plus *Upgrade All*; Enter upgrades |
| **Upgrade All Packages** | `winget upgrade --all` |
| **Export Packages** | Saves `winget-packages-<date>.json` to Documents |
| **Import Packages** | Pick an export file and install everything in it |

Installs, upgrades and removals open a console window so you can follow progress and answer any UAC prompt. Uninstalling asks for confirmation first. Esc (or Backspace on an empty box) goes back.

### Clipboard history

**Clipboard History** (or a hotkey you set in Settings › General, e.g. `Ctrl+Shift+V`) opens a page with everything you've copied: text, files and images, grouped by Today / Yesterday / Earlier, with a preview pane and details (source app, time, size).

- **Enter** pastes into the window you were in, **Shift+Enter** just copies, **Shift+Delete** removes an entry.
- Content that apps mark as private (password managers such as 1Password do) is never recorded.
- Text and file history is saved encrypted with Windows DPAPI, so only your Windows account can read it. Images are kept in memory only (the last 20). The last 250 entries are kept.
- Turn it off under Settings › Modules; *Clear Clipboard History* wipes it.

### App hotkeys, Hyper key and remaps (replaces AutoHotkey + Raycast hotkeys + PowerToys Keyboard Manager)

- **Hyper key:** set Caps Lock to *Hyper* and it acts as Ctrl+Shift+Alt while held. Tapped alone it does nothing (the release is masked so no Alt menu pops up). Caps Lock is switched off automatically when you enable a remap of it.
- **App and command hotkeys:** Settings › Hotkeys › *Add app or command hotkey* turns the launcher into a picker; choose an app or a SIGMA5 command (Pick Colour, Search Files, Clipboard History, …), then press the shortcut (e.g. Caps+C). Commands run straight away; ones that open a page show the launcher on it. If another app already owns the shortcut (PowerToys, for example), SIGMA5's keyboard hook takes it over, so it still works. Like Raycast: pressing it launches the app if it isn't running, brings it to the front if it is, and minimises it if it's already in front. Pressing again restores exactly the windows it minimised. Windows are matched by AppUserModelID, package identity (Store/MSIX apps like Claude, Outlook, Spotify) or exe.
- **Remaps:** Settings › Keyboard › *Add remap*: press the key to change, then pick what it sends: another key or shortcut (record it), a media key (Play/Pause, Next, Previous, Stop, Volume up/down, Mute), Hyper, or Disabled. Numpad keys are recognised with Num Lock on or off, and Numpad Enter is separate from Enter.
- **Recording** uses the keyboard hook, so it sees keys before other apps' global shortcuts. A combo that's taken is reported instead of silently failing.
- **Cost:** a single `WH_KEYBOARD_LL` hook on its own thread, running only while a remap, the Windows-key option or recording is active. Each key costs microseconds; nothing is polled.
- **Limitation:** Windows blocks injected keys into apps running as administrator, so remaps don't apply there. The Windows-key option also stands aside so Win can't get stuck.

```toml
caps_lock = "hyper"

[[app_hotkeys]]
hotkey = "Ctrl+Alt+Shift+C"
app = "Chrome"
name = "Google Chrome"

[[remaps]]
from = "Numpad5"
to = "MediaPlayPause"
```

### Replacing the Start menu

With **Open with Windows key** on, a lone tap of `Win` opens SIGMA5 and the Start menu stays closed. It works like this:

- A low-level keyboard hook on its own thread watches for `Win` pressed and released with no other key in between (and within 1.5 s).
- On a tap it swallows the release and injects an unused "mask" key before releasing `Win`, the same trick AutoHotkey uses. Explorer sees a key combination, so Start doesn't open.
- Every Win shortcut (`Win+E`, `Win+L`, `Win+Shift+S`, …) passes through untouched, and `Ctrl+Esc` or clicking the Start button still opens Start.
- When an elevated (admin) window has focus, Windows blocks injected input, so SIGMA5 stands aside there and Start opens normally.

Everything lives in `%APPDATA%\SIGMA5\config.toml`, which you can also edit by hand:

```toml
hotkey = "Alt+Space"            # e.g. "Ctrl+Alt+Space", "Ctrl+Shift+K", "Win+F1"
theme = "system"                # "system" | "dark" | "light"
accent = "violet"               # system, violet, blue, teal, green, orange, red, pink
size = "default"                # "compact" | "default" | "large"
windows_key = false             # tap Win to open SIGMA5 instead of Start
start_with_windows = false      # Run entry + Task Manager "startup approved" record
auto_update = true              # check GitHub for new versions every 12 hours
keep_query_secs = 30            # reopen within N seconds to keep your last search
recent_count = 6
disabled_modules = []           # e.g. ["commands", "files"]
```

Other files in that folder: `history.tsv` (launch counts for ranking) and `apps.cache` (app list, so results are ready instantly at startup).

> **Tip:** if you enable `start_with_windows`, copy `sigma5.exe` somewhere permanent first (e.g. `%LOCALAPPDATA%\SIGMA5\`). The login entry points at whatever path the exe was running from.

## Architecture

```
src/
  main.rs            single-instance check, startup
  fuzzy.rs           query matcher + scoring (unit-tested)
  history.rs         frecency store
  config.rs          TOML settings with hot reload
  launch.rs          shell launching, start-at-login (Run entry + startup approval)
  install.rs         self-install / uninstall, version + codename
  update.rs          GitHub Releases check, verified download (WinHTTP + BCrypt SHA-256)
  focus.rs           focus-or-launch for app hotkeys (window matching)
  calc/              calculator: parser, dimensional evaluator, unit table, formatting
  winget.rs          WinGet queries (table parser) and console actions
  clipboard.rs       clipboard history: capture, privacy filters, DPAPI-encrypted store
  keys.rs            key names/codes, numpad handling, remap targets
  files.rs           file index (compact, fuzzy two-pass search) and Windows' recent files
  colors.rs          colour parsing, formats and picked-colour history
  media.rs           Now Playing: Windows media sessions, album art (WIC), output level meter
  typing.rs          typing test: words, scoring, personal bests
  solar.rs           solar system simulation, word → material harvest, save file
  greek.rs           Greek letter map (SMath / Symbol-font keys)
  sysinfo.rs         top bar readings (CPU, RAM, GPU, temps, battery, network, disk)
  providers/         ── result sources ("plugins")
    mod.rs           Provider trait, Item, Action, Registry
    apps.rs          shell:AppsFolder indexer + icon extraction (background thread)
    commands.rs      SIGMA5's own commands
  ui/
    window.rs        launcher window state, message handling, keyboard/mouse
    paint.rs         launcher rendering
    settings.rs      Settings window (custom-drawn controls, hotkey recorder)
    draw.rs          shared GDI primitives, anti-aliased shapes, fonts
    fidget.rs        fidget mode (shake detection, particle physics) and the shared click-through overlay
    solar.rs         Solar System on the desktop: particle-clump planets, pixel interface, launcher star
    pixfont.rs       5×7 pixel font for the Solar System
    picker.rs        screen colour picker (click-catching layer + loupe)
    theme.rs         colours, accents, layout metrics
    icons.rs         icon bitmap cache
    hotkey.rs        hotkey parsing/registration
    input.rs         keyboard hook: remaps, Hyper, key capture, Windows-key tap
    tray.rs          notification-area icon + menu
```

### Adding a feature ("plugin")

Plugins are compiled in, not loaded at runtime. To add one, for example web search:

1. Create `src/providers/web.rs` with a struct implementing `Provider` (`id`, `section`, `search`, `browse`, `lookup`).
2. Add a `ModuleInfo` entry to `MODULES` (`src/providers/mod.rs`) so it gets an on/off toggle in Settings › Modules.
3. If it needs new behaviour on Enter, add a variant to `Action` (e.g. `OpenUrl(String)`) and handle it in `Launcher::activate` (`src/ui/window.rs`).
4. Register it in `Registry` (`src/providers/mod.rs`).

The UI renders any `Item` (title, subtitle, accessory label, app icon or Fluent glyph), so most providers need no UI changes.

## Roadmap

- [x] App launcher (Win32 + Store apps), fuzzy search, frecency, tray, hotkey, themes
- [x] Settings window (hotkey recorder, theme/accent/size, modules, reset)
- [x] Optional Start menu replacement (tap the Windows key)
- [x] App hotkeys (focus or launch), Hyper Caps Lock, key remaps incl. media keys
- [x] WinGet: search, install, upgrade, uninstall, export/import
- [x] Clipboard history with preview, paste-back and privacy filtering
- [x] Self-updating from GitHub Releases (0.2 “Euler”)
- [x] Draggable launcher and fidget mode (shake it to dissolve the letters into orbiting pixels)
- [x] File search, colour picker, top bar, appearance customisation, modules (0.3 “Charpy”)
- [x] Now Playing (Spotify and any media player) in the top bar, Greek letter hotkey, typing test, Solar System (0.4 “Newton”)
- [ ] Google search fallback ("Search Google for …")
- [ ] Ask AI (inline answer view)
- [ ] Actions panel (`Ctrl+K`): run as admin, open file location, copy path
- [x] Calculator / unit conversions with dimensional analysis
- [ ] Windows settings pages & system commands (lock, sleep, restart)

## License

[MIT](LICENSE) © 2026 Nathan Smith
