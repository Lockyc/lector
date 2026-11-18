//! A state-machine model of popped-out tabs across window renames, closes, reopens, deletes and
//! redocks. Each step is right on its own; these runs look for an order that leaves
//! [`AppState::detached`] pointing at the wrong window or its server under a stale key.
//!
//! The window set is a [`Windows`] fixture built on shell-core's own label plan
//! ([`assign`], [`menu_entries`]), standing in for the `WindowSet`. Each SUT step calls the
//! `AppState` methods the matching `lib.rs` path calls, in the same order.

use crate::commands::{AppState, LectorDetached};
use proptest::prelude::*;
use proptest::test_runner::Config;
use proptest_state_machine::{prop_state_machine, ReferenceStateMachine, StateMachineTest};
use shell_core::menu::WindowEntry;
use shell_core::windows::plan::{assign, entry_for_title, menu_entries, WindowDecl};
use std::collections::{BTreeMap, HashSet};

/// Titles that collide once sanitized ("work"/"work!", "a b"/"a-b"), so freed and suffixed labels
/// get reused.
const TITLES: &[&str] = &["work", "work!", "job", "a b", "a-b", "docs:x"];

/// The configured windows, in config order, and the label each open one holds.
#[derive(Debug, Clone, Default)]
struct Windows {
    titles: Vec<String>,
    live: BTreeMap<String, String>,
}

impl Windows {
    fn decls(&self) -> Vec<WindowDecl> {
        self.titles
            .iter()
            .map(|t| WindowDecl {
                title: t.clone(),
                colour: None,
                width: 800.0,
                height: 600.0,
                open_on_start: true,
            })
            .collect()
    }

    fn entries(&self) -> Vec<WindowEntry> {
        menu_entries(&assign(&self.decls(), &self.live))
    }

    fn is_open(&self, title: &str) -> bool {
        self.live.contains_key(title)
    }

    /// Open the window titled `title` under the label the plan gives it; that label.
    fn open(&mut self, title: &str) -> String {
        let label = entry_for_title(&self.entries(), title)
            .expect("a configured window")
            .id
            .clone();
        self.live.insert(title.to_string(), label.clone());
        label
    }

    /// Insert a window at config position `at` (clamped), opening it.
    fn add(&mut self, title: &str, at: usize) -> String {
        self.titles
            .insert(at.min(self.titles.len()), title.to_string());
        self.open(title)
    }

    fn rename(&mut self, old: &str, new: &str) {
        let at = self
            .titles
            .iter()
            .position(|t| t == old)
            .expect("configured");
        self.titles[at] = new.to_string();
        if let Some(label) = self.live.remove(old) {
            self.live.insert(new.to_string(), label);
        }
    }

    fn close(&mut self, title: &str) {
        self.live.remove(title);
    }

    fn delete(&mut self, title: &str) {
        self.titles.retain(|t| t != title);
        self.live.remove(title);
    }
}

#[derive(Debug, Clone)]
enum Step {
    /// A window added to the config at a position, so it can take a closed window's label.
    Add(String, usize),
    /// An in-place rename of an open window (the Edit Window… form).
    Rename {
        old: String,
        new: String,
    },
    Close(String),
    Reopen(String),
    /// The window leaves the config, open or closed.
    Delete(String),
    PopOut(String),
    /// Close the popped-out window with this detached label.
    Redock(String),
}

/// A popped-out tab as the spec sees it: the title of the window it came from, and its port.
#[derive(Debug, Clone)]
struct Popped {
    origin_title: String,
    port: u16,
}

#[derive(Debug, Clone, Default)]
struct Model {
    windows: Windows,
    popped: BTreeMap<String, Popped>,
    next_port: u16,
}

/// A pop-out's tab label and detached label. Every pop-out is a distinct tab (its port numbers
/// it), so two tabs never share a label by construction.
fn pop_labels(window_label: &str, port: u16) -> (String, String) {
    let tab = shell_core::child_label(window_label, &format!("tab-{port}"));
    let detached = shell_core::detach::detached_label(&crate::detach_window_token(&tab));
    (tab, detached)
}

impl ReferenceStateMachine for Model {
    type State = Model;
    type Transition = Step;

    fn init_state() -> BoxedStrategy<Model> {
        Just(Model {
            next_port: 1,
            ..Model::default()
        })
        .boxed()
    }

    fn transitions(state: &Model) -> BoxedStrategy<Step> {
        let titles = state.windows.titles.clone();
        let open: Vec<String> = titles
            .iter()
            .filter(|t| state.windows.is_open(t))
            .cloned()
            .collect();
        let closed: Vec<String> = titles
            .iter()
            .filter(|t| !state.windows.is_open(t))
            .cloned()
            .collect();
        let free: Vec<String> = TITLES
            .iter()
            .map(|t| t.to_string())
            .filter(|t| !titles.contains(t))
            .collect();
        let popped: Vec<String> = state.popped.keys().cloned().collect();

        let mut steps: Vec<BoxedStrategy<Step>> = Vec::new();
        if !free.is_empty() {
            steps.push(
                (proptest::sample::select(free.clone()), 0..=titles.len())
                    .prop_map(|(t, at)| Step::Add(t, at))
                    .boxed(),
            );
        }
        if !open.is_empty() {
            if !free.is_empty() {
                steps.push(
                    (
                        proptest::sample::select(open.clone()),
                        proptest::sample::select(free),
                    )
                        .prop_map(|(old, new)| Step::Rename { old, new })
                        .boxed(),
                );
            }
            steps.push(
                proptest::sample::select(open.clone())
                    .prop_map(Step::Close)
                    .boxed(),
            );
            steps.push(
                proptest::sample::select(open)
                    .prop_map(Step::PopOut)
                    .boxed(),
            );
        }
        if !closed.is_empty() {
            steps.push(
                proptest::sample::select(closed)
                    .prop_map(Step::Reopen)
                    .boxed(),
            );
        }
        if !titles.is_empty() {
            steps.push(
                proptest::sample::select(titles)
                    .prop_map(Step::Delete)
                    .boxed(),
            );
        }
        if !popped.is_empty() {
            steps.push(
                proptest::sample::select(popped)
                    .prop_map(Step::Redock)
                    .boxed(),
            );
        }
        proptest::strategy::Union::new(steps).boxed()
    }

    fn preconditions(state: &Model, step: &Step) -> bool {
        let w = &state.windows;
        let configured = |t: &String| w.titles.contains(t);
        match step {
            Step::Add(t, _) => !configured(t),
            Step::Rename { old, new } => w.is_open(old) && !configured(new),
            Step::Close(t) | Step::PopOut(t) => w.is_open(t),
            Step::Reopen(t) => configured(t) && !w.is_open(t),
            Step::Delete(t) => configured(t),
            Step::Redock(d) => state.popped.contains_key(d),
        }
    }

    fn apply(mut state: Model, step: &Step) -> Model {
        match step {
            Step::Add(t, at) => {
                state.windows.add(t, *at);
            }
            Step::Rename { old, new } => {
                state.windows.rename(old, new);
                for p in state.popped.values_mut() {
                    if p.origin_title == *old {
                        p.origin_title = new.clone();
                    }
                }
            }
            Step::Close(t) => state.windows.close(t),
            Step::Reopen(t) => {
                state.windows.open(t);
            }
            // A window leaving the config takes its popped-out tabs with it.
            Step::Delete(t) => {
                state.windows.delete(t);
                state.popped.retain(|_, p| p.origin_title != *t);
            }
            Step::PopOut(t) => {
                let port = state.next_port;
                state.next_port += 1;
                let (_, detached) = pop_labels(&state.windows.live[t], port);
                state.popped.insert(
                    detached,
                    Popped {
                        origin_title: t.clone(),
                        port,
                    },
                );
            }
            // Redocking reopens a closed origin.
            Step::Redock(d) => {
                let p = state.popped.remove(d).expect("precondition");
                if !state.windows.is_open(&p.origin_title) {
                    state.windows.open(&p.origin_title);
                }
            }
        }
        state
    }
}

/// lector's state and the window set it runs against.
struct Lector {
    state: AppState,
    windows: Windows,
}

impl Lector {
    /// `open_window`'s bookkeeping for a window built under `label`.
    fn built(&self, label: &str, title: &str) {
        self.state.popped_out_of(label, title);
    }

    /// `window_closed`: a window was destroyed.
    fn closed(&self) {
        self.state.settle_detached(&self.windows.entries());
    }

    /// `apply_clean_load` after the reload that dropped a window, before that window's
    /// `Destroyed` (a closed window has none).
    fn reloaded(&self) {
        self.state.settle_detached(&self.windows.entries());
    }

    /// `pop_out_tab`'s bookkeeping.
    fn pop_out(&self, title: &str, port: u16) {
        let wid = self.windows.live[title].clone();
        let (tab, detached) = pop_labels(&wid, port);
        self.state.servers.register_unserved_for_test(&tab, port);
        self.state.detached.lock().unwrap().insert(
            detached,
            LectorDetached {
                origin_wid: wid,
                origin_title: title.trim().to_string(),
                tab_label: tab,
                port,
            },
        );
    }

    /// `redock`'s bookkeeping, in its order.
    fn redock(&mut self, detached: &str) {
        let home = self.state.resolve_origin(detached, &self.windows.entries());
        let Some((origin_wid, tab)) = self
            .state
            .detached
            .lock()
            .unwrap()
            .get(detached)
            .map(|d| (d.origin_wid.clone(), d.tab_label.clone()))
        else {
            return;
        };
        if matches!(home, Some((_, false))) {
            let title = self
                .windows
                .entries()
                .into_iter()
                .find(|e| e.id == origin_wid)
                .expect("the resolved origin is listed")
                .title;
            let label = self.windows.open(&title);
            assert_eq!(label, origin_wid, "a reopen keeps the resolved label");
            self.built(&label, &title);
        }
        self.state.detached.lock().unwrap().remove(detached);
        if home.is_none() {
            self.state.servers.stop(&tab);
        }
    }
}

impl StateMachineTest for Lector {
    type SystemUnderTest = Lector;
    type Reference = Model;

    fn init_test(_: &Model) -> Lector {
        Lector {
            state: AppState::new(),
            windows: Windows::default(),
        }
    }

    fn apply(mut sut: Lector, model: &Model, step: Step) -> Lector {
        match step {
            // The reload's `WindowSet::apply` builds the window before `apply_clean_load` settles.
            Step::Add(t, at) => {
                let label = sut.windows.add(&t, at);
                sut.built(&label, &t);
                sut.reloaded();
            }
            // `window_retitled`, then the reload echo of the written config.
            Step::Rename { old, new } => {
                sut.windows.rename(&old, &new);
                sut.state.retitle_origins(&old, &new);
                sut.reloaded();
            }
            Step::Close(t) => {
                sut.windows.close(&t);
                sut.closed();
            }
            Step::Reopen(t) => {
                let label = sut.windows.open(&t);
                sut.built(&label, &t);
            }
            Step::Delete(t) => {
                let was_open = sut.windows.is_open(&t);
                sut.windows.delete(&t);
                sut.reloaded();
                if was_open {
                    sut.closed();
                }
            }
            Step::PopOut(t) => sut.pop_out(&t, model.next_port - 1),
            Step::Redock(d) => sut.redock(&d),
        }
        sut
    }

    fn check_invariants(sut: &Lector, model: &Model) {
        let detached = sut.state.detached.lock().unwrap();
        assert_eq!(
            detached.keys().collect::<std::collections::BTreeSet<_>>(),
            model.popped.keys().collect(),
            "the popped-out tabs"
        );
        let mut tabs = HashSet::new();
        for (label, d) in detached.iter() {
            let spec = &model.popped[label];
            assert_eq!(d.origin_title, spec.origin_title, "{label}'s origin title");
            assert!(
                sut.windows.titles.contains(&d.origin_title),
                "{label}'s origin {:?} left the config but the tab stayed out",
                d.origin_title
            );
            assert_eq!(d.port, spec.port);
            assert_eq!(
                sut.state.servers.port(&d.tab_label),
                Some(d.port),
                "{label}'s server is keyed by its tab label {:?}",
                d.tab_label
            );
            assert!(
                tabs.insert(d.tab_label.clone()),
                "{label} shares a tab label"
            );
            if let Some(open) = sut.windows.live.get(&d.origin_title) {
                assert_eq!(&d.origin_wid, open, "{label} names its open origin's label");
                assert_eq!(
                    shell_core::window_of(&d.tab_label),
                    Some(open.as_str()),
                    "{label}'s tab label sits under its open origin"
                );
            }
        }
        assert_eq!(
            sut.windows.live, model.windows.live,
            "the fixture tracks the model"
        );
    }
}

/// `PROPTEST_CASES` raises the count (`just soak`).
fn cases() -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(64)
}

prop_state_machine! {
    #![proptest_config(Config { cases: cases(), ..Config::default() })]

    #[test]
    fn popped_out_tabs_follow_their_origin_window(sequential 1..40 => Lector);
}

/// The plan's labels are what the fixture's windows get, so a freed label really is reused.
#[test]
fn the_fixture_reuses_a_freed_label() {
    let mut w = Windows::default();
    assert_eq!(w.add("work", 0), "work");
    w.rename("work", "job");
    w.close("job");
    assert_eq!(w.add("work!", 1), "work");
    assert_eq!(w.open("job"), "job");
}

/// The body of the top-level `fn name` in `lib.rs`.
fn lib_fn_body(name: &str) -> &'static str {
    const LIB_RS: &str = include_str!("lib.rs");
    let start = LIB_RS
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("lib.rs defines {name}"));
    let body = &LIB_RS[start..];
    &body[..body.find("\n}\n").expect("the function closes")]
}

/// The model's `reloaded` and `closed` stand in for `lib.rs` paths it can't drive without a running
/// app, so this holds those paths to the steps the model gives them.
#[test]
fn the_lib_rs_paths_settle_where_the_model_does() {
    let load = lib_fn_body("apply_clean_load");
    let applied = load
        .find(".apply(app")
        .expect("apply_clean_load applies the window set");
    let settled = load
        .find("settle_detached(app)")
        .expect("apply_clean_load settles the popped-out tabs: a closed origin has no Destroyed");
    assert!(
        applied < settled,
        "apply_clean_load settles against the entries from before WindowSet::apply"
    );
    assert!(
        lib_fn_body("window_closed").contains("settle_detached(app)"),
        "window_closed settles the popped-out tabs"
    );
}
