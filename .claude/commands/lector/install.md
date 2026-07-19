You are installing or updating **lector** — a minimal macOS app that renders a curated,
declarative set of local documentation repos as live, hot-reloading tabs from
`~/.config/lector/config.toml`.

GitHub: `https://github.com/Lockyc/lector`

Build it from source and install `lector.app` to `/Applications`. Source lives in a
persistent clone at `~/.lector`; updates are `git pull` + rebuild.

---

## Steps

### 1. Detect context

Check whether the current working directory is a lector checkout:

```bash
[ -f install.sh ] && [ -f src-tauri/tauri.conf.json ] && echo "IN_REPO" || echo "NOT_IN_REPO"
```

**If IN_REPO:** you will run the local `install.sh` in step 4 (it builds this checkout).
**If NOT_IN_REPO:** you will run the published installer over curl in step 4 (it manages a `~/.lector` clone).

### 2. Check prerequisites and offer to install

Probe each build prerequisite:

```bash
command -v git         >/dev/null 2>&1 && echo "git: ok"       || echo "git: MISSING"
command -v cargo       >/dev/null 2>&1 && echo "cargo: ok"     || echo "cargo: MISSING"
command -v cargo-tauri >/dev/null 2>&1 && echo "tauri-cli: ok" || echo "tauri-cli: MISSING"
xcode-select -p        >/dev/null 2>&1 && echo "clt: ok"       || echo "clt: MISSING"
command -v brew        >/dev/null 2>&1 && echo "brew: ok"      || echo "brew: MISSING"
```

For each MISSING prerequisite (other than brew), use AskUserQuestion to offer to install it.
Only run an install command on confirmation:

- **Xcode Command Line Tools** (also provides `git`): `xcode-select --install`
  (opens a GUI installer — tell the user to finish it, then continue).
- **Rust** (`cargo`): `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y`,
  then advise sourcing `~/.cargo/env` or restarting the shell.
- **Tauri CLI** (`cargo-tauri`): `cargo install tauri-cli --version '^2' --locked`
  (compiles the CLI — takes a few minutes). `install.sh` backstops this too, so it
  is safe to skip here and let the core install handle it.

lector has **no Node/npm dependency** — do not probe for it. If a prerequisite the user
declined to install is still missing (other than the Tauri CLI, which `install.sh` installs
anyway), warn that `install.sh` will refuse to build until it is present, and ask whether to
continue anyway.

### 3. Probe current state

For smarter messaging:

```bash
[ -d /Applications/lector.app ] && echo "app: installed" || echo "app: absent"
if [ ! -e ~/.lector ]; then echo "src: fresh";
elif [ -d ~/.lector/.git ]; then echo "src: clone";
else echo "src: not-a-clone"; fi
pgrep -f "/Applications/lector.app/" >/dev/null && echo "running: yes" || echo "running: no"
```

If `src: not-a-clone`, tell the user `~/.lector` exists but is not a git clone; `install.sh`
will refuse to touch it. They must move it aside before continuing (NOT_IN_REPO path only).

### 4. Run the core install

**If IN_REPO:**

```bash
PATH="$HOME/.cargo/bin:$PATH" bash install.sh
```

**If NOT_IN_REPO:**

```bash
curl -fsSL https://raw.githubusercontent.com/Lockyc/lector/main/install.sh | PATH="$HOME/.cargo/bin:$PATH" bash
```

(The `PATH="$HOME/.cargo/bin:$PATH"` prefix ensures a Rust toolchain / Tauri CLI you may
have just installed via rustup/cargo in step 2 is found — a fresh shell won't have picked
up rustup's profile changes yet.)

IN_REPO builds the current checkout; NOT_IN_REPO clones/updates `~/.lector` and builds from it.
Both back the Tauri CLI install if absent, run `cargo tauri build`, install the app to
`/Applications/lector.app`, and seed `~/.config/lector/config.toml` if absent. The build takes
a few minutes. **If it fails, show the full output and stop** — do not run later steps.

### 5. Configure

`install.sh` has already seeded `~/.config/lector/config.toml` from the example if it was
absent. Use AskUserQuestion to offer to open it for editing now:

- **Open in editor** → `open -e ~/.config/lector/config.toml`
- **Reveal in Finder** → `open -R ~/.config/lector/config.toml`
- **Skip** — leave it for later.

Briefly note the format: one or more `[[window]]` blocks, each with loose `[[window.tab]]`
entries and/or `[[window.group]]` sections of `[[window.group.tab]]`s. Each tab points at a
**`dir`** (a local doc repo path), not a URL. A `[[window.root]]` block instead points at a
projects dir and **auto-discovers** every git repo under it (optional scan `depth`), rendered
as a collapsible folder tree with a `⟳` rescan button. The app also has a **Config** menu
(Edit Config, Reveal Config in Finder) and a **Tab** menu (Close Tab ⌘W, Pop Out Tab ⌘⇧O).

### 6. Offer to launch

Use AskUserQuestion: **"Launch lector now?"**

- **Launch** → `open /Applications/lector.app`
- **Not now** — skip.

### 7. Self-install this command

So `/lector:install` is available in future Claude Code sessions:

```bash
mkdir -p ~/.claude/commands/lector
```

Copy `install.md` verbatim to `~/.claude/commands/lector/install.md`. Source it from the
cwd checkout if IN_REPO (`.claude/commands/lector/install.md`), otherwise from
`~/.lector/.claude/commands/lector/install.md` (present after step 4 cloned it).

### 8. Summary

Read the version from `src-tauri/Cargo.toml` (the `version` field — lector's `tauri.conf.json`
carries no `version` key) — use the cwd checkout if IN_REPO, else `~/.lector/src-tauri/Cargo.toml`.

Print:

**Installed**
- lector vX.Y.Z → `/Applications/lector.app` ✓
- Source clone → `~/.lector` (NOT_IN_REPO) or "built from checkout" (IN_REPO)
- Config → `~/.config/lector/config.toml` (seeded from example / already existed)

**Next steps**
- Edit `~/.config/lector/config.toml` to curate your doc tabs (hot-reloads on save).
- Selecting a tab starts a live-reloading `compositor serve` on a loopback port; edit any
  `.md` in the repo and the tab updates immediately — nothing is deployed or published.
- Update any time by re-running `/lector:install` (or `curl … | bash`) — it git-pulls and
  rebuilds.
