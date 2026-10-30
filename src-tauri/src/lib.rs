use lector_config::WindowTabs;
use shell_core::windows::plan::{Slot, WindowDecl, WindowOp};
use shell_core::windows::WindowSet;
use std::collections::HashSet;
use tauri::{Emitter, Manager};

mod commands;
mod reload;
mod servers;
mod webviews;
mod windows;

/// The opaque token identifying a popped-out tab's detached window
/// (→ [`shell_core::detach::detached_label`]). A lector content-webview label is already
/// `{window}:tab-<hash>` — globally unique across windows and Tauri-label-safe — so it *is* the
/// token; the origin window is tracked on [`commands::LectorDetached`], never parsed back out of
/// the label. Mirrors curator's `detach_window_token` exactly.
pub(crate) fn detach_window_token(tab_label: &str) -> String {
    tab_label.to_string()
}

/// Window theme for a given `dark_mode` setting. `None` = follow the system.
pub(crate) fn theme_for(dark_mode: bool) -> Option<tauri::Theme> {
    dark_mode.then_some(tauri::Theme::Dark)
}

/// Print config-load warnings to stderr. Shared shape with `validate_cli`'s own printing (kept
/// separate rather than factored together — that one also prints the resolved tab tree, this one
/// only ever prints warnings).
fn log_config_warnings(warnings: &[lector_config::Warning]) {
    for w in warnings {
        eprintln!("config warning [{}]: {}", w.window, w.message);
    }
}

/// The labels of every open window, config order.
fn open_labels(app: &tauri::AppHandle) -> Vec<String> {
    app.state::<WindowSet>()
        .entries()
        .into_iter()
        .filter(|e| e.open)
        .map(|e| e.id)
        .collect()
}

/// Emit an event to every open window's chrome sidebar.
pub(crate) fn emit_to_all_chrome<S: serde::Serialize + Clone>(
    app: &tauri::AppHandle,
    event: &str,
    payload: S,
) {
    for label in open_labels(app) {
        let _ = app.emit_to(label.as_str(), event, payload.clone());
    }
}

/// Every window of the last clean load under its [`WindowSet`] label, open or closed.
fn labelled_windows(app: &tauri::AppHandle) -> Vec<(String, lector_config::WindowConfig)> {
    let set = app.state::<WindowSet>();
    app.state::<commands::AppState>()
        .last_cfg()
        .windows
        .into_iter()
        .filter_map(|w| set.label_for_title(&w.title).map(|label| (label, w)))
        .collect()
}

/// Resolve every window's tabs under its current label and run the shared [`reload::reconcile`]
/// (tab-server reconciliation — the same path for launch-time eager-start and hot-reload), then
/// drop the webviews and active marks of tabs it removed.
fn sync_tabs(app: &tauri::AppHandle) {
    let state = app.state::<commands::AppState>();
    let views = reload::reconcile(&state, &labelled_windows(app));

    // A tab gone from the config (or relabelled by a `dir` change) has just lost its server: close
    // its content webview and clear it as active, so neither a dead page nor an unhighlighted
    // selection is left behind. A popped-out tab lives in its detached window and is kept.
    let mut keep: HashSet<String> = views.into_iter().map(|v| v.label).collect();
    keep.extend(state.detached_tab_labels());
    state.retain_active(&keep);
    for label in open_labels(app) {
        let Some(win) = app.get_window(&label) else {
            continue;
        };
        for wv in win.webviews() {
            if wv.label() != win.label() && !keep.contains(wv.label()) {
                let _ = wv.close();
            }
        }
    }
}

/// Apply a freshly-loaded config's app-wide settings and every open window's theme, then
/// [`sync_tabs`]. `cfg` must already be [`commands::AppState::set_last_cfg`]'s.
fn apply_config(
    app: &tauri::AppHandle,
    cfg: &lector_config::Config,
    warnings: &[lector_config::Warning],
) {
    log_config_warnings(warnings);
    let state = app.state::<commands::AppState>();
    state.set_global(
        cfg.density,
        cfg.sidebar_drag,
        cfg.auto_update,
        cfg.dark_mode,
        cfg.tab_digit_keys,
    );
    for label in open_labels(app) {
        if let Some(win) = app.get_window(&label) {
            let _ = win.set_theme(theme_for(cfg.dark_mode));
        }
    }
    sync_tabs(app);
}

/// Show a freshly built window's first tab exactly the way a click would (`commands::select`): the
/// tab it last had active (a reopened window — closing it ended the webview, not the server or the
/// mark), else its startup tab (`WindowTabs::startup_label` — the first load_on_open tab by
/// default, or whatever `open_on_launch` overrides to). A popped-out tab is never picked: its
/// webview label belongs to the detached window. A failure (e.g. the tab's dir doesn't exist yet)
/// stays cold.
fn show_first_tab(app: &tauri::AppHandle, label: &str) {
    let state = app.state::<commands::AppState>();
    let detached = state.detached_tab_labels();
    let startup = || {
        let title = app
            .state::<WindowSet>()
            .entries()
            .into_iter()
            .find(|e| e.id == label)?
            .title;
        state.window_cfg(&title)?.startup_label(label)
    };
    let Some(tab) = state
        .active_for(label)
        .or_else(startup)
        .filter(|t| !detached.contains(t))
    else {
        return;
    };
    if let Err(e) = commands::select(app, &state, &tab) {
        eprintln!("couldn't show tab {tab:?}: {e}");
        state.clear_active_if(&tab);
    }
}

/// [`WindowHost::build`](shell_core::windows::WindowHost::build): build `slot`'s window and record
/// its accent. Its tabs are shown by the caller once they resolve under its label.
pub(crate) fn open_window(
    app: &tauri::AppHandle,
    slot: &Slot,
) -> tauri::Result<tauri::WebviewWindow> {
    let window = webviews::build_window(
        app,
        &slot.label,
        &slot.decl.title,
        slot.decl.width,
        slot.decl.height,
    )?;
    let state = app.state::<commands::AppState>();
    state.set_colour(&slot.label, slot.decl.colour.clone());
    state.popped_out_of(&slot.label, &slot.decl.title);
    Ok(window)
}

/// [`WindowHost::closed`](shell_core::windows::WindowHost::closed). A window the user closed keeps
/// its servers and active mark for a reopen; one gone from the config takes its popped-out tabs
/// with it (their servers stop at the next [`reload::reconcile`], which no longer sees them).
pub(crate) fn window_closed(app: &tauri::AppHandle) {
    let set = app.state::<WindowSet>();
    // By title: a window renamed in place and closed is no longer listed under `label`.
    let orphans = app
        .state::<commands::AppState>()
        .take_orphaned_detached(&set.entries());
    if !orphans.is_empty() {
        for detached in orphans {
            if let Some(w) = app.get_window(&detached) {
                let _ = w.destroy();
            }
        }
        sync_detached(app);
    }
    // During ⌘Q every window is being destroyed and the menu dies with the app.
    if !set.is_quitting() {
        refresh_menu(app);
    }
}

/// [`WindowHost::retitled`](shell_core::windows::WindowHost::retitled): `label`'s title or colour
/// changed in place; carry it into `last_cfg` (so [`labelled_windows`] keeps the window before the
/// reload echo) and into the origin titles of the tabs popped out of it, and repaint its banner.
pub(crate) fn window_retitled(app: &tauri::AppHandle, label: &str, decl: &WindowDecl) {
    let state = app.state::<commands::AppState>();
    let set = app.state::<WindowSet>();
    // The set's entries and `last_cfg` list the same windows in the same order; `label` is the
    // pre-rename one, which a closed window has already lost.
    let title = decl.title.trim();
    let old = set
        .entries()
        .iter()
        .position(|e| e.title.trim() == title)
        .and_then(|i| state.last_cfg().windows.get(i).map(|w| w.title.clone()));
    if let Some(old) = old {
        state.retitle_origins(&old, title);
    }
    state.retitle_window(&decl.title, decl.colour.clone(), |t| {
        set.label_for_title(t).is_some()
    });
    state.set_colour(label, decl.colour.clone());
    let _ = app.emit_to(label, "config-reloaded", ());
}

/// Mirror the popped-out window count into the [`WindowSet`], which keeps the home surface away
/// while one is up.
pub(crate) fn sync_detached(app: &tauri::AppHandle) {
    let count = app
        .state::<commands::AppState>()
        .detached
        .lock()
        .expect("detached lock")
        .len();
    app.state::<WindowSet>().set_detached_count(app, count);
}

/// The `nav-tab` payload: step `dir` from `from`, a popped-out tab, or else from the highlight.
#[derive(Clone, serde::Serialize)]
struct NavTab {
    dir: i32,
    from: Option<String>,
}

/// Runs a spine command in the window `shell_core::menu::route` chose. A window's chrome is its
/// main webview, so the target label *is* the chrome's.
///
/// **An emit here only scopes anything because `chrome.js` binds its listeners to the chrome
/// webview.** A JS `listen()` with no target registers `EventTarget::Any`, which tauri delivers to
/// regardless of the label here — see the per-window-events footgun in CLAUDE.md.
fn on_spine_action(app: &tauri::AppHandle, routed: shell_core::menu::Routed) {
    use shell_core::menu::{SpineAction, TabNavAction};
    let target = routed.target.as_str();
    if routed.from_detached.is_some() {
        match app.get_window(target) {
            Some(w) => {
                let _ = w.set_focus();
            }
            // The pop-out's origin was closed: bring it back. Its chrome mounts fresh, with nothing
            // listening yet for this command, so the command ends there.
            None => {
                open_or_focus_window(app, target);
                return;
            }
        }
    }
    let from = routed.from_detached.as_deref().and_then(|l| {
        let state = app.state::<commands::AppState>();
        let detached = state.detached.lock().expect("detached lock");
        detached.get(l).map(|d| d.tab_label.clone())
    });
    let emit = |event: &str| {
        let _ = app.emit_to(target, event, ());
    };
    match routed.action {
        SpineAction::Nav(TabNavAction::Next) => {
            let _ = app.emit_to(target, "nav-tab", NavTab { dir: 1, from });
        }
        SpineAction::Nav(TabNavAction::Prev) => {
            let _ = app.emit_to(target, "nav-tab", NavTab { dir: -1, from });
        }
        SpineAction::Nav(TabNavAction::Jump(n)) => {
            let _ = app.emit_to(target, "jump-tab", n);
        }
        // ⌘W unloads the ACTIVE TAB — it does not close the window. The chrome owns which tab is
        // active and the dot repaint, so it drives unload_tab off this event.
        SpineAction::CloseTab => emit("close-tab"),
        // Closing a pop-out window docks its tab: its `Destroyed` handler runs `redock`.
        SpineAction::DockPoppedOut | SpineAction::CloseWindow => {
            if let Some(w) = app.get_window(target) {
                let _ = w.close();
            }
        }
        SpineAction::PopOutTab => emit("pop-out-tab"),
        // ⌘⇧F: the content webview may hold first responder, so make the chrome the key view
        // before it focuses its field.
        SpineAction::FindInSidebar => {
            if let Some(chrome) = app.get_webview(target) {
                let _ = chrome.set_focus();
            }
            emit("find-in-sidebar");
        }
        // chrome-core owns self-update; forward to its checkForUpdateNow().
        SpineAction::CheckUpdates => emit("check-update"),
    }
}

/// The menu spine's Window submenu selector, and the home surface's per-window button
/// (`shell_home_open_window`): focus `label`'s window, or reopen it. A reopened window can sit on a
/// new label (renamed while open), so its tabs are resolved again before its first tab shows.
fn open_or_focus_window(app: &tauri::AppHandle, label: &str) {
    let was_open = app.get_window(label).is_some();
    app.state::<WindowSet>().open_or_focus(app, label);
    if !was_open && app.get_window(label).is_some() {
        sync_tabs(app);
        show_first_tab(app, label);
    }
    refresh_menu(app);
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
/// `AppState::detached` — so the reopen never picks the popped-out tab as its first tab. Only then
/// is the bookkeeping removed and the webview recreated on the origin, which then becomes active.
pub(crate) fn redock(app: &tauri::AppHandle, detached_label: &str) {
    // ⌘Q teardown: `RunEvent::ExitRequested` fires before every window's `Destroyed`. Don't reopen
    // an origin or recreate a webview mid-quit — everything is being torn down, and the servers are
    // about to be shut down wholesale anyway (see `run`'s `RunEvent::Exit` arm).
    if app.state::<WindowSet>().is_quitting() {
        return;
    }
    let state = app.state::<commands::AppState>();

    // Find the origin by title and point the bookkeeping (and the tab's server) at its current
    // label, then peek the origin + tab + port without removing (see the doc comment's ordering
    // rationale). With no origin left in the config, the tab ends below like a removed one.
    let home = state.resolve_origin(detached_label, &app.state::<WindowSet>().entries());
    let Some((origin_wid, tab_label, port)) = state
        .detached
        .lock()
        .expect("detached lock")
        .get(detached_label)
        .map(|d| (d.origin_wid.clone(), d.tab_label.clone(), d.port))
    else {
        return; // already redocked (double-close) — nothing to do
    };

    // Reopen the origin if the user closed it while the tab was popped out.
    if matches!(home, Some((_, false))) {
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
    sync_detached(app);

    let view = state.view(&tab_label);
    if home.is_none() || view.is_none() {
        // The tab (or its whole window) was removed from the config while it was popped out — it
        // simply ends rather than coming back. The server it was keeping alive is no longer wanted
        // by anything: stop it explicitly (rather than leaving it to be swept up only at quit) so a
        // config edit doesn't leak a serve loop + watcher for the rest of the session. A view
        // still under its label belongs to a window that took the origin's freed label, and keeps
        // the server.
        if view.is_none() {
            state.servers.stop(&tab_label);
        }
        return;
    }

    // Recreate the content webview on the origin, reusing the SAME port — this is what makes the
    // round trip near-lossless: the compositor serve loop (and its watcher) never stopped, so the
    // page picks up exactly where the detached window's copy left off.
    let _ = webviews::show(app, &tab_label, port);
    state.set_active(&tab_label);

    // Re-render the origin chrome so the returned row loses its ⤢ detached mark and reflects the new
    // active tab. `config-reloaded` drives the chrome's refresh() (a get_tabs re-fetch); emit_to
    // targets only that window's chrome (scoped only in concert with chrome.js's webview-bound
    // listeners, as `on_spine_action`'s emits are). If the origin was just reopened its
    // fresh mount already refreshes, so a missed emit self-corrects.
    let _ = app.emit_to(origin_wid.as_str(), "config-reloaded", ());
}

/// Load the config and apply it — the home surface's "Create a starter config" button, which turns
/// a `NoConfig` state into a real config the app has never seen.
pub(crate) fn reload_now(app: &tauri::AppHandle) {
    let path = lector_config::resolve_config_path();
    match lector_config::load_config(&path) {
        Ok((cfg, warnings)) => apply_clean_load(app, &cfg, &warnings),
        Err(e) => eprintln!("config error: {e}"),
    }
}

/// Apply a cleanly loaded config — the one sequence the config watcher, [`reload_now`] and the
/// chrome's ⟳ rescan all run: the [`WindowSet`] opens added windows, closes removed ones and
/// recolours kept ones (a window the user closed stays closed); then tab state is applied, each
/// newly opened window shows its first tab, every chrome refreshes, and the app menu is rebuilt (so
/// a `tab_digit_keys` flip and the Window submenu track the config).
pub(crate) fn apply_clean_load(
    app: &tauri::AppHandle,
    cfg: &lector_config::Config,
    warnings: &[lector_config::Warning],
) {
    app.state::<commands::AppState>().set_last_cfg(cfg.clone());
    let opened: Vec<String> = app
        .state::<WindowSet>()
        .apply(app, windows::decls(cfg))
        .into_iter()
        .filter_map(|op| match op {
            WindowOp::Open(slot) => Some(slot.label),
            _ => None,
        })
        .collect();
    apply_config(app, cfg, warnings);
    for label in &opened {
        show_first_tab(app, label);
    }
    emit_to_all_chrome(app, "config-reloaded", ());
    refresh_menu(app);
}

/// Surface a config that failed to load: every chrome and the home surface show the error, and
/// the last-good config (and every running server) is left untouched.
pub(crate) fn surface_load_error(app: &tauri::AppHandle, msg: &str) {
    emit_to_all_chrome(app, "config-error", msg.to_string());
    app.state::<WindowSet>()
        .set_load_error(app, Some(msg.to_string()));
}

/// Build and install the app menu from the current state: shell-core's App, Edit, Tab, Config and
/// Window submenus (lector adds no items of its own).
///
/// Rebuilt on every clean load (a `tab_digit_keys` flip applies without a relaunch) and whenever a
/// window opens, closes, is renamed or deleted (the Window submenu's ticks and "(closed)" labels).
fn refresh_menu(app: &tauri::AppHandle) {
    let state = app.state::<commands::AppState>();
    let entries = app.state::<WindowSet>().entries();
    let built = shell_core::menu::build_spine(
        app,
        shell_core::menu::SpineConfig {
            app_name: "lector",
            windows: &entries,
        },
        env!("CARGO_PKG_VERSION"),
        env!("BUILD_GIT_SHA"),
        env!("BUILD_DATE"),
    )
    .and_then(|spine| {
        let cycle = state.tab_digit_keys().is_cycle();
        let tab_menu = shell_core::menu::build_tab_menu(app, &spine, cycle, &[])?;
        let items: Vec<&dyn tauri::menu::IsMenuItem<_>> = vec![
            &spine.submenus[0], // App
            &spine.edit,
            &tab_menu,
            &spine.submenus[1], // Config
            &spine.submenus[2], // Window
        ];
        tauri::menu::MenuBuilder::new(app).items(&items).build()
    });
    match built {
        Ok(menu) => {
            let _ = app.set_menu(menu);
        }
        Err(e) => eprintln!("menu build failed: {e}"),
    }
}

pub fn run() {
    let config_path = lector_config::resolve_config_path();
    let app = shell_core::register_plugins(tauri::Builder::default(), Some(&config_path), &[])
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
            app.state::<commands::AppState>().set_last_cfg(cfg.clone());

            // The window set opens the `open_on_start` windows, lists the rest, and shows the home
            // surface whenever no window is open (a failed load, no `[[window]]`, all closed). The
            // user closing the last window quits instead.
            app.manage(
                WindowSet::new("lector", path.clone(), windows::LectorHost)
                    .quit_on_last_close(true),
            );
            let set = app.state::<WindowSet>();
            match load_error {
                None => set.materialize(app.handle(), windows::decls(&cfg)),
                Some(e) => set.set_load_error(app.handle(), Some(e)),
            }

            apply_config(app.handle(), &cfg, &warnings);

            // Native mouse side-button (back/forward) navigation — the shared shell-core NSEvent monitor
            // (WKWebView never delivers the side buttons to the DOM). lector supplies the
            // focused-active-webview resolver; shell-core owns the monitor + native goBack/goForward.
            let mouse_nav_handle = app.handle().clone();
            shell_core::mouse_nav::install(move || {
                let win = mouse_nav_handle.get_focused_window()?;
                let label = mouse_nav_handle
                    .state::<commands::AppState>()
                    .shown_tab(win.label())?;
                win.get_webview(&label)
            });

            for label in open_labels(app.handle()) {
                show_first_tab(app.handle(), &label);
            }
            refresh_menu(app.handle());

            let cfg_for_menu = path.clone();
            app.on_menu_event(move |app, event| {
                let id = event.id().as_ref();
                // The spine's file-acting ids need no window — let it consume them first.
                if shell_core::menu::handle_spine_event(id, &cfg_for_menu) {
                    return;
                }
                // Tab ▸ Cycle Tabs: save the flip; the watcher's reload rebuilds the menu. muda
                // already flipped the tick, so a failed write rebuilds to undo it.
                if id == shell_core::menu::ids::TAB_DIGIT_CYCLE {
                    if let Err(e) = commands::toggle_tab_digit_keys(&cfg_for_menu) {
                        surface_load_error(app, &e);
                        refresh_menu(app);
                    }
                    return;
                }
                if app.state::<WindowSet>().handle_menu(app, id) {
                    return;
                }
                if let Some(label) = shell_core::menu::selected_window(id) {
                    open_or_focus_window(app, label);
                    return;
                }
                let Some(focused) = app.get_focused_window() else {
                    return;
                };
                let origin_of = |l: &str| {
                    let entries = app.state::<WindowSet>().entries();
                    app.state::<commands::AppState>()
                        .resolve_origin(l, &entries)
                        .map(|(wid, _)| wid)
                };
                if let Some(routed) = shell_core::menu::route(id, focused.label(), origin_of) {
                    on_spine_action(app, routed);
                }
            });

            // Watch the config file and hot-reload on change, keeping the last-good config (and
            // surfacing the error to every window's chrome) if the new contents don't
            // parse/validate. A failed reload tears nothing down — see `reload::reconcile`'s doc
            // and `lector_config`'s "missing dir warns, never errors" rule this mirrors: an
            // un-cloned repo, or a config with one bad line, must not strand every other tab.
            // The shared shell-core watcher owns the parent-dir watch, the file-name match (macOS
            // FSEvents-robust, so a symlinked config dir's events are seen), and the echo-swallow
            // (via the `Option<String>` the closure returns on a format write). lector supplies just
            // the parse + apply.
            let app_handle = app.handle().clone();
            let fmt_path = path.clone();
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
                        apply_clean_load(&app_handle, &new_cfg, &warnings);
                        self_write
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        eprintln!("config error: {msg}");
                        surface_load_error(&app_handle, &msg);
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
            commands::window_menu,
        ])
        .build(tauri::generate_context!())
        .expect("error while building lector");

    app.run(|app_handle, event| {
        // ExitRequested fires before every window's Destroyed during ⌘Q; mark quitting so a
        // detached window's teardown doesn't reopen its origin mid-quit (see `redock`) and the
        // closing windows don't raise the home surface.
        if let tauri::RunEvent::ExitRequested { .. } = event {
            if let Some(set) = app_handle.try_state::<WindowSet>() {
                set.mark_quitting();
            }
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
                for v in w.tab_views(&w.title) {
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
        // A content-webview label is `{window}:tab-<hash>` — the token IS that label, and it
        // must survive shell-core's detached-label wrapping so a detached window is recognised
        // (is_detached_label) and its token recoverable (detach_token). Deterministic, too. Mirrors
        // curator's identical test for its own `detach_window_token`.
        let tab_label = "Docs:tab-00112233445566ff";
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
