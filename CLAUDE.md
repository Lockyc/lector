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
manifest pinning all four cores, `build.rs`, `tauri.conf.json`, `window_state_filename()`, the
`Servers` supervisor (`servers.rs` — one `compositor::serve_handle()` per open tab, idempotent
start/stop/retain/shutdown_all, dead-thread detection via `reap`/`is_alive`), the chrome controller
(`src/chrome.js` + the commands it calls in `commands.rs`), content-webview management with link
escape (`webviews.rs` — `is_own_origin` keeps a doc's off-site links from stranding the tab, and
another tab's own loopback port is escaped too), and `run()`'s full setup: config load, window
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
  fields to its own commands.
- **config-core** (`https://github.com/Lockyc/config-core`) — the TOML config engine (parse,
  validate, format, hot-reload diff) behind the shared house formatter. Pinned by rev in
  `crates/lector-config/Cargo.toml`, kept at the **same rev** as warden's and curator's own
  config-core pins.
- **shell-core** (`https://github.com/Lockyc/shell-core`) — shared release tooling (the three
  release scripts, materialized git-ignored into `scripts/`) plus a sliver of Tauri runtime
  setup (`register_plugins` for window-state/updater/process). **Pinned twice in
  `src-tauri/Cargo.toml`**, per its zero-dep/runtime feature split:
  - `[build-dependencies]`: `default-features = false` — `build.rs` needs only `build_stamp()`
    and the embedded script consts, which are zero-dependency.
  - `[dependencies]`: `features = ["runtime"]` — `register_plugins` needs the tauri tree.

  **The `default-features = false` on the build-dep is load-bearing**: without it, every
  consumer's `[build-dependencies]` drags in the whole tauri tree just to run `build.rs`.
  Resolver 2 resolves the two feature sets independently. Bump both entries to the same rev in
  lockstep.

All four are git dependencies, git-ignored/materialized, or `[patch]`-overridable for local dev
— never vendor one in-tree.

## The menu spine, home surface, and app-name strip — all shared, none app-specific here

lector adopted shell-core's `menu::build_spine` and `home::{home_state, show_home, close_home}`,
and chrome-core's `appName` field, from scratch — it never had its own menu, error window, or
launcher to delete, so it's the cleanest case of the three apps consuming these.

- **lector contributes no app-specific menu items.** The App, Config, and Window submenus are the
  shared spine (`shell_core::menu::build_spine`, called in `lib.rs`'s `run()`); lector's own **Tab**
  submenu holds only the spine's own `Close Tab` (⌘W) and `Pop Out Tab` (⌘⇧O) items — both defined
  by the spine, just spliced into lector's Tab submenu rather than the spine's own. curator's Tab
  submenu has Reload Tab / Reset All Tabs / Open Developer Tools; warden's has digit-mode jumps and
  Reopen Last Closed — none of *those* map onto lector: **compositor's file watcher already
  live-reloads every open tab on save**, so a manual "Reload Tab" item would be a no-op button;
  there is no per-tab "session" to reset (a compositor `SiteServer` has no state beyond the served
  files); and DevTools is one keystroke away regardless. So today the Tab submenu holds only ⌘W and
  ⌘⇧O (the family's Close Tab standard, plus the pop-out feature — see *Tab pop-out* below). Other
  tab-scoped actions (reveal-in-Finder, copy the served URL, open the repo dir) are open roadmap,
  not excluded — the submenu is just empty of them for now.
- **The home surface** (`shell_core::home::HOME_LABEL`, `skip_labels` in `register_plugins`) is
  what a fresh install shows: before this, lector built zero windows and no menu when
  `~/.config/lector/config.toml` didn't exist, so it launched to a live, invisible, unrecoverable
  process. Its "Create a starter config" button is `commands::shell_home_create_config`, calling
  `config_core::write_default_config` with the tracked `src/default-config.toml` template.
- **`appName: "lector"`** (`src/chrome.js`'s mount config) names the app in chrome-core's
  `#cc-titlebar` strip beside the traffic lights; `src/chrome.css`'s `#sidebar` carries no
  `padding-top` of its own — chrome-core owns that inset now.

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
- **The crux footgun this shipped with: the server must survive the entire hop, and three
  separate paths could have killed it.**
  - `pop_out_tab` itself closes the origin's content webview but **never calls `Servers::stop`** —
    only `webviews::close` (webview teardown), never `AppState::unload` (which would stop the
    server).
  - `unload_tab` **no-ops on a detached tab** (`state.detached_tab_labels().contains(&label)` guard,
    checked first thing): chrome-core's live/unload dot stays fully interactive on a detached row
    (only the ⤢ control itself is suppressed), so a stray hover-✕ or the ⌘W accelerator can still
    reach `unload_tab` for a popped-out tab. Without the guard it would stop the server backing the
    still-open detached window out from under it.
  - `reload::reconcile`'s `Servers::retain(&labels)` **unions in `state.detached_tab_labels()`**
    before retaining, so a hot-reload whose new config drops (or relabels, via a changed `dir`) a
    popped-out tab does not stop that tab's server mid-session. The server-stop for a *genuinely*
    removed tab is correctly deferred to `redock`'s own tab-removed branch (`state.view(&tab_label)
    .is_none()` → `state.servers.stop(&tab_label)`), which only fires once the tab actually comes
    home to find no config slot left for it.
- **State:** `AppState.detached: Mutex<HashMap<detached_label, LectorDetached>>`, where
  `LectorDetached { origin_wid, tab_label, port }` — kept **separate from `window_meta`** so
  hot-reload reconcile and the Window submenu never see a detached window as a real one; the home
  surface still counts it (`reload::reconcile_home` folds `has_detached` into `has_windows`) since
  it's a real surface on screen. `detached_tab_labels()` flattens the map to the set of tab labels,
  used by `tab_dtos` to set each row's `detached` flag and by the `unload_tab`/`retain` guards above.
- **Chrome-facing:** `TabPayload.detached` is forwarded through `chrome.js`'s DTO mapping (a new
  DTO field is invisible to the chrome until that mapping forwards it — the same trap the `tree`/
  `treePath` footgun elsewhere in this file describes); a detached row renders muted with its ⤢
  control suppressed, and clicking it calls `raise_popped_window` instead of `select_tab` — there's
  no local webview to select, so "select" means "bring the popped-out window forward."
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

## The `fnv1a_64` / `DefaultHasher` footgun

`window_state_filename()` (keys `tauri-plugin-window-state`'s persisted-bounds file to the
config path) must use a **fixed** hash algorithm, never `std::hash::DefaultHasher` — its output
is **not** guaranteed stable across Rust releases, so a toolchain bump would silently change the
filename and reset every window's saved bounds. It reads as "the app forgot my layout", never as
a toolchain problem. curator shipped this bug (fixed 2026-07-16); lector must not repeat it.
**lector's single copy of the fix lives in `crates/lector-config/src/hash.rs`** — a small
`fnv1a_64`, pinned by a known-vectors test. shell-core deliberately does **not** own this hash
(see shell-core's own CLAUDE.md dividing line): each consumer hashes its own config path, so the
~8-line duplication is the accepted cost of that boundary. Do not reintroduce `DefaultHasher` —
its output's cross-release instability is the real constraint here; the hash's *location* is not,
so sharing it later is fine if the shell-core boundary ever changes.

## The capabilities-file footgun: `has_app_manifest()` bypass does not cover core plugins

`commands.rs`'s header doc works out that with no `src-tauri/capabilities/*.json` at all, Tauri's
IPC dispatch lets every one of *this crate's own* `#[tauri::command]`s through unconditionally for
the local sidebar webview (`has_app_manifest()` is false, and dispatch only requires a resolved ACL
when `has_app_acl_manifest || !is_local`). **That analysis is correct, but it does not extend to
core *plugin* commands** (`core:event`, `core:window`, `updater`, `process`, …) — those carry their
own default-denied permission set independent of whether the app ships a manifest at all.

**Footgun (found 2026-07-17): lector shipped with zero capabilities for two tasks' worth of work,
and `event.listen`/window-drag silently no-op'd the whole time.** Config hot-reload's
`config-reloaded`/`config-error` events (`lib.rs`'s watcher, `src/chrome.js`'s `listen(...)`) never
fired — `emit_to` on the Rust side returned `Ok(())` every time (success there only means no
serialization/argument error, never that a listener existed), while the JS `listen()` promise
silently rejected with `"event.listen not allowed. Permissions associated with this command:
core:event:allow-listen, core:event:default"`. The sidebar's `data-tauri-drag-region` (the
`sidebar_drag` config flag) was equally broken, needing `core:window:allow-start-dragging`. The fix
is `src-tauri/capabilities/default.json`, granting the sidebar (`windows: ["*"]`, `webviews:
["*"]`, no `remote` block — content webviews are `Origin::Remote` and never match) exactly
`core:event:allow-listen`/`allow-unlisten`, `core:window:allow-start-dragging`/
`allow-internal-toggle-maximize`, and the updater's `updater:default`/`process:allow-restart`
— the last two being exactly what the direct-dependency entries below exist to make
discoverable.

**`src-tauri/Cargo.toml`'s `tauri-plugin-updater`/`tauri-plugin-process` entries are load-bearing
despite nothing in this crate calling them — do not delete them as dead weight.** Registration flows
through shell-core's `register_plugins`, but tauri-build's ACL/permission-schema discovery only walks
a crate's *direct* dependencies, so a plugin behind shell-core is invisible to it and
`capabilities/default.json` cannot name its permission. The direct entries exist solely to make
discovery see them (curator does the same). Removing them while the capability names remain fails the
build loudly (`"Permission updater:default not found"`); removing both together is the quiet failure —
the updater silently loses its grant and simply stops updating.

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

Conscious deferrals are tracked in [`docs/FOLLOWUPS.md`](docs/FOLLOWUPS.md) — the outstanding
first-release prerequisites (`main` doesn't exist yet; the updater endpoint 404s until a
release lands), the installer trio deferred to land *with* that release, and the GitHub repo
surface. **Consult it before "fixing" a gap you've just noticed** — it may be a recorded
deferral with a reason.

## The public repo

lector is a public repo — every tracked file must be self-contained: no references to
machine-local paths, scripts, or personal tooling. Clones, the build-from-source path, and any
future CI only have what's in the tree.
