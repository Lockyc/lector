use lector_config::hash::fnv1a_64;
use tauri::{Emitter, Manager};

mod commands;
mod reload;
mod servers;
mod webviews;

/// Filename for the window-state plugin's saved bounds, scoped per config file. The plugin keys
/// window state by Tauri label *within one file*; two different configs can reuse a window title,
/// so scope the filename by a stable hash of the (canonicalized) config path to keep their bounds
/// separate — otherwise `just run` (examples/config.toml) and a real ~/.config/lector/config.toml
/// collide on identical window titles and the dev window restores the real one's geometry.
///
/// The hash is `fnv1a_64` — a fixed, toolchain-independent algorithm — deliberately NOT `std`'s
/// `DefaultHasher`, whose output isn't guaranteed stable across Rust releases: a
/// `rust-toolchain.toml` bump would silently change the filename and reset every window to default
/// bounds, reading as "lector forgot my layout" rather than as a toolchain problem. curator shipped
/// exactly that bug (fixed 2026-07-16). The algorithm is pinned by known-vectors in
/// `lector_config::hash` — imported, not re-implemented here.
///
/// Moving/renaming the config orphans its saved bounds — acceptable, since the path is otherwise
/// stable.
fn window_state_filename() -> String {
    let path = lector_config::resolve_config_path();
    let canonical = std::fs::canonicalize(&path).unwrap_or(path);
    let hash = fnv1a_64(canonical.as_os_str().as_encoded_bytes());
    format!(".window-state-{hash:016x}.json")
}

/// Print config-load warnings to stderr. Shared shape with `validate_cli`'s own printing (kept
/// separate rather than factored together — that one also prints the resolved tab tree, this one
/// only ever prints warnings).
fn log_config_warnings(warnings: &[lector_config::Warning]) {
    for w in warnings {
        eprintln!("config warning [{}]: {}", w.window, w.message);
    }
}

/// Apply a freshly-loaded config to live state, on both the initial load and every hot-reload: log
/// its warnings, install the app-global chrome settings, and run the shared [`reload::reconcile`]
/// (tab-server reconciliation — the same path for launch-time eager-start and hot-reload). Callers
/// that need more than this — launch's per-window `open_on_launch` selection, hot-reload's
/// chrome-refresh event and format-on-save (which needs the raw source this function doesn't take)
/// — do it themselves, after calling this.
fn apply_config(
    app: &tauri::AppHandle,
    cfg: &lector_config::Config,
    warnings: &[lector_config::Warning],
) {
    log_config_warnings(warnings);
    let state = app.state::<commands::AppState>();
    state.set_global(cfg.density, cfg.sidebar_drag, cfg.auto_update);
    reload::reconcile(&state, &cfg.windows);
}

/// Build every window in `cfg.windows` that doesn't already have a live Tauri window, installing
/// its accent colour and a [`commands::WindowMeta`] entry (so the menu spine's Window submenu and a
/// later reopen can find it — see that struct's doc). Existing windows are left untouched. Returns
/// the ids it actually built, so a caller running `open_on_launch` selection afterwards (which
/// needs `apply_config` to have populated tab state first, and must not re-select on top of an
/// already-open window's current tab) knows which ones are new.
///
/// Used by both `run()`'s setup (every window is new there) and [`reload_now`] (only "Create a
/// starter config" cold-starts a window this way — the config-file watcher's hot-reload never
/// rebuilds the window set, only tabs, via `reload::reconcile`).
fn build_missing_windows(app: &tauri::AppHandle, cfg: &lector_config::Config) -> Vec<String> {
    let state = app.state::<commands::AppState>();
    let mut meta = state.window_meta();
    let mut built = Vec::new();
    for win_cfg in &cfg.windows {
        let wid = lector_config::identity::window_id(&win_cfg.title);
        if app.get_window(&wid).is_some() {
            continue;
        }
        if webviews::build_window(
            app,
            &wid,
            &win_cfg.title,
            win_cfg.width as f64,
            win_cfg.height as f64,
        )
        .is_err()
        {
            continue;
        }
        state.set_colour(&wid, win_cfg.colour.clone());
        meta.push(commands::WindowMeta {
            id: wid.clone(),
            title: win_cfg.title.clone(),
            width: win_cfg.width as f64,
            height: win_cfg.height as f64,
            colour: win_cfg.colour.clone(),
        });
        built.push(wid);
    }
    state.set_window_meta(meta);
    built
}

/// Emit an event to just the focused window's chrome sidebar — the menu spine's ⌘W (Close Tab)
/// and Check for Updates… both act on whichever window has key focus. Modelled on curator's
/// equivalent (the chrome is the window's main webview, so its label *is* the window id).
fn emit_to_focused_chrome<S: serde::Serialize + Clone>(
    app: &tauri::AppHandle,
    event: &str,
    payload: S,
) {
    if let Some(win) = app.get_focused_window() {
        let _ = app.emit_to(win.label(), event, payload);
    }
}

/// The menu spine's Window submenu selector, and the home surface's per-window button
/// (`shell_home_open_window`): focus `window_id` if it's already open, otherwise rebuild it from
/// its stored [`commands::WindowMeta`] (present iff `window_id` was ever built this run — the
/// spine and the home surface only ever offer ids that came from that same list, so a lookup miss
/// here would mean one of them drifted out of sync with it). Reconciles the home surface afterwards
/// so it closes now that a real window exists again.
fn open_or_focus_window(app: &tauri::AppHandle, window_id: &str) {
    if let Some(win) = app.get_window(window_id) {
        let _ = win.set_focus();
        return;
    }
    let meta = app.state::<commands::AppState>().window_meta();
    if let Some(m) = meta.iter().find(|m| m.id == window_id) {
        let _ = webviews::build_window(app, &m.id, &m.title, m.width, m.height);
    }
    let entries = reload::window_entries(app, &meta);
    let path = lector_config::resolve_config_path();
    reload::reconcile_home(app, &entries, &path.to_string_lossy(), path.exists(), None);
}

/// Re-run launch reconciliation for windows the app has never built — currently only the home
/// surface's "Create a starter config" button, which turns a `NoConfig` state (necessarily zero
/// windows built yet, since no config ever loaded) into a real config the app has never seen.
/// Builds every window the freshly-written config defines, applies tab state, runs `open_on_launch`
/// selection on exactly the newly-built windows (mirroring `run()`'s setup), and reconciles the
/// home surface — which closes it, since a real window now exists.
///
/// Deliberately NOT reused by the config-file watcher: that path already has its own bookkeeping
/// (format-on-save echo suppression, per-reload event emission) built around windows that already
/// exist; this one is specifically for windows that don't.
pub(crate) fn reload_now(app: &tauri::AppHandle) {
    let path = lector_config::resolve_config_path();
    let (cfg, warnings) = match lector_config::load_config(&path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("config error: {e}");
            return;
        }
    };

    let built = build_missing_windows(app, &cfg);
    apply_config(app, &cfg, &warnings);

    let state = app.state::<commands::AppState>();
    for win_cfg in &cfg.windows {
        let wid = lector_config::identity::window_id(&win_cfg.title);
        if !built.contains(&wid) {
            continue;
        }
        if let Some(label) = win_cfg.startup_label() {
            if let Err(e) = commands::select(app, &state, &label) {
                eprintln!("startup selection failed for {label:?}: {e}");
            }
        }
    }

    let meta = state.window_meta();
    let entries = reload::window_entries(app, &meta);
    reload::reconcile_home(app, &entries, &path.to_string_lossy(), path.exists(), None);
}

pub fn run() {
    let app = shell_core::register_plugins(
        tauri::Builder::default(),
        window_state_filename(),
        &[shell_core::home::HOME_LABEL],
    )
    .manage(commands::AppState::new())
    .setup(move |app| {
        let path = lector_config::resolve_config_path();
        let mut load_error: Option<String> = None;
        let (cfg, warnings) = match lector_config::load_config(&path) {
            Ok((c, warnings)) => (c, warnings),
            Err(e) => {
                eprintln!("config error: {e}");
                load_error = Some(e.to_string());
                (lector_config::Config::default(), Vec::new())
            }
        };

        // Build every configured window (+ its accent colour) first, so the reconcile below —
        // and the `open_on_launch` selection after it — have somewhere to point a webview at.
        // Every window is new at launch, so `built` below is always all of `window_ids`.
        let built = build_missing_windows(app.handle(), &cfg);
        let window_ids: Vec<String> = cfg
            .windows
            .iter()
            .map(|w| lector_config::identity::window_id(&w.title))
            .collect();

        apply_config(app.handle(), &cfg, &warnings);

        // Launch selection (`WindowConfig::startup_label` — the first load_on_open tab by default,
        // or whatever `open_on_launch` overrides to): select it exactly the way a click would
        // (`commands::select` — start-if-cold, show, mark active), never a shadow copy of that
        // logic. A failure (e.g. the tab's dir doesn't exist yet) just stays cold; it isn't fatal
        // to launch.
        let state = app.state::<commands::AppState>();
        for win_cfg in &cfg.windows {
            let wid = lector_config::identity::window_id(&win_cfg.title);
            if !built.contains(&wid) {
                continue;
            }
            if let Some(label) = win_cfg.startup_label() {
                if let Err(e) = commands::select(app.handle(), &state, &label) {
                    eprintln!("startup selection failed for {label:?}: {e}");
                }
            }
        }

        // The menu spine: App/Config/Window are shared (shell-core), Tab is lector's own — it
        // holds nothing but the spine's Close Tab (⌘W), since compositor's watcher makes a
        // Reload Tab meaningless here and there is no Reset All analogue.
        let window_entries = reload::window_entries(app.handle(), &state.window_meta());
        let spine = shell_core::menu::build_spine(
            app,
            shell_core::menu::SpineConfig {
                app_name: "lector",
                config_path: &path,
                windows: &window_entries,
            },
            env!("CARGO_PKG_VERSION"),
            env!("BUILD_GIT_SHA"),
            env!("BUILD_DATE"),
        )?;
        let tab_menu = tauri::menu::SubmenuBuilder::new(app, "Tab")
            .item(&spine.close_tab)
            .build()?;
        let items: Vec<&dyn tauri::menu::IsMenuItem<_>> = vec![
            &spine.submenus[0], // App
            &tab_menu,
            &spine.submenus[1], // Config
            &spine.submenus[2], // Window
        ];
        app.set_menu(tauri::menu::MenuBuilder::new(app).items(&items).build()?)?;

        let cfg_for_menu = path.clone();
        app.on_menu_event(move |app, event| {
            let id = event.id().as_ref();
            // The spine's file-acting ids need no window — let it consume them first.
            if shell_core::menu::handle_spine_event(id, &cfg_for_menu) {
                return;
            }
            match id {
                // chrome-core owns self-update; forward to its checkForUpdateNow().
                shell_core::menu::ids::CHECK_UPDATES => {
                    emit_to_focused_chrome(app, "check-update", ())
                }
                // ⌘W unloads the ACTIVE TAB — it does not close the window. The chrome owns
                // which tab is active and the dot repaint, so it drives unload_tab off this
                // event (warden's model, now the family standard).
                shell_core::menu::ids::CLOSE_TAB => emit_to_focused_chrome(app, "close-tab", ()),
                shell_core::menu::ids::CLOSE_WINDOW => {
                    if let Some(win) = app.get_focused_window() {
                        let _ = win.close();
                    }
                }
                id => {
                    if let Some(wid) = shell_core::menu::selected_window(id) {
                        open_or_focus_window(app, wid);
                    }
                }
            }
        });

        // The home surface: never stranded invisible. `has_windows` is derived inside
        // `reconcile_home` from `window_entries`'s own `open` flags, which are all `true` here
        // (every window built above is live) — so this only actually shows anything when the
        // config had zero `[[window]]` blocks or failed to load at all.
        reload::reconcile_home(
            app.handle(),
            &window_entries,
            &path.to_string_lossy(),
            path.exists(),
            load_error.as_deref(),
        );

        // Watch the config file and hot-reload on change, keeping the last-good config (and
        // surfacing the error to every window's chrome) if the new contents don't
        // parse/validate. A failed reload tears nothing down — see `reload::reconcile`'s doc
        // and `lector_config`'s "missing dir warns, never errors" rule this mirrors: an
        // un-cloned repo, or a config with one bad line, must not strand every other tab.
        let watch_path = path.clone();
        let app_handle = app.handle().clone();
        std::thread::spawn(move || {
            use notify::{RecursiveMode, Watcher};
            let (tx, rx) = std::sync::mpsc::channel();
            let Ok(mut watcher) = notify::recommended_watcher(tx) else {
                return;
            };
            // Watch the parent dir, not the file: editors that atomic-save (write temp +
            // rename) replace the inode, which silently breaks a single-file watch.
            let dir = watch_path.parent().unwrap_or(&watch_path);
            if Watcher::watch(&mut watcher, dir, RecursiveMode::NonRecursive).is_err() {
                return;
            }
            // The exact bytes of our own most recent format-on-save write, so we can swallow
            // the watch event it triggers and reload exactly once per user save.
            let mut self_write: Option<String> = None;
            for res in rx {
                let Ok(event) = res else { continue };
                if !event.paths.iter().any(|p| p == &watch_path) {
                    continue;
                }
                let Ok(src) = std::fs::read_to_string(&watch_path) else {
                    continue;
                };
                // Swallow the echo of our own format-on-save write — the user save that
                // prompted it already reloaded. `take()` clears the marker either way, so at
                // worst a missed echo costs one redundant no-op reload.
                if self_write.take().as_deref() == Some(src.as_str()) {
                    continue;
                }
                match lector_config::parse_and_validate(&src) {
                    Ok((new_cfg, warnings)) => {
                        // Format-on-save: rewrite in house style on a clean reload.
                        // `format_file` is itself diff-guarded (a no-op on already-formatted
                        // bytes); the pre-check here is what lets us capture the formatted
                        // bytes as `self_write` so the watch event our own write triggers is
                        // swallowed above — one reload per user save, not two.
                        if new_cfg.format_on_save {
                            let formatted = lector_config::format_str(&src);
                            if formatted != src {
                                match lector_config::format_file(&watch_path) {
                                    Ok(_) => self_write = Some(formatted),
                                    Err(e) => eprintln!("config format error: {e}"),
                                }
                            }
                        }
                        apply_config(&app_handle, &new_cfg, &warnings);
                        for wid in &window_ids {
                            let _ = app_handle.emit_to(wid.as_str(), "config-reloaded", ());
                        }
                        let state = app_handle.state::<commands::AppState>();
                        let entries = reload::window_entries(&app_handle, &state.window_meta());
                        reload::reconcile_home(
                            &app_handle,
                            &entries,
                            &watch_path.to_string_lossy(),
                            true,
                            None,
                        );
                    }
                    Err(e) => {
                        // Last-good-on-failure: `apply_config` never ran, so state (and every
                        // running server) is untouched — only the error is surfaced.
                        let msg = e.to_string();
                        eprintln!("config error: {msg}");
                        for wid in &window_ids {
                            let _ = app_handle.emit_to(wid.as_str(), "config-error", msg.clone());
                        }
                        let state = app_handle.state::<commands::AppState>();
                        let entries = reload::window_entries(&app_handle, &state.window_meta());
                        reload::reconcile_home(
                            &app_handle,
                            &entries,
                            &watch_path.to_string_lossy(),
                            true,
                            Some(&msg),
                        );
                    }
                }
            }
        });

        Ok(())
    })
    .invoke_handler(tauri::generate_handler![
        commands::get_tabs,
        commands::window_identity,
        commands::select_tab,
        commands::unload_tab,
        commands::home_tab,
        commands::nav_back,
        commands::nav_forward,
        commands::set_hole_rect,
        commands::shell_home_create_config,
        commands::shell_home_edit_config,
        commands::shell_home_open_window,
    ])
    .build(tauri::generate_context!())
    .expect("error while building lector");

    app.run(|app_handle, event| {
        // Shut down every live server and join its threads on quit — otherwise a compositor
        // serve loop (and its watcher) outlives the app.
        if let tauri::RunEvent::Exit = event {
            app_handle
                .state::<commands::AppState>()
                .servers
                .shutdown_all();
        }
    });
}

/// `lector validate [path]`: load + validate a config and print its resolved window/tab tree plus
/// any non-fatal warnings. Exit 0 on success, 1 on a load/parse/validation error. Mirrors
/// `curator validate` / `warden validate`.
pub fn validate_cli(path: Option<std::path::PathBuf>) -> i32 {
    let path = path.unwrap_or_else(lector_config::resolve_config_path);
    match lector_config::load_config(&path) {
        Ok((cfg, warnings)) => {
            println!("ok: {} ({} window(s))", path.display(), cfg.windows.len());
            for w in &cfg.windows {
                println!("  window {:?}", w.title);
                for v in w.tab_views() {
                    let group = v
                        .group
                        .as_deref()
                        .map(|g| format!(" group={g:?}"))
                        .unwrap_or_default();
                    println!(
                        "    tab {:?} dir={} load_on_open={}{}",
                        v.title, v.dir, v.load_on_open, group
                    );
                }
            }
            for warn in &warnings {
                eprintln!("warning [{}]: {}", warn.window, warn.message);
            }
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_state_filename_shape_is_stable() {
        // Same config path → same filename, every run (no per-run seed).
        assert_eq!(window_state_filename(), window_state_filename());
        assert!(window_state_filename().starts_with(".window-state-"));
        assert!(window_state_filename().ends_with(".json"));
    }
}
