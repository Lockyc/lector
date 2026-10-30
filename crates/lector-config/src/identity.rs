//! Webview label identity. A window's Tauri label (assigned by shell-core's `WindowSet`)
//! namespaces its content webview labels, since Tauri labels are app-global and two windows sharing
//! a repo would otherwise collide.

/// Namespace a within-window label (e.g. `tab-<hash>`) under a window label. `:` is a legal Tauri
/// label char, so `window_label:within` composites are valid labels.
pub fn namespaced(window_label: &str, within: &str) -> String {
    format!("{window_label}:{within}")
}

/// The window label a [`namespaced`] tab label belongs to. Split at the **last** `:`: a window
/// label may itself contain `:` (a title like `a:b`), a within-window label never does.
pub fn window_of(tab_label: &str) -> &str {
    tab_label.rsplit_once(':').map_or("", |(w, _)| w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaced_composes_with_a_colon() {
        assert_eq!(namespaced("Docs", "chrome"), "Docs:chrome");
    }

    #[test]
    fn window_of_recovers_a_window_label_that_contains_a_colon() {
        assert_eq!(window_of(&namespaced("Docs", "tab-00ff")), "Docs");
        assert_eq!(window_of(&namespaced("a:b", "tab-00ff-1")), "a:b");
        assert_eq!(window_of("no-colon"), "");
    }
}
