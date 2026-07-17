//! The one reconciliation path shared by launch-time eager-start and config hot-reload.
//!
//! `load_on_open` (start a repo's server at launch) and config hot-reload's "start a newly-added
//! eager tab" are the same operation — both are "make the live-server set match what the current
//! config says should be eager, without disturbing anything already running." Splitting that into
//! two copies (one thrown away after the first reload) would be exactly the shadow-implementation
//! this codebase avoids elsewhere (see the root CLAUDE.md's "one source of truth"). So `run()`'s
//! setup and the hot-reload watcher both call [`reconcile`].

use crate::commands::{AppState, WindowMeta};
use lector_config::{TabView, WindowConfig};
use std::collections::HashSet;

/// Re-resolve every window's tabs against `windows`, install them as the new last-good views,
/// retain only servers whose tab survived (this is what stops a removed tab's server — see
/// `Servers::retain`'s doc for the leak it prevents), and eager-start every `load_on_open` tab
/// that isn't already live.
///
/// Never tears down a server outside of what `retain` drops: an already-live tab that is still
/// present keeps running (whether or not `load_on_open` is still true for it — going from eager to
/// lazy in the config doesn't stop an already-open tab, it only stops it from being *reopened*
/// automatically next time), and a `load_on_open` tab that's already alive is left alone rather
/// than restarted.
///
/// Returns the new tab views, already installed into `state` — callers that need to act on them
/// further (launch's `open_on_launch` selection) don't need a second `views_for_window` round-trip.
pub fn reconcile(state: &AppState, windows: &[WindowConfig]) -> Vec<TabView> {
    let all_views: Vec<TabView> = windows.iter().flat_map(WindowConfig::tab_views).collect();
    state.set_views(all_views.clone());

    let labels: HashSet<String> = all_views.iter().map(|v| v.label.clone()).collect();
    state.servers.retain(&labels);

    for view in &all_views {
        if view.load_on_open && !state.servers.is_alive(&view.label) {
            let dir = lector_config::expand_tilde(&view.dir);
            if let Err(e) = state.servers.start(&view.label, &dir) {
                eprintln!("eager-start failed for tab {:?}: {e}", view.title);
            }
        }
    }

    all_views
}

/// Project every window's fixed [`WindowMeta`] into the menu spine's / home surface's shared
/// [`shell_core::menu::WindowEntry`] shape, re-checking each one's live `open` state against the
/// running app (the one field `WindowMeta` deliberately doesn't cache, since it changes independent
/// of config — a window closes and reopens with no reload involved).
pub fn window_entries(
    app: &tauri::AppHandle,
    meta: &[WindowMeta],
) -> Vec<shell_core::menu::WindowEntry> {
    use tauri::Manager;
    meta.iter()
        .map(|m| shell_core::menu::WindowEntry {
            id: m.id.clone(),
            title: m.title.clone(),
            open: app.get_window(&m.id).is_some(),
            colour: m.colour.clone(),
        })
        .collect()
}

/// Show or close the home surface to match the current state — the other half of "the app is never
/// stranded invisible," run after the initial load and after every hot-reload (both a clean one and
/// a failed one, so a config edited from working into broken while every window happens to be closed
/// still explains itself rather than going dark). `has_windows` is derived from `entries` itself
/// (any window currently open) rather than taken as a separate argument — one fewer thing for a
/// caller to get out of sync with the list it just built.
pub fn reconcile_home(
    app: &tauri::AppHandle,
    entries: &[shell_core::menu::WindowEntry],
    config_path: &str,
    config_exists: bool,
    load_error: Option<&str>,
) {
    let has_windows = entries.iter().any(|w| w.open);
    match shell_core::home::home_state(has_windows, config_exists, config_path, load_error, entries)
    {
        Some(state) => {
            let _ = shell_core::home::show_home(app, &state, "lector");
        }
        None => shell_core::home::close_home(app),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lector-reload-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs/index.md"), "# X\n").unwrap();
        dir
    }

    fn window_with(title: &str, dir: &std::path::Path, load_on_open: bool) -> WindowConfig {
        let src = format!(
            "[[window]]\ntitle = \"{title}\"\n[[window.tab]]\ntitle = \"T\"\ndir = \"{}\"\nload_on_open = {load_on_open}\n",
            dir.display()
        );
        lector_config::parse_and_validate(&src)
            .unwrap()
            .0
            .windows
            .remove(0)
    }

    #[test]
    fn reconcile_starts_eager_tabs_and_installs_views() {
        let dir = scratch("eager");
        let win = window_with("W", &dir, true);
        let state = AppState::new();

        let views = reconcile(&state, std::slice::from_ref(&win));

        assert_eq!(views.len(), 1);
        assert!(
            state.servers.is_alive(&views[0].label),
            "a load_on_open tab must be started by reconcile"
        );
        state.servers.shutdown_all();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reconcile_leaves_non_eager_tabs_cold() {
        let dir = scratch("lazy");
        let win = window_with("W", &dir, false);
        let state = AppState::new();

        let views = reconcile(&state, std::slice::from_ref(&win));

        assert!(!state.servers.is_alive(&views[0].label));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reconcile_stops_a_removed_tabs_server() {
        // The leak the whole task hinges on: a tab dropped from the config must have its server
        // shut down, or every edit leaks a watcher and a port.
        let dir = scratch("removed");
        let win = window_with("W", &dir, true);
        let state = AppState::new();
        reconcile(&state, std::slice::from_ref(&win));
        let label = state.views_for_window(&lector_config::identity::window_id("W"))[0]
            .label
            .clone();
        assert!(state.servers.is_alive(&label));

        // New config for the same window has no tabs at all.
        let empty = lector_config::parse_and_validate("[[window]]\ntitle = \"W\"\n")
            .unwrap()
            .0
            .windows
            .remove(0);
        reconcile(&state, std::slice::from_ref(&empty));

        assert!(
            !state.servers.is_alive(&label),
            "a tab removed from the config must have its server stopped"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reconcile_does_not_restart_an_already_live_tab() {
        // An already-live load_on_open tab must be left alone, not bounced (which would drop the
        // reader's connection and reset the watcher for no reason).
        let dir = scratch("stable");
        let win = window_with("W", &dir, true);
        let state = AppState::new();
        reconcile(&state, std::slice::from_ref(&win));
        let label = state.views_for_window(&lector_config::identity::window_id("W"))[0]
            .label
            .clone();
        let port_before = state.servers.port(&label);

        // Reconcile again with the identical config.
        reconcile(&state, std::slice::from_ref(&win));

        assert_eq!(
            state.servers.port(&label),
            port_before,
            "reconcile must not restart an already-live tab"
        );
        state.servers.shutdown_all();
        std::fs::remove_dir_all(&dir).ok();
    }
}
