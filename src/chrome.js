// lector's chrome controller: binds the shared chrome-core ChromeSidebar (the view) to lector's
// Tauri backend. The sidebar rendering, rows, dots, groups, resize, and error bar all live in
// chrome-core; this file only maps callbacks → commands and events → setters.
//
// Adapted from curator's src/chrome.js (the closest template — curator, like lector, sets `active`
// app-side, so `onSelect` does not auto-fire). Deltas from curator:
//   - The nav pill has three buttons, not four: no reload. compositor's watcher live-reloads the
//     page on every edit, so a manual reload button would be redundant here — don't "restore" it.
//   - No badge/notification machinery at all: docs don't notify, so `attention` is always `null`
//     (there is no shim/sentinel layer here to feed it from).
//   - No kill concept: `killable` is always `false`, `onKillClose` is omitted (chrome-core only
//     renders the ☠ control when a callback is supplied).
//   - `onSelect`'s rejection is surfaced via `sb.setError(String(e))`, not swallowed. This is the
//     app's only error channel for a missing repo: lector-config's `dir` validation deliberately
//     only *warns* on a missing/non-existent dir (an un-cloned repo must not strand every other
//     tab on last-good config), so the failure surfaces here instead, when the user selects that
//     tab and `select_tab` rejects. Swallowing it would make a missing repo look like a dead click.
//   - The nav pill's own rejections (nav_back/nav_forward/home_tab) surface the same way — see
//     `buildNavPill` below.
//   - Pop-out (`onPopOut`/`pop_out_tab`) mirrors curator's implementation closely (same
//     detachedLabels mirror, same onSelect-on-a-detached-row → raise_popped_window redirect, same
//     ⌘⇧O "pop-out-tab" event), with one lector-specific difference: `popOutTab`'s rejection
//     surfaces via `sb.setError`, not swallowed — consistent with this file's own error-surfacing
//     convention above (curator swallows it, since curator's failure modes there are effectively
//     unreachable; lector's `pop_out_tab` can genuinely fail, e.g. a `dir` that stopped existing).

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// ── Nav pill ─────────────────────────────────────────────────────────────────
// compositor's page shell has a tree-nav, a TOC, and prev/next links, but prev/next is reading
// order, not history, and there's no history.back() anywhere in it — following a link three levels
// deep leaves no way back. This pill is that missing back button, plus home (surfacing home_tab,
// which already existed but was only reachable by re-clicking the active tab).
// SVGs: exact geometry so icons align (carried over from curator's BACK_SVG/FWD_SVG/HOME_SVG).
const BACK_SVG = `<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 18l-6-6 6-6"/></svg>`;
const FWD_SVG = `<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 6l6 6-6 6"/></svg>`;
const HOME_SVG = `<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 11l9-8 9 8"/><path d="M5 10v10h14V10"/></svg>`;

let activeLabel = null; // controller mirror of the component's active tab (the nav pill acts on it)
let navBtns = [];
// Labels currently popped out into their own detached window (from the DTO). A click on a detached
// row means "raise its window" (raise_popped_window), not "select" — so onSelect consults this.
const detachedLabels = new Set();

function buildNavPill() {
  const pill = document.createElement("div");
  pill.className = "nav-pill";
  const wire = (id, icon, cmd) => {
    const btn = document.createElement("button");
    btn.className = "nav-btn";
    btn.id = id;
    btn.innerHTML = icon;
    btn.disabled = true; // no tab active at construction time — see setActiveLabel
    btn.addEventListener("click", () => {
      if (activeLabel) invoke(cmd, { label: activeLabel }).catch((e) => sb.setError(String(e)));
    });
    pill.appendChild(btn);
    return btn;
  };
  navBtns = [
    wire("nav-back", BACK_SVG, "nav_back"),
    wire("nav-forward", FWD_SVG, "nav_forward"),
    wire("nav-home", HOME_SVG, "home_tab"),
  ];
  return pill;
}

// Buttons render always; only the disabled state tracks whether a tab is active — never remove
// them. Called wherever activeLabel changes, including the clear path (unload_tab → active: null
// on the next refresh), so a live-looking Back button never sits over the empty pane.
function setActiveLabel(label) {
  activeLabel = label;
  for (const b of navBtns) b.disabled = !label;
}

// ── DTO mapping ─────────────────────────────────────────────────────────────
async function buildDto() {
  const id = await invoke("window_identity");
  const tabs = await invoke("get_tabs");
  // Refresh the detached-label mirror so onSelect can tell a popped-out row (raise its window) from
  // a normal one (select). Rebuilt each DTO so it clears when a tab redocks.
  detachedLabels.clear();
  tabs.forEach((t) => { if (t.detached) detachedLabels.add(t.label); });
  return {
    title: (id && id.title) || "",
    colour: (id && id.colour) ?? null,
    density: (id && id.density) || "comfortable",
    // sidebar_drag (global config, default on): make the non-interactive chrome a window-move drag
    // handle. Absent field defaults on, matching the config default.
    windowDrag: !(id && id.sidebar_drag === false),
    // lector's Rust side owns which tab is active — pass it so chrome-core honours it (no auto-fire).
    active: (tabs.find((t) => t.active) || {}).label ?? null,
    tabs: tabs.map((t) => ({
      id: t.label,
      title: t.title,
      group: t.group ?? null,
      live: t.loaded,
      attention: null, // docs don't notify — there is no badge/notification path to feed this
      presence: null, // lector has no session-presence concept
      killable: false, // lector has no kill concept
      // Popped out into its own window: chrome-core renders the ⤢ mark and routes a row click to
      // onSelect, which the controller maps to "raise the window". Invisible unless forwarded here.
      detached: !!t.detached,
      warn: false,
    })),
  };
}

// ── Mount + refresh ─────────────────────────────────────────────────────────
let sb = null;

// The empty-state (muted lector mark) shows only when no tab is active — otherwise a content
// webview covers the hole. It's composited BEHIND the content webviews, so this is occluded
// whenever a tab is shown; toggling on `active` keeps it from peeking during transitions.
function paintEmptyState(active) {
  document.getElementById("empty-state").style.display = active ? "none" : "flex";
}

// Report the #content-hole's CSS rect so Rust positions the content webviews to match. This is the
// single source of truth for content placement (warden/curator's model): chrome-core owns the
// sidebar width and clamp, the flex hole follows from CSS, and Rust just applies what's measured
// here.
function reportRect() {
  const r = document.getElementById("content-hole").getBoundingClientRect();
  invoke("set_hole_rect", { rect: { x: r.x, y: r.y, width: r.width, height: r.height } }).catch(() => {});
}

async function refresh() {
  const dto = await buildDto();
  sb.update(dto);
  setActiveLabel(dto.active);
  paintEmptyState(dto.active);
  reportRect();
}

// Shared by chrome-core's own row-unload control (onUnload below) and the ⌘W menu shortcut's
// "close-tab" event (below) — both mean the same thing (unload the active/given tab to cold), so
// this is the one place that does it rather than two copies.
async function unloadTab(tabId) {
  await invoke("unload_tab", { label: tabId }).catch(() => {});
  // Re-render so the highlight + loaded dots follow the new state (get_tabs carries it).
  await refresh();
}

// Shared by chrome-core's per-row ⤢ control (onPopOut) and the ⌘⇧O menu shortcut's "pop-out-tab"
// event — both pop the given/active tab out into its own window. A failure surfaces the same way
// every other command here does (this file's header note) rather than being swallowed. Refresh so
// the origin's sidebar shows the row's ⤢ detached mark and its newly-promoted active tab (get_tabs
// carries both).
async function popOutTab(tabId) {
  await invoke("pop_out_tab", { label: tabId }).catch((e) => sb.setError(String(e)));
  await refresh();
}

async function mountChrome() {
  const id = await invoke("window_identity");
  const title = (id && id.title) || "";
  // lector's `window_identity` (commands.rs) deliberately carries no `default_width` field (its
  // Identity struct is fixed to title/colour/density/sidebar_drag/auto_update), so this always
  // falls back to the literal below — kept in lockstep BY HAND with webviews.rs's `CHROME_W`
  // constant (no value crosses the IPC boundary to do it automatically; see that constant's own
  // comment).
  const defaultWidth = (id && id.default_width) || 240;

  sb = window.ChromeSidebar.mount(
    document.getElementById("sidebar"),
    {
      onSelect(tabId, { wasActive }) {
        // A popped-out row has no local webview to select — a click raises its detached window.
        if (detachedLabels.has(tabId)) {
          invoke("raise_popped_window", { label: tabId }).catch(() => {});
          return;
        }
        // Mirror chrome-core's own optimistic highlight move so the nav pill enables immediately
        // on click, not only after the next refresh() (curator does the same in its onSelect).
        setActiveLabel(tabId);
        // Re-clicking the active tab snaps it home (curator's home-on-active); otherwise select
        // it. Either rejection surfaces in the chrome's error bar — see this file's header note.
        invoke(wasActive ? "home_tab" : "select_tab", { label: tabId }).catch((e) => sb.setError(String(e)));
      },
      onUnload: unloadTab,
      // Pop the tab out into its own window (recreated webview, same running server/port). Refresh
      // so the row picks up its ⤢ detached mark and the origin's newly-promoted active tab.
      onPopOut: popOutTab,
      // Dock a popped-out tab back in (the ↩ overlay on a detached row's tile): close its window,
      // whose Destroyed handler runs redock (re-showing the tab on the same running server/port).
      onPopIn(tabId) {
        invoke("pop_in_tab", { label: tabId }).catch((e) => sb.setError(String(e)));
      },
      onResize(width) {
        // The chrome is the window's full-size main webview: the sidebar's visible width is CSS
        // (set here); the flex #content-hole follows, and reportRect tells Rust where to put the
        // content webviews. Rust never computes or clamps a width — chrome-core is the sole clamp
        // (bounds below).
        setSidebarWidth(width);
        reportRect();
      },
      // onKillClose: unused — lector sets killable:false, so the component never invokes it, and
      // omitting the callback is what keeps the ☠ control off the row entirely (capability-by-
      // presence).
    },
    {
      header: buildNavPill(),
      appName: "lector",
      storageKey: "lector:sidebar-width:" + title,
      defaultWidth,
      minWidth: MIN_W,
      maxWidth: MAX_W,
      // The chrome is the full-window main webview, so chrome-core's `window.innerWidth` IS the
      // window width and this is the ≤40% cap — the same value curator uses, and for the same
      // reason (this is NOT an isolated child webview, where `window.innerWidth` would be the
      // sidebar's own width and the cap would pin every drag to minWidth instead).
      maxFraction: MAX_FRACTION,
      // chrome-core's self-updater gate: run the launch + periodic checks when lector's config
      // allows (auto_update, default true).
      autoUpdate: id ? id.auto_update !== false : false,
    }
  );

  await refresh();

  // First-run width: chrome-core restores a saved width itself (firing onResize → CSS +
  // reportRect); if none is saved, apply the default. Setting the sidebar CSS width reflows the
  // flex #content-hole, so the ResizeObserver below fires reportRect and Rust realigns the content.
  const saved = parseFloat(localStorage.getItem("lector:sidebar-width:" + title));
  if (!(saved > 0)) {
    setSidebarWidth(defaultWidth);
  }
}

// Sidebar width bounds passed to chrome-core (the single clamp) — curator's values, carried over
// per this file's header note on the maxFraction gotcha.
const MIN_W = 160, MAX_W = 520, MAX_FRACTION = 0.4;

function setSidebarWidth(w) {
  document.getElementById("sidebar").style.width = Math.round(w) + "px";
}

// A window resize can push the sidebar past the ≤40% cap; re-clamp it here, then report the new
// hole so Rust repositions the content (there's no Rust-side resize relayout — JS drives it).
window.addEventListener("resize", () => {
  const el = document.getElementById("sidebar");
  const cur = parseInt(el.style.width, 10) || parseInt(getComputedStyle(el).width, 10);
  if (Number.isFinite(cur)) {
    const upper = Math.min(MAX_W, window.innerWidth * MAX_FRACTION);
    setSidebarWidth(Math.max(MIN_W, Math.min(cur, upper)));
  }
  reportRect();
});

// The content webviews track the hole: re-report whenever it resizes (sidebar drag, window resize).
// ResizeObserver fires once when observation begins, which is what makes the initial report happen.
const holeObserver = new ResizeObserver(() => reportRect());
holeObserver.observe(document.getElementById("content-hole"));

// ── Events ──────────────────────────────────────────────────────────────────
// Emitted by the Rust-side config watcher (lib.rs) on every hot-reload: a clean reload re-resolves
// tabs/servers and fires config-reloaded (refresh picks up the new tab set + live dots); a failed
// parse/validate leaves state untouched and fires config-error with the message instead, so the
// existing tabs keep working on last-good config.
listen("config-reloaded", () => {
  // Clear any error banner from a previous failed reload — this reload was clean, so whatever was
  // wrong got fixed. chrome-core's setError is sticky (it doesn't auto-clear on the next update),
  // so a fixed config would otherwise leave a stale error bar up forever.
  if (sb) sb.clearError();
  refresh().catch(() => {});
});
listen("config-error", (event) => {
  if (sb) sb.setError(String(event.payload));
});

// The menu spine's ⌘W (Tab ▸ Close Tab): unloads whichever tab is active in THIS window. lib.rs
// routes it via emit_to_focused_chrome, so only the focused window's chrome receives it.
listen("close-tab", () => {
  if (activeLabel) unloadTab(activeLabel);
});
// The menu spine's ⌘⇧O (Tab ▸ Pop Out Tab): pop THIS window's active tab out into its own window.
// lib.rs routes it via emit_to_focused_chrome, so only the focused window's chrome receives it.
listen("pop-out-tab", () => {
  if (activeLabel) popOutTab(activeLabel);
});

mountChrome();
