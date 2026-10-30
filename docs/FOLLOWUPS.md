---
type: reference
description: Conscious, intentionally-deferred follow-ups for lector.
links:
  - rel: part-of
    to: CLAUDE.md
---

# lector — deferred follow-ups

Known, intentionally-deferred work. Each item is a conscious deferral, not an oversight —
recorded here so it isn't lost. Remove an item when it's done.

## Tab submenu — more per-tab actions

lector's **Tab** submenu holds only the family's shared items (`shell_core::menu::build_tab_menu`
with no app groups). Other tab-scoped actions are natural next additions, deferred only because
nothing yet needs them:

- **Reveal repo dir in Finder** — the doc repo backing the active tab (`dir`).
- **Copy the served URL** — the ephemeral `http://127.0.0.1:<port>/` the tab's server is bound to.
- **Open the repo dir** in the user's editor / terminal.

None of curator's or warden's Tab items map onto lector: compositor's file watcher already
live-reloads every open tab on save (so a "Reload Tab" would be a no-op), a `SiteServer` has no
per-tab session to reset, and a release build has no inspector. To add any of the above, pass them
as a group to `build_tab_menu` in `lib.rs`'s `refresh_menu` and back them with a command in
`commands.rs`.
