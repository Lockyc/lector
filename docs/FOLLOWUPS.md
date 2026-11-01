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

## Route content-webview teardown through shell-core — closed tabs still run

shell-core owns the page-ending teardown (`shell_core::web::content::{close_content_webview,
close_window_pages}`: wry over-retains a dropped `WKWebView`, so a closed tab's page keeps running
unless `-[WKWebView _close]` ends it first). lector's tab teardowns still call a bare `close()`
(`webviews.rs` stale-webview replace and `close`, `lib.rs`'s unload and window-close paths), so
its closed tabs keep running in the background — silent, since lector has no notification shims to
make it audible. **Unlock:** route every one of those paths through the two shell-core functions.
