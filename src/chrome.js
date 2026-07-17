// lector's chrome controller: binds the shared chrome-core ChromeSidebar (the view) to lector's
// Tauri backend. The sidebar rendering, rows, dots, groups, resize, and error bar all live in
// chrome-core; this file only maps callbacks → commands and events → setters.
//
// Adapted from curator's src/chrome.js (the closest template — curator, like lector, sets `active`
// app-side, so `onSelect` does not auto-fire). Deltas from curator:
//   - No nav pill: a locally-rendered doc site has no browser-navigation concept to expose.
//   - No badge/notification machinery at all: docs don't notify, so `attention` is always `null`
//     (there is no shim/sentinel layer here to feed it from).
//   - No kill concept: `killable` is always `false`, `onKillClose` is omitted (chrome-core only
//     renders the ☠ control when a callback is supplied).
//   - `onSelect`'s rejection is surfaced via `sb.setError(String(e))`, not swallowed. This is the
//     app's only error channel for a missing repo: lector-config's `dir` validation deliberately
//     only *warns* on a missing/non-existent dir (an un-cloned repo must not strand every other
//     tab on last-good config), so the failure surfaces here instead, when the user selects that
//     tab and `select_tab` rejects. Swallowing it would make a missing repo look like a dead click.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// ── DTO mapping ─────────────────────────────────────────────────────────────
async function buildDto() {
  const id = await invoke("window_identity");
  const tabs = await invoke("get_tabs");
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
  paintEmptyState(dto.active);
  reportRect();
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
        // Re-clicking the active tab snaps it home (curator's home-on-active); otherwise select
        // it. Either rejection surfaces in the chrome's error bar — see this file's header note.
        invoke(wasActive ? "home_tab" : "select_tab", { label: tabId }).catch((e) => sb.setError(String(e)));
      },
      async onUnload(tabId) {
        await invoke("unload_tab", { label: tabId }).catch(() => {});
        // Re-render so the highlight + loaded dots follow the new state (get_tabs carries it).
        await refresh();
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
// config-reloaded / config-error (Task 10, config hot-reload) have no emitter yet, so no listeners
// here for them — nothing on the Rust side fires those events until that task wires them up.

mountChrome();
