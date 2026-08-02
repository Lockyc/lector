use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{Emitter, Manager};

mod commands;
mod reload;
mod servers;
mod webviews;

/// Set once on `RunEvent::ExitRequested` (see [`run`]), which fires before every window's
/// `Destroyed` during ⌘Q. Checked at the top of [`redock`] so a detached window's teardown doesn't
/// reopen its (already-closing) origin window mid-quit. Never reset — lector doesn't prevent exit.
static IS_QUITTING: AtomicBool = AtomicBool::new(false);

/// Mark the app as quitting. Called once, from `RunEvent::ExitRequested`.
pub(crate) fn mark_quitting() {
    IS_QUITTING.store(true, Ordering::SeqCst);
}

/// Whether the app is quitting — see [`IS_QUITTING`].
pub(crate) fn is_quitting() -> bool {
    IS_QUITTING.load(Ordering::SeqCst)
}

/// The opaque token identifying a popped-out tab's detached window
/// (→ [`shell_core::detach::detached_label`]). A lector content-webview label is already
/// `{window_id}:tab-<hash>` — globally unique across windows and Tauri-label-safe — so it *is* the
/// token; the origin window is tracked on [`commands::LectorDetached`], never parsed back out of
/// the label. Mirrors curator's `detach_window_token` exactly.
pub(crate) fn detach_window_token(tab_label: &str) -> String {
    tab_label.to_string()
}

/// The detached window's banner height (matches shell-core's `detach.html` `#banner`, 2.25rem ≈
/// 36px). Only used to size the recreated webview's BIRTH rect so it doesn't flash full-height for
/// one frame before `detach.html`'s own `set_hole_rect` lands and reports the exact hole.
pub(crate) const DETACH_BANNER_H: f64 = 36.0;

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

/// Return a popped-out tab to its origin window when its detached window closes — the `on_close`
/// wired via [`shell_core::detach::wire_return`] (`commands::pop_out_tab`). Runs on the main thread
/// (Tauri delivers the window `Destroyed` event there). lector can't move a webview between windows
/// either, so — like [`commands::pop_out_tab`] itself — this *recreates* the tab's content webview on
/// the origin, reusing the SAME port [`commands::LectorDetached`] recorded (the server was never
/// stopped across the hop, so nothing on disk or in the compositor watcher was disturbed).
///
/// Order matters: the origin+tab+port are read (not removed) first, so that if the origin window was
/// closed while the tab was out, [`open_or_focus_window`] reopens it while the tab is STILL in
/// `AppState::detached` — harmless, since reopening a window never touches `detached` or recreates
/// any content webview on its own (see that function's doc: it only rebuilds the window shell).
/// Only then is the bookkeeping removed and the webview recreated on the origin.
pub(crate) fn redock(app: &tauri::AppHandle, detached_label: &str) {
    // ⌘Q teardown: `RunEvent::ExitRequested` fires before every window's `Destroyed`. Don't reopen
    // an origin or recreate a webview mid-quit — everything is being torn down, and the servers are
    // about to be shut down wholesale anyway (see `run`'s `RunEvent::Exit` arm).
    if is_quitting() {
        return;
    }
    let state = app.state::<commands::AppState>();

    // Peek the origin + tab + port without removing (see the doc comment's ordering rationale).
    let Some((origin_wid, tab_label, port)) = state
        .detached
        .lock()
        .expect("detached lock")
        .get(detached_label)
        .map(|d| (d.origin_wid.clone(), d.tab_label.clone(), d.port))
    else {
        return; // already redocked (double-close) — nothing to do
    };

    // Reopen the origin if the user closed it while the tab was popped out (case: another window or
    // the detached window itself kept the app alive past last-window-quit).
    if app.get_window(&origin_wid).is_none() {
        open_or_focus_window(app, &origin_wid);
    }

    // Now take the bookkeeping — a raced double-close (two Destroyed events for the same window)
    // would find nothing left here and bail.
    if state
        .detached
        .lock()
        .expect("detached lock")
        .remove(detached_label)
        .is_none()
    {
        return;
    }

    if state.view(&tab_label).is_none() {
        // The tab (or its whole window) was removed from the config while it was popped out — it
        // simply ends rather than coming back. The server it was keeping alive is no longer wanted
        // by anything: stop it explicitly (rather than leaving it to be swept up only at quit) so a
        // config edit doesn't leak a serve loop + watcher for the rest of the session.
        state.servers.stop(&tab_label);
        return;
    }

    // Recreate the content webview on the origin, reusing the SAME port — this is what makes the
    // round trip near-lossless: the compositor serve loop (and its watcher) never stopped, so the
    // page picks up exactly where the detached window's copy left off.
    let _ = webviews::show(app, &tab_label, port);
    state.set_active(&tab_label);

    // Re-render the origin chrome so the returned row loses its ⤢ detached mark and reflects the new
    // active tab. `config-reloaded` drives the chrome's refresh() (a get_tabs re-fetch); emit_to
    // targets only that window's chrome (lector's per-window emit scoping, `emit_to_focused_chrome`'s
    // sibling for a specific label rather than the focused one). If the origin was just reopened its
    // fresh mount already refreshes, so a missed emit self-corrects.
    let _ = app.emit_to(origin_wid.as_str(), "config-reloaded", ());
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

/// Build and install the app menu: the shared spine (App/Config/Window) plus lector's own Tab
/// submenu — shell-core's tab-nav block (⌘⇧[ / ⌘⇧] , ⌘1–9 jumps, and the ⌘1/⌘2 cycle aliases
/// when `mode` asks for them) around the spine's Close Tab (⌘W) and Pop Out Tab (⌘⇧O).
///
/// Called at setup **and again on every clean hot-reload**, so a `tab_digit_keys` flip applies
/// without a relaunch — matching warden and curator.
fn install_app_menu(
    app: &tauri::AppHandle,
    config_path: &std::path::Path,
    mode: lector_config::TabDigitKeys,
    window_entries: &[shell_core::menu::WindowEntry],
) -> tauri::Result<()> {
    let spine = shell_core::menu::build_spine(
        app,
        shell_core::menu::SpineConfig {
            app_name: "lector",
            config_path,
            windows: window_entries,
        },
        env!("CARGO_PKG_VERSION"),
        env!("BUILD_GIT_SHA"),
        env!("BUILD_DATE"),
    )?;
    let nav = shell_core::menu::build_tab_nav(app, mode.is_cycle())?;
    let mut tab_menu = tauri::menu::SubmenuBuilder::new(app, "Tab");
    for it in &nav.nav {
        tab_menu = tab_menu.item(it);
    }
    tab_menu = tab_menu
        .separator()
        .item(&spine.close_tab)
        .item(&spine.pop_out_tab)
        .separator();
    for it in &nav.jumps {
        tab_menu = tab_menu.item(it);
    }
    let tab_menu = tab_menu.build()?;
    let items: Vec<&dyn tauri::menu::IsMenuItem<_>> = vec![
        &spine.submenus[0], // App
        &tab_menu,
        &spine.submenus[1], // Config
        &spine.submenus[2], // Window
    ];
    app.set_menu(tauri::menu::MenuBuilder::new(app).items(&items).build()?)?;
    Ok(())
}

pub fn run() {
    let config_path = lector_config::resolve_config_path();
    let app = shell_core::register_plugins(
        tauri::Builder::default(),
        Some(&config_path),
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

        // Native mouse side-button (back/forward) navigation — the shared shell-core NSEvent monitor
        // (WKWebView never delivers the side buttons to the DOM). lector supplies the
        // focused-active-webview resolver; shell-core owns the monitor + native goBack/goForward.
        let mouse_nav_handle = app.handle().clone();
        shell_core::mouse_nav::install(move || {
            let win = mouse_nav_handle.get_focused_window()?;
            let label = mouse_nav_handle
                .state::<commands::AppState>()
                .active_for(win.label())?;
            win.get_webview(&label)
        });

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

        // The menu spine: App/Config/Window are shared (shell-core), Tab is lector's own — see
        // `install_app_menu`'s doc comment for what it holds and why it's rebuilt on every
        // clean hot-reload, not just here at setup.
        let window_entries = reload::window_entries(app.handle(), &state.window_meta());
        install_app_menu(app.handle(), &path, cfg.tab_digit_keys, &window_entries)?;

        let cfg_for_menu = path.clone();
        app.on_menu_event(move |app, event| {
            let id = event.id().as_ref();
            // The spine's file-acting ids need no window — let it consume them first.
            if shell_core::menu::handle_spine_event(id, &cfg_for_menu) {
                return;
            }
            // Tab navigation (⌘⇧[ / ⌘⇧] , ⌘1–9, and the ⌘1/⌘2 cycle aliases). shell-core routes
            // the id, so this handler is mode-blind — the aliases arrive as plain Next/Prev. The
            // chrome resolves the target row and selects it through the normal click path, so a
            // cold tab still starts its server on demand.
            if let Some(action) = shell_core::menu::tab_nav_action(id) {
                use shell_core::menu::TabNavAction;
                match action {
                    TabNavAction::Next => emit_to_focused_chrome(app, "nav-tab", 1i32),
                    TabNavAction::Prev => emit_to_focused_chrome(app, "nav-tab", -1i32),
                    TabNavAction::Jump(n) => emit_to_focused_chrome(app, "jump-tab", n),
                }
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
                // ⌘⇧O pops the focused window's active tab out into its own window. The chrome owns
                // which tab is active, so it drives pop_out_tab off this event (routed to only the
                // focused window's chrome, the same per-window emit pattern as close-tab).
                shell_core::menu::ids::POP_OUT_TAB => {
                    emit_to_focused_chrome(app, "pop-out-tab", ())
                }
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
        // The shared shell-core watcher owns the parent-dir watch, the file-name match (macOS
        // FSEvents-robust — the fix for the old exact-path bug that silently missed every event
        // under a symlinked config dir), and the echo-swallow (via the `Option<String>` the
        // closure returns on a format write). lector supplies just the parse + apply.
        let app_handle = app.handle().clone();
        let fmt_path = path.clone();
        let menu_path = path.clone();
        shell_core::watch::watch_config(path.clone(), move |src| {
            match lector_config::parse_and_validate(src) {
                Ok((new_cfg, warnings)) => {
                    // Format-on-save: rewrite in house style on a clean reload. `format_file` is
                    // diff-guarded (a no-op on already-formatted bytes); when it rewrites, return
                    // the formatted bytes so the watcher swallows the echo — one reload per user
                    // save, not two.
                    let self_write = if new_cfg.format_on_save {
                        let formatted = lector_config::format_str(src);
                        if formatted != src {
                            match lector_config::format_file(&fmt_path) {
                                Ok(_) => Some(formatted),
                                Err(e) => {
                                    eprintln!("config format error: {e}");
                                    None
                                }
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    apply_config(&app_handle, &new_cfg, &warnings);
                    for wid in &window_ids {
                        let _ = app_handle.emit_to(wid.as_str(), "config-reloaded", ());
                    }
                    let state = app_handle.state::<commands::AppState>();
                    let entries = reload::window_entries(&app_handle, &state.window_meta());
                    // The app menu is global, not part of the per-window reconcile: rebuild it so a
                    // `tab_digit_keys` flip (and the Window submenu's entries) track the new config.
                    let _ =
                        install_app_menu(&app_handle, &menu_path, new_cfg.tab_digit_keys, &entries);
                    reload::reconcile_home(
                        &app_handle,
                        &entries,
                        &fmt_path.to_string_lossy(),
                        true,
                        None,
                    );
                    self_write
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
                        &fmt_path.to_string_lossy(),
                        true,
                        Some(&msg),
                    );
                    None
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
        commands::pop_out_tab,
        commands::raise_popped_window,
        commands::pop_in_tab,
        commands::rescan_root,
        commands::shell_home_create_config,
        commands::shell_home_edit_config,
        commands::shell_home_open_window,
    ])
    .build(tauri::generate_context!())
    .expect("error while building lector");

    app.run(|app_handle, event| {
        // ExitRequested fires before every window's Destroyed during ⌘Q; mark quitting so a
        // detached window's teardown doesn't reopen its origin mid-quit (see `redock`).
        if let tauri::RunEvent::ExitRequested { .. } = event {
            mark_quitting();
        }
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
    fn detach_token_round_trips_through_the_detached_label() {
        // A content-webview label is `{origin_wid}:tab-<hash>` — the token IS that label, and it
        // must survive shell-core's detached-label wrapping so a detached window is recognised
        // (is_detached_label) and its token recoverable (detach_token). Deterministic, too. Mirrors
        // curator's identical test for its own `detach_window_token`.
        let tab_label = "w0123456789abcdef:tab-00112233445566ff";
        let token = detach_window_token(tab_label);
        assert_eq!(token, detach_window_token(tab_label)); // stable
        let label = shell_core::detach::detached_label(&token);
        assert!(shell_core::detach::is_detached_label(&label));
        assert_eq!(
            shell_core::detach::detach_token(&label),
            Some(token.as_str())
        );
        // A real (config-defined) window label is never mistaken for a detached one.
        assert!(!shell_core::detach::is_detached_label(tab_label));
    }
}
