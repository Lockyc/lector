# lector — agent notes

## Purpose

lector is a macOS Tauri v2 app: a console of grouped tabs over locally-rendered documentation
sites. One tab is one doc repo on disk. Selecting a tab starts a `compositor serve` loop on an
ephemeral loopback port and points a webview at it, so editing a `.md` file live-reloads the tab
with nothing deployed — no build step, no publish, nothing to keep in sync. It's the third
sibling to two existing apps: **warden** curates terminals, **curator** curates browser tabs,
lector curates local documentation.

## Current state vs intended architecture

**This repo currently contains only the baseline scaffold** — the workspace root, the
shared-core `[patch]` machinery, the toolchain pin, `just` recipes, and hooks. No application
code exists yet: `src-tauri/` and `crates/lector-config/` are declared as workspace members but
not yet written, so `cargo build` / `cargo check` currently error on missing manifests. That is
expected at this stage, not a bug — treat every reference below to those crates as the intended
shape, not a claim that it's built yet.

The intended shape mirrors curator's: a Cargo workspace with a platform-neutral config crate
(`crates/lector-config` — parse/validate/format/identity, no Tauri deps, unit-tested standalone)
consumed by the Tauri app crate (`src-tauri/`, package `lector`) — windows, a `SiteServer`
supervisor managing one `compositor::serve_handle()` loop per open tab, the chrome controller,
commands, config hot-reload.

## Workspace layout

- **`src-tauri/`** — the macOS Tauri app (not yet written): windows, the `SiteServer`
  supervisor (one `compositor::serve_handle()` per open tab, each bound to its own ephemeral
  loopback port), the chrome controller, commands, config hot-reload.
- **`crates/lector-config/`** — the config parser (not yet written), extracted into its own
  platform-neutral crate like curator's `curator-config` / warden's `warden-config`: the
  `window → group → tab` schema (a lector tab carries a `dir` — a local doc repo path — rather
  than a `url`), `parse_and_validate` / `load_config`, `resolve_config_path`, identity/hash
  helpers. Re-exports config-core's shared house formatter + colour parsing, the same way
  curator/warden do.

The `[patch]` overrides for all four shared cores live in the **workspace-root `Cargo.toml`** (a
`[patch]` must sit at the workspace root, not a member manifest) — see *The `[patch]` rule and
land order* below.

## The four shared cores — consumption contract

lector pins **four** git dependencies — one more than curator's and warden's three, because
lector also consumes the render/serve engine:

- **compositor** (`https://github.com/Lockyc/compositor`) — the Markdown render/serve engine.
  lector's `SiteServer` calls its `serve_handle()` once per open tab: a non-blocking call that
  runs a doc repo's live-reloading site on a loopback port and returns once bound. Pinned by rev
  in `src-tauri/Cargo.toml`.
- **chrome-core** (`https://github.com/Lockyc/chrome-core`) — the sidebar chrome (grouped tab
  rows, resize-drag, density tokens), shared with warden and curator. A **build-dependency**:
  `src-tauri/build.rs` materializes its CSS/JS into `src/chrome-core.{css,js}` (git-ignored)
  before Tauri embeds `src/`. Pinned by rev in `src-tauri/Cargo.toml`.
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

## The `fnv1a_64` / `DefaultHasher` footgun

`window_state_filename()` (keys `tauri-plugin-window-state`'s persisted-bounds file to the
config path) must use a **fixed** hash algorithm, never `std::hash::DefaultHasher` — its output
is **not** guaranteed stable across Rust releases, so a toolchain bump would silently change the
filename and reset every window's saved bounds. It reads as "the app forgot my layout", never as
a toolchain problem. curator shipped this bug (fixed 2026-07-16); lector must not repeat it.
**lector's single copy of the fix lives in `crates/lector-config/src/hash.rs`** — a small
`fnv1a_64`, pinned by a known-vectors test. shell-core deliberately does **not** own this hash
(see shell-core's own CLAUDE.md dividing line): each consumer hashes its own config path, so the
~8-line duplication is the accepted cost of that boundary. Do not reintroduce `DefaultHasher`,
and do not move this hash into a shared crate to "deduplicate" it.

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

Version lives in **`src-tauri/Cargo.toml`** (once it exists) — the single source of truth, like
curator's and warden's. Releases are notarized `.app` bundles with an in-app minisign-signed
updater (`tauri-plugin-updater`), mirroring curator's and warden's release shape: bump the
version, tag `v<version>`, fast-forward `main`, publish a GitHub release with notes, then attach
the notarized artifacts via `just release` (`scripts/release.sh`, generated from shell-core,
parameterized by the tracked `scripts/tooling.env`). There is no CI — run `just gate` locally
and confirm it's green before tagging.

## Generated scripts belong to shell-core

`scripts/release.sh`, `scripts/gen-latest-json.sh`, and `scripts/install-app.sh` are
**generated, git-ignored** — materialized by `src-tauri/build.rs` from the pinned shell-core
rev. **Edit them in shell-core, never here**; a local edit is silently overwritten on the next
build. The tracked `scripts/tooling.env` is the only per-app input they read.

## The public repo

lector is a public repo — every tracked file must be self-contained: no references to
machine-local paths, scripts, or personal tooling. Clones, the build-from-source path, and any
future CI only have what's in the tree.
