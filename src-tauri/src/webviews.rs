//! Window + content webview management. lector's sidebar chrome is the window's MAIN webview
//! (hole-punch, matching curator/warden): it spans the whole window, so `data-tauri-drag-region` in
//! it moves the window natively (a child webview can't). Rust-positioned `add_child` content
//! webviews composite ABOVE it, filling the content hole to the right of the sidebar. lector shows
//! exactly one tab at a time (no curator-style "stay live in the background" tabs), so switching is
//! just show-this-one-hide-the-rest.
//!
//! **Link escape.** A doc linking off-site (`https://github.com/…`) would otherwise navigate the
//! tab off its local compositor site and strand it — the tab has no back button and no way home.
//! `is_own_origin` gates every navigation in a content webview's `on_navigation`: only a URL whose
//! scheme, host, AND port all match this tab's own `127.0.0.1:{port}` stays in the webview;
//! anything else — including *another tab's* loopback port, which is still loopback but would
//! silently show a different repo's site inside this tab — opens in the system browser instead.

use tauri::webview::WebviewBuilder;
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, TitleBarStyle, Url, WebviewUrl, Window,
};

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// Default sidebar width. `Identity` (in `commands.rs`) deliberately carries no `default_width`
/// field, so `src/chrome.js`'s `(id && id.default_width) || 240` always takes its literal `240`
/// fallback — this constant MUST match that literal by hand (no value crosses the IPC boundary to
/// keep them in sync); see `src/chrome.js`'s own comment at the same value.
pub const CHROME_W: f64 = 240.0;

/// The content hole's rect in logical px (top-left origin), exactly as the chrome measures its
/// `#content-hole` element via `getBoundingClientRect` and reports it through `set_hole_rect`. This
/// is the single source of truth for content-webview placement — the chrome owns the sidebar width
/// and its resize clamp; Rust just tracks and applies the rect it reports.
#[derive(Debug, Clone, Copy)]
pub struct HoleRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The best-guess hole before the chrome's first `set_hole_rect`: full height, offset by the
/// default sidebar width. The first report corrects it.
fn initial_hole(win_w: f64, win_h: f64) -> HoleRect {
    HoleRect {
        x: CHROME_W,
        y: 0.0,
        width: (win_w - CHROME_W).max(0.0),
        height: win_h,
    }
}

/// Each open window's current content hole, keyed by window id. Module-owned rather than a field on
/// the shared `AppState`: webview placement is entirely this module's concern, and keeping it here
/// avoids adding a field to `AppState` that only this file would ever touch.
static HOLES: LazyLock<Mutex<HashMap<String, HoleRect>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn hole_for(window_id: &str) -> HoleRect {
    HOLES
        .lock()
        .expect("holes lock")
        .get(window_id)
        .copied()
        .unwrap_or(HoleRect {
            x: CHROME_W,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        })
}

/// Build a window and its chrome (sidebar) webview. `window_id` becomes the window label and
/// namespaces the content webview labels (`{window_id}:tab-<hash>`, from `lector_config::identity`).
pub fn build_window(
    app: &AppHandle,
    window_id: &str,
    title: &str,
    win_w: f64,
    win_h: f64,
) -> tauri::Result<Window> {
    let webview_window =
        tauri::WebviewWindowBuilder::new(app, window_id, WebviewUrl::App("index.html".into()))
            .title(title)
            .inner_size(win_w, win_h)
            .hidden_title(true)
            .title_bar_style(TitleBarStyle::Overlay)
            .build()?;
    let window = webview_window.as_ref().window();

    HOLES
        .lock()
        .expect("holes lock")
        .insert(window_id.to_string(), initial_hole(win_w, win_h));

    Ok(window)
}

/// Position every content webview to fill the given hole. The chrome is skipped (its label equals
/// the window label, unlike a content webview's `{window_id}:tab-<hash>`).
fn layout_webviews(window: &Window, hole: HoleRect) {
    for wv in window.webviews() {
        if wv.label() == window.label() {
            continue;
        }
        let _ = wv.set_position(LogicalPosition::new(hole.x, hole.y));
        let _ = wv.set_size(LogicalSize::new(hole.width.max(0.0), hole.height.max(0.0)));
    }
}

/// Record a freshly-reported hole and reposition this window's content webviews to match. Called by
/// the `set_hole_rect` command on chrome mount, on a sidebar resize-drag, and on window resize.
pub fn set_hole_rect(window: &Window, hole: HoleRect) {
    HOLES
        .lock()
        .expect("holes lock")
        .insert(window.label().to_string(), hole);
    layout_webviews(window, hole);
}

/// Hide every content webview in the window except `label` (which is shown). The single switching
/// primitive: lector shows exactly one tab at a time.
fn raise_only(window: &Window, label: &str) -> Result<(), String> {
    for wv in window.webviews() {
        if wv.label() == window.label() {
            continue;
        }
        if wv.label() == label {
            wv.show().map_err(|e| e.to_string())?;
        } else {
            wv.hide().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// True iff `url` is this tab's own site — scheme, host, AND port must all match. Anything else
/// (an off-site doc link, or even *another tab's* loopback port) opens in the system browser: a doc
/// linking to github.com would otherwise navigate this tab off its site and strand it, since the tab
/// has no back button and no way home.
fn is_own_origin(url: &str, port: u16) -> bool {
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    authority == format!("127.0.0.1:{port}")
}

/// Hand a URL to the macOS default handler (the user's default browser). Side-effecting; not
/// unit-tested — mirrors curator's `escape::escape_to_default_browser`.
fn open_in_system_browser(url: &str) {
    let _ = std::process::Command::new("open").arg(url).spawn();
}

/// Create-or-navigate `label`'s content webview to `http://127.0.0.1:{port}/`, then show it and
/// hide every other tab in the window. This is where a cold tab's webview is born.
///
/// The webview's `on_navigation` is gated by [`is_own_origin`]: only this tab's own loopback origin
/// navigates in place, everything else escapes to the system browser (see the module doc).
pub fn show(app: &AppHandle, label: &str, port: u16) -> Result<(), String> {
    let window_id = label.split(':').next().unwrap_or_default();
    let window = app.get_window(window_id).ok_or("no such window")?;
    let url: Url = format!("http://127.0.0.1:{port}/")
        .parse()
        .map_err(|e| format!("{e}"))?;

    if let Some(wv) = window.get_webview(label) {
        wv.navigate(url).map_err(|e| e.to_string())?;
    } else {
        let hole = hole_for(window_id);
        let builder =
            WebviewBuilder::new(label, WebviewUrl::External(url)).on_navigation(move |target| {
                if is_own_origin(target.as_str(), port) {
                    true
                } else {
                    open_in_system_browser(target.as_str());
                    false
                }
            });
        window
            .add_child(
                builder,
                LogicalPosition::new(hole.x, hole.y),
                LogicalSize::new(hole.width.max(0.0), hole.height.max(0.0)),
            )
            .map_err(|e| e.to_string())?;
    }
    raise_only(&window, label)
}

/// Destroy `label`'s content webview if it exists, freeing its memory and removing it from the
/// window. No-op if it was never created (an unload of a tab that was never selected). Used by
/// `unload_tab` so a cold tab doesn't strand a dead-connection page on screen — without this, closing
/// the server but leaving the webview up shows a broken page instead of the app's empty state.
pub fn close(app: &AppHandle, label: &str) {
    let window_id = label.split(':').next().unwrap_or_default();
    let Some(window) = app.get_window(window_id) else {
        return;
    };
    if let Some(wv) = window.get_webview(label) {
        let _ = wv.close();
    }
}

/// Navigate an already-created content webview to `url` (curator's `reload_canonical`, applied to a
/// local site — used by `home_tab`). No-op if the webview hasn't been created yet.
pub fn navigate(app: &AppHandle, label: &str, url: &str) -> Result<(), String> {
    let window_id = label.split(':').next().unwrap_or_default();
    let Some(window) = app.get_window(window_id) else {
        return Ok(());
    };
    let Some(wv) = window.get_webview(label) else {
        return Ok(());
    };
    let url: Url = url.parse().map_err(|e| format!("{e}"))?;
    wv.navigate(url).map_err(|e| e.to_string())
}

/// Evaluate `script` in `label`'s content webview (the nav pill's back/forward — `commands.rs`'s
/// `nav_back`/`nav_forward`). No-op if the webview hasn't been created yet, same as [`navigate`].
pub fn eval_on(app: &AppHandle, label: &str, script: &str) -> Result<(), String> {
    let window_id = label.split(':').next().unwrap_or_default();
    let Some(window) = app.get_window(window_id) else {
        return Ok(());
    };
    let Some(wv) = window.get_webview(label) else {
        return Ok(());
    };
    wv.eval(script).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_hole_is_offset_by_the_default_sidebar_width() {
        let h = initial_hole(1400.0, 900.0);
        assert_eq!(h.x, CHROME_W);
        assert_eq!(h.y, 0.0);
        assert_eq!(h.width, 1400.0 - CHROME_W);
        assert_eq!(h.height, 900.0);
    }

    #[test]
    fn initial_hole_never_goes_negative_on_a_narrow_window() {
        let h = initial_hole(100.0, 900.0);
        assert_eq!(
            h.width, 0.0,
            "a window narrower than the sidebar must clamp to zero, not go negative"
        );
    }

    #[test]
    fn hole_for_an_unknown_window_falls_back_to_a_zero_width_default() {
        // No window has registered this id (build_window never ran for it) — must not panic or
        // return a stale/garbage rect.
        let h = hole_for("no-such-window-ever");
        assert_eq!(h.x, CHROME_W);
        assert_eq!(h.width, 0.0);
    }

    #[test]
    fn only_this_tabs_own_origin_stays_in_the_webview() {
        // Same origin — navigate in place.
        assert!(is_own_origin("http://127.0.0.1:8080/", 8080));
        assert!(is_own_origin("http://127.0.0.1:8080/guides/x.html", 8080));
        assert!(is_own_origin("http://127.0.0.1:8080/__reload", 8080));

        // A DIFFERENT tab's port is NOT this tab's origin — it is loopback, but navigating there
        // would silently show another repo's site inside this tab, with the sidebar still
        // highlighting this one. Escape it.
        assert!(!is_own_origin("http://127.0.0.1:9999/", 8080));

        // Off-site links escape to the system browser.
        assert!(!is_own_origin("https://github.com/lockyc/lector", 8080));
        assert!(!is_own_origin("http://example.test/", 8080));
        assert!(
            !is_own_origin("https://127.0.0.1:8080/", 8080),
            "scheme must match"
        );
        // A lookalike host must not pass a naive prefix check.
        assert!(!is_own_origin("http://127.0.0.1.evil.test/", 8080));
        assert!(!is_own_origin("http://127.0.0.1:8080.evil.test/", 8080));
    }
}
