//! Window label identity. The window id namespaces a window's tab labels (Tauri labels are
//! app-global, so two windows sharing a repo would otherwise collide). It's a purely mechanical,
//! run-ephemeral label key — nothing persistent is tied to it, so renaming a window is harmless.
//! Derived from the title via frozen FNV-1a.

use crate::hash::fnv1a_64;

/// Stable, label-safe window id from the window title. `:` is a legal Tauri label char, so
/// `window_id:within` composites are valid labels.
pub fn window_id(title: &str) -> String {
    format!("w{:016x}", fnv1a_64(title.as_bytes()))
}

/// Namespace a within-window label (e.g. `chrome`, `tab-<hash>`) under a window id.
pub fn namespaced(window_id: &str, within: &str) -> String {
    format!("{window_id}:{within}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_id_is_stable_and_label_safe() {
        assert_eq!(window_id("Docs"), window_id("Docs"));
        assert_ne!(window_id("Docs"), window_id("Other"));
        assert!(window_id("Docs").starts_with('w'));
        assert_eq!(window_id("Docs").len(), 17);
    }

    #[test]
    fn namespaced_composes_with_a_colon() {
        assert_eq!(
            namespaced("wdeadbeefdeadbeef", "chrome"),
            "wdeadbeefdeadbeef:chrome"
        );
    }
}
