# lector — deferred follow-ups

Known, intentionally-deferred work. Each item is a conscious deferral, not an oversight —
recorded here so it isn't lost. Remove an item when it's done.

## The first release — what's already wired, and the two things that aren't

lector has never been released: **zero tags, no GitHub release**. The release *machinery* is
complete and should not be rebuilt — see CLAUDE.md › *Release model* for the model itself. What
exists: the version at `src-tauri/Cargo.toml` (`tauri.conf.json` deliberately carries no
`version` key — the bundle inherits it), the `just release` recipe, the tracked
`scripts/tooling.env`, and a minted minisign keypair whose public half is in
`tauri.conf.json`'s `plugins.updater.pubkey`.

Two prerequisites are genuinely outstanding:

- **`main` does not exist** — not locally, not on the remote. The branch model (CLAUDE.md ›
  *Branch model*) has `main` carry the latest release as a clean ancestor of `dev`, so the
  first release has to *create* it at the release commit. `scripts/release.sh` will not do
  this for you: it only asserts the tag exists and that `gh release view` finds the release,
  then builds and attaches artifacts. Creating and pushing `main` is a manual first-release
  step.
- **The updater endpoint 404s until that release lands.** `tauri.conf.json`'s
  `plugins.updater.endpoints` points at `releases/latest/download/latest.json`, and there are
  no releases — so a locally `just deploy`-ed lector's update check fails on launch. This
  resolves itself at the first release *provided* it goes out through `just release`, which
  generates and attaches `latest.json`. A release tagged and published by hand, without that
  step, leaves every install unable to update.

## Installer (`install.sh` + `/lector:install`) — deferred to land *with* the first release

warden and curator both ship a keyless `install.sh`, a matching `/<repo>:install` slash
command, and a README `## Install` section; lector ships none of the three, so the only path a
reader has today is a manual clone plus `just build`. This is a conscious deferral, not an
oversight: an installer is a net-new *user-facing* surface, and it should arrive with the
first release rather than ahead of one — an install path that fetches from a releases page
with no releases on it is worse than no install path at all.

When it lands, template it from the house `install.{sh,md}.tauri` with `<TAURI_DIR>=src-tauri`
and `<APP_BUNDLE>=target/release/bundle/macos/lector.app` (this is a Cargo workspace, so
`target/` sits at the repo root, not under `src-tauri/`).

## GitHub repo surface — topics, Wiki, Projects

The repo is public with a good description, but carries **no topics**, and has **Wiki and
Projects enabled** — all three diverge from the house `github-repo-standards` baseline that
warden/curator/compositor follow. Deferred only because it's an outward-facing `gh repo edit`
that wants a human to authorise it, not because there's anything to decide.

## The compositor pin sits behind, and that is fine — but the drift detector goes stale with it

lector is compositor's **only** consumer and pins it by rev in `src-tauri/Cargo.toml`.
compositor pins its own `rust-toolchain.toml` a channel *ahead* of the constellation's, and
lector compiles compositor's **source** with lector's own pin (rustup resolves from the
directory `cargo` runs in, never from `~/.cargo/git/checkouts/`) — so **lector's build is the
designated drift detector** for compositor's source against the constellation channel. See
CLAUDE.md › *Toolchain lockstep*; do not "fix" the channel divergence by matching compositor's.

The consequence that is easy to miss: **while the pin sits still, the detector is not
detecting.** Every compositor commit made since the pinned rev is source that has never been
compiled under lector's channel, so the drift it exists to catch is *deferred, not absent* —
it surfaces all at once, loudly and locally, at the next `just compositor-pin`. That is the
designed failure mode and it is a good one, but it means a long-idle pin quietly accumulates
the very risk the arrangement was set up to surface early.

**Pinning forward is a decision, not a fix** — a lagging pin is not a defect as long as the
pinned rev is pushed and resolvable (it is; that's the invariant that actually matters for
every other clone and CI). Worth doing deliberately before cutting the first release, so v0.1.0
doesn't ship an engine that far behind the one it's developed against.
