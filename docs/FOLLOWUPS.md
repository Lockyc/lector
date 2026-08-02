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

lector's **Tab** submenu currently holds the family's `Close Tab` (⌘W), `Pop Out Tab` (⌘⇧O), and
the shared keyboard tab-nav block (⌘⇧[ / ⌘⇧] , ⌘1–9). Other tab-scoped actions are natural next
additions, deferred only because nothing yet needs them:

- **Reveal repo dir in Finder** — the doc repo backing the active tab (`dir`).
- **Copy the served URL** — the ephemeral `http://127.0.0.1:<port>/` the tab's server is bound to.
- **Open the repo dir** in the user's editor / terminal.

None of curator's or warden's Tab items map onto lector: compositor's file watcher already
live-reloads every open tab on save (so a "Reload Tab" would be a no-op), a `SiteServer` has no
per-tab session to reset, and DevTools is one keystroke away. To add any of the above, extend
the shared menu spine's Tab-submenu splice in `lib.rs` and back it with a command in `commands.rs`.

## Window submenu doesn't refresh on open/close

lector's Window submenu (`install_app_menu`, `lib.rs`) is rebuilt at setup and on every clean
config hot-reload, but nothing rebuilds it when a window opens or closes in between — it still
only tracks the window set as of the last load/reload. curator closes this exact gap with
`refresh_window_menu`, called from its window-destroy path, so its Window submenu is always
current. Wiring lector's window open (`open_or_focus_window`) and close paths to
`install_app_menu` the same way would close it here too.
