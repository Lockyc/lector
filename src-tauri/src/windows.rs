//! lector's side of shell-core's shared window layer: the [`WindowHost`] that builds and tears
//! down a window's content and writes the window form's changes to the config, and the config →
//! [`WindowDecl`] mapping.

use lector_config::edit::{self, At};
use shell_core::windows::plan::{Slot, WindowDecl};
use shell_core::windows::{WindowHost, WindowWrite};
use tauri::{AppHandle, WebviewWindow};

/// What shell-core needs to know about one `[[window]]`.
pub(crate) fn decl(w: &lector_config::WindowConfig) -> WindowDecl {
    WindowDecl {
        title: w.title.clone(),
        colour: w.colour.clone(),
        width: w.width as f64,
        height: w.height as f64,
        open_on_start: w.open_on_start,
    }
}

/// Every window's [`decl`], config order.
pub(crate) fn decls(cfg: &lector_config::Config) -> Vec<WindowDecl> {
    cfg.windows.iter().map(decl).collect()
}

/// lector's per-window content, built and torn down by the managed
/// [`WindowSet`](shell_core::windows::WindowSet).
pub(crate) struct LectorHost;

impl WindowHost for LectorHost {
    fn build(&self, app: &AppHandle, slot: &Slot) -> tauri::Result<WebviewWindow> {
        crate::open_window(app, slot)
    }

    fn closed(&self, app: &AppHandle, _label: &str) {
        crate::window_closed(app);
    }

    fn retitled(&self, app: &AppHandle, label: &str, decl: &WindowDecl) {
        crate::window_retitled(app, label, decl);
    }

    fn write(&self, _app: &AppHandle, write: &WindowWrite) -> Result<(), String> {
        let path = lector_config::resolve_config_path();
        match write {
            WindowWrite::Add { title, colour } => edit::add_window(&path, title, colour.as_deref()),
            WindowWrite::Set {
                title,
                new_title,
                colour,
            } => edit::edit_window(
                &path,
                title,
                new_title.as_deref(),
                colour.as_ref().map(Option::as_deref),
            ),
            WindowWrite::Remove { title } => edit::remove(
                &path,
                At {
                    window: title,
                    group: None,
                    tab: None,
                },
            ),
        }
        .map_err(|e| e.to_string())
    }

    fn edit_failed(&self, app: &AppHandle, message: &str) {
        eprintln!("window edit failed: {message}");
        crate::emit_to_all_chrome(app, "config-error", message.to_string());
    }

    fn changed(&self, app: &AppHandle) {
        crate::refresh_menu(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(src: &str) -> lector_config::Config {
        lector_config::parse_and_validate(src).unwrap().0
    }

    #[test]
    fn windows_map_to_decls_in_config_order() {
        let c = cfg("[[window]]\ntitle = \"Docs\"\nwidth = 800\nheight = 600\ncolour = \"#abc\"\nopen_on_start = false\nopen_on_launch = true\n[[window]]\ntitle = \"Side\"\n");
        assert_eq!(
            decls(&c),
            vec![
                WindowDecl {
                    title: "Docs".into(),
                    colour: Some("#abc".into()),
                    width: 800.0,
                    height: 600.0,
                    open_on_start: false,
                },
                WindowDecl {
                    title: "Side".into(),
                    colour: None,
                    width: 1500.0,
                    height: 1000.0,
                    open_on_start: true,
                },
            ]
        );
    }

    #[test]
    fn a_reload_leaves_a_user_closed_window_closed() {
        // The reload echo of an unchanged config: "Side" is declared but not live (the user closed
        // it), so nothing reopens it. Only a newly declared title opens.
        let old = decls(&cfg(
            "[[window]]\ntitle = \"Docs\"\n[[window]]\ntitle = \"Side\"\n",
        ));
        let live = std::collections::BTreeMap::from([("Docs".to_string(), "Docs".to_string())]);
        assert!(shell_core::windows::plan::reconcile(&old, &old, &live).is_empty());
        let grown = decls(&cfg(
            "[[window]]\ntitle = \"Docs\"\n[[window]]\ntitle = \"Side\"\n[[window]]\ntitle = \"New\"\n",
        ));
        let ops = shell_core::windows::plan::reconcile(&old, &grown, &live);
        assert!(
            matches!(ops.as_slice(), [shell_core::windows::plan::WindowOp::Open(s)] if s.decl.title == "New"),
            "{ops:?}"
        );
    }
}
