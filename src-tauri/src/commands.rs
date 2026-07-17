//! The Tauri command surface + the managed [`AppState`]. lector's sidebar chrome (`src/chrome.js`)
//! is the only caller of these — there is no per-caller trust gate (unlike curator's
//! `is_chrome_caller`) because Task 9 does not build one; see CLAUDE.md / the task report for that
//! deliberate scope line.

use crate::servers::Servers;
use lector_config::{Density, TabView};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

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
}

impl Default for AppState {
    fn default() -> Self {
        AppState::new()
    }
}

/// Select a tab: start its server if cold, then point its webview at the port. A start failure
/// leaves the tab cold and returns the message — the caller surfaces it via the chrome's setError.
#[tauri::command]
pub fn select_tab(
    label: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let view = state
        .view(&label)
        .ok_or_else(|| format!("no such tab: {label}"))?;
    let dir = lector_config::expand_tilde(&view.dir);
    let port = state.servers.start(&label, &dir)?;
    crate::webviews::show(&app, &label, port)?;
    state.set_active(&label);
    Ok(())
}

/// Unload a tab: stop its server and free its thread, watcher, and port. The tab goes cold; its
/// content is regenerated from disk on the next select, so nothing is lost.
#[tauri::command]
pub fn unload_tab(label: String, state: tauri::State<'_, AppState>) {
    state.servers.stop(&label);
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
}
