use lector_config::hash::fnv1a_64;
use tauri::Manager;

mod commands;
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

pub fn run() {
    shell_core::register_plugins(tauri::Builder::default(), window_state_filename(), &[])
        .manage(commands::AppState::new())
        .setup(move |app| {
            let path = lector_config::resolve_config_path();
            let (cfg, warnings) = match lector_config::load_config(&path) {
                Ok((c, warnings)) => (c, warnings),
                Err(e) => {
                    eprintln!("config error: {e}");
                    (lector_config::Config::default(), Vec::new())
                }
            };
            log_config_warnings(&warnings);

            let state = app.state::<commands::AppState>();
            state.set_global(cfg.density, cfg.sidebar_drag, cfg.auto_update);

            // Build every configured window, all cold: nothing is auto-started at launch (no
            // eager load_on_open, no open_on_launch active-tab selection) — that eager-start
            // wiring is Task 10's concern (it shares config hot-reload's reconciliation code, and
            // duplicating a slice of it here just to throw it away on the first reload would be
            // exactly the shadow the codebase avoids elsewhere). Every tab starts cold; the user's
            // first click is what starts a server and creates its webview.
            let mut all_views = Vec::new();
            for win_cfg in &cfg.windows {
                let wid = lector_config::identity::window_id(&win_cfg.title);
                webviews::build_window(
                    app.handle(),
                    &wid,
                    &win_cfg.title,
                    win_cfg.width as f64,
                    win_cfg.height as f64,
                )?;
                state.set_colour(&wid, win_cfg.colour.clone());
                all_views.extend(win_cfg.tab_views());
            }
            state.set_views(all_views);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_tabs,
            commands::window_identity,
            commands::select_tab,
            commands::unload_tab,
            commands::home_tab,
            commands::set_hole_rect,
        ])
        .run(tauri::generate_context!())
        .expect("error while running lector");
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
