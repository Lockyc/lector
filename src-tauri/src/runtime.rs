//! What lector's load paths ask of the Tauri runtime beyond [`tauri::Runtime`]: the window set, a
//! window build, whether a window is live, a content webview's native decorations, and the menu
//! bar. The app runs on [`tauri::Wry`]; the tests drive the same paths on
//! `tauri::test::MockRuntime` (`mock_app.rs`), which can't build a menu off the main thread and
//! never reports a window destroyed.

use shell_core::menu::WindowEntry;
use shell_core::windows::plan::{WindowDecl, WindowOp};
use shell_core::windows::WindowSet;
use tauri::webview::WebviewWindowBuilder;
use tauri::{AppHandle, Manager, Webview, WebviewWindow, Window, Wry};

/// A runtime lector's load paths run on.
pub(crate) trait LectorRuntime: tauri::Runtime {
    /// The managed window set.
    type Windows: Windows<Self>;

    /// Build the window `builder` describes under `label`.
    fn build_window(
        app: &AppHandle<Self>,
        label: &str,
        builder: WebviewWindowBuilder<'_, Self, AppHandle<Self>>,
    ) -> tauri::Result<WebviewWindow<Self>>;

    /// The window under `label`, if it is live.
    fn live_window(app: &AppHandle<Self>, label: &str) -> Option<Window<Self>>;

    /// Install a freshly built content webview's loading bar, tinted `accent` (sRGB rgba, 0–1),
    /// and its pointer watch.
    fn decorate_content(webview: &Webview<Self>, accent: (f64, f64, f64, f64));

    /// Close a content webview so its page stops: a bare `close()` leaves wry's WKWebView, and
    /// the page in its WebContent process, running.
    fn close_content(webview: &Webview<Self>);

    /// Rebuild the app menu from the current state ([`crate::refresh_menu`]).
    fn refresh_menu(app: &AppHandle<Self>);
}

/// The window set's surface the load paths use: [`WindowSet`]'s methods of the same names.
pub(crate) trait Windows<R: tauri::Runtime>: Send + Sync + 'static {
    fn entries(&self) -> Vec<WindowEntry>;
    fn apply(&self, app: &AppHandle<R>, decls: Vec<WindowDecl>) -> Vec<WindowOp>;
    fn open_or_focus(&self, app: &AppHandle<R>, label: &str);
    fn is_quitting(&self) -> bool;
    fn set_detached_count(&self, app: &AppHandle<R>, count: usize);
    fn set_load_error(&self, app: &AppHandle<R>, error: Option<String>);
}

impl Windows<Wry> for WindowSet {
    fn entries(&self) -> Vec<WindowEntry> {
        WindowSet::entries(self)
    }

    fn apply(&self, app: &AppHandle, decls: Vec<WindowDecl>) -> Vec<WindowOp> {
        WindowSet::apply(self, app, decls)
    }

    fn open_or_focus(&self, app: &AppHandle, label: &str) {
        WindowSet::open_or_focus(self, app, label)
    }

    fn is_quitting(&self) -> bool {
        WindowSet::is_quitting(self)
    }

    fn set_detached_count(&self, app: &AppHandle, count: usize) {
        WindowSet::set_detached_count(self, app, count)
    }

    fn set_load_error(&self, app: &AppHandle, error: Option<String>) {
        WindowSet::set_load_error(self, app, error)
    }
}

impl LectorRuntime for Wry {
    type Windows = WindowSet;

    fn build_window(
        _app: &AppHandle,
        _label: &str,
        builder: WebviewWindowBuilder<'_, Wry, AppHandle>,
    ) -> tauri::Result<WebviewWindow> {
        builder.build()
    }

    fn live_window(app: &AppHandle, label: &str) -> Option<Window> {
        app.get_window(label)
    }

    fn decorate_content(webview: &Webview, accent: (f64, f64, f64, f64)) {
        shell_core::progress_bar::install(webview, accent);
        // The page composites over the chrome, which can't see the pointer leave for it.
        shell_core::web::pointer::watch(webview);
    }

    fn close_content(webview: &Webview) {
        shell_core::web::content::close_content_webview(webview);
    }

    fn refresh_menu(app: &AppHandle) {
        crate::refresh_menu(app)
    }
}
