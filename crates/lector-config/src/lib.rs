//! lector-config: parse, validate, and resolve lector's TOML config (windows + doc-repo tabs).
//!
//! The house-style TOML formatter + colour parsing are shared with warden and curator via the
//! config-core crate, re-exported here so the app (`src-tauri`) uses
//! `lector_config::{Colour, format_file, format_str}`.
pub use config_core::{fmt_cli, format_file, format_str, Colour, ColourError};

use serde::{Deserialize, Serialize};

/// What to open when a window launches. `false` (default) → blank; `true` → its first tab;
/// a string → the tab whose `title` matches (falling back to the first).
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
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub name: String,
    #[serde(default, rename = "tab")]
    pub tabs: Vec<Tab>,
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
}
