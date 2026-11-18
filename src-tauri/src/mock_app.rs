//! A lector app on tauri's `MockRuntime`, so tests drive the `lib.rs` load paths themselves.
//!
//! shell-core's `WindowSet` takes a Wry `AppHandle`, so [`MockSet`] stands in for it: the same
//! decls and live labels under shell-core's own plan ([`assign`], [`reconcile`], [`menu_entries`]),
//! and each of its effects calls the lector hook `windows::LectorHost` wires to it, in
//! `WindowSet`'s order. A host hook never runs with the set's lock held.
//!
//! `MockRuntime` never reports a window destroyed, so a window it builds stays in tauri's manager
//! for good: [`MockSet`] tracks which are live, a reopen reuses the window, and a close ends the
//! window's content webviews the way `WindowSet`'s close does.

use crate::commands::{AppState, LectorDetached};
use crate::runtime::{LectorRuntime, Windows};
use shell_core::menu::WindowEntry;
use shell_core::windows::plan::{assign, menu_entries, reconcile, Slot, WindowDecl, WindowOp};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use tauri::test::MockRuntime;
use tauri::webview::WebviewWindowBuilder;
use tauri::{AppHandle, Manager, Webview, WebviewWindow, Window};

/// shell-core's `WindowSet` for the mock (module doc).
#[derive(Default)]
pub(crate) struct MockSet {
    state: Mutex<SetState>,
    /// How many times the menu bar was rebuilt.
    pub menus: AtomicUsize,
    pub load_error: Mutex<Option<String>>,
    pub detached: AtomicUsize,
}

#[derive(Default)]
struct SetState {
    decls: Vec<WindowDecl>,
    /// Trimmed title → the label of its open window.
    live: BTreeMap<String, String>,
    /// Until the first apply, an added window opens only if `open_on_start`.
    launched: bool,
}

impl SetState {
    fn slots(&self) -> Vec<Slot> {
        assign(&self.decls, &self.live)
    }

    fn slot(&self, label: &str) -> Option<(usize, Slot)> {
        self.slots()
            .into_iter()
            .enumerate()
            .find(|(_, s)| s.label == label)
    }

    /// `WindowSet`'s `open`: mark `slot` live; it is built next.
    fn open(&mut self, slot: &Slot) {
        self.live
            .insert(slot.decl.title.trim().to_string(), slot.label.clone());
    }
}

/// One of `WindowSet`'s effects.
enum Effect {
    Build(Slot),
    Close(String),
    Retitled {
        label: String,
        old: String,
        decl: WindowDecl,
    },
}

impl MockSet {
    fn lock(&self) -> MutexGuard<'_, SetState> {
        self.state.lock().unwrap()
    }

    pub fn is_live(&self, label: &str) -> bool {
        self.lock().live.values().any(|l| l == label)
    }

    /// The trimmed title → label of every open window.
    pub fn live(&self) -> BTreeMap<String, String> {
        self.lock().live.clone()
    }

    fn perform(&self, app: &AppHandle<MockRuntime>, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Build(slot) => {
                    if crate::open_window(app, &slot).is_err() {
                        self.lock().live.retain(|_, l| *l != slot.label);
                    }
                }
                Effect::Close(label) => {
                    crate::window_closed(app);
                    close_pages(app, &label);
                }
                Effect::Retitled { label, old, decl } => {
                    crate::window_retitled(app, &label, &old, &decl)
                }
            }
        }
    }

    /// The user closed `label`'s window: `WindowSet`'s close request (its pages end), then its
    /// `Destroyed`.
    pub fn user_close(&self, app: &AppHandle<MockRuntime>, label: &str) {
        close_pages(app, label);
        let was_open = {
            let mut st = self.lock();
            let before = st.live.len();
            st.live.retain(|_, l| l != label);
            st.live.len() != before
        };
        if was_open {
            crate::window_closed(app);
        }
    }

    /// `WindowSet::rename`, as the window form runs it after writing the config.
    pub fn rename(&self, app: &AppHandle<MockRuntime>, label: &str, new_title: &str) {
        let effects = {
            let mut st = self.lock();
            let Some((idx, slot)) = st.slot(label) else {
                return;
            };
            let (old, title) = (slot.decl.title.trim().to_string(), new_title.trim());
            if old == title {
                return;
            }
            st.decls[idx].title = title.to_string();
            if slot.open {
                st.live.remove(&old);
                st.live.insert(title.to_string(), slot.label.clone());
            }
            vec![Effect::Retitled {
                label: slot.label,
                old,
                decl: st.decls[idx].clone(),
            }]
        };
        self.perform(app, effects);
    }

    /// `WindowSet::delete`, as the window form runs it after writing the config.
    pub fn delete(&self, app: &AppHandle<MockRuntime>, label: &str) {
        let effects = {
            let mut st = self.lock();
            let Some((idx, slot)) = st.slot(label) else {
                return;
            };
            st.decls.remove(idx);
            if slot.open {
                st.live.remove(slot.decl.title.trim());
                vec![Effect::Close(slot.label)]
            } else {
                Vec::new()
            }
        };
        self.perform(app, effects);
    }

    /// `WindowHost::changed`, which the window form calls once its steps landed.
    pub fn form_changed(&self, app: &AppHandle<MockRuntime>) {
        crate::window_form_landed(app);
    }
}

impl Windows<MockRuntime> for MockSet {
    fn entries(&self) -> Vec<WindowEntry> {
        menu_entries(&self.lock().slots())
    }

    fn apply(&self, app: &AppHandle<MockRuntime>, decls: Vec<WindowDecl>) -> Vec<WindowOp> {
        *self.load_error.lock().unwrap() = None;
        let (ops, effects) = {
            let mut st = self.lock();
            let mut ops = reconcile(&st.decls, &decls, &st.live);
            if !std::mem::replace(&mut st.launched, true) {
                ops.retain(|op| !matches!(op, WindowOp::Open(s) if !s.decl.open_on_start));
            }
            st.decls = decls;
            let effects = ops
                .iter()
                .cloned()
                .map(|op| match op {
                    WindowOp::Open(slot) => {
                        st.open(&slot);
                        Effect::Build(slot)
                    }
                    WindowOp::Close(label) => {
                        st.live.retain(|_, l| *l != label);
                        Effect::Close(label)
                    }
                    WindowOp::Update { label, decl } => Effect::Retitled {
                        label,
                        old: decl.title.trim().to_string(),
                        decl,
                    },
                })
                .collect();
            (ops, effects)
        };
        self.perform(app, effects);
        ops
    }

    fn open_or_focus(&self, app: &AppHandle<MockRuntime>, label: &str) {
        let effects = {
            let mut st = self.lock();
            match st.slot(label) {
                Some((_, slot)) if !slot.open => {
                    st.open(&slot);
                    vec![Effect::Build(slot)]
                }
                _ => Vec::new(),
            }
        };
        self.perform(app, effects);
    }

    fn is_quitting(&self) -> bool {
        false
    }

    fn set_detached_count(&self, _app: &AppHandle<MockRuntime>, count: usize) {
        self.detached.store(count, Ordering::Relaxed);
    }

    fn set_load_error(&self, _app: &AppHandle<MockRuntime>, error: Option<String>) {
        *self.load_error.lock().unwrap() = error;
    }
}

/// End `label`'s content webviews, as a window's close does.
fn close_pages(app: &AppHandle<MockRuntime>, label: &str) {
    if let Some(win) = app.get_window(label) {
        for wv in win.webviews() {
            if wv.label() != label {
                let _ = wv.close();
            }
        }
    }
}

impl LectorRuntime for MockRuntime {
    type Windows = MockSet;

    /// A window built before and closed since is still in tauri's manager (module doc): reuse it.
    fn build_window(
        app: &AppHandle<Self>,
        label: &str,
        builder: WebviewWindowBuilder<'_, Self, AppHandle<Self>>,
    ) -> tauri::Result<WebviewWindow<Self>> {
        match app.get_webview_window(label) {
            Some(w) => Ok(w),
            None => builder.build(),
        }
    }

    fn live_window(app: &AppHandle<Self>, label: &str) -> Option<Window<Self>> {
        let detached = app
            .state::<AppState>()
            .detached
            .lock()
            .unwrap()
            .contains_key(label);
        (detached || app.state::<MockSet>().is_live(label))
            .then(|| app.get_window(label))
            .flatten()
    }

    fn decorate_content(_webview: &Webview<Self>, _accent: (f64, f64, f64, f64)) {}

    fn refresh_menu(app: &AppHandle<Self>) {
        app.state::<MockSet>().menus.fetch_add(1, Ordering::Relaxed);
    }
}

/// A lector app on the mock runtime with nothing loaded yet: a first run, before any config.
pub(crate) fn mock_app() -> tauri::App<MockRuntime> {
    tauri::test::mock_builder()
        .manage(AppState::new())
        .manage(MockSet::default())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("mock app")
}

/// A doc repo compositor can serve, at a fresh path under the temp dir named by `name`.
pub(crate) fn doc_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lector-mock-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    std::fs::write(dir.join("docs/index.md"), "# X\n").unwrap();
    dir
}

/// A window of a test config: its title and its tabs as `(dir, load_on_open)`.
pub(crate) type WindowSpec<'a> = (&'a str, Vec<(&'a Path, bool)>);

/// The config holding `windows`, in order.
pub(crate) fn config(windows: &[WindowSpec<'_>]) -> lector_config::Config {
    lector_config::parse_and_validate(&config_toml(windows))
        .expect("a valid test config")
        .0
}

pub(crate) fn config_toml(windows: &[WindowSpec<'_>]) -> String {
    let mut src = String::new();
    for (title, tabs) in windows {
        src.push_str(&format!("[[window]]\ntitle = {title:?}\n"));
        for (i, (dir, eager)) in tabs.iter().enumerate() {
            src.push_str(&format!(
                "[[window.tab]]\ntitle = \"T{i}\"\ndir = {:?}\nload_on_open = {eager}\n",
                dir.display().to_string()
            ));
        }
    }
    src
}

/// The label of the tab on `dir` in the window labelled `window`.
pub(crate) fn tab_label(state: &AppState, window: &str, dir: &Path) -> String {
    let dir = dir.display().to_string();
    state
        .views_for_window(window)
        .into_iter()
        .find(|v| v.dir == dir)
        .unwrap_or_else(|| panic!("{window} has a tab on {dir}"))
        .label
}

/// `commands::pop_out_tab`'s bookkeeping for the tab `label` of the window titled `title`,
/// without the detached window itself (`shell_core::detach::open_detached` builds a window
/// the mock could never destroy) or the neighbour it promotes; its detached label.
pub(crate) fn pop_out(app: &AppHandle<MockRuntime>, title: &str, label: &str) -> String {
    let state = app.state::<AppState>();
    let view = state.view(label).expect("a resolved tab");
    let port = state
        .servers
        .start(label, &lector_config::expand_tilde(&view.dir))
        .expect("its server starts");
    state.clear_active_if(label);
    crate::webviews::close(app, label);
    let detached = shell_core::detach::detached_label(&crate::detach_window_token(label));
    state.detached.lock().unwrap().insert(
        detached.clone(),
        LectorDetached {
            origin_wid: shell_core::window_of(label).unwrap().to_string(),
            origin_title: title.trim().to_string(),
            tab_label: label.to_string(),
            port,
        },
    );
    crate::sync_detached(app);
    detached
}
