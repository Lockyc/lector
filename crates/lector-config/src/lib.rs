//! lector-config: parse, validate, and resolve lector's TOML config (windows + doc-repo tabs).
//!
//! The house-style TOML formatter + colour parsing are shared with warden and curator via the
//! config-core crate, re-exported here so the app (`src-tauri`) uses
//! `lector_config::{Colour, format_file, format_str}`.
pub use config_core::{
    discover_projects, fmt_cli, format_file, format_str, write_default_config, Colour, ColourError,
    DiscoveredProject, RootDir, SeedError,
};

pub mod hash;
pub mod identity;

use serde::{Deserialize, Serialize};

/// What to open when a window launches. The default (`false` / unset) opens the first
/// `load_on_open` (loaded) tab, else the blank background — the first tab isn't always loaded, so
/// it isn't forced. `true` opens the first tab even if it isn't loaded; a string opens the tab
/// whose `title` matches (falling back to the first).
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum OpenOnLaunch {
    Toggle(bool),
    Tab(String),
}
impl Default for OpenOnLaunch {
    fn default() -> Self {
        OpenOnLaunch::Toggle(false)
    }
}

/// Chrome sizing mode (whole-app). `Comfortable` (default) is the standard sizing; `Compact`
/// proportionally condenses the chrome's type + spacing for denser tab lists. The chrome maps
/// this to a `data-density` attribute → CSS variables; it serializes to the lowercase token the
/// chrome reads. An unrecognised value is a parse error (same as any bad enum here).
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    #[default]
    Comfortable,
    Compact,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Force dark appearance app-wide (applied per window at setup). Omit/false = follow system.
    /// Themes the chrome sidebar only — compositor's page shell has its own light/dark toggle that
    /// persists in-page, and the two can disagree. Accepted for v1 (see CLAUDE.md › Known warts).
    #[serde(default)]
    pub dark_mode: bool,
    /// Reformat the config file in place (house style) on a clean hot-reload. Default false.
    /// The rewrite is diff-guarded, so an already-formatted file is a no-op and the writer can't
    /// loop its own watcher. Also available on demand via `lector fmt`.
    #[serde(default)]
    pub format_on_save: bool,
    /// Chrome sizing mode (whole-app). Default comfortable; `compact` proportionally condenses
    /// the chrome. See [`Density`].
    #[serde(default)]
    pub density: Density,
    /// Whether the sidebar chrome acts as a window-move drag handle (whole-app). Default true;
    /// `false` turns it off. The chrome maps this to the component's `windowDrag` flag.
    #[serde(default = "default_true")]
    pub sidebar_drag: bool,
    /// Whether lector checks for a new release on launch (whole-app). Default true; `false`
    /// suppresses the automatic launch check. The manual **Check for Updates…** menu item stays
    /// available regardless. The chrome gates its launch check on this.
    #[serde(default = "default_true")]
    pub auto_update: bool,
    #[serde(default, rename = "window")]
    pub windows: Vec<WindowConfig>,
}

// Hand-written (not derived) so `sidebar_drag`/`auto_update` default to `true`, matching serde's
// `default_true` parse default — a derived `bool` default would be `false` and silently disagree
// with parsing an empty config. The struct literal makes this drift-proof: adding a field fails to
// compile until it's set here too.
impl Default for Config {
    fn default() -> Self {
        Config {
            dark_mode: false,
            format_on_save: false,
            density: Density::Comfortable,
            sidebar_drag: true,
            auto_update: true,
            windows: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WindowConfig {
    pub title: String,
    #[serde(default = "default_window_width")]
    pub width: u32,
    #[serde(default = "default_window_height")]
    pub height: u32,
    #[serde(default)]
    pub open_on_launch: OpenOnLaunch,
    /// Optional per-window accent colour (`#rgb` or `#rrggbb`). The chrome shows it as a
    /// name banner + a faint tint, giving each window an at-a-glance identity. Omit → neutral.
    #[serde(default)]
    pub colour: Option<String>,
    /// Loose (ungrouped) tabs. They render in a leading headerless section, before any group.
    #[serde(default, rename = "tab")]
    pub tabs: Vec<Tab>,
    #[serde(default, rename = "group")]
    pub groups: Vec<Group>,
    #[serde(default, rename = "root")]
    pub roots: Vec<RawRoot>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub name: String,
    #[serde(default, rename = "tab")]
    pub tabs: Vec<Tab>,
}

/// A project-tree root: scan `dir` (to `depth`) for git repos, each rendered as a discovered doc
/// tab. lector's leaf is empty — `shell`/`cmd`/`probe`/`kill` are warden-only and rejected by
/// `deny_unknown_fields`. `name` defaults to `basename(dir)`; `depth` to config-core's default.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RawRoot {
    pub name: Option<String>,
    pub dir: String,
    pub depth: Option<u32>,
}

/// One doc repo. The leaf is where the sibling apps diverge — curator: `url`/`session`;
/// warden: `dir`/`shell`/`probe`; lector: `dir` only.
///
/// Deliberately absent: `session` (no logins in a local render), `reload_every` (compositor's
/// watcher makes polling meaningless), and `docs_dir`/`home` (compositor already synthesizes both —
/// encoding them here would be a second decider for a question compositor already answers).
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Tab {
    pub title: String,
    /// The doc repo path — tilde-expanded and canonicalized at resolve time.
    pub dir: String,
    /// Start this repo's server at launch.
    #[serde(default)]
    pub load_on_open: bool,
}

/// True for a `#rgb` or `#rrggbb` hex colour — the forms the chrome banner accepts. Delegates to
/// the shared `config_core` parser so all three apps validate accent colours identically.
fn is_hex_colour(s: &str) -> bool {
    config_core::Colour::parse(s).is_ok()
}

fn default_true() -> bool {
    true
}
fn default_window_width() -> u32 {
    1500
}
fn default_window_height() -> u32 {
    1000
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(toml::de::Error),
    EmptyField(&'static str),
    DuplicateWindowTitle(String),
    DuplicateTabTitle { window: String, title: String },
    DuplicateGroupName { window: String, name: String },
    InvalidWindowSize { width: u32, height: u32 },
    InvalidColour { title: String, colour: String },
    EmptyRootDir { window: String },
    EmptyRootName { window: String },
    InvalidRootDepth { window: String, depth: u32 },
    DuplicateSection { window: String, name: String },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "cannot read config: {e}"),
            ConfigError::Parse(e) => write!(f, "invalid TOML: {e}"),
            ConfigError::EmptyField(field) => write!(f, "empty {field}"),
            ConfigError::DuplicateWindowTitle(t) => write!(f, "duplicate window title: {t}"),
            ConfigError::DuplicateTabTitle { window, title } => {
                write!(f, "window {window:?} has duplicate tab title: {title:?}")
            }
            ConfigError::DuplicateGroupName { window, name } => {
                write!(f, "window {window:?} has duplicate group name: {name:?}")
            }
            ConfigError::InvalidWindowSize { width, height } => {
                write!(f, "window size must be positive, got {width}×{height}")
            }
            ConfigError::InvalidColour { title, colour } => {
                write!(f, "window \"{title}\" has invalid colour: {colour}")
            }
            ConfigError::EmptyRootDir { window } => {
                write!(f, "window {window:?} has a root with an empty dir")
            }
            ConfigError::EmptyRootName { window } => {
                write!(f, "window {window:?} has a root with an empty name")
            }
            ConfigError::InvalidRootDepth { window, depth } => {
                write!(f, "window {window:?} has a root with invalid depth {depth}")
            }
            ConfigError::DuplicateSection { window, name } => {
                write!(f, "window {window:?} has duplicate section name: {name:?}")
            }
        }
    }
}

/// A non-fatal config issue surfaced to the user (logged on load, printed by `lector validate`)
/// without rejecting the config. Two producers: a `dir` repeated within a window, and a `dir` that
/// is missing or is not a directory.
#[derive(Debug, Clone, PartialEq)]
pub struct Warning {
    pub window: String,
    pub message: String,
}

/// Per-tab field validation shared by loose and grouped tabs: non-empty title + dir.
///
/// A missing/non-directory `dir` is deliberately NOT an error — see `check_tab`'s warning.
fn validate_tab(tab: &Tab) -> Result<(), ConfigError> {
    if tab.title.trim().is_empty() {
        return Err(ConfigError::EmptyField("title"));
    }
    if tab.dir.trim().is_empty() {
        return Err(ConfigError::EmptyField("dir"));
    }
    Ok(())
}

pub fn parse_and_validate(src: &str) -> Result<(Config, Vec<Warning>), ConfigError> {
    let cfg: Config = toml::from_str(src).map_err(ConfigError::Parse)?;
    let mut seen_titles = std::collections::HashSet::new();
    let mut warnings: Vec<Warning> = Vec::new();
    for w in &cfg.windows {
        if w.title.trim().is_empty() {
            return Err(ConfigError::EmptyField("title"));
        }
        if !seen_titles.insert(w.title.clone()) {
            return Err(ConfigError::DuplicateWindowTitle(w.title.clone()));
        }
        if w.width == 0 || w.height == 0 {
            return Err(ConfigError::InvalidWindowSize {
                width: w.width,
                height: w.height,
            });
        }
        if let Some(colour) = &w.colour {
            if !is_hex_colour(colour) {
                return Err(ConfigError::InvalidColour {
                    title: w.title.clone(),
                    colour: colour.clone(),
                });
            }
        }
        // Uniqueness is window-wide for tab titles (across loose + grouped) and per-window for
        // group names — both keep the labels and the menu/CLI unambiguous. A dir repeated within a
        // window is non-fatal (the labels still disambiguate) but warned once. Per-window, not
        // global: labels are namespaced `{window_id}:{dir_hash}`, so the same repo in two windows
        // is no collision — and it's a supported pattern (one repo, two windows).
        let mut tab_titles = std::collections::HashSet::new();
        let mut group_names = std::collections::HashSet::new();
        let mut seen_dirs = std::collections::HashSet::new();
        let mut warned_dirs = std::collections::HashSet::new();
        let window_title = w.title.clone();
        let mut check_tab = |tab: &Tab| -> Result<(), ConfigError> {
            validate_tab(tab)?;
            if !tab_titles.insert(tab.title.trim().to_string()) {
                return Err(ConfigError::DuplicateTabTitle {
                    window: window_title.clone(),
                    title: tab.title.clone(),
                });
            }
            if !seen_dirs.insert(tab.dir.clone()) && warned_dirs.insert(tab.dir.clone()) {
                warnings.push(Warning {
                    window: window_title.clone(),
                    message: format!("duplicate dir: {}", tab.dir),
                });
            }
            // A missing dir must NOT be an error: an un-cloned repo would otherwise fail a
            // hot-reload and strand every *other* tab on last-good config. It surfaces honestly on
            // select instead — the server start fails and the message goes to the chrome's
            // setError. That is the error channel, not a per-repo build-health feature.
            if !expand_tilde(&tab.dir).is_dir() {
                warnings.push(Warning {
                    window: window_title.clone(),
                    message: format!("dir missing or not a directory: {}", tab.dir),
                });
            }
            Ok(())
        };
        for tab in &w.tabs {
            check_tab(tab)?;
        }
        for group in &w.groups {
            if group.name.trim().is_empty() {
                return Err(ConfigError::EmptyField("name"));
            }
            if !group_names.insert(group.name.trim().to_string()) {
                return Err(ConfigError::DuplicateGroupName {
                    window: w.title.clone(),
                    name: group.name.clone(),
                });
            }
            for tab in &group.tabs {
                check_tab(tab)?;
            }
        }
        for raw in &w.roots {
            let rd = config_core::resolve_root_dir(raw.name.as_deref(), &raw.dir, raw.depth)
                .map_err(|e| match e {
                    config_core::RootError::EmptyDir => ConfigError::EmptyRootDir {
                        window: w.title.clone(),
                    },
                    config_core::RootError::EmptyName => ConfigError::EmptyRootName {
                        window: w.title.clone(),
                    },
                    config_core::RootError::ZeroDepth(d) => ConfigError::InvalidRootDepth {
                        window: w.title.clone(),
                        depth: d,
                    },
                })?;
            if !group_names.insert(rd.name.clone()) {
                return Err(ConfigError::DuplicateSection {
                    window: w.title.clone(),
                    name: rd.name,
                });
            }
            if !rd.dir.is_dir() {
                warnings.push(Warning {
                    window: w.title.clone(),
                    message: format!("root dir missing or not a directory: {}", rd.dir.display()),
                });
            }
        }
    }
    Ok((cfg, warnings))
}

/// Expand a leading `~` to the home dir. Not canonicalization — a missing dir must still produce a
/// path (it warns, it doesn't error), so this never touches the filesystem for resolution.
pub fn expand_tilde(dir: &str) -> std::path::PathBuf {
    let trimmed = dir.trim();
    match trimmed.strip_prefix("~/") {
        Some(rest) => dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(rest),
        None if trimmed == "~" => dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from(".")),
        None => std::path::PathBuf::from(trimmed),
    }
}

pub fn load_config(path: &std::path::Path) -> Result<(Config, Vec<Warning>), ConfigError> {
    let src = std::fs::read_to_string(path).map_err(ConfigError::Io)?;
    parse_and_validate(&src)
}

/// This app's env override for the config path.
const CONFIG_ENV: &str = "LECTOR_CONFIG";
/// This app's `~/.config` subdirectory.
const CONFIG_DIR: &str = "lector";

/// Config path to load at launch: `$LECTOR_CONFIG` if set and non-empty, else
/// [`default_config_path`]. Shared with warden and curator via config-core — including the
/// set-but-empty fall-through, which this app previously got wrong.
///
/// The env override lets `just run` point at the repo's `examples/config.toml` so iterating on
/// lector never touches the developer's real `~/.config/lector/config.toml`.
pub fn resolve_config_path() -> std::path::PathBuf {
    config_core::resolve_config_path(CONFIG_ENV, CONFIG_DIR)
}

/// Default config location: `~/.config/lector/config.toml`.
///
/// Deliberately `~/.config` (not `dirs::config_dir()`, which on macOS is
/// `~/Library/Application Support`) so the config slots into the dotfiles bare-repo workflow.
pub fn default_config_path() -> std::path::PathBuf {
    config_core::default_config_path(CONFIG_DIR)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TabView {
    pub label: String,
    /// The group this tab renders under, or `None` for a loose (ungrouped) tab — the chrome
    /// shows a section header only for `Some(name)`. Serialized to the sidebar as `null` for loose.
    pub group: Option<String>,
    pub title: String,
    pub dir: String,
    pub load_on_open: bool,
    /// A project-tree (root)-discovered row: chrome renders these as a collapsible folder tree.
    /// `false` for every curated tab. Serialized so the DTO can forward it.
    pub tree: bool,
    /// Folder segments between the root dir and this project (empty for curated tabs) — chrome-core
    /// nests the tree by these.
    pub tree_path: Vec<String>,
}

/// Stable within-window tab label derived from a tab's **canonicalized** dir — the spec's identity
/// rule. Position-independent, so inserting/removing/reordering tabs doesn't remap an existing
/// webview, and title-independent, so retitling is free.
///
/// Canonicalization makes identity a property of the repo rather than of how the config spelled its
/// path (`~/x`, `/Users/me/x`, and `/Users/me/./x` are one repo, one server, one label). A dir that
/// doesn't resolve falls back to the tilde-expanded path: a missing repo only warns, so it must
/// still get a label — and it gets the same one once the repo is cloned at that path.
///
/// `fnv1a_64`, never `DefaultHasher` — see `hash`'s module docs.
fn dir_label(dir: &str) -> String {
    let expanded = expand_tilde(dir);
    let canonical = std::fs::canonicalize(&expanded).unwrap_or(expanded);
    format!(
        "tab-{:016x}",
        crate::hash::fnv1a_64(canonical.as_os_str().as_encoded_bytes())
    )
}

impl WindowConfig {
    /// Flatten this window's loose tabs + groups → ordered tabs with stable labels (file order:
    /// loose tabs first as a headerless section, then each group).
    pub fn tab_views(&self) -> Vec<TabView> {
        let wid = crate::identity::window_id(&self.title);
        let mut views = Vec::new();
        let mut seen: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        // Loose tabs (group `None`) first, then each group's tabs (group `Some(name)`), all in
        // file order, sharing one dir-label dedup map so duplicate dirs across the window still
        // get distinct labels.
        let entries = self.tabs.iter().map(|t| (t, Option::<String>::None)).chain(
            self.groups
                .iter()
                .flat_map(|g| g.tabs.iter().map(move |t| (t, Some(g.name.clone())))),
        );
        for (tab, group) in entries {
            let base = dir_label(&tab.dir);
            let n = seen.entry(base.clone()).or_insert(0);
            let within = if *n == 0 {
                base.clone()
            } else {
                format!("{base}-{n}")
            };
            *n += 1;
            views.push(TabView {
                label: crate::identity::namespaced(&wid, &within),
                group,
                title: tab.title.clone(),
                dir: tab.dir.clone(),
                load_on_open: tab.load_on_open,
                tree: false,
                tree_path: Vec::new(),
            });
        }
        views
    }

    /// Curated tab views, then this window's discovered projects as `tree: true` views. A
    /// discovered project whose label (canonicalized-dir identity) already appears is dropped — a
    /// curated tab at the same repo shadows it, and a repo reachable via two roots lands once
    /// (first wins).
    pub fn tab_views_with_discovered(
        &self,
        discovered: &[config_core::DiscoveredProject],
    ) -> Vec<TabView> {
        let wid = crate::identity::window_id(&self.title);
        let mut views = self.tab_views();
        let mut seen: std::collections::HashSet<String> =
            views.iter().map(|v| v.label.clone()).collect();
        for proj in discovered {
            let base = dir_label(&proj.path.to_string_lossy());
            // Compare namespaced labels, matching how `seen` was seeded from curated views (whose
            // labels already carry the `wid:` prefix) — comparing bare `base` against those would
            // never collide and defeat the dedup entirely.
            let label = crate::identity::namespaced(&wid, &base);
            if !seen.insert(label.clone()) {
                continue; // shadowed by a curated tab or an earlier root
            }
            let title = proj
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| proj.path.to_string_lossy().into_owned());
            views.push(TabView {
                label,
                group: Some(proj.section.clone()),
                title,
                dir: proj.path.to_string_lossy().into_owned(),
                load_on_open: false,
                tree: true,
                tree_path: proj.tree_path.clone(),
            });
        }
        views
    }

    /// This window's roots, validated + resolved to config-core `RootDir`s. Infallible here because
    /// `parse_and_validate` already rejected any invalid root; a stray invalid one is silently
    /// skipped rather than panicking a live reconcile.
    pub fn resolved_roots(&self) -> Vec<config_core::RootDir> {
        self.roots
            .iter()
            .filter_map(|r| config_core::resolve_root_dir(r.name.as_deref(), &r.dir, r.depth).ok())
            .collect()
    }

    /// Label of the tab to make active on launch. Default (`false`/unset): the first `load_on_open`
    /// (loaded) tab, else `None` (blank) — the first tab isn't always loaded, so it isn't forced.
    /// `true`: the first tab even if cold. A title string: the matching tab (else the first).
    pub fn startup_label(&self) -> Option<String> {
        let views = self.tab_views();
        match &self.open_on_launch {
            OpenOnLaunch::Toggle(false) => views
                .iter()
                .find(|v| v.load_on_open)
                .map(|v| v.label.clone()),
            OpenOnLaunch::Toggle(true) => views.first().map(|v| v.label.clone()),
            OpenOnLaunch::Tab(title) => views
                .iter()
                .find(|v| v.title == *title)
                .or_else(|| views.first())
                .map(|v| v.label.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
[[window]]
title = "Docs"

[[window.tab]]
title = "homelab"
dir = "~/Developer/homelab"

[[window.group]]
name = "tools"
[[window.group.tab]]
title = "compositor"
dir = "~/Developer/compositor"
load_on_open = true
"#;

    #[test]
    fn parses_a_valid_config() {
        let (cfg, warnings) = parse_and_validate(VALID).unwrap();
        assert_eq!(cfg.windows.len(), 1);
        assert_eq!(cfg.windows[0].tabs[0].dir, "~/Developer/homelab");
        assert_eq!(cfg.windows[0].groups[0].tabs[0].title, "compositor");
        assert!(cfg.windows[0].groups[0].tabs[0].load_on_open);
        // A dir that doesn't exist warns (see rejects/warns tests) but never errors.
        assert!(!warnings.is_empty());
    }

    #[test]
    fn sidebar_drag_defaults_true_and_parses_false() {
        assert!(parse_and_validate(VALID).unwrap().0.sidebar_drag);
        let cfg = parse_and_validate(&format!("sidebar_drag = false\n{VALID}"))
            .unwrap()
            .0;
        assert!(!cfg.sidebar_drag);
        // The derived-vs-serde default trap: an empty config must agree with Config::default().
        assert!(Config::default().sidebar_drag);
    }

    #[test]
    fn auto_update_defaults_true_and_parses_false() {
        assert!(parse_and_validate(VALID).unwrap().0.auto_update);
        let cfg = parse_and_validate(&format!("auto_update = false\n{VALID}"))
            .unwrap()
            .0;
        assert!(!cfg.auto_update);
        assert!(Config::default().auto_update);
    }

    #[test]
    fn rejects_empty_and_duplicate_window_titles() {
        assert!(matches!(
            parse_and_validate("[[window]]\ntitle = \"  \"\n").unwrap_err(),
            ConfigError::EmptyField("title")
        ));
        let src = "[[window]]\ntitle = \"W\"\n[[window]]\ntitle = \"W\"\n";
        assert!(matches!(
            parse_and_validate(src).unwrap_err(),
            ConfigError::DuplicateWindowTitle(_)
        ));
    }

    #[test]
    fn rejects_zero_window_dimension() {
        let src = "[[window]]\ntitle = \"W\"\nwidth = 0\n";
        assert!(matches!(
            parse_and_validate(src).unwrap_err(),
            ConfigError::InvalidWindowSize { .. }
        ));
    }

    #[test]
    fn rejects_invalid_colour() {
        let src = "[[window]]\ntitle = \"W\"\ncolour = \"octarine\"\n";
        assert!(matches!(
            parse_and_validate(src).unwrap_err(),
            ConfigError::InvalidColour { .. }
        ));
    }

    #[test]
    fn rejects_empty_tab_title_and_dir() {
        let src = "[[window]]\ntitle = \"W\"\n[[window.tab]]\ntitle = \"\"\ndir = \"/tmp\"\n";
        assert!(matches!(
            parse_and_validate(src).unwrap_err(),
            ConfigError::EmptyField("title")
        ));
        let src = "[[window]]\ntitle = \"W\"\n[[window.tab]]\ntitle = \"T\"\ndir = \"  \"\n";
        assert!(matches!(
            parse_and_validate(src).unwrap_err(),
            ConfigError::EmptyField("dir")
        ));
    }

    #[test]
    fn rejects_duplicate_tab_title_window_wide_across_loose_and_grouped() {
        // Window-wide, not per-section: a loose tab and a grouped tab may not share a title.
        let src = r#"
[[window]]
title = "W"
  [[window.tab]]
  title = "Dup"
  dir = "/tmp"
  [[window.group]]
  name = "G"
    [[window.group.tab]]
    title = "Dup"
    dir = "/usr"
"#;
        assert!(matches!(
            parse_and_validate(src).unwrap_err(),
            ConfigError::DuplicateTabTitle { .. }
        ));
    }

    #[test]
    fn rejects_empty_and_duplicate_group_names() {
        let src = "[[window]]\ntitle = \"W\"\n[[window.group]]\nname = \" \"\n";
        assert!(matches!(
            parse_and_validate(src).unwrap_err(),
            ConfigError::EmptyField("name")
        ));
        let src = "[[window]]\ntitle = \"W\"\n[[window.group]]\nname = \"G\"\n[[window.group]]\nname = \"G\"\n";
        assert!(matches!(
            parse_and_validate(src).unwrap_err(),
            ConfigError::DuplicateGroupName { .. }
        ));
    }

    #[test]
    fn rejects_unknown_keys_including_the_dropped_sibling_leaves() {
        // deny_unknown_fields must reject curator's `url`/`session`/`reload_every` and warden's
        // `shell`/`probe` loudly rather than silently ignoring them — someone adapting a sibling
        // config must be told, not have half their config quietly dropped.
        for bad in [
            "url = \"https://x.test/\"",
            "session = \"work\"",
            "reload_every = 15",
            "shell = \"fish\"",
            "probe = true",
        ] {
            let src = format!(
                "[[window]]\ntitle = \"W\"\n[[window.tab]]\ntitle = \"T\"\ndir = \"/tmp\"\n{bad}\n"
            );
            assert!(
                matches!(parse_and_validate(&src).unwrap_err(), ConfigError::Parse(_)),
                "must reject: {bad}"
            );
        }
        // App-global: the dropped `session` and `allow_insecure`.
        for bad in ["session = \"work\"", "allow_insecure = [\"x.test\"]"] {
            let src = format!("{bad}\n[[window]]\ntitle = \"W\"\n");
            assert!(
                matches!(parse_and_validate(&src).unwrap_err(), ConfigError::Parse(_)),
                "must reject: {bad}"
            );
        }
    }

    #[test]
    fn duplicate_dir_within_window_warns_once() {
        let src = r#"
[[window]]
title = "W"
  [[window.tab]]
  title = "A"
  dir = "/tmp"
  [[window.tab]]
  title = "B"
  dir = "/tmp"
  [[window.tab]]
  title = "C"
  dir = "/tmp"
"#;
        let (_cfg, warnings) = parse_and_validate(src).unwrap();
        let dups: Vec<_> = warnings
            .iter()
            .filter(|w| w.message.contains("duplicate dir"))
            .collect();
        // Three copies, one warning — the warned_dirs guard.
        assert_eq!(dups.len(), 1);
        assert_eq!(dups[0].window, "W");
    }

    #[test]
    fn missing_dir_warns_but_never_errors() {
        // The whole point: an un-cloned repo must not fail a hot-reload and strand every *other*
        // tab on last-good config. It surfaces on select instead, via the chrome's error bar.
        let src = "[[window]]\ntitle = \"W\"\n[[window.tab]]\ntitle = \"Gone\"\ndir = \"/definitely/not/here\"\n";
        let (cfg, warnings) = parse_and_validate(src).unwrap();
        assert_eq!(cfg.windows[0].tabs.len(), 1);
        assert!(warnings.iter().any(|w| w.message.contains("dir missing")));
    }

    #[test]
    fn a_file_is_not_a_directory_and_warns() {
        let tmp = std::env::temp_dir().join(format!("lector-cfg-file-{}", std::process::id()));
        std::fs::write(&tmp, "not a dir").unwrap();
        let src = format!(
            "[[window]]\ntitle = \"W\"\n[[window.tab]]\ntitle = \"T\"\ndir = \"{}\"\n",
            tmp.display()
        );
        let (_cfg, warnings) = parse_and_validate(&src).unwrap();
        assert!(warnings
            .iter()
            .any(|w| w.message.contains("not a directory")));
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn an_existing_dir_produces_no_dir_warning() {
        let src = "[[window]]\ntitle = \"W\"\n[[window.tab]]\ntitle = \"T\"\ndir = \"/tmp\"\n";
        let (_cfg, warnings) = parse_and_validate(src).unwrap();
        assert!(!warnings.iter().any(|w| w.message.contains("dir missing")));
    }

    #[test]
    fn expand_tilde_resolves_home_and_leaves_absolute_paths_alone() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_tilde("~/x"), home.join("x"));
        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("/tmp"), std::path::PathBuf::from("/tmp"));
        // A bare `~something` is not a home reference — leave it verbatim.
        assert_eq!(
            expand_tilde("~notahome"),
            std::path::PathBuf::from("~notahome")
        );
    }

    #[test]
    fn loose_tabs_resolve_before_groups_with_none_group() {
        let src = r#"
[[window]]
title = "W"
  [[window.tab]]
  title = "Loose"
  dir = "/tmp"
  [[window.group]]
  name = "G"
    [[window.group.tab]]
    title = "Grouped"
    dir = "/usr"
"#;
        let cfg = parse_and_validate(src).unwrap().0;
        let views = cfg.windows[0].tab_views();
        assert_eq!(views[0].title, "Loose");
        assert_eq!(views[0].group, None);
        assert_eq!(views[1].title, "Grouped");
        assert_eq!(views[1].group.as_deref(), Some("G"));
    }

    #[test]
    fn label_is_the_canonicalized_dir_hash_so_retitling_is_free() {
        // The spec's identity rule: identity is the hash of the canonicalized repo dir, NOT the
        // title. Retitling must not remap the webview.
        let a = parse_and_validate(
            "[[window]]\ntitle = \"W\"\n[[window.tab]]\ntitle = \"Before\"\ndir = \"/tmp\"\n",
        )
        .unwrap()
        .0;
        let b = parse_and_validate(
            "[[window]]\ntitle = \"W\"\n[[window.tab]]\ntitle = \"After\"\ndir = \"/tmp\"\n",
        )
        .unwrap()
        .0;
        assert_eq!(
            a.windows[0].tab_views()[0].label,
            b.windows[0].tab_views()[0].label,
            "retitling a tab must not change its label"
        );
    }

    #[test]
    fn two_spellings_of_one_dir_share_a_label() {
        // Canonicalization is what makes identity a property of the *repo*, not of how the config
        // spelled its path. `/tmp/.` and `/tmp` are the same repo.
        let a = parse_and_validate(
            "[[window]]\ntitle = \"W\"\n[[window.tab]]\ntitle = \"A\"\ndir = \"/tmp\"\n",
        )
        .unwrap()
        .0;
        let b = parse_and_validate(
            "[[window]]\ntitle = \"W\"\n[[window.tab]]\ntitle = \"A\"\ndir = \"/tmp/.\"\n",
        )
        .unwrap()
        .0;
        assert_eq!(
            a.windows[0].tab_views()[0].label,
            b.windows[0].tab_views()[0].label
        );
    }

    #[test]
    fn label_is_stable_when_a_tab_is_inserted_before_it() {
        let base = parse_and_validate(VALID).unwrap().0;
        let first = base.windows[0]
            .tab_views()
            .into_iter()
            .find(|t| t.title == "compositor")
            .unwrap()
            .label;
        let src = r#"
[[window]]
title = "Docs"
  [[window.tab]]
  title = "New"
  dir = "/tmp"
  [[window.tab]]
  title = "homelab"
  dir = "~/Developer/homelab"
  [[window.group]]
  name = "tools"
    [[window.group.tab]]
    title = "compositor"
    dir = "~/Developer/compositor"
"#;
        let inserted = parse_and_validate(src).unwrap().0;
        let after = inserted.windows[0]
            .tab_views()
            .into_iter()
            .find(|t| t.title == "compositor")
            .unwrap();
        assert_eq!(
            after.label, first,
            "a tab's label must not change when a tab is inserted before it"
        );
    }

    #[test]
    fn duplicate_dirs_get_distinct_labels() {
        // A repeated dir warns (not errors), so the labels must still disambiguate or the two tabs
        // would share one webview.
        let src = "[[window]]\ntitle = \"W\"\n[[window.tab]]\ntitle = \"A\"\ndir = \"/tmp\"\n[[window.tab]]\ntitle = \"B\"\ndir = \"/tmp\"\n";
        let cfg = parse_and_validate(src).unwrap().0;
        let views = cfg.windows[0].tab_views();
        assert_ne!(views[0].label, views[1].label);
    }

    #[test]
    fn startup_label_honours_open_on_launch() {
        // Default (no `open_on_launch` key) opens the first LOADED (load_on_open) tab — not
        // necessarily the first tab. In VALID the first tab (homelab) is cold and the group's
        // `compositor` is load_on_open, so it wins.
        let default = parse_and_validate(VALID).unwrap().0;
        let loaded = default.windows[0]
            .tab_views()
            .into_iter()
            .find(|v| v.title == "compositor")
            .unwrap()
            .label;
        assert_eq!(default.windows[0].startup_label(), Some(loaded));

        // `open_on_launch = true` forces the first tab even though it's cold (homelab).
        let src = VALID.replace(
            "title = \"Docs\"",
            "title = \"Docs\"\nopen_on_launch = true",
        );
        let forced = parse_and_validate(&src).unwrap().0;
        assert_eq!(
            forced.windows[0].startup_label(),
            Some(forced.windows[0].tab_views()[0].label.clone())
        );

        // No loaded tab anywhere → blank background, nothing forced.
        let none = parse_and_validate(
            "[[window]]\ntitle = \"W\"\n[[window.tab]]\ntitle = \"a\"\ndir = \"/tmp\"\n",
        )
        .unwrap()
        .0;
        assert_eq!(none.windows[0].startup_label(), None);

        let src = VALID.replace(
            "title = \"Docs\"",
            "title = \"Docs\"\nopen_on_launch = \"compositor\"",
        );
        let named = parse_and_validate(&src).unwrap().0;
        let want = named.windows[0]
            .tab_views()
            .into_iter()
            .find(|v| v.title == "compositor")
            .unwrap()
            .label;
        assert_eq!(named.windows[0].startup_label(), Some(want));
    }

    #[test]
    fn load_config_reads_a_file() {
        let tmp = std::env::temp_dir().join(format!("lector-load-{}.toml", std::process::id()));
        std::fs::write(&tmp, VALID).unwrap();
        let (cfg, _warnings) = load_config(&tmp).unwrap();
        assert_eq!(cfg.windows[0].title, "Docs");
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_config_missing_file_is_an_io_error() {
        let err = load_config(std::path::Path::new("/definitely/not/here.toml")).unwrap_err();
        assert!(matches!(err, ConfigError::Io(_)));
    }

    #[test]
    fn resolve_config_path_honours_env_override() {
        // NB: mutates process env — not thread-safe under a parallel runner, and a hard error on
        // edition 2024. The crate stays on 2021 (see Cargo.toml).
        std::env::set_var("LECTOR_CONFIG", "/tmp/x.toml");
        assert_eq!(
            resolve_config_path(),
            std::path::PathBuf::from("/tmp/x.toml")
        );
        std::env::remove_var("LECTOR_CONFIG");
        assert_eq!(resolve_config_path(), default_config_path());
    }

    #[test]
    fn default_config_path_is_dot_config_not_library() {
        let p = default_config_path();
        assert!(p.ends_with(".config/lector/config.toml"), "{}", p.display());
    }

    #[test]
    fn a_set_but_empty_env_var_falls_through_to_the_default() {
        // lector shipped the bug this fixes: `var_os(..).map(PathBuf::from)` turned an empty
        // LECTOR_CONFIG into PathBuf::from(""), whose only symptom was
        // "cannot read config: No such file or directory".
        std::env::set_var("LECTOR_CONFIG", "");
        assert_eq!(resolve_config_path(), default_config_path());
        std::env::remove_var("LECTOR_CONFIG");
    }

    #[test]
    fn parses_window_roots() {
        let src = r#"
[[window]]
title = "W"
[[window.root]]
name = "Dev"
dir = "~/Developer"
depth = 3
[[window.root]]
dir = "~/work"
"#;
        let (cfg, _w) = parse_and_validate(src).unwrap();
        let roots = cfg.windows[0].resolved_roots();
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].name, "Dev");
        assert_eq!(roots[0].depth, 3);
        assert_eq!(roots[1].name, "work"); // basename default
        assert_eq!(roots[1].depth, config_core::DEFAULT_ROOT_DEPTH);
    }

    #[test]
    fn root_with_empty_dir_errors() {
        let src = "[[window]]\ntitle = \"W\"\n[[window.root]]\ndir = \"  \"\n";
        assert!(matches!(
            parse_and_validate(src).unwrap_err(),
            ConfigError::EmptyRootDir { .. }
        ));
    }

    #[test]
    fn root_name_colliding_with_group_errors() {
        let src = r#"
[[window]]
title = "W"
[[window.group]]
name = "tools"
[[window.root]]
name = "tools"
dir = "/tmp"
"#;
        assert!(matches!(
            parse_and_validate(src).unwrap_err(),
            ConfigError::DuplicateSection { .. }
        ));
    }

    #[test]
    fn root_rejects_warden_only_leaf_keys() {
        // deny_unknown_fields on RawRoot: shell/cmd/probe/kill belong to warden, not lector.
        for bad in [
            "shell = \"fish\"",
            "cmd = \"x\"",
            "probe = \"p\"",
            "kill = \"k\"",
        ] {
            let src =
                format!("[[window]]\ntitle = \"W\"\n[[window.root]]\ndir = \"/tmp\"\n{bad}\n");
            assert!(
                matches!(parse_and_validate(&src).unwrap_err(), ConfigError::Parse(_)),
                "must reject {bad}"
            );
        }
    }

    #[test]
    fn discovered_projects_become_tree_views_after_curated() {
        let src = "[[window]]\ntitle = \"W\"\n[[window.tab]]\ntitle = \"loose\"\ndir = \"/tmp\"\n";
        let w = parse_and_validate(src).unwrap().0.windows.remove(0);
        let disc = vec![config_core::DiscoveredProject {
            path: std::path::PathBuf::from("/tmp/gh/proj"),
            tree_path: vec!["gh".into()],
            section: "Dev".into(),
        }];
        let views = w.tab_views_with_discovered(&disc);
        assert_eq!(views.len(), 2);
        assert!(!views[0].tree); // curated
        assert!(views[1].tree);
        assert_eq!(views[1].tree_path, vec!["gh".to_string()]);
        assert_eq!(views[1].group.as_deref(), Some("Dev"));
        assert_eq!(views[1].title, "proj");
        assert!(!views[1].load_on_open);
    }

    #[test]
    fn a_curated_tab_shadows_a_same_dir_discovered_project() {
        let src =
            "[[window]]\ntitle = \"W\"\n[[window.tab]]\ntitle = \"curated\"\ndir = \"/tmp\"\n";
        let w = parse_and_validate(src).unwrap().0.windows.remove(0);
        let disc = vec![config_core::DiscoveredProject {
            path: std::path::PathBuf::from("/tmp"), // same repo the curated tab names
            tree_path: vec![],
            section: "Dev".into(),
        }];
        let views = w.tab_views_with_discovered(&disc);
        assert_eq!(
            views.len(),
            1,
            "discovered duplicate of a curated tab must be dropped"
        );
        assert_eq!(views[0].title, "curated");
    }

    #[test]
    fn bundled_example_config_parses() {
        // Compile-time path dependency on the repo root — `just run` and `just gate` both point at
        // this file, so a config that doesn't parse must fail the build, not the app launch.
        let src = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/config.toml"
        ));
        let (cfg, _warnings) = parse_and_validate(src).unwrap();
        assert!(!cfg.windows.is_empty());
    }
}
