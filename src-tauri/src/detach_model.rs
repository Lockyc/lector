//! A state-machine model of popped-out tabs across window adds, deletes, renames, closes, reopens,
//! reloads and redocks. Each step is right on its own; these runs look for an order that leaves a
//! popped-out tab pointing at the wrong window, shown twice, or holding another window's tab.
//!
//! The system under test is lector itself on the mock runtime (`mock_app.rs`): each step is the
//! `lib.rs` path the app runs for it. The window form's Edit and Delete land in the window set
//! at once and in the app only when the config watcher echoes the file it wrote, so they are
//! separate steps from that echo ([`Step::Reload`]), and a form can target a window the user
//! closed while it was open.

use crate::commands::AppState;
use crate::mock_app::{config, doc_dir, mock_app, pop_out, MockSet, WindowSpec};
use crate::runtime::Windows as _;
use proptest::prelude::*;
use proptest::test_runner::Config;
use proptest_state_machine::{prop_state_machine, ReferenceStateMachine, StateMachineTest};
use shell_core::menu::WindowEntry;
use shell_core::windows::plan::{
    assign, child_label, entry_for_title, menu_entries, within_of, WindowDecl,
};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::LazyLock;
use tauri::test::MockRuntime;
use tauri::{AppHandle, Manager};

/// Titles that collide once sanitized ("work"/"work!", "a b"/"a-b"), so freed and suffixed labels
/// get reused.
const TITLES: &[&str] = &["work", "work!", "job", "a b", "a-b", "docs:x"];

/// The doc repos every window's two tabs serve: one eager, one lazy. Shared, so two windows' tabs
/// differ only by the window label they sit under.
struct Repos {
    eager: PathBuf,
    lazy: PathBuf,
    /// Each tab's label within its window, eager first.
    within: [String; 2],
}

static REPOS: LazyLock<Repos> = LazyLock::new(|| {
    let (eager, lazy) = (doc_dir("model-eager"), doc_dir("model-lazy"));
    let cfg = config(&[("w", vec![(&eager, true), (&lazy, false)])]);
    let views = lector_config::WindowTabs::tab_views(&cfg.windows[0], "w");
    let within = [0, 1].map(|i| within_of(&views[i].label).to_string());
    Repos {
        eager,
        lazy,
        within,
    }
});

impl Repos {
    fn spec<'a>(&'a self, title: &'a str) -> WindowSpec<'a> {
        (title, vec![(&self.eager, true), (&self.lazy, false)])
    }

    /// A tab's label within its window: the eager one, or the lazy one.
    fn within(&self, eager: bool) -> String {
        self.within[usize::from(!eager)].clone()
    }
}

fn config_of(titles: &[String]) -> lector_config::Config {
    let specs: Vec<WindowSpec<'_>> = titles.iter().map(|t| REPOS.spec(t)).collect();
    config(&specs)
}

/// The configured windows, in config order, and the label each open one holds: shell-core's plan.
#[derive(Debug, Clone, Default)]
struct Plan {
    titles: Vec<String>,
    live: BTreeMap<String, String>,
}

impl Plan {
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

    fn label(&self, title: &str) -> String {
        entry_for_title(&self.entries(), title)
            .expect("a configured window")
            .id
            .clone()
    }

    fn is_open(&self, title: &str) -> bool {
        self.live.contains_key(title)
    }

    fn open(&mut self, title: &str) {
        let label = self.label(title);
        self.live.insert(title.to_string(), label);
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

    fn remove(&mut self, title: &str) {
        self.titles.retain(|t| t != title);
        self.live.remove(title);
    }
}

#[derive(Debug, Clone)]
enum Step {
    /// The config gains a window at a position, so it can take a closed window's label; the
    /// watcher reloads.
    Add(String, usize),
    /// The config loses a window, open or closed; the watcher reloads.
    Delete(String),
    /// A reload of the config as it stands: the watcher's echo of a form's write, or ⟳.
    Reload,
    /// The window form renames a window: config write, then `WindowSet::rename`.
    FormRename {
        old: String,
        new: String,
    },
    /// The window form deletes a window: config write, then `WindowSet::delete`.
    FormDelete(String),
    Close(String),
    /// Window ▸ the window, closed.
    Reopen(String),
    /// Pop the window's eager (or lazy) tab out.
    PopOut(String, bool),
    /// Close the popped-out window with this detached label.
    Redock(String),
}

/// A popped-out tab as the spec sees it: the title of the window it came from, and its label
/// within that window.
#[derive(Debug, Clone)]
struct Popped {
    origin_title: String,
    within: String,
}

#[derive(Debug, Clone, Default)]
struct Model {
    plan: Plan,
    popped: BTreeMap<String, Popped>,
}

impl Model {
    /// A popped-out tab whose origin left the config ends.
    fn settle(&mut self) {
        let titles = &self.plan.titles;
        self.popped.retain(|_, p| titles.contains(&p.origin_title));
    }

    /// The detached label popping `title`'s tab out would give.
    fn detached_label(&self, title: &str, eager: bool) -> String {
        let tab = child_label(&self.plan.live[title], &REPOS.within(eager));
        shell_core::detach::detached_label(&crate::detach_window_token(&tab))
    }
}

impl ReferenceStateMachine for Model {
    type State = Model;
    type Transition = Step;

    fn init_state() -> BoxedStrategy<Model> {
        Just(Model::default()).boxed()
    }

    fn transitions(state: &Model) -> BoxedStrategy<Step> {
        let titles = state.plan.titles.clone();
        let open: Vec<String> = titles
            .iter()
            .filter(|t| state.plan.is_open(t))
            .cloned()
            .collect();
        let closed: Vec<String> = titles
            .iter()
            .filter(|t| !state.plan.is_open(t))
            .cloned()
            .collect();
        let free: Vec<String> = TITLES
            .iter()
            .map(|t| t.to_string())
            .filter(|t| !titles.contains(t))
            .collect();
        let popped: Vec<String> = state.popped.keys().cloned().collect();

        let mut steps: Vec<BoxedStrategy<Step>> = vec![Just(Step::Reload).boxed()];
        if !free.is_empty() {
            steps.push(
                (proptest::sample::select(free.clone()), 0..=titles.len())
                    .prop_map(|(t, at)| Step::Add(t, at))
                    .boxed(),
            );
        }
        if !titles.is_empty() {
            let any = proptest::sample::select(titles);
            steps.push(any.clone().prop_map(Step::Delete).boxed());
            steps.push(any.clone().prop_map(Step::FormDelete).boxed());
            if !free.is_empty() {
                steps.push(
                    (any, proptest::sample::select(free))
                        .prop_map(|(old, new)| Step::FormRename { old, new })
                        .boxed(),
                );
            }
        }
        if !open.is_empty() {
            let open = proptest::sample::select(open);
            steps.push(open.clone().prop_map(Step::Close).boxed());
            steps.push(
                (open, any::<bool>())
                    .prop_map(|(t, eager)| Step::PopOut(t, eager))
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
        let p = &state.plan;
        let configured = |t: &String| p.titles.contains(t);
        match step {
            Step::Add(t, _) => !configured(t),
            Step::Delete(t) | Step::FormDelete(t) => configured(t),
            Step::Reload => true,
            Step::FormRename { old, new } => configured(old) && !configured(new),
            // The user closing the last window, with nothing popped out, quits.
            Step::Close(t) => p.is_open(t) && (p.live.len() > 1 || !state.popped.is_empty()),
            Step::Reopen(t) => configured(t) && !p.is_open(t),
            Step::PopOut(t, eager) => {
                p.is_open(t) && {
                    let within = REPOS.within(*eager);
                    !state
                        .popped
                        .values()
                        .any(|q| q.origin_title == *t && q.within == within)
                        && !state.popped.contains_key(&state.detached_label(t, *eager))
                }
            }
            Step::Redock(d) => state.popped.contains_key(d),
        }
    }

    fn apply(mut state: Model, step: &Step) -> Model {
        match step {
            Step::Add(t, at) => {
                let at = (*at).min(state.plan.titles.len());
                state.plan.titles.insert(at, t.clone());
                state.plan.open(t);
                state.settle();
            }
            // A window leaving the config takes its popped-out tabs with it, however it left.
            Step::Delete(t) | Step::FormDelete(t) => {
                state.plan.remove(t);
                state.settle();
            }
            Step::Reload => state.settle(),
            Step::FormRename { old, new } => {
                state.plan.rename(old, new);
                for p in state.popped.values_mut() {
                    if p.origin_title == *old {
                        p.origin_title = new.clone();
                    }
                }
            }
            Step::Close(t) => {
                state.plan.live.remove(t);
            }
            Step::Reopen(t) => state.plan.open(t),
            Step::PopOut(t, eager) => {
                let detached = state.detached_label(t, *eager);
                state.popped.insert(
                    detached,
                    Popped {
                        origin_title: t.clone(),
                        within: REPOS.within(*eager),
                    },
                );
            }
            // Redocking reopens a closed origin that is still configured.
            Step::Redock(d) => {
                let p = state.popped.remove(d).expect("precondition");
                if state.plan.titles.contains(&p.origin_title)
                    && !state.plan.is_open(&p.origin_title)
                {
                    state.plan.open(&p.origin_title);
                }
            }
        }
        state
    }
}

/// lector on the mock runtime, and the config file it reloads from.
struct Lector {
    app: tauri::App<MockRuntime>,
    file: Vec<String>,
}

impl Lector {
    fn app(&self) -> &AppHandle<MockRuntime> {
        self.app.handle()
    }

    fn set(&self) -> tauri::State<'_, MockSet> {
        self.app.state::<MockSet>()
    }

    fn state(&self) -> tauri::State<'_, AppState> {
        self.app.state::<AppState>()
    }

    /// The label the window set gives the window titled `title`.
    fn label(&self, title: &str) -> String {
        entry_for_title(&self.set().entries(), title)
            .expect("a configured window")
            .id
            .clone()
    }

    /// The config watcher's reload of the file.
    fn reload(&self) {
        crate::apply_clean_load(self.app(), &config_of(&self.file), &[]);
    }
}

impl Drop for Lector {
    fn drop(&mut self) {
        self.state().servers.shutdown_all();
    }
}

impl StateMachineTest for Lector {
    type SystemUnderTest = Lector;
    type Reference = Model;

    fn init_test(_: &Model) -> Lector {
        Lector {
            app: mock_app(),
            file: Vec::new(),
        }
    }

    fn apply(mut sut: Lector, _: &Model, step: Step) -> Lector {
        let app = sut.app().clone();
        match step {
            Step::Add(t, at) => {
                let at = at.min(sut.file.len());
                sut.file.insert(at, t);
                sut.reload();
            }
            Step::Delete(t) => {
                sut.file.retain(|f| *f != t);
                sut.reload();
            }
            Step::Reload => sut.reload(),
            Step::FormRename { old, new } => {
                let label = sut.label(&old);
                let at = sut
                    .file
                    .iter()
                    .position(|f| *f == old)
                    .expect("in the file");
                sut.file[at] = new.clone();
                sut.set().rename(&app, &label, &new);
                sut.set().form_changed(&app);
            }
            Step::FormDelete(t) => {
                let label = sut.label(&t);
                sut.file.retain(|f| *f != t);
                sut.set().delete(&app, &label);
                sut.set().form_changed(&app);
            }
            Step::Close(t) => {
                let label = sut.label(&t);
                sut.set().user_close(&app, &label);
            }
            Step::Reopen(t) => crate::open_or_focus_window(&app, &sut.label(&t)),
            Step::PopOut(t, eager) => {
                let tab = child_label(&sut.label(&t), &REPOS.within(eager));
                pop_out(&app, &t, &tab);
            }
            Step::Redock(d) => crate::redock(&app, &d),
        }
        sut
    }

    fn check_invariants(sut: &Lector, model: &Model) {
        let state = sut.state();
        let entries = sut.set().entries();
        assert_eq!(sut.set().live(), model.plan.live, "the open windows");
        let detached = state.detached.lock().unwrap();
        assert_eq!(
            detached.keys().collect::<std::collections::BTreeSet<_>>(),
            model.popped.keys().collect(),
            "the popped-out tabs"
        );
        let mut tabs = HashSet::new();
        for (label, d) in detached.iter() {
            let spec = &model.popped[label];
            assert_eq!(d.origin_title, spec.origin_title, "{label}'s origin title");
            assert_eq!(
                state.servers.port(&d.tab_label),
                Some(d.port),
                "{label}'s server is keyed by its tab label {:?}",
                d.tab_label
            );
            assert!(
                tabs.insert(d.tab_label.clone()),
                "{label} shares a tab label"
            );
            let home = entry_for_title(&entries, &d.origin_title).expect("a configured origin");
            if home.open {
                assert_eq!(
                    d.origin_wid, home.id,
                    "{label} names its open origin's label"
                );
                assert_eq!(
                    shell_core::window_of(&d.tab_label),
                    Some(home.id.as_str()),
                    "{label}'s tab label sits under its open origin"
                );
            }
            // What the user sees, whatever the bookkeeping says: the origin's copy of this tab
            // is neither shown nor served by a second server.
            let docked = child_label(&home.id, &spec.within);
            if home.open {
                assert_ne!(
                    state.active_for(&home.id),
                    Some(docked.clone()),
                    "{label} is also shown in its origin"
                );
            }
            assert!(
                state.servers.port(&docked).is_none_or(|p| p == d.port),
                "{label} has a second server under {docked}"
            );
        }
        // No window shows one of its own tabs as popped out unless it is.
        let out: HashSet<String> = detached.values().map(|d| d.tab_label.clone()).collect();
        for e in entries.iter().filter(|e| e.open) {
            for eager in [true, false] {
                let within = REPOS.within(eager);
                let tab = child_label(&e.id, &within);
                let is_out = model
                    .popped
                    .values()
                    .any(|p| p.origin_title == e.title && p.within == within);
                assert_eq!(
                    out.contains(&tab),
                    is_out,
                    "{tab} of {:?} reads as popped out",
                    e.title
                );
            }
        }
    }
}

/// `PROPTEST_CASES` raises the count (`just soak`).
fn cases() -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(32)
}

prop_state_machine! {
    #![proptest_config(Config { cases: cases(), ..Config::default() })]

    #[test]
    fn popped_out_tabs_follow_their_origin_window(sequential 1..40 => Lector);
}

/// The plan's labels are what the model's windows get, so a freed label really is reused.
#[test]
fn the_plan_reuses_a_freed_label() {
    let mut p = Plan::default();
    p.titles.push("work".into());
    p.open("work");
    assert_eq!(p.live["work"], "work");
    p.rename("work", "job");
    p.live.remove("job");
    p.titles.push("work!".into());
    p.open("work!");
    assert_eq!(p.live["work!"], "work");
    p.open("job");
    assert_eq!(p.live["job"], "job");
}
