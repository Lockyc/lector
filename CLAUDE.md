---
type: architecture
description: Agent orientation for lector — purpose, the four shared cores, footguns, and build/release model.
---

# lector — agent notes

## Purpose

lector is a macOS Tauri v2 app: a console of grouped tabs over locally-rendered documentation
sites. One tab is one doc repo on disk. Selecting a tab starts a `compositor serve` loop on an
ephemeral loopback port and points a webview at it, so editing a `.md` file live-reloads the tab
with nothing deployed — no build step, no publish, nothing to keep in sync. It's the third
sibling to two existing apps: **warden** curates terminals, **curator** curates browser tabs,
lector curates local documentation.

## Current state vs intended architecture

**The app is complete and functional end-to-end.** `crates/lector-config` is complete
(parse/validate/format/identity, unit-tested standalone). `src-tauri` has the full app shell: the
manifest pinning all four cores, `build.rs`, `tauri.conf.json`, the
`Servers` supervisor (`servers.rs` — one `compositor::serve_handle()` per open tab, idempotent
start/stop/retain/shutdown_all, dead-thread detection via `reap`/`is_alive`), the chrome controller
(`src/chrome.js` + the commands it calls in `commands.rs`), content-webview management with link
escape (`webviews.rs` — `is_own_origin` keeps a doc's off-site links from stranding the tab, and
another tab's own loopback port is escaped too), native mouse side-button back/forward (shell-core's
shared `mouse_nav::install` NSEvent monitor, wired in the setup hook with lector's
focused-active-webview resolver — WKWebView never delivers the side buttons to the DOM, so it can't
be done in the page; see shell-core's CLAUDE.md), a thin determinate **loading bar** per content
webview (shell-core's `progress_bar::install` at the `show_in` build site, driven by WKWebView
`estimatedProgress`, tinted with the window accent via `AppState::colour_for`), and `run()`'s full
setup: config load, window
build, the shared launch/hot-reload reconciliation path (`reload.rs`), `open_on_launch` startup
selection, a config-file watcher (format-on-save, last-good-on-failure), and a clean-quit
`RunEvent::Exit` handler that shuts down every server. The `validate`/`fmt` CLI subcommands round
it out. The in-app updater is fully wired: it has its permission (see the capabilities footgun
below) and `tauri.conf.json` carries the real minisign `pubkey`, whose private half lives only on
the maintainer's machine and in their password manager — never in this repo.

The shape mirrors curator's: a Cargo workspace with a platform-neutral config crate
(`crates/lector-config` — parse/validate/format/identity, no Tauri deps, unit-tested standalone)
consumed by the Tauri app crate (`src-tauri/`, package `lector`) — windows, the `Servers`
supervisor managing one `compositor::serve_handle()` loop per open tab, the chrome controller,
commands, config hot-reload.

## Workspace layout

- **`src-tauri/`** — the macOS Tauri app: the manifest, `build.rs`, `tauri.conf.json`,
  `capabilities/default.json`, plugin registration, the `Servers` supervisor (`servers.rs`), the
  chrome controller + commands (`commands.rs`), content-webview management + link escape
  (`webviews.rs`), the shared launch/hot-reload reconciliation path (`reload.rs`), `run()`'s full
  setup (`lib.rs`), and the `validate`/`fmt` CLI.
- **`crates/lector-config`** — the config parser: the `window → group → tab` schema (a lector tab
  carries a `dir` — a local doc repo path — rather than a `url`), `parse_and_validate` /
  `load_config`, `resolve_config_path`, identity/hash helpers, `tab_views()`/`startup_label()`.
  Re-exports config-core's shared house formatter + colour parsing, the same way curator/warden do.

The `[patch]` overrides for all four shared cores live in the **workspace-root `Cargo.toml`** (a
`[patch]` must sit at the workspace root, not a member manifest) — see *The `[patch]` rule and
land order* below.

## The four shared cores — consumption contract

lector pins **four** git dependencies — one more than curator's and warden's three, because
lector also consumes the render/serve engine:

- **compositor** (`https://github.com/Lockyc/compositor`) — the Markdown render/serve engine.
  lector's `SiteServer` calls its `serve_handle()` once per open tab: a non-blocking call that
  runs a doc repo's live-reloading site on a loopback port and returns once bound. Pinned by rev
  in `src-tauri/Cargo.toml`. **Keep this pin current** — re-pin it forward (`just compositor-pin`)
  as compositor advances rather than letting it drift behind. lector is compositor's *only*
  consumer, so a stale pin silently splits the source lector develops against from the source it
  ships; and because lector's build is the drift detector for compositor's source under lector's
  channel (see *Toolchain lockstep*), a long-idle pin *banks* that risk instead of surfacing it —
  the divergence lands all at once, loudly, only when the pin finally moves. Keeping it current
  keeps the detector live and the split from opening. A lagging pin is resolvable (the pinned rev
  is pushed) so it never breaks another clone or CI — but "resolvable" is not "current," and
  current is the standard here.
- **chrome-core** (`https://github.com/Lockyc/chrome-core`) — the sidebar chrome (grouped tab
  rows, resize-drag, density tokens), shared with warden and curator. A **build-dependency**:
  `src-tauri/build.rs` materializes its CSS/JS into `src/chrome-core.{css,js}` (git-ignored)
  before Tauri embeds `src/`. Pinned by rev in `src-tauri/Cargo.toml`.
  **The sidebar chrome IS chrome-core — read [its CLAUDE.md](https://github.com/Lockyc/chrome-core/blob/main/CLAUDE.md)
  before assuming a sidebar feature is app-specific or missing.** Its frontend source is
  `assets/sidebar.{css,js}` (not `src/`), and it already provides shared **opt-in** capabilities
  lector can enable via DTO fields rather than building — e.g. project-tree/root sections
  (`tree`/`treePath` rows → a collapsible folder tree + `⟳` rescan button firing `onRescan`) and
  the self-updater. lector's `src/chrome.js` is a thin controller mapping those callbacks/DTO
  fields to its own commands. **The forwarding premise (referenced throughout below): a new DTO
  field is inert until `chrome.js`'s map copies it across — invisible to chrome-core otherwise.**
  Each forwarded field is commented at its map site in `src/chrome.js`.
- **config-core** (`https://github.com/Lockyc/config-core`) — the TOML config engine (parse,
  validate, format, hot-reload diff) behind the shared house formatter. Pinned by rev in
  `crates/lector-config/Cargo.toml`, kept at the **same rev** as warden's and curator's own
  config-core pins.
- **shell-core** (`https://github.com/Lockyc/shell-core`) — shared release tooling (the three
  release scripts, materialized git-ignored into `scripts/`) plus a sliver of Tauri runtime
  setup (`register_plugins` for window geometry/updater/process). **Pinned twice in
  `src-tauri/Cargo.toml`**, per its zero-dep/runtime feature split:
  - `[build-dependencies]`: `default-features = false` — `build.rs` needs only `build_stamp()`
    and the embedded script consts, which are zero-dependency.
  - `[dependencies]`: `features = ["runtime"]` — `register_plugins` needs the tauri tree.

  **The `default-features = false` on the build-dep is load-bearing**: without it, every
  consumer's `[build-dependencies]` drags in the whole tauri tree just to run `build.rs`.
  Resolver 2 resolves the two feature sets independently. Bump both entries to the same rev in
  lockstep.

  **Window geometry** — shell-core's own `geometry` module, replacing `tauri-plugin-window-state`
  (which lector never depended on directly). It persists each window's size/position in **AppKit
  points**, clamps every restore to the target monitor's work area, and never records geometry
  while a window is fullscreen or minimized (covers classic Split View; Sequoia's drag-to-edge
  *tiling* is not a fullscreen space, so a tiled window's ordinary bounds are still recorded,
  correctly). The home surface and any popped-out tab window are excluded from save and restore
  **structurally**, inside the module — `register_plugins`'s `skip_labels` is reserved for an
  app's *own* transient windows, and lector passes `&[]` (it has none).

All four are git dependencies, git-ignored/materialized, or `[patch]`-overridable for local dev
— never vendor one in-tree.

## The menu spine, home surface, and app-name strip — all shared, none app-specific here

lector adopted shell-core's `menu::build_spine` and `home::{home_state, show_home, close_home}`,
and chrome-core's `appName` field, from scratch — it never had its own menu, error window, or
launcher to delete, so it's the cleanest case of the three apps consuming these.

- **lector contributes no app-specific menu items.** The App, Config, and Window submenus are the
  shared spine (`shell_core::menu::build_spine`); lector's own **Tab** submenu is built entirely
  from shell-core pieces too — `shell_core::menu::build_tab_nav` (⌘⇧[ / ⌘⇧] cycle, ⌘1–9 jump, and
  the ⌘1/⌘2 cycle aliases in `cycle` mode — see *Keyboard tab navigation* below) around the spine's
  `Close Tab` (⌘W) and `Pop Out Tab` (⌘⇧O). curator's Tab submenu additionally has Reload Tab /
  Reset All Tabs / Open Developer Tools; warden's has Reopen Last Closed — none of *those* map onto
  lector: **compositor's file watcher already live-reloads every open tab on save**, so a manual
  "Reload Tab" item would be a no-op button; there is no per-tab "session" to reset (a compositor
  `SiteServer` has no state beyond the served files); and DevTools is one keystroke away regardless.
  Other tab-scoped actions (reveal-in-Finder, copy the served URL, open the repo dir) are open
  roadmap, not excluded — the submenu is just empty of them for now.
- **The whole menu is built by one function, `install_app_menu` (`lib.rs`), called at setup AND
  again on every clean hot-reload** — not just once at launch like the spine/home-surface adoption
  originally left it. This is what lets `tab_digit_keys` (below) flip live: a hot-reload's
  `install_app_menu` call rebuilds the Tab submenu in the new mode and the Window submenu's entries
  in the same pass, without a relaunch.
- **The home surface** (`shell_core::home::{home_state, show_home, close_home}`) is
  what a fresh install shows: before this, lector built zero windows and no menu when
  `~/.config/lector/config.toml` didn't exist, so it launched to a live, invisible, unrecoverable
  process. Its "Create a starter config" button is `commands::shell_home_create_config`, calling
  `config_core::write_default_config` with the tracked `src/default-config.toml` template.
- **`appName: "lector"`** (`src/chrome.js`'s mount config) names the app in chrome-core's
  `#cc-titlebar` strip beside the traffic lights; `src/chrome.css`'s `#sidebar` carries no
  `padding-top` of its own — chrome-core owns that inset now.

## Keyboard tab navigation

The Tab menu's nav block (⌘⇧[ / ⌘⇧] cycle, ⌘1–9 jump) is `shell_core::menu::build_tab_nav`,
shared with warden and curator — see *The menu spine* above for where it's spliced into lector's
Tab submenu and rebuilt on hot-reload. **`tab_digit_keys`** (`Config`, whole-app, no per-window
cascade) picks what ⌘1/⌘2 do: default `jump` — ⌘1–⌘9 jump straight to a tab position; `cycle`
makes ⌘1 next tab / ⌘2 previous and shifts the jump items to ⌘3–⌘9. An unrecognised token is a
load error (`config_core::TabDigitKeys`'s own serde impl), not a silent fallback.

- **The menu handler is mode-blind.** `shell_core::menu::tab_nav_action` resolves a fired item's id
  to `Next`/`Prev`/`Jump(n)` — the ⌘1/⌘2 cycle aliases collapse onto `Next`/`Prev` before lector
  ever sees them — so `lib.rs`'s `on_menu_event` just emits `nav-tab` (payload ±1) or `jump-tab`
  (payload = 1-based position) to the focused window's chrome, exactly like `close-tab`/
  `pop-out-tab`.
- **The chrome routes through the normal click path.** `src/chrome.js`'s `nav-tab`/`jump-tab`
  listeners call chrome-core's `selectByOffset`/`selectByIndex`, so a cold tab that a cycle or
  jump lands on still starts its `compositor serve` loop on demand — nothing here bypasses
  `select_tab`. Cycling spans **every** tab (`liveOnly: false`): lector has no cold-skip concept
  the way a throttled/hidden tab might mean elsewhere.

## Tab pop-out: detach a tab into its own window

A ⤢ control on each sidebar row (chrome-core's `onPopOut`) and **Pop Out Tab (⌘⇧O)** (the shared
menu spine, spliced into lector's own Tab submenu — see above) pop the origin window's active tab
into its own **banner-only detached window** (`shell-core`'s shared detach shell,
`shell_core::detach::open_detached`). Closing that window returns the tab to its origin — reopening
the origin first if the user closed it while the tab was out.

- **The defining property: recreate on the SAME running server, not stop-and-restart.** A lector
  tab's content is a `compositor serve_handle()` loop bound to a loopback port; lector can move
  neither a server nor a webview between windows, so `pop_out_tab` (`commands.rs`) closes the
  origin's content webview and `webviews::show_on` builds a fresh one on the detached window
  pointed at the exact same port. Returning (`redock`, in `lib.rs`) does the mirror-image
  recreation back on the origin, reusing the port `LectorDetached` recorded. The compositor serve
  loop and its file watcher run continuously across the whole hop, so the pop is near-lossless —
  live-reload-on-save keeps working throughout, and only in-page state a webview recreation can't
  carry (scroll position, in-page navigation) is lost.
- **The server must survive the whole hop — three guards keep it alive and are load-bearing:**
  `pop_out_tab` tears down only the webview (`webviews::close`), never the server; `unload_tab`
  no-ops on a detached tab (so a stray hover-✕ or ⌘W can't stop the server behind a still-open
  detached window); and `reload::reconcile`'s `Servers::retain` unions in the detached labels so a
  hot-reload can't drop a popped-out tab's server mid-session. A *genuinely* removed tab's
  server-stop is deferred to `redock`'s tab-removed branch. See `commands.rs` / `reload.rs`.
- **State:** `AppState.detached: Mutex<HashMap<detached_label, LectorDetached>>`, where
  `LectorDetached { origin_wid, tab_label, port }` — kept **separate from `window_meta`** so
  hot-reload reconcile and the Window submenu never see a detached window as a real one; the home
  surface still counts it (`reload::reconcile_home` folds `has_detached` into `has_windows`) since
  it's a real surface on screen. `detached_tab_labels()` flattens the map to the set of tab labels,
  used by `tab_dtos` to set each row's `detached` flag and by the `unload_tab`/`retain` guards above.
- **Chrome-facing:** `chrome.js`'s DTO map forwards `detached` (the forwarding premise above); a
  detached row renders muted with its ⤢ control suppressed, and clicking it calls
  `raise_popped_window` instead of `select_tab` — no local webview to select, so "select" means
  "raise the popped-out window."
- **Neither `pop_out_tab` nor `redock` ever holds a lock across the window build or a webview
  call.** `pop_out_tab` reads `window_meta()`/starts the server (both release their locks before
  returning) and only then calls `open_detached`/`show_on` with no lock held; `redock` peeks
  `detached` (clone out, lock released), reopens the origin window if needed, then takes and removes
  the entry in a second short-lived lock before recreating the webview.
- **`is_quitting` makes `redock` a no-op during ⌘Q.** A new `RunEvent::ExitRequested` arm spliced
  into the existing `app.run(|_, event| …)` closure sets a `static IS_QUITTING: AtomicBool`
  (`mark_quitting`/`is_quitting`, `lib.rs`) before any window's `Destroyed` fires; `redock` checks
  it first and returns immediately, so a detached window's teardown during quit doesn't reopen its
  origin or recreate a webview while every server is about to be shut down wholesale anyway
  (`RunEvent::Exit`'s `servers.shutdown_all()`).

## Project-tree roots: discover repos under a dir

A `[[window.root]]` block points at a projects dir; every git repo under it (to `depth`, default
`config_core::DEFAULT_ROOT_DEPTH`) becomes a discovered doc tab, rendered as chrome-core's
collapsible folder-tree section with a `⟳` rescan button. The leaf is `{ name, dir, depth }` only —
`shell`/`cmd`/`probe`/`kill` are warden's and are rejected by `deny_unknown_fields`.

- **Discovery is shared, synthesis is per-app.** config-core owns the leaf-free discovery machinery
  (`scan_root` walks the tree stopping at every `.git`; `resolve_root_dir` validates the
  `{name,dir,depth}`; `discover_projects(&[RootDir]) → Vec<DiscoveredProject>` flattens roots into
  `{path, tree_path, section}`), shared with warden. lector owns only the *synthesis*:
  `WindowConfig::tab_views_with_discovered` maps each `DiscoveredProject` onto a doc `TabView`. This
  is the constellation's "config-core owns discovery, the consuming app bridges" seam — never a
  leaf-generic in config-core (see its charter).
- **Synthesized tabs are ALWAYS lazy** (`load_on_open: false`) — a root can synthesize dozens of
  tabs, and eager-starting a `compositor serve` for each at launch is never wanted. A discovered
  tab's server starts on select, like any cold tab.
- **One choke point re-scans.** `reload::reconcile` scans every window's `resolved_roots()` on each
  call, so launch, config hot-reload, and the rescan button all pick up roots through the same path;
  a root-less window scans nothing and behaves exactly as before. Identity is the canonicalized-dir
  label, so a **curated tab shadows a same-dir discovered project** (the discovered duplicate is
  dropped) and a repo reachable via two roots lands once — the opposite of the `-n` suffixing two
  hand-authored same-dir tabs get.
- **`tree`/`tree_path` are forwarded through `chrome.js`'s DTO map** (the forwarding premise above);
  chrome-core renders the folder tree + `⟳` from `tree: true` rows and fires `onRescan`.
- **The `⟳` button is chrome-core's, not a menu item — there is no ⌘R.** `onRescan` invokes the
  `rescan_root` command, which re-reads the config and re-runs `reconcile` (re-scanning disk) via
  `apply_config`, then emits `config-reloaded` to refresh each window; a config that now fails to
  parse keeps last-good and surfaces `config-error`, mirroring the file watcher.

## The `fnv1a_64` / `DefaultHasher` footgun

Any hash driving a **persistent identifier** must use a **fixed** algorithm (`fnv1a_64`), never
`std::hash::DefaultHasher` — std doesn't guarantee stability across Rust releases, so a toolchain
bump silently changes the value and the app reads as "forgot my layout." This is a
constellation-wide rule, single-sourced in
[shell-core's CLAUDE.md](https://github.com/Lockyc/shell-core/blob/main/CLAUDE.md); the
config-crate copy's own rationale and test vectors live at `crates/lector-config/src/hash.rs`.
lector has three such hashes:

- **Window id** — `hash.rs::fnv1a_64` hashes a window's `title` into `window_id`
  (`identity::window_id`), which **is** the Tauri window label shell-core's geometry module keys a
  window's saved bounds by. A rustc bump that reshuffled the hash would silently reset every
  window's saved geometry to defaults — the same "forgot my layout" symptom the intro names, one
  layer up from the geometry-store filename below. Renaming a window's title has the same effect
  on purpose (see `identity.rs`): it's a *new* label, so its old saved bounds are orphaned, not
  corrupted.
- **Tab label identity** — `hash.rs::fnv1a_64` hashes a tab's canonicalized dir into its stable
  webview label; this copy stays in the config crate (label identity is its domain). **A tab's
  *title* is display-only, never an identity or address — duplicates are allowed**, and
  `open_on_launch` is a plain `bool` (unset/`false` = first `load_on_open` tab, `true` = first tab
  even if cold). **Don't reintroduce title-as-address** — an `open_on_launch = "<title>"` arm (or
  any title-keyed lookup) re-imposes title uniqueness and gives first-match on a duplicate. curator,
  lector, and warden share this "title is display-only" rule, so changing it is a family-wide
  decision.
- **Window geometry-store filename** — owned by shell-core, not lector: `register_plugins` derives
  it from the resolved config path via `shell_core::geometry_filename`, which hashes the
  canonicalized path with shell-core's **own** copy of `fnv1a_64` (see shell-core's CLAUDE.md) — a
  separate instance from this crate's, hashing a different domain (a config path, not a window
  title or a tab dir). lector just hands over the path.

## The capabilities-file footgun

lector's own `#[tauri::command]`s need no capabilities entry (with no app ACL manifest, IPC dispatch
lets local-sidebar commands through and rejects remote content webviews) — but that bypass does
**not** cover core *plugin* commands (`core:event`, `core:window`, `updater`, `process`), which
carry their own default-denied permissions regardless. So the sidebar's hot-reload events, drag
region, and updater each need an explicit grant in `src-tauri/capabilities/default.json`, and a
missing grant fails **silently** (the JS `listen()` promise rejects while Rust-side `emit_to` still
returns `Ok`). The full command-isolation model is single-sourced in
[shell-core's CLAUDE.md](https://github.com/Lockyc/shell-core/blob/main/CLAUDE.md)
("command-isolation security model"); lector's verification of it against the pinned tauri source is
in `commands.rs`'s header.

A plugin's permission can only be *named* if tauri-build's ACL discovery sees the plugin as a
**direct** dependency — so `src-tauri/Cargo.toml` carries `tauri-plugin-updater`/
`tauri-plugin-process` entries despite nothing here calling them. **Do not delete them as dead
weight** — the reason and the silent-failure modes are on those entries in `src-tauri/Cargo.toml`.

## Toolchain lockstep

`rustup` picks a toolchain by walking up from the directory `cargo` runs in — never from what's
being compiled. The shared cores are git dependencies, built out of
`~/.cargo/git/checkouts/`, which rustup never looks in — so **a core's own `rust-toolchain.toml`
is inert when lector builds it**; lector compiles the whole dependency graph with its own pin.

- This repo's `rust-toolchain.toml` matches **config-core's canonical pin** — bump config-core
  first, then match it here (and in warden/curator, which share the same pin). The two files
  are the source of truth for the channel; don't restate it in prose, here or anywhere else.
- **compositor is the deliberate exception**: it pins its own, newer channel for its own
  CI/gate equivalence. lector compiles compositor's *source* with lector's pin, not
  compositor's — lector's build is the drift detector for compositor's source; do not "fix"
  the mismatch by matching compositor's channel here. (`cargo tree` and the two
  `rust-toolchain.toml` files answer "which channels?" truthfully; a number written here
  goes stale the next time either moves.)

## The `[patch]` rule and land order

An active `[patch]` path override builds green locally (against a sibling checkout, including
uncommitted edits) and is **unresolvable for every other clone and CI**. Two guards enforce
this: `.githooks/pre-commit` (activated per clone via `git config core.hooksPath .githooks`) and
`just gate`'s own check. **Never hand-edit the `#PATCH:<core>#` lines or commit one active** —
use the recipes:

- `just <core>-dev` (`chrome-dev` / `config-dev` / `shell-dev` / `compositor-dev`) activates
  that core's patch against the sibling `../<core>` checkout.
- `just <core>-pin` re-pins to that sibling's pushed HEAD and deactivates the patch.
- `just cores-pin` re-pins **all four** in one go — it is a fanout over the four `*-pin`
  recipes above, not a second implementation of pinning. If a change to `cores-pin` ever needs a
  `sed` that rewrites a rev directly, that's a sign it has drifted into a shadow implementation
  — fix it to call the recipe instead.

Land order for a change that spans a shared core and lector: push the core change first (its
new rev must exist on the remote before lector can pin it), then run `just <core>-pin` in
lector, build/test, and commit lector's re-pinned `Cargo.toml`/`Cargo.lock`. Run `just gate` (or
at minimum `just cores-pin`) before every commit that isn't itself a core re-pin, to catch a
patch left active by habit.

## Branch model

Two long-lived branches: **`dev`** is the integration trunk — all work lands here. **`main`**
carries the latest tagged release and stays a clean ancestor of `dev`; it advances only by
fast-forwarding to a release commit, or by a documentation-only commit immediately
forward-merged back into `dev`. Never commit code directly to `main`.

## Release model

Version lives in **`src-tauri/Cargo.toml`** — the single source of truth, like
curator's and warden's. Releases are notarized `.app` bundles with an in-app minisign-signed
updater (`tauri-plugin-updater`), mirroring curator's and warden's release shape: bump the
version, tag `v<version>`, fast-forward `main`, publish a GitHub release with notes, then attach
the notarized artifacts via `just release` (`scripts/release.sh`, generated from shell-core,
parameterized by the tracked `scripts/tooling.env`). CI (`.github/workflows/ci.yml`) runs
`just gate` on every push/PR to `main` and `dev`; run `just gate` locally too — the fast loop —
and confirm it's green before tagging.

**What `just release` needs from the build environment** (it is env-driven, and refuses to run
without them): `TAURI_SIGNING_PRIVATE_KEY` — the **contents** of the updater key file, conventionally
`~/.tauri/lector-updater.key` and mode-600, never a path — plus
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, and the Apple notary creds (`APPLE_SIGNING_IDENTITY` pointing at
a Developer ID Application cert, plus `APPLE_ID`/`APPLE_PASSWORD`/`APPLE_TEAM_ID`, or the
`APPLE_API_KEY*` trio). How a given maintainer supplies those is theirs to decide — this repo only
states the interface.

**Building without any of them is supported and expected.** `just build` with the signing/notary vars
unset produces an ad-hoc, unsigned bundle, and `just deploy` strips the Gatekeeper quarantine xattr so
a local copy still runs — the from-source path a contributor gets. Signing is enabled by the
environment, never pinned in `tauri.conf.json`: `createUpdaterArtifacts` is switched on release-only
via `release.sh`'s `--config` override, because baking it in would make **every** `cargo tauri build`
demand the signing key and break the keyless path.

## Generated scripts belong to shell-core

`scripts/release.sh`, `scripts/gen-latest-json.sh`, and `scripts/install-app.sh` are
**generated, git-ignored** — materialized by `src-tauri/build.rs` from the pinned shell-core
rev. **Edit them in shell-core, never here**; a local edit is silently overwritten on the next
build. The tracked `scripts/tooling.env` is the only per-app input they read.

## Deferred work

Conscious deferrals are tracked in [`docs/FOLLOWUPS.md`](docs/FOLLOWUPS.md) — currently the
per-tab Tab-submenu actions (reveal-in-Finder, copy served URL, open repo dir) that nothing yet
needs. **Consult it before "fixing" a gap you've just noticed** — it may be a recorded deferral
with a reason.

## The public repo

lector is a public repo — every tracked file must be self-contained: no references to
machine-local paths, scripts, or personal tooling. Clones, the build-from-source path, and any
future CI only have what's in the tree.
