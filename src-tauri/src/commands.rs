//! The Tauri command surface + the managed [`AppState`]. lector's sidebar chrome (`src/chrome.js`)
//! is the intended caller of these, but unlike curator there is no `is_chrome_caller`-style
//! per-caller gate here — Tauri's own IPC dispatch already does the job for the shape this app has.
//!
//! The general model — why a command needs a label gate, or doesn't — is single-sourced in
//! **shell-core's CLAUDE.md ("command-isolation security model")**; what follows is lector's own
//! verification of it against the pinned tauri source.
//!
//! The sidebar is the window's MAIN webview, loaded from `frontendDist` (`tauri://…`), so Tauri's
//! `is_local_url` (`webview/mod.rs`) classifies it `Origin::Local`. Every content webview instead
//! loads `http://127.0.0.1:{port}/` — a compositor server's loopback address, which matches none of
//! `is_local_url`'s cases (the `tauri://` asset protocol, the configured dev URL, or a registered
//! custom scheme) — so Tauri classifies it `Origin::Remote`.
//!
//! This crate's own `#[tauri::command]`s (everything below) need no capabilities entry: with no
//! app-defined ACL manifest, `RuntimeAuthority::has_app_manifest()` is false and dispatch
//! (`webview/mod.rs:on_message`) only *requires* a resolved ACL when `has_app_acl_manifest ||
//! !is_local` — for the local sidebar that's false, so every command here is let through
//! unconditionally; for a remote content webview `!is_local` is true and the invoke is rejected
//! before any command body runs, gate or no gate. Verified against the pinned `tauri = 2.11.5`
//! (`Cargo.lock`) vendored source.
//!
//! **This bypass does NOT extend to core *plugin* commands** (`core:event`, `core:window`, …) —
//! those are gated by their own default-denied permission set regardless of `has_app_manifest()`,
//! and lector genuinely ships a `src-tauri/capabilities/default.json` granting the sidebar
//! `core:event:allow-listen`/`allow-unlisten` (hot-reload's `config-reloaded`/`config-error`
//! events, `lib.rs`) and `core:window:allow-start-dragging` (the sidebar's
//! `data-tauri-drag-region`). **Footgun (found 2026-07-17): before that file existed, both
//! silently no-op'd** — `event.listen` rejects with `"not allowed"` from the plugin's own ACL, not
//! from this crate's dispatch path, so the analysis above (which is correct for *this crate's*
//! commands) doesn't cover it. Discovered because hot-reload's `listen("config-reloaded", …)`
//! never fired despite the Rust side confirmed reconciling and `emit_to` returning `Ok(())` —
//! `emit_to` succeeding only means no serialization/argument error, never that a listener actually
//! received it. The same gap still applies to `updater:default`/`process:allow-restart`
//! (chrome-core's in-app updater) — see `capabilities/default.json`'s own doc for why those two
//! can't be added the same way.
//!
//! **Footgun:** the local/remote split (for this crate's own commands) is by *origin*, not by
//! "chrome vs. anything else local" — it does not distinguish a second local-origin surface from
//! the sidebar. **This is no longer hypothetical**: the home surface (`shell_core::home`, wired in
//! `lib.rs`) is exactly that second surface, serving `home.html` over its own registered
//! `shell-home://` custom protocol — which `is_local_url` also classifies `Origin::Local` (Tauri
//! treats any Builder-registered custom protocol as local; see `shell_core::home::HOME_SCHEME`'s
//! own doc). It rides the same free pass the sidebar gets, reaching this crate's three
//! `shell_home_*` commands with no per-caller gate. Accepted, not a bug to fix: unlike curator's
//! hypothetical case, the home surface's HTML is fixed (bundled into shell-core, never
//! user-supplied), so there is no untrusted content to isolate it from — the risk this footgun
//! originally flagged (an attacker-controlled local surface) doesn't apply here. Content webviews
//! staying `External` (as `webviews.rs` documents) is what keeps *that* risk closed; a real third
//! local surface hosting anything other than fixed, shell-core-bundled content would need an
//! explicit gate (a curator-style `is_chrome_caller`).

use crate::servers::Servers;
use lector_config::{Density, TabView};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::Manager as _;

/// lector's starter config. Tracked, and `include_str!`'d so a missing/renamed template is a build
/// error rather than a runtime surprise.
const DEFAULT_CONFIG: &str = include_str!("../../src/default-config.toml");

/// One row as the chrome controller consumes it. Deliberately flat and app-shaped — chrome-core has
/// no Rust DTO; `src/chrome.js` maps this onto the component's TabDTO.
#[derive(Debug, Clone, Serialize)]
pub struct TabPayload {
    pub label: String,
    pub title: String,
    pub group: Option<String>,
    /// True iff this repo's server is registered AND still answering — the registry's membership
    /// probed for life, never the config's `load_on_open`. See `Servers::is_alive`.
    pub loaded: bool,
    pub active: bool,
    /// Popped out into its own detached window (`pop_out_tab`). Its server keeps running (`loaded`
    /// reflects that, unaffected) — only its content webview on THIS window is gone. The chrome
    /// renders a ⤢ mark and maps a click on the row to "raise the popped-out window"
    /// (`raise_popped_window`) rather than selecting it. Invisible to the component until
    /// `chrome.js`'s DTO mapping forwards it.
    pub detached: bool,
    /// A project-tree (root)-discovered row. chrome-core renders a run of these under one section as
    /// a collapsible folder tree with a `⟳` rescan button. `false` for curated tabs. Invisible to
    /// the component until `chrome.js` forwards it (the same trap as `detached`).
    pub tree: bool,
    /// Folder segments between the root dir and this project — chrome-core nests by these.
    pub tree_path: Vec<String>,
}

/// Project the resolved tabs + the live registry into rows for the chrome. `views` must already be
/// in the order the sidebar should render (the ordering itself is `AppState::views_for_window`'s
/// job, not this function's). `detached` is the set of tab labels currently popped out into their
/// own window (`AppState::detached_tab_labels`), passed in rather than an `&AppState` so this stays
/// unit-testable with plain values, like the rest of this function's inputs.
pub fn tab_dtos(
    views: &[TabView],
    servers: &Servers,
    active: Option<&str>,
    detached: &HashSet<String>,
) -> Vec<TabPayload> {
    views
        .iter()
        .map(|v| TabPayload {
            label: v.label.clone(),
            title: v.title.clone(),
            group: v.group.clone(),
            loaded: servers.is_alive(&v.label),
            active: active == Some(v.label.as_str()),
            detached: detached.contains(&v.label),
            tree: v.tree,
            tree_path: v.tree_path.clone(),
        })
        .collect()
}

/// A configured window's fixed identity — enough to rebuild it after the user closes it (the
/// Window menu's reopen path) or to describe it to the menu spine / home surface. `id`/`title`/
/// `colour` never change after launch (window blocks aren't added/removed by hot-reload — only
/// tabs are, via `reload::reconcile`); `width`/`height` are its *initial* size, reused verbatim on
/// a rebuild since Tauri doesn't remember a closed window's last size once it's gone.
#[derive(Debug, Clone)]
pub struct WindowMeta {
    pub id: String,
    pub title: String,
    pub width: f64,
    pub height: f64,
    pub colour: Option<String>,
}

/// A tab popped out into its own detached window (`pop_out_tab`). Kept in [`AppState::detached`],
/// **separate from `window_meta`**, so hot-reload reconcile and the Window submenu never see it —
/// the same reason curator keeps its equivalent (`CuratorDetached`) out of its window registry.
/// `port` is recorded rather than re-derived from `Servers::port` on redock: the server is never
/// stopped across the hop (that's the whole point of a near-lossless pop-out), so the port a
/// `pop_out_tab` call resolved is still the right one, and storing it here means `redock` needs no
/// extra registry lookup to reuse it.
pub struct LectorDetached {
    pub origin_wid: String,
    pub tab_label: String,
    pub port: u16,
}

/// The app's managed state. One instance, registered with `.manage()` in `run()`.
///
/// Deviates from the task brief's literal shape in two ways, both load-bearing:
/// - `views` is a single ordered `Vec<TabView>`, not a `HashMap` alongside a second ordering
///   structure. `WindowConfig::tab_views()` already returns tabs in the spec's required order
///   (loose-first, then groups in file order — pinned by a `lector-config` test); concatenating
///   each window's `tab_views()` in turn and filtering/cloning from that one `Vec` preserves it for
///   free. A `HashMap` plus a parallel ordering `Vec` would be two structures that can drift — the
///   "one source of truth" rule this codebase holds elsewhere. `view()`'s linear scan is fine at
///   this scale (tens of tabs, not thousands).
/// - `colours` and the three global-config fields (`density`/`sidebar_drag`/`auto_update`) are new:
///   the brief's `AppState` (servers/views/active only) has no way to answer `window_identity`'s
///   `colour`/`density`/`sidebar_drag`/`auto_update` fields, which come from `WindowConfig`/`Config`
///   and are never stored anywhere else. `title` needs no such field — it's read straight off the
///   live Tauri `Window` (set once at `build_window` time), which is one fewer thing to keep in
///   sync.
pub struct AppState {
    /// The live serve-loop registry.
    pub servers: Servers,
    /// Last-good resolved tabs, in `tab_views()` order (see the struct doc above). Swapped
    /// wholesale on a clean config hot-reload; a failed reload leaves the previous value in place
    /// (last-good-on-failure), which is why a missing `dir` warns rather than errors.
    views: Mutex<Vec<TabView>>,
    /// The active tab per window label. Chrome-owned selection: lector's Rust side decides, and the
    /// DTO's `active` field tells chrome-core to honour it rather than auto-firing onSelect.
    active: Mutex<HashMap<String, String>>,
    /// Per-window accent colour (window id → the window's optional hex colour), set once at setup.
    colours: Mutex<HashMap<String, Option<String>>>,
    /// Whole-app chrome density from the config, set once at setup. `window_identity` re-reads it
    /// live (rather than trusting a launch-time JS snapshot) so a future hot-reload can update it.
    density: Mutex<Density>,
    /// Whole-app `sidebar_drag` from the config, set once at setup.
    sidebar_drag: AtomicBool,
    /// Whole-app `auto_update` from the config, set once at setup.
    auto_update: AtomicBool,
    /// Every configured window's fixed identity (see [`WindowMeta`]), for the menu spine's Window
    /// submenu and reopening a closed window from it. Grows as `lib.rs` builds windows (at launch,
    /// and later via `reload_now`); never shrinks — a window that's closed stays in this list so it
    /// can be rebuilt.
    window_meta: Mutex<Vec<WindowMeta>>,
    /// Tabs currently popped out into their own detached window, keyed by the detached window's
    /// Tauri label ([`shell_core::detach::detached_label`]). **Separate from `window_meta`** so
    /// reconcile/window-state never touch these ephemeral windows, and so the home-surface check can
    /// still count them (`reload::reconcile_home`) — a detached window is a real surface on screen.
    pub detached: Mutex<HashMap<String, LectorDetached>>,
}

impl AppState {
    pub fn new() -> Self {
        AppState {
            servers: Servers::new(),
            views: Mutex::new(Vec::new()),
            active: Mutex::new(HashMap::new()),
            colours: Mutex::new(HashMap::new()),
            density: Mutex::new(Density::default()),
            sidebar_drag: AtomicBool::new(true),
            auto_update: AtomicBool::new(true),
            window_meta: Mutex::new(Vec::new()),
            detached: Mutex::new(HashMap::new()),
        }
    }

    pub fn view(&self, label: &str) -> Option<TabView> {
        self.views
            .lock()
            .expect("views lock")
            .iter()
            .find(|v| v.label == label)
            .cloned()
    }

    pub fn set_views(&self, views: Vec<TabView>) {
        *self.views.lock().expect("views lock") = views;
    }

    /// This window's tabs, in the order they were resolved (config order — see the struct doc).
    pub fn views_for_window(&self, window_id: &str) -> Vec<TabView> {
        let prefix = format!("{window_id}:");
        self.views
            .lock()
            .expect("views lock")
            .iter()
            .filter(|v| v.label.starts_with(&prefix))
            .cloned()
            .collect()
    }

    pub fn set_active(&self, label: &str) {
        // The window id is the label's namespace prefix (`{window_id}:{within}`), so active is
        // tracked per window without threading a window handle through every command.
        let window = label.split(':').next().unwrap_or_default().to_string();
        self.active
            .lock()
            .expect("active lock")
            .insert(window, label.to_string());
    }

    pub fn active_for(&self, window_id: &str) -> Option<String> {
        self.active
            .lock()
            .expect("active lock")
            .get(window_id)
            .cloned()
    }

    /// Stop `label`'s server and, if it was its window's active tab, clear active for that window.
    /// This is the state half of `unload_tab` — split out from the `#[tauri::command]` wrapper so it
    /// needs no `AppHandle`/webview and is directly unit-testable.
    ///
    /// This function itself never promotes a neighbour — it only tears down and clears. The
    /// `unload_tab` **command** layers neighbour-promotion on top, via `neighbour_label` delegating
    /// to `shell_core::pick_live_neighbour`. Clearing active here is therefore now specifically the
    /// *last-live-tab* (empty background) path, not the always-path: lector keeps every visited tab's
    /// server live in the background (`select`/`webviews::show` never stop a previous tab, and
    /// `webviews::raise_only` only hides it), so a live sibling to promote to usually exists. Leaving
    /// a stale `active` label here regardless is exactly the Finding-1 bug: `get_tabs`' DTO would keep
    /// reporting the cold tab as active, `chrome.js`'s `wasActive` would stay true on a re-click, and
    /// the click would route to `home_tab` (which errors — the server is stopped) instead of
    /// `select_tab` (which would restart it).
    pub fn unload(&self, label: &str) {
        self.servers.stop(label);
        self.clear_active_if(label);
    }

    /// Clear this window's active tab iff it currently points at `label`. No-op when a different
    /// tab (or none) is active — unloading a tab that isn't showing must not disturb the one that is.
    fn clear_active_if(&self, label: &str) {
        let window = label.split(':').next().unwrap_or_default();
        let mut active = self.active.lock().expect("active lock");
        if active.get(window).map(String::as_str) == Some(label) {
            active.remove(window);
        }
    }

    pub fn set_colour(&self, window_id: &str, colour: Option<String>) {
        self.colours
            .lock()
            .expect("colours lock")
            .insert(window_id.to_string(), colour);
    }

    fn colour_for(&self, window_id: &str) -> Option<String> {
        self.colours
            .lock()
            .expect("colours lock")
            .get(window_id)
            .cloned()
            .flatten()
    }

    pub fn set_global(&self, density: Density, sidebar_drag: bool, auto_update: bool) {
        *self.density.lock().expect("density lock") = density;
        self.sidebar_drag.store(sidebar_drag, Ordering::Relaxed);
        self.auto_update.store(auto_update, Ordering::Relaxed);
    }

    /// Every window built so far, in build order. Clones — callers get a snapshot, never a lock
    /// held across their own work (the menu spine and `reload_now` both hold this while calling
    /// back into Tauri APIs that could otherwise re-enter).
    pub fn window_meta(&self) -> Vec<WindowMeta> {
        self.window_meta.lock().expect("window_meta lock").clone()
    }

    /// Replace the whole window-meta list. Callers build the new list starting from
    /// [`window_meta`](Self::window_meta) (append-only in practice — see the field's own doc).
    pub fn set_window_meta(&self, meta: Vec<WindowMeta>) {
        *self.window_meta.lock().expect("window_meta lock") = meta;
    }

    /// Every tab label currently popped out into its own window, across every window. Labels are
    /// already window-namespaced (`{window_id}:tab-hash`), so a flat set needs no per-window split —
    /// used by [`tab_dtos`] to set each row's `detached` flag.
    pub fn detached_tab_labels(&self) -> HashSet<String> {
        self.detached
            .lock()
            .expect("detached lock")
            .values()
            .map(|d| d.tab_label.clone())
            .collect()
    }
}

impl Default for AppState {
    fn default() -> Self {
        AppState::new()
    }
}

/// Select a tab: start its server if cold, then point its webview at the port. A start failure
/// leaves the tab cold and returns the message. Split out from the `#[tauri::command]` wrapper so
/// `run()`'s launch-time `open_on_launch` selection can call the exact same path a click does,
/// rather than a shadow copy of start-then-show-then-set_active.
pub fn select(app: &tauri::AppHandle, state: &AppState, label: &str) -> Result<(), String> {
    let view = state
        .view(label)
        .ok_or_else(|| format!("no such tab: {label}"))?;
    let dir = lector_config::expand_tilde(&view.dir);
    let port = state.servers.start(label, &dir)?;
    crate::webviews::show(app, label, port)?;
    state.set_active(label);
    Ok(())
}

/// The `#[tauri::command]` wrapper around [`select`] — the caller surfaces a start failure via the
/// chrome's setError.
#[tauri::command]
pub fn select_tab(
    label: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    select(&app, &state, &label)
}

/// The label to promote to active after `unloaded` is closed, given this window's views in render
/// order and a parallel liveness vector (`live[i]` = view `i`'s server is up). Delegates the index
/// policy to shell-core so lector agrees with warden and curator: nearest live neighbour, else
/// `None` (→ empty background). Pure — split from `unload_tab` so it is unit-testable with plain
/// bools, no real servers.
fn neighbour_label(views: &[TabView], unloaded: &str, live: &[bool]) -> Option<String> {
    let idx = views.iter().position(|v| v.label == unloaded)?;
    shell_core::pick_live_neighbour(idx, live).map(|n| views[n].label.clone())
}

/// Unload a tab: stop its server, close its content webview, and — if it was the window's active
/// tab — clear active (see `AppState::unload`'s doc for the full failure chain this prevents), then
/// promote the nearest still-live neighbour so the hole shows another loaded tab rather than the
/// empty background — background only when this was the last live tab. The tab's content is
/// regenerated from disk on the next select, so nothing is lost.
///
/// No-op if `label` is currently popped out into its own detached window. chrome-core's live/unload
/// dot stays fully interactive on a `detached` row (only the pop-out control itself is suppressed —
/// see chrome-core's `_renderRow`), so a stray hover-✕ or the ⌘W accelerator can still reach here for
/// a detached tab. Its origin webview is already gone (nothing to close), and — unlike curator, which
/// has no server to protect — stopping the server here would kill the detached window's still-live
/// content out from under it, defeating the whole point of a near-lossless pop-out.
#[tauri::command]
pub fn unload_tab(label: String, app: tauri::AppHandle, state: tauri::State<'_, AppState>) {
    if state.detached_tab_labels().contains(&label) {
        return;
    }
    let window = label.split(':').next().unwrap_or_default().to_string();
    let was_active = state.active_for(&window).as_deref() == Some(label.as_str());
    // Tear down this tab: stop its server and clear active if it was showing (AppState::unload),
    // then destroy its content webview.
    state.unload(&label);
    crate::webviews::close(&app, &label);
    // If the tab we just closed was the active one, promote the nearest still-live neighbour so the
    // hole shows another loaded tab rather than the empty background — background only when this was
    // the last live tab. The neighbour is already live, so `select`'s `servers.start` no-ops on it.
    if was_active {
        let views = state.views_for_window(&window);
        let live: Vec<bool> = views
            .iter()
            .map(|v| state.servers.is_alive(&v.label))
            .collect();
        if let Some(next) = neighbour_label(&views, &label, &live) {
            let _ = select(&app, &state, &next);
        }
    }
}

/// Pop the tab `label` out of its origin window into its own detached window. lector can't move a
/// server or a webview between windows, so a NEW content webview is created on the detached window,
/// pointed at the SAME running port as the tab's existing (or freshly started) server — this is
/// what makes the pop near-lossless: the compositor serve loop (and its watcher) keeps running
/// throughout, so the detached window's copy picks up exactly where the origin's left off, live
/// reload included. The origin's own content webview for `label` is closed (its app-global label
/// must be free for the detached window's copy) but its **server is never stopped** here — see
/// [`crate::webviews::close`] (webview only) vs [`Servers::stop`] (this command touches neither the
/// latter nor [`AppState::unload`], which calls it).
#[tauri::command]
pub fn pop_out_tab(
    label: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    // Defensive: the chrome already suppresses the pop-out control on a row it knows is detached
    // (`!t.detached` in chrome-core), but a stale double-click could still race the DTO refresh. A
    // second pop of the same label would try to build a second Tauri window under the SAME label
    // (the token is the tab label itself) — `WindowLabelAlreadyExists` — which would then trip the
    // failure-path restore below and stomp the first, still-live detached window's content webview.
    // Reject up front instead.
    if state.detached_tab_labels().contains(&label) {
        return Err(format!("{label} is already popped out"));
    }
    let view = state
        .view(&label)
        .ok_or_else(|| format!("no such tab: {label}"))?;
    let window_id = label.split(':').next().unwrap_or_default().to_string();

    // Resolve the origin window's size/colour up front, before any teardown below — a lookup miss
    // here would mean `window_id` drifted out of sync with `window_meta` (every real window is
    // pushed there at build time, before any command from its webview can run), so it's rejected
    // rather than guessed at, and rejected before this call has any side effect to undo.
    let meta = state.window_meta();
    let (width, height, colour) = meta
        .iter()
        .find(|m| m.id == window_id)
        .map(|m| (m.width, m.height, m.colour.clone()))
        .ok_or_else(|| format!("no window metadata for {window_id}"))?;

    // Ensure the server is live and get its port — idempotent if it's already running (the common
    // case: popping out a tab you're currently looking at). Never stopped by this command.
    let dir = lector_config::expand_tilde(&view.dir);
    let port = state.servers.start(&label, &dir)?;

    // If this was the origin window's active tab, clear it and promote the nearest still-live
    // neighbour into view — the same policy `unload_tab` uses. The popped tab's own server stays
    // "live" throughout, but `neighbour_label`/`pick_live_neighbour` never consider the index being
    // vacated, so it can't promote itself.
    let was_active = state.active_for(&window_id).as_deref() == Some(label.as_str());
    state.clear_active_if(&label);
    crate::webviews::close(&app, &label);
    if was_active {
        let views = state.views_for_window(&window_id);
        let live: Vec<bool> = views
            .iter()
            .map(|v| state.servers.is_alive(&v.label))
            .collect();
        if let Some(next) = neighbour_label(&views, &label, &live) {
            let _ = select(&app, &state, &next);
        }
    }

    // Build the detached window: banner title = the TAB's title (not the window's), accent/size =
    // the ORIGIN window's colour/configured size (resolved up front above) — lector has no per-tab
    // size, same choice curator makes for its equivalent.
    let spec = shell_core::detach::DetachSpec {
        title: view.title.clone(),
        colour,
        width,
        height,
    };
    // A content-webview label is already globally unique (`{window_id}:tab-hash`), so it doubles as
    // the detach token — mirrors curator's `detach_window_token`.
    let token = crate::detach_window_token(&label);
    let birth_hole = crate::webviews::HoleRect {
        x: 0.0,
        y: crate::DETACH_BANNER_H,
        width,
        height: (height - crate::DETACH_BANNER_H).max(0.0),
    };
    let label_for_birth = label.clone();
    let build = shell_core::detach::open_detached(&app, &token, &spec, "lector", |win| {
        let w = win.as_ref().window();
        // The detached window is never passed to `webviews::build_window`, so it has no HOLES
        // entry yet — seed one before docking content, or `show_on` would fall back to a
        // zero-size hole for the first frame. `detach.html`'s own `set_hole_rect` (already a
        // generic command — it doesn't care whether the window is "real" or detached) overwrites
        // this with the exact measured rect moments later.
        crate::webviews::seed_hole(w.label(), birth_hole);
        crate::webviews::show_on(&w, &label_for_birth, port)
            .map_err(|e| tauri::Error::Io(std::io::Error::other(e)))
    });

    let detached_label = match build {
        Ok(l) => l,
        Err(e) => {
            // Build/dock failed: the tab has no webview anywhere (the origin's was already closed,
            // and `open_detached` tears its own half-built window back down on `Err`). Restore it
            // on the origin — visible, since the restored webview IS what `show`'s raise_only just
            // put on screen, so `active` must follow it regardless of whether this tab was active
            // before the attempt. The server was never touched, so nothing to undo there.
            let _ = crate::webviews::show(&app, &label, port);
            state.set_active(&label);
            return Err(format!("couldn't pop out tab: {e}"));
        }
    };

    state.detached.lock().expect("detached lock").insert(
        detached_label.clone(),
        LectorDetached {
            origin_wid: window_id,
            tab_label: label,
            port,
        },
    );
    {
        let app2 = app.clone();
        let label2 = detached_label.clone();
        // wire_return resolves the (multi-webview) detached window by label via get_window itself —
        // a get_webview_window lookup here returns None for it and would silently skip the wiring.
        shell_core::detach::wire_return(&app, &detached_label, move || {
            crate::redock(&app2, &label2)
        });
    }
    Ok(())
}

/// Raise the detached window hosting tab `label` (popped out via [`pop_out_tab`]). The chrome calls
/// this instead of `select_tab` when a row already marked `detached` is clicked — there is no local
/// webview to select, so "select" means "bring its popped-out window forward". No-op if the tab
/// isn't actually popped out (a stale click) or its window is gone.
#[tauri::command]
pub fn raise_popped_window(
    label: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) {
    let target = state
        .detached
        .lock()
        .expect("detached lock")
        .iter()
        .find(|(_, d)| d.tab_label == label)
        .map(|(l, _)| l.clone());
    if let Some(l) = target {
        if let Some(win) = app.get_window(&l) {
            let _ = win.unminimize();
            let _ = win.set_focus();
        }
    }
}

/// Dock tab `label` back into its origin window — the ↩ pop-in overlay on a detached row. Closes
/// the tab's detached window, whose `Destroyed` handler runs `redock` (re-showing the tab on its
/// still-running server/port); the same return path as closing the popped-out window by hand.
#[tauri::command]
pub fn pop_in_tab(label: String, app: tauri::AppHandle, state: tauri::State<'_, AppState>) {
    let target = state
        .detached
        .lock()
        .expect("detached lock")
        .iter()
        .find(|(_, d)| d.tab_label == label)
        .map(|(l, _)| l.clone());
    if let Some(l) = target {
        if let Some(win) = app.get_window(&l) {
            let _ = win.close();
        }
    }
}

/// The invoking window's identity for the chrome banner, plus the whole-app chrome settings.
#[derive(Serialize)]
pub struct Identity {
    pub title: String,
    pub colour: Option<String>,
    pub density: Density,
    pub sidebar_drag: bool,
    pub auto_update: bool,
}

/// Return the calling window's identity so the chrome can paint a per-window banner and apply the
/// whole-app chrome settings. `title` is read straight off the live Tauri window rather than kept
/// as a second copy of the config's title — one source, no chance of drift after a rename.
#[tauri::command]
pub fn window_identity(window: tauri::Window, state: tauri::State<'_, AppState>) -> Identity {
    let window_id = window.label().to_string();
    Identity {
        title: window.title().unwrap_or_default(),
        colour: state.colour_for(&window_id),
        density: *state.density.lock().expect("density lock"),
        sidebar_drag: state.sidebar_drag.load(Ordering::Relaxed),
        auto_update: state.auto_update.load(Ordering::Relaxed),
    }
}

/// The rows for this window's sidebar, with the live registry reaped first so a died-in-the-night
/// server reads as cold rather than as live.
#[tauri::command]
pub fn get_tabs(window: tauri::Window, state: tauri::State<'_, AppState>) -> Vec<TabPayload> {
    state.servers.reap();
    let window_id = window
        .label()
        .split(':')
        .next()
        .unwrap_or_default()
        .to_string();
    let views = state.views_for_window(&window_id);
    tab_dtos(
        &views,
        &state.servers,
        state.active_for(&window_id).as_deref(),
        &state.detached_tab_labels(),
    )
}

/// Re-selecting the already-active tab snaps it home — curator's home-on-active, applied to a local
/// site: navigate back to `/` rather than reloading, so a deep page returns to the site root.
#[tauri::command]
pub fn home_tab(
    label: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let port = state.servers.port(&label).ok_or("tab is not live")?;
    crate::webviews::navigate(&app, &label, &format!("http://127.0.0.1:{port}/"))
}

/// Step the tab's content webview back through its in-page history. No-op if the webview isn't
/// created or there's nothing to go back to (WKWebView history isn't exposed here). Unlike
/// curator's `nav_back`, this has no `require_chrome` gate — see this module's header doc for the
/// verified reason lector's command surface doesn't need one.
#[tauri::command]
pub fn nav_back(label: String, app: tauri::AppHandle) -> Result<(), String> {
    crate::webviews::eval_on(&app, &label, "history.back()")
}

/// Step the tab's content webview forward through its in-page history. No-op if the webview isn't
/// created or there's nothing to go forward to.
#[tauri::command]
pub fn nav_forward(label: String, app: tauri::AppHandle) -> Result<(), String> {
    crate::webviews::eval_on(&app, &label, "history.forward()")
}

/// A content-hole rect reported by the chrome (logical px, top-left), deserialized from the
/// `set_hole_rect` command's `{ rect: {x, y, width, height} }` argument. The chrome (chrome-core)
/// owns the sidebar width and its resize clamp; the flex `#content-hole` follows from CSS, and the
/// chrome reports the measured rect here on mount, on a resize-drag, and on window resize (via a
/// `ResizeObserver`) — this is warden/curator's `set_hole_rect` model, so there is no Rust-side
/// sidebar-width computation to keep in sync with the JS.
#[derive(Deserialize)]
pub struct RectArg {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Position the calling window's content webviews from the chrome's reported `#content-hole` rect.
#[tauri::command]
pub fn set_hole_rect(rect: RectArg, window: tauri::Window) -> Result<(), String> {
    if ![rect.x, rect.y, rect.width, rect.height]
        .iter()
        .all(|v| v.is_finite())
    {
        return Err("non-finite hole rect".into());
    }
    crate::webviews::set_hole_rect(
        &window,
        crate::webviews::HoleRect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        },
    );
    Ok(())
}

/// The home surface's "Create a starter config" button. This is where config-core is called (via
/// `lector_config`'s re-export — this crate never pins config-core directly, the same one-source
/// rule as its other re-exported house helpers) — shell-core owns the surface but never touches
/// config-core (the cores stay independent).
#[tauri::command]
pub fn shell_home_create_config(app: tauri::AppHandle) -> Result<(), String> {
    let path = lector_config::resolve_config_path();
    match lector_config::write_default_config(&path, DEFAULT_CONFIG) {
        // A config already existed — say so rather than report a success that didn't happen.
        Ok(false) => Err(format!(
            "{} already exists — left untouched",
            path.display()
        )),
        Ok(true) => {
            crate::reload_now(&app);
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

/// The home surface's "Edit Config" button (shown for a `Broken` config). Reuses the spine's own
/// Edit Config action rather than a second `open` spawn — same one-source-of-truth reason the menu
/// handler consumes `handle_spine_event` first.
#[tauri::command]
pub fn shell_home_edit_config() {
    let path = lector_config::resolve_config_path();
    shell_core::menu::handle_spine_event(shell_core::menu::ids::EDIT_CONFIG, &path);
}

/// The home surface's per-window button (shown for the `Windows` list state): open, or focus if
/// already open. Same path the menu spine's Window submenu uses for the same id.
#[tauri::command]
pub fn shell_home_open_window(id: String, app: tauri::AppHandle) {
    crate::open_or_focus_window(&app, &id);
}

/// Re-scan every window's `[[window.root]]`s (and re-read the config) and reconcile — the chrome's
/// `⟳` rescan button (chrome-core `onRescan`). Projects added/removed on disk appear/vanish without
/// a config edit. Reuses the whole hot-reload path: `crate::apply_config` re-scans roots (via
/// `reload::reconcile`, which calls `discover_projects` on every window's `resolved_roots()` on
/// each invocation — see that function's doc) and reinstalls the global chrome settings, then a
/// `config-reloaded` emit drives each window's `refresh()`. On a config that now fails to parse,
/// keeps last-good and surfaces `config-error`, exactly like the config file watcher in `lib.rs`'s
/// `run()`.
#[tauri::command]
pub fn rescan_root(app: tauri::AppHandle, state: tauri::State<'_, AppState>) {
    use tauri::Emitter;
    let path = lector_config::resolve_config_path();
    match lector_config::load_config(&path) {
        Ok((cfg, warnings)) => {
            crate::apply_config(&app, &cfg, &warnings);
            for m in state.window_meta() {
                let _ = app.emit_to(m.id.as_str(), "config-reloaded", ());
            }
            let entries = crate::reload::window_entries(&app, &state.window_meta());
            crate::reload::reconcile_home(
                &app,
                &entries,
                &path.to_string_lossy(),
                path.exists(),
                None,
            );
        }
        Err(e) => {
            let msg = e.to_string();
            for m in state.window_meta() {
                let _ = app.emit_to(m.id.as_str(), "config-error", msg.clone());
            }
            let entries = crate::reload::window_entries(&app, &state.window_meta());
            crate::reload::reconcile_home(
                &app,
                &entries,
                &path.to_string_lossy(),
                path.exists(),
                Some(&msg),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_dto_live_reflects_the_registry_not_the_config() {
        // The spec: the `live` dot means exactly "this repo's server is up and watching" — never
        // "load_on_open was true in the config".
        let dir = std::env::temp_dir().join(format!("lector-cmd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs/index.md"), "# X\n").unwrap();

        let servers = crate::servers::Servers::new();
        let views = vec![lector_config::TabView {
            label: "t1".into(),
            group: None,
            title: "X".into(),
            dir: dir.display().to_string(),
            load_on_open: true, // config says eager…
            tree: false,
            tree_path: Vec::new(),
        }];

        // …but nothing has started it, so it is cold.
        let dtos = tab_dtos(&views, &servers, None, &HashSet::new());
        assert!(!dtos[0].loaded, "load_on_open must not imply live");

        servers.start("t1", &dir).unwrap();
        let dtos = tab_dtos(&views, &servers, None, &HashSet::new());
        assert!(dtos[0].loaded, "a started server must read as live");

        servers.shutdown_all();
        let dtos = tab_dtos(&views, &servers, None, &HashSet::new());
        assert!(!dtos[0].loaded, "a stopped server must read as cold");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tab_dto_marks_the_active_tab() {
        let views = vec![
            lector_config::TabView {
                label: "t1".into(),
                group: None,
                title: "A".into(),
                dir: "/tmp".into(),
                load_on_open: false,
                tree: false,
                tree_path: Vec::new(),
            },
            lector_config::TabView {
                label: "t2".into(),
                group: Some("G".into()),
                title: "B".into(),
                dir: "/usr".into(),
                load_on_open: false,
                tree: false,
                tree_path: Vec::new(),
            },
        ];
        let servers = crate::servers::Servers::new();
        let dtos = tab_dtos(&views, &servers, Some("t2"), &HashSet::new());
        assert!(!dtos[0].active);
        assert!(dtos[1].active);
        assert_eq!(dtos[1].group.as_deref(), Some("G"));
    }

    #[test]
    fn tab_dto_marks_the_detached_flag_from_the_passed_in_set() {
        // `tab_dtos` takes the detached-label set as a plain value (not `&AppState`) so this stays
        // unit-testable without a live pop-out — see the function's own doc.
        let views = vec![
            lector_config::TabView {
                label: "t1".into(),
                group: None,
                title: "A".into(),
                dir: "/tmp".into(),
                load_on_open: false,
                tree: false,
                tree_path: Vec::new(),
            },
            lector_config::TabView {
                label: "t2".into(),
                group: None,
                title: "B".into(),
                dir: "/usr".into(),
                load_on_open: false,
                tree: false,
                tree_path: Vec::new(),
            },
        ];
        let servers = crate::servers::Servers::new();
        let detached: HashSet<String> = ["t2".to_string()].into_iter().collect();
        let dtos = tab_dtos(&views, &servers, None, &detached);
        assert!(!dtos[0].detached, "t1 was not popped out");
        assert!(dtos[1].detached, "t2 was popped out");
    }

    #[test]
    fn tab_dtos_forward_tree_fields() {
        let views = vec![lector_config::TabView {
            label: "tab-1".into(),
            group: Some("Dev".into()),
            title: "proj".into(),
            dir: "/tmp/proj".into(),
            load_on_open: false,
            tree: true,
            tree_path: vec!["gh".into()],
        }];
        let servers = Servers::new();
        let dtos = tab_dtos(&views, &servers, None, &HashSet::new());
        assert!(dtos[0].tree);
        assert_eq!(dtos[0].tree_path, vec!["gh".to_string()]);
        servers.shutdown_all();
    }

    #[test]
    fn tab_payload_serializes_the_detached_flag() {
        // The chrome renders the ⤢ mark off this field; if it stops being serialized the sidebar
        // silently can't distinguish a detached tab (the "invisible until forwarded" trap, Rust
        // side — the same one `chrome.js`'s `toComponentDto`-equivalent mapping must also forward).
        let item = TabPayload {
            label: "w1:tab-abc".into(),
            title: "Docs".into(),
            group: None,
            loaded: true,
            active: false,
            detached: true,
            tree: false,
            tree_path: Vec::new(),
        };
        let json = serde_json::to_value(&item).unwrap();
        assert_eq!(json["detached"], serde_json::json!(true));
        assert_eq!(json["loaded"], serde_json::json!(true));
        assert_eq!(json["label"], serde_json::json!("w1:tab-abc"));
    }

    #[test]
    fn detached_tab_labels_reflects_the_detached_map() {
        // AppState::detached_tab_labels flattens AppState.detached (keyed by detached WINDOW label)
        // into the set of TAB labels `tab_dtos`/`get_tabs` need — this is the glue between the two.
        let state = AppState::new();
        assert!(state.detached_tab_labels().is_empty());

        state.detached.lock().unwrap().insert(
            "shell-detach:w1:tab-abc".into(),
            LectorDetached {
                origin_wid: "w1".into(),
                tab_label: "w1:tab-abc".into(),
                port: 8080,
            },
        );
        let labels = state.detached_tab_labels();
        assert_eq!(labels.len(), 1);
        assert!(labels.contains("w1:tab-abc"));
    }

    #[test]
    fn unload_tab_state_check_skips_a_detached_label() {
        // unload_tab's #[tauri::command] wrapper needs a real AppHandle to invoke (not unit
        // testable here), but its guard condition — "is this label currently popped out" — is
        // exactly detached_tab_labels(), which is. This pins the invariant the guard relies on: a
        // tab marked detached must read as such regardless of what else is going on in `views`.
        let state = AppState::new();
        state.detached.lock().unwrap().insert(
            "shell-detach:w1:tab-a".into(),
            LectorDetached {
                origin_wid: "w1".into(),
                tab_label: "w1:tab-a".into(),
                port: 1234,
            },
        );
        assert!(state.detached_tab_labels().contains("w1:tab-a"));
        assert!(!state.detached_tab_labels().contains("w1:tab-b"));
    }

    #[test]
    fn views_for_window_preserves_tab_views_order() {
        // The load-bearing note: get_tabs must not reshuffle the sidebar on every refresh. A
        // HashMap-backed store couldn't guarantee this; the Vec-backed one does by construction.
        let state = AppState::new();
        let views = vec![
            lector_config::TabView {
                label: "w1:tab-a".into(),
                group: None,
                title: "Loose".into(),
                dir: "/tmp".into(),
                load_on_open: false,
                tree: false,
                tree_path: Vec::new(),
            },
            lector_config::TabView {
                label: "w1:tab-b".into(),
                group: Some("G".into()),
                title: "Grouped".into(),
                dir: "/usr".into(),
                load_on_open: false,
                tree: false,
                tree_path: Vec::new(),
            },
            lector_config::TabView {
                label: "w2:tab-c".into(),
                group: None,
                title: "OtherWindow".into(),
                dir: "/opt".into(),
                load_on_open: false,
                tree: false,
                tree_path: Vec::new(),
            },
        ];
        state.set_views(views);
        let got = state.views_for_window("w1");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].title, "Loose");
        assert_eq!(got[1].title, "Grouped");
    }

    #[test]
    fn unload_clears_active_so_a_reclick_routes_to_select_not_home() {
        // Regression for the Finding-1 bug: unloading the active tab must clear AppState.active for
        // its window. Before the fix, `unload_tab` only stopped the server — `active` kept pointing
        // at the now-cold tab, so `get_tabs`' DTO kept reporting it `active: true`, `chrome.js`'s
        // `wasActive = this.active === id` stayed true on the next click, and the click routed to
        // `home_tab` instead of `select_tab`. `home_tab` does `servers.port(&label).ok_or("tab is
        // not live")?` against a stopped server — an immediate, permanent error until some other tab
        // was selected first (which is the only thing that used to overwrite `active`).
        let state = AppState::new();
        let views = vec![lector_config::TabView {
            label: "w1:tab-a".into(),
            group: None,
            title: "A".into(),
            dir: "/tmp".into(),
            load_on_open: false,
            tree: false,
            tree_path: Vec::new(),
        }];
        state.set_views(views);
        state.set_active("w1:tab-a");
        assert_eq!(state.active_for("w1").as_deref(), Some("w1:tab-a"));

        state.unload("w1:tab-a");

        assert_eq!(
            state.active_for("w1"),
            None,
            "unloading the active tab must clear AppState.active for its window, not leave it \
             pointing at a cold tab"
        );

        // The routing consequence: `get_tabs`' DTO (what chrome.js's `wasActive` is computed from)
        // must now say this tab is NOT active — otherwise the JS-side re-click would still take the
        // home_tab branch even though Rust's `active` map is clear.
        let views = state.views_for_window("w1");
        let dtos = tab_dtos(
            &views,
            &state.servers,
            state.active_for("w1").as_deref(),
            &HashSet::new(),
        );
        assert!(
            !dtos[0].active,
            "the unloaded tab must no longer read as active in the DTO chrome.js consumes"
        );
    }

    #[test]
    fn unload_of_a_non_active_tab_leaves_active_untouched() {
        // The other half of the state machine: unloading a tab that ISN'T showing must not disturb
        // whichever tab actually is active.
        let state = AppState::new();
        state.set_active("w1:tab-a");
        state.set_active("w1:tab-b"); // tab-b is now the sole active tab for w1

        state.unload("w1:tab-a");

        assert_eq!(
            state.active_for("w1").as_deref(),
            Some("w1:tab-b"),
            "unloading a non-active tab must not clear or change a different tab's active state"
        );
    }

    fn view(label: &str) -> lector_config::TabView {
        lector_config::TabView {
            label: label.into(),
            group: None,
            title: label.into(),
            dir: "/tmp".into(),
            load_on_open: false,
            tree: false,
            tree_path: Vec::new(),
        }
    }

    #[test]
    fn neighbour_label_promotes_the_nearest_live_left_neighbour() {
        let views = vec![view("w1:tab-a"), view("w1:tab-b"), view("w1:tab-c")];
        // unloading the middle tab, with a and c both live → prefer the left one (came-from).
        assert_eq!(
            neighbour_label(&views, "w1:tab-b", &[true, false, true]),
            Some("w1:tab-a".to_string())
        );
    }

    #[test]
    fn neighbour_label_is_none_when_it_was_the_last_live_tab() {
        let views = vec![view("w1:tab-a"), view("w1:tab-b")];
        // tab-b unloaded (its own slot false), nothing else live → background.
        assert_eq!(neighbour_label(&views, "w1:tab-b", &[false, false]), None);
    }

    #[test]
    fn neighbour_label_never_self_promotes_even_when_the_popped_tab_stays_live() {
        // Unlike unload_tab (which stops the server, so the vacated slot is naturally false),
        // pop_out_tab's own `live` vector still reports the popped tab's OWN slot as true — its
        // server is deliberately never stopped. This pins that pick_live_neighbour's index-based
        // exclusion still keeps it from "promoting" itself back into view: with only the popped
        // tab live, the result must be None (background), not Some(popped tab).
        let views = vec![view("w1:tab-a"), view("w1:tab-b"), view("w1:tab-c")];
        assert_eq!(
            neighbour_label(&views, "w1:tab-b", &[false, true, false]),
            None,
            "the popped tab's own liveness must never make it its own neighbour"
        );
    }
}
