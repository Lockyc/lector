<p align="center">
  <img src="src-tauri/icons/icon.png" alt="lector app icon" width="128" height="128">
</p>

<h1 align="center">lector</h1>

<p align="center">
  <a href="https://github.com/Lockyc/lector/releases/latest"><img src="https://img.shields.io/github/v/release/lockyc/lector?sort=semver&label=release" alt="Release"></a>
  <img src="https://img.shields.io/badge/platform-macOS-000000?logo=apple&logoColor=white" alt="Platform: macOS">
  <img src="https://img.shields.io/badge/built%20with-Tauri%20v2-24C8DB?logo=tauri&logoColor=white" alt="Built with Tauri v2">
  <a href="LICENSE"><img src="https://img.shields.io/github/license/Lockyc/lector" alt="License: MIT"></a>
</p>

A macOS console of grouped tabs over locally-rendered documentation sites. One tab is one doc
repo on disk, rendered live from its working tree: editing a `.md` file updates the tab
immediately — no build step, no deploy, nothing published.

lector has two sibling apps built the same way: **[warden](https://github.com/Lockyc/warden)**
curates terminals and **[curator](https://github.com/Lockyc/curator)** curates browser tabs;
lector curates local documentation.

## Status

**In use and released** — see the [latest release](https://github.com/Lockyc/lector/releases/latest).

## Install

**Download (no build):** grab `lector-<version>-macos.zip` from the
[latest release](https://github.com/Lockyc/lector/releases/latest), unzip, and move `lector.app`
to `/Applications`. macOS only.

In **Claude Code**, run `/lector:install` — it checks prerequisites (offering to install
any that are missing), builds lector from source into `~/.lector`, installs `lector.app`
to `/Applications`, and seeds your config.

Or install from a terminal:

```sh
curl -fsSL https://raw.githubusercontent.com/Lockyc/lector/main/install.sh | bash
```

Re-running either path rebuilds lector from the latest source; a downloaded or installed lector
also updates itself (below).

## Updates

lector updates itself — no reinstall. On launch, every 6 hours while open, and via
**lector ▸ Check for Updates…**, it checks GitHub for a newer release; when one exists the
sidebar shows an *Update available* bar with a one-click **Update & Relaunch**.

- **Confirm-to-install** — nothing installs silently; you approve each update.
- **Signed** — each update's signature is verified before it installs.
- **Opt out** with `auto_update = false` (the **Check for Updates…** menu item still works).

## Model

- **`config.toml` is the source of truth**: one or more `[[window]]` blocks, each containing
  loose `[[window.tab]]` entries and/or `[[window.group]]` sections of `[[window.group.tab]]`s.
  Each tab points at a **`dir`** — a local doc repo. Edits hot-reload on save; a
  `[[window]]` added, removed or retitled opens or closes live.
- **`[[window.root]]` discovers repos for you.** Point a root at a projects dir (`dir`, with an
  optional scan `depth`) and every git repo under it becomes a doc tab, shown as a collapsible
  folder tree with a `⟳` rescan button. Discovered tabs are lazy — they load on select.
- **Tabs load on select.** Selecting a tab renders its repo live with
  [compositor](https://github.com/Lockyc/compositor); the sidebar dot shows whether it's loaded.
  **⌘W** unloads the active tab.
- **Sidebar search** — the field above the tab list narrows it as you type, matching tab titles,
  folder paths and group names. **⌘⇧F** jumps into it; **↑**/**↓** pick a match, **Enter** opens
  it, **Esc** clears the search.
- **Pop-out tabs** — **⌘⇧O** (or the pop-out icon on a row's letter tile) moves the active tab
  into its own window. It stays loaded across the hop; closing the window returns the
  tab to where it came from.
- **Windows and the home surface** — **⌘⇧W** closes a window and the **Window** menu reopens it.
  With no window open, lector shows a home surface listing every window; with no config, it
  offers a **Create a starter config** button. The **Config** menu opens the config file or
  reveals it in Finder.
- **Native navigation.** Mouse side-buttons drive back/forward through a tab's history, and a
  determinate progress bar tracks page loading — both alongside the sidebar's ◀ ▶ ⌂ nav pill.
- **Keyboard tab navigation** (the **Tab** menu) — **⌘⇧[** / **⌘⇧]** cycle to the previous/next
  tab and **⌘1–⌘9** jump to a position; set `tab_digit_keys = "cycle"` to make **⌘1** / **⌘2**
  cycle instead (jumps shift to **⌘3–⌘9**).

## Config

`~/.config/lector/config.toml` (override with `LECTOR_CONFIG`). A fuller example is
[`examples/config.toml`](examples/config.toml).

```toml
density = "compact"

[[window]]
title  = "Docs"
colour = "#4a9"

  [[window.tab]]
  title        = "handbook"
  dir          = "~/Developer/handbook"
  load_on_open = true

  [[window.group]]
  name = "Specs"

    [[window.group.tab]]
    title = "api"
    dir   = "~/Developer/api/docs"

  [[window.root]]
  dir   = "~/Developer"
  depth = 4
```

### Global

| Key | Default | What it does |
|---|---|---|
| `dark_mode` | `false` | Force dark window appearance; `false` follows the system. Hot-reloads. |
| `format_on_save` | `false` | Reformat the config in house style on each clean hot-reload (same as `lector fmt`). |
| `density` | `comfortable` | Chrome sizing: `comfortable` or `compact`. |
| `sidebar_drag` | `true` | Whether the sidebar chrome is a window-move drag handle. |
| `auto_update` | `true` | Check for a new release automatically; **Check for Updates…** works either way. |
| `tab_digit_keys` | `jump` | What ⌘1/⌘2 do: `jump` — ⌘1–⌘9 jump to a position; `cycle` — ⌘1 next, ⌘2 previous, jumps shift to ⌘3–⌘9. Hot-reloads. |

### `[[window]]`

| Key | Default | What it does |
|---|---|---|
| `title` | *required* | Window title; unique across windows. |
| `width` / `height` | `1500` / `1000` | First-run size in logical pixels; after that lector remembers each window's size and position. |
| `open_on_launch` | `false` | `false` opens the first `load_on_open` tab, else a blank screen; `true` opens the first tab even if it isn't loaded. |
| `colour` | none | `#rgb` / `#rrggbb` accent for the window's name banner and tint. |

### `[[window.tab]]` / `[[window.group.tab]]`

| Key | Default | What it does |
|---|---|---|
| `title` | *required* | Display label. |
| `dir` | *required* | The doc repo to render (`~` expanded). A missing dir is a warning, not an error. |
| `load_on_open` | `false` | Load this tab at launch instead of on first select. |

### `[[window.group]]`

| Key | Default | What it does |
|---|---|---|
| `name` | *required* | Section header; unique within its window, including against root names. |

### `[[window.root]]`

| Key | Default | What it does |
|---|---|---|
| `dir` | *required* | Directory scanned for git repos, each becoming a lazy doc tab. |
| `name` | basename of `dir` | The root's folder label in the sidebar. |
| `depth` | `6` | How many levels deep to scan (≥ 1). |

### CLI

The app binary doubles as a config tool — `/Applications/lector.app/Contents/MacOS/lector`:

- **`lector validate [path]`** prints the resolved windows and tabs plus any warnings, exiting
  non-zero on a load/parse/validation error.
- **`lector fmt [--check] [path]`** rewrites a config in lector's house TOML style; `--check`
  reports without writing.

Both default to the config path above.

## Build

Needs Rust and the Tauri CLI (`cargo install tauri-cli --version ^2`).

```sh
just run      # launch against examples/config.toml (never touches your real config)
just test     # cargo test --workspace
just gate     # the full pre-merge gate (no active [patch], fmt-check, clippy, tests, example-config fmt)
just build    # build lector.app
git config core.hooksPath .githooks   # once per clone: arms the pre-commit / pre-push hooks
```

lector is built on four pinned shared crates —
[compositor](https://github.com/Lockyc/compositor) (the render/serve engine),
[chrome-core](https://github.com/Lockyc/chrome-core) (the
sidebar), [config-core](https://github.com/Lockyc/config-core) (config primitives, formatter,
root discovery) and [shell-core](https://github.com/Lockyc/shell-core) (release tooling and app
shell). To build against a sibling checkout of one, see the `*-dev` / `*-pin` recipes in
[`CLAUDE.md`](CLAUDE.md).

## License

MIT — see [`LICENSE`](LICENSE).
