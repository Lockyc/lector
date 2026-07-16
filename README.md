# lector

A macOS console of grouped tabs over locally-rendered documentation sites. One tab is one doc
repo on disk: selecting it starts a live-reloading server on an ephemeral loopback port and
points a webview at it, so editing a `.md` file updates the tab immediately — no build step, no
deploy, nothing published.

It's the third sibling to two existing apps: **[warden](https://github.com/Lockyc/warden)**
curates terminals, **[curator](https://github.com/Lockyc/curator)** curates browser tabs, and
lector curates local documentation the same way.

## Status

**Work in progress — not yet usable.** This repository currently holds only the workspace
scaffold: the shared-core `[patch]` machinery, the toolchain pin, dev tooling, and hooks. No
application code exists yet — `src-tauri/` and `crates/lector-config/` are declared as workspace
members but not yet written, so `cargo build` currently errors on missing manifests. There is
nothing to install or run yet; follow this repo's commit history for progress.

## Model (intended)

- **`config.toml` is the source of truth**, in the same shape as curator's and warden's: one or
  more `[[window]]` blocks, each containing loose `[[window.tab]]` entries and/or
  `[[window.group]]` sections of `[[window.group.tab]]`s. A lector tab points at a **`dir`** — a
  local doc repo path — rather than a URL.
- **One tab, one live server.** Selecting a tab starts a
  [compositor](https://github.com/Lockyc/compositor) `serve` loop on an ephemeral loopback port
  and points the tab's webview at it; the tab's title bar tracks whether that server is live.
- **Nothing deployed.** There is no build/publish step in the loop — the whole point is to
  render a doc repo's *working tree* as you edit it.

## Build / run / test

Once the workspace has real crates (`src-tauri/`, `crates/lector-config/`), the usual `just`
recipes apply:

- `just run` — runs the app against the repo's demo config (`examples/config.toml`), never
  touching a real `~/.config/lector/config.toml`
- `just build` — builds a release `.app` bundle (needs the Tauri CLI:
  `cargo install tauri-cli --version ^2`)
- `just test` — runs the Rust test suite (`cargo test --workspace`)
- `just gate` — the full pre-merge gate: fmt-check, clippy, tests, config fmt-check, and the
  active-`[patch]` guard. There is no CI — run this locally and confirm it's green before
  committing or tagging a release.

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
