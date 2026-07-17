use lector_config::hash::fnv1a_64;

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

pub fn run() {
    shell_core::register_plugins(tauri::Builder::default(), window_state_filename(), &[])
        .setup(move |_app| Ok(()))
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
