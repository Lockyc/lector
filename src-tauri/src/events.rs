//! Every event Rust delivers to a window's chrome (`src/chrome.js`), named once. An emit site takes
//! a [`ChromeEvent`], never a string literal, so [`ChromeEvent::ALL`] is the whole set and the
//! contract tests (`contract.rs`) can hold `chrome.js`'s `listen(...)` calls against it.

/// An event a window's chrome listens for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChromeEvent {
    ConfigReloaded,
    ConfigError,
    NavTab,
    JumpTab,
    CloseTab,
    PopOutTab,
    FindInSidebar,
    ToggleSidebar,
    CheckUpdate,
    /// Emitted by shell-core's `web::pointer` watcher, not by lector, but `chrome.js` listens for
    /// it all the same.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "shell-core emits it; listed for the contract")
    )]
    PointerAway,
}

impl ChromeEvent {
    #[cfg(test)]
    pub(crate) const ALL: [ChromeEvent; 10] = [
        ChromeEvent::ConfigReloaded,
        ChromeEvent::ConfigError,
        ChromeEvent::NavTab,
        ChromeEvent::JumpTab,
        ChromeEvent::CloseTab,
        ChromeEvent::PopOutTab,
        ChromeEvent::FindInSidebar,
        ChromeEvent::ToggleSidebar,
        ChromeEvent::CheckUpdate,
        ChromeEvent::PointerAway,
    ];

    /// The wire name `chrome.js` listens on.
    pub(crate) fn name(self) -> &'static str {
        match self {
            ChromeEvent::ConfigReloaded => "config-reloaded",
            ChromeEvent::ConfigError => "config-error",
            ChromeEvent::NavTab => "nav-tab",
            ChromeEvent::JumpTab => "jump-tab",
            ChromeEvent::CloseTab => "close-tab",
            ChromeEvent::PopOutTab => "pop-out-tab",
            ChromeEvent::FindInSidebar => "find-in-sidebar",
            ChromeEvent::ToggleSidebar => "toggle-sidebar",
            ChromeEvent::CheckUpdate => "check-update",
            ChromeEvent::PointerAway => shell_core::web::pointer::POINTER_AWAY_EVENT,
        }
    }
}
