//! The Tauri command surface + the managed [`AppState`]. lector's sidebar chrome (`src/chrome.js`)
//! is the intended caller of these, but unlike curator there is no `is_chrome_caller`-style
//! per-caller gate here — Tauri's own IPC dispatch already does the job for the shape this app has.
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
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

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
}

/// Project the resolved tabs + the live registry into rows for the chrome. `views` must already be
/// in the order the sidebar should render (the ordering itself is `AppState::views_for_window`'s
/// job, not this function's).
pub fn tab_dtos(views: &[TabView], servers: &Servers, active: Option<&str>) -> Vec<TabPayload> {
    views
        .iter()
        .map(|v| TabPayload {
            label: v.label.clone(),
            title: v.title.clone(),
            group: v.group.clone(),
            loaded: servers.is_alive(&v.label),
            active: active == Some(v.label.as_str()),
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
    /// Clearing (rather than promoting a neighbour, as curator does) is deliberate: lector shows
    /// exactly one tab at a time and never eager-starts others (see `lib.rs`'s `setup` — load_on_open
    /// eager-start is Task 10's concern), so there is never an already-live sibling to promote to.
    /// Leaving a stale `active` label here is exactly the Finding-1 bug: `get_tabs`' DTO would keep
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

/// Unload a tab: stop its server, close its content webview, and — if it was the window's active
/// tab — clear active so the empty state paints and the next click restarts it via `select_tab`
/// rather than erroring through `home_tab` (see `AppState::unload`'s doc for the full failure chain
/// this prevents). The tab's content is regenerated from disk on the next select, so nothing is lost.
#[tauri::command]
pub fn unload_tab(label: String, app: tauri::AppHandle, state: tauri::State<'_, AppState>) {
    state.unload(&label);
    crate::webviews::close(&app, &label);
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
        }];

        // …but nothing has started it, so it is cold.
        let dtos = tab_dtos(&views, &servers, None);
        assert!(!dtos[0].loaded, "load_on_open must not imply live");

        servers.start("t1", &dir).unwrap();
        let dtos = tab_dtos(&views, &servers, None);
        assert!(dtos[0].loaded, "a started server must read as live");

        servers.shutdown_all();
        let dtos = tab_dtos(&views, &servers, None);
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
            },
            lector_config::TabView {
                label: "t2".into(),
                group: Some("G".into()),
                title: "B".into(),
                dir: "/usr".into(),
                load_on_open: false,
            },
        ];
        let servers = crate::servers::Servers::new();
        let dtos = tab_dtos(&views, &servers, Some("t2"));
        assert!(!dtos[0].active);
        assert!(dtos[1].active);
        assert_eq!(dtos[1].group.as_deref(), Some("G"));
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
            },
            lector_config::TabView {
                label: "w1:tab-b".into(),
                group: Some("G".into()),
                title: "Grouped".into(),
                dir: "/usr".into(),
                load_on_open: false,
            },
            lector_config::TabView {
                label: "w2:tab-c".into(),
                group: None,
                title: "OtherWindow".into(),
                dir: "/opt".into(),
                load_on_open: false,
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
        let dtos = tab_dtos(&views, &state.servers, state.active_for("w1").as_deref());
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
}
