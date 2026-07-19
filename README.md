<p align="center">
  <img src="src-tauri/icons/icon.png" alt="lector app icon" width="128" height="128">
</p>

<h1 align="center">lector</h1>

<p align="center">
  <img src="https://img.shields.io/badge/platform-macOS-000000?logo=apple&logoColor=white" alt="Platform: macOS">
  <img src="https://img.shields.io/badge/built%20with-Tauri%20v2-24C8DB?logo=tauri&logoColor=white" alt="Built with Tauri v2">
  <a href="LICENSE"><img src="https://img.shields.io/github/license/Lockyc/lector" alt="License: MIT"></a>
</p>

A macOS console of grouped tabs over locally-rendered documentation sites. One tab is one doc
repo on disk: selecting it starts a live-reloading server on an ephemeral loopback port and
points a webview at it, so editing a `.md` file updates the tab immediately — no build step, no
deploy, nothing published.

It's the third sibling to two existing apps: **[warden](https://github.com/Lockyc/warden)**
curates terminals, **[curator](https://github.com/Lockyc/curator)** curates browser tabs, and
lector curates local documentation the same way.

## Status

**Complete and functional end-to-end, and released.** The full app works: the config
parser/validator/formatter, the `Servers` supervisor (one live `compositor serve` per open tab),
project-tree root discovery, tab pop-out into detached windows, the sidebar chrome and its
commands, content-webview link escape, config hot-reload, and the `validate`/`fmt` CLI are all
built and tested. The in-app minisign-signed updater is fully wired.

## Install

In **Claude Code**, run `/lector:install` — it checks prerequisites (offering to install
any that are missing), builds lector from source into `~/.lector`, installs `lector.app`
to `/Applications`, and seeds your config.

Or install from a terminal:

```sh
curl -fsSL https://raw.githubusercontent.com/Lockyc/lector/main/install.sh | bash
```

Re-running either path updates lector (`git pull` + rebuild). The sections below describe the
manual / contributor flow.

## Updates

lector updates itself — no reinstall. On launch, periodically while open, and via
**lector ▸ Check for Updates…**, it checks GitHub for a newer release; when one exists the
sidebar shows an *Update available* bar with a one-click **Update & Relaunch**.

- **Confirm-to-install** — nothing installs silently; you approve each update.
- **Signed** — each update is verified against lector's own minisign key before it installs,
  independent of Apple notarization.
- **Opt out** with `auto_update = false` (the **Check for Updates…** menu item still works).

Re-running `install.sh` is only needed to bootstrap the first updater-capable version, or to
build from source.

## Model

- **`config.toml` is the source of truth**, in the same shape as curator's and warden's: one or
  more `[[window]]` blocks, each containing loose `[[window.tab]]` entries and/or
  `[[window.group]]` sections of `[[window.group.tab]]`s. A lector tab points at a **`dir`** — a
  local doc repo path — rather than a URL.
- **`[[window.root]]` discovers repos for you.** Point a root at a projects dir (`dir`, with an
  optional scan `depth`) and every git repo under it becomes a doc tab, shown as a collapsible
  folder tree with a `⟳` rescan button. Discovered tabs are lazy — their server starts on select.
- **One tab, one live server.** Selecting a tab starts a
  [compositor](https://github.com/Lockyc/compositor) `serve` loop on an ephemeral loopback port
  and points the tab's webview at it; the tab's title bar tracks whether that server is live.
- **Nothing deployed.** There is no build/publish step in the loop — the whole point is to
  render a doc repo's *working tree* as you edit it.

## Build / run / test

- `just run` — runs the app against the repo's demo config (`examples/config.toml`), never
  touching a real `~/.config/lector/config.toml`
- `just build` — builds a release `.app` bundle (needs the Tauri CLI:
  `cargo install tauri-cli --version ^2`)
- `just test` — runs the Rust test suite (`cargo test --workspace`)
- `just gate` — the full pre-merge gate: fmt-check, clippy, tests, config fmt-check, and the
  active-`[patch]` guard. CI (`.github/workflows/ci.yml`) runs this same gate on every push/PR to
  `main` and `dev`; run it locally too — the fast loop — and confirm it's green before committing
  or tagging a release.

Run `just` with no arguments to list every recipe.

## Shared cores and the sibling-checkout requirement

lector is built on four shared library crates, each a pinned git dependency — a plain
`cargo build` / `just run` resolves all four with nothing extra to install:

- **[compositor](https://github.com/Lockyc/compositor)** — the Markdown render/serve engine.
  lector calls its `serve_handle()` once per open tab.
- **[chrome-core](https://github.com/Lockyc/chrome-core)** — the sidebar chrome (grouped tab
  rows, resize-drag, density tokens), shared with warden and curator.
- **[config-core](https://github.com/Lockyc/config-core)** — the TOML config engine (parse,
  validate, format, hot-reload diff).
- **[shell-core](https://github.com/Lockyc/shell-core)** — shared release tooling plus a sliver
  of Tauri runtime setup.

To iterate on a core itself rather than its last pinned rev, clone it as a **sibling checkout**
next to this repo (`../chrome-core`, `../config-core`, `../shell-core`, `../compositor`), then:

```sh
just chrome-dev       # (or config-dev / shell-dev / compositor-dev)
```

activates that core's `[patch]` so lector builds against the sibling checkout, including any
uncommitted edits there. `just chrome-pin` (or the matching `*-pin`, or `just cores-pin` for all
four at once) re-pins to that sibling's pushed HEAD and deactivates the patch. **Never commit an
active patch** — it builds green for you but is unresolvable for every other clone and CI;
`just gate` and `.githooks/pre-commit` both refuse to pass while one is active.

## Setup

This repo uses custom git hooks — after cloning, run once:

```sh
git config core.hooksPath .githooks
```

This activates the active-`[patch]` guard on commit and the docgraph doc-audit gate on push.
