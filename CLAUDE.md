# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

gitti is a GPU-rendered git GUI in Rust (GPUI via `gpui-kit` 0.7) whose key feature is hunk
**and line** staging/unstaging/discarding. `gitty` is its terminal sibling (ratatui) on the
same core. Describe gitti on its own terms: no comparisons with or names of other git GUIs in
the repo, docs, issues or UI (interop such as the editor presets is fine).

**Product goal: the best git UI there is.** Everything git does is
supported — correctly and completely, in all its variants (nested `.gitignore` /
`.gitattributes`, `info/exclude`, worktrees, submodules, LFS, SHA-256, …), never only the
common case. Every feature is easy to reach (gitti: menu bar, context menus, palette; gitty:
menus, palette) and understandable: dialogs say in plain words what an action does and what
follows from it (what gets written where, what still has to be staged / committed / pushed).
A missing tool or setup (e.g. git-lfs not installed) gives a clear warning or error with
what to do, never a silent wrong result. Both frontends get a feature together. No half
solutions: what isn't done yet is tracked as an open point, never dropped silently.

If `CLAUDE.local.md` exists (gitignored, per machine), follow it as well: it holds local
working notes.

Language: code, comments and docs in English.

Licences: the apps (`gitti-app`, `gitty`) are GPL-3.0-or-later; `gitti-core`, `gitti-test` and
`xtask` are MIT OR Apache-2.0. Keep code that other projects should be able to reuse in
`gitti-core`, and new dependencies GPL-3.0-compatible (no GPL-2.0-only).

**Committing**: one commit per logical step once it is done and verified (tests, clippy
green), English message in the existing style (`gitti: …`, `gitty: …`, `gitti-core: …`).

**Worktree slots** (`crates/xtask`): `cargo xtask worktree add <name>` gives a long-lived
worktree (branch `gitti-<name>` from main; prints its path) that keeps its `target/`: a fresh
worktree costs a ~7 min cold build, a warm slot only compiles the diff. Where slots live and
how many is machine-local (`cargo xtask config`, a file outside the repo; default
`<repo>/.claude/worktrees`); exit code 3 = the configured place is not available (drive not
mounted). After a branch is merged: `cargo xtask worktree done <name>` (deletes the branch,
parks the slot on main; refuses unmerged branches). `worktree list` shows the slots,
`worktree prepare` creates and pre-builds missing ones, `worktree drop <N|all>` frees the disk.

**Tests — keep them cheap.** One test binary per crate (`core`, `tui`, `ui` + core's
`askpass_env`), line-tables-only debuginfo: a link is cheap. **Every test at most 2 s,
otherwise it fails** — write tests with `#[gitti_test::test]`, never a plain `#[test]` (a test
rejects it): over 2 s it fails, after 10 s the process exits naming it; UI scenarios the
same (harness), a scripted gitty run is killed after 10 s. Make it cheaper or split it,
never raise a limit. git processes are the cost (~35 ms each on Windows, more under
load): build fixtures with few of them (`git fast-import`, config written directly).
`GITTI_TEST_TIMES=1` prints every libtest test's time. Sweeps (sizes × fonts × …) check
representative points; the whole matrix only with `GITTI_TEST_FULL=1`, now and then, never
per merge.
**Release tests: `release-tests/`** holds the old slow tests and long randomized ones
(`staging_fuzz`). Not compiled by default (commented-out `[[test]]` blocks at the end of the
crates' `Cargo.toml`). **Only before a release / overnight — never in daily work, never per
merge, never committed uncommented.** New long or randomized tests go there; a bug they find
gets a fast daily test. Plain cargo, no runner:
- while working: the area only (`cargo test --workspace --test ui -- keys`,
  `cargo test --workspace --test core -- staging::`, `--test tui -- keys::`);
- a step is done: `cargo test --workspace --no-fail-fast` once + `cargo clippy --workspace --all-targets`.

**Always `--workspace`, never `-p`, for builds and tests** (in a warm worktree slot a plain
`cargo build -p gitti-app --tests` can take minutes). cargo
unifies dependency features over the *selected packages*, so `cargo test -p X` resolves
~40 crates (rustix, regex, gpui-pre-collections, …) differently from the workspace build
and recompiles them and the gpui stack above; `--test NAME` only picks targets.
`worktree prepare` pre-builds `cargo test --workspace --no-run`, `cargo clippy
--workspace --all-targets`, `cargo build -p gitti-app --features screenshot` and `cargo
build -p gitty` — use those (or run `target/debug/gitti` / `gitty` after them).

```bash
cargo run --release -p gitti-app -- /path/to/repo   # binary is `gitti`; default repo = cwd
cargo run --release -p gitty -- /path/to/repo       # TUI; binary is `gitty`
cargo test --workspace --no-fail-fast               # the whole suite (~5 min; without the flag it stops at the first red binary)
cargo test --workspace --test core -- staging::     # core integration tests of one module (temp repos)
cargo test --workspace --lib -- <name>              # unit tests (highlight, store, …)
cargo test --workspace --test ui [-- FILTER]        # headless UI tests (real fonts, no window): FILTER = suite (`layout`,
                                                    # `speed`, `keys`, `scroll`, `menus`, …), `suite::scenario` or a scenario;
                                                    # LAYOUT_DUMP=1 prints probes, KEYS_REPORT=1 git calls / frames per key
cargo test --workspace --test tui [-- keys::]       # gitty headless E2E
GITTI_TEST_FULL=1 cargo test --workspace --test ui  # sweeps' whole matrix (now and then, not per merge)
cargo clippy --workspace                            # kept clean
scripts/smoke.sh [outdir]                           # E2E UI test, drives the real app; run after UI changes
scripts/check-windows.sh                            # cargo check for x86_64-pc-windows-msvc from macOS; run after platform-specific changes
scripts\install-windows.cmd                         # on Windows: build + install gitti/gitty, Start menu entry, icon cache refresh
cargo run -p gitti-app --example make_icons         # assets/icon/*.svg → icns/ico/PNGs + sources.hash (checked in; bundle/install scripts run it)
cargo run --release -p gitti-core --example bench -- /path/to/repo   # timings (BENCH_HISTORY_ONLY=1 skips working state)
cargo run --release -p gitti-core --example stage_bench -- /scratch/clone  # every refresh query + a stage cycle (writes the index!)
```

Don't pipe a build into `tail` and then run the binary — a failed build leaves a stale binary.

- `Cargo.lock` must keep `cc` at 1.2.x (tree-sitter-sequel requires `~1.2`). If it drifts:
  `cargo update -p cc --precise 1.2.67`.
- Dev profile builds dependencies with `opt-level = 3`, own crates with `opt-level = 1`.

## Architecture

Three crates; the core is strictly separated from both frontends:

- `crates/gitti-core` — **no gpui dependency**. Sync, thread-safe API; the UI calls it on
  background executors. Git access is split deliberately:
  - reads of history/refs/objects via **gix** (`history.rs`, `graph.rs`, `blobs.rs`);
  - status and working-tree diffs via the git CLI, because its unified diff is exactly what
    `git apply` needs;
  - all writes via the **git CLI** (`git_cli.rs`, `ops.rs`, `repo.rs`) so hooks, credential
    helpers, signing and user config work. No libgit2/git2.
  - Every git call goes through `Git::run_raw`, which forces `core.quotepath=false`, `LC_ALL=C`,
    `GIT_TERMINAL_PROMPT=0`, `GIT_OPTIONAL_LOCKS=0` (otherwise our own `git status` rewrites the
    index and the fs watcher loops), runs under `setsid` on unix, and records into the shared
    `CommandLog` (shown in the UI's Git Log panel, changing commands persisted as JSONL).
- `crates/gitti-app` — GPUI frontend. `workspace.rs` is the window root (one `GittiApp` entity per
  repo tab); `app/mod.rs` holds `GittiApp` state, `refresh` → `Snapshot`, and `run_op`/`git_op`
  (busy flag, then refresh). Lists are virtualised with uniform row heights: the diff is
  flattened into `DiffRow`s (`gitti_core::rows`, wrapping pre-split into continuation rows), the commit
  graph is painted per row (`graph_paint.rs`).
- `crates/gitty` — terminal frontend (ratatui 0.30 + its re-exported crossterm). One
  `mpsc` channel of `Msg` (terminal events, background job results, askpass prompts, fs
  watcher) drives `App::handle`; jobs are plain threads (`app::spawn`). Shares with gitti:
  `gitti_core::rows` (diff rows, expansion, line selection), `paths::{config_dir,
  log_file_for}` (same persisted command log), askpass, watcher. The palette in
  `gitty/src/theme.rs` mirrors `gitti-app/src/theme.rs` — keep them in sync.

### Partial staging (the core feature, `patch.rs`)

Stage = build a patch from the *unstaged* hunk containing only the selected lines, then
`git apply --cached --recount` via stdin; unselected `+` lines are dropped, unselected `-` lines
become context. Unstage/discard build from the staged (resp. unstaged) hunk and apply with
`--reverse` / `-R`, so the roles swap. Hunk headers are recomputed. New/deleted-file headers are
rewritten to modifications for partial patches. With ignore-whitespace on, the shown diff is
`-w` but staging uses the exact `patch_diff` and `patch::map_selection` transfers the selection.
`crates/gitti-core/tests/core/staging.rs` covers these cases against temp repos — extend it when
touching patch logic.

### History for huge repos

`history.rs` loads structure only (ids, parents, time) — preferring the commit-graph file — and
fetches commit metadata lazily per visible row (`History::meta`). A quick partial history of the
newest commits is shown before the full load. `graph.rs` stores per-row lane events with
checkpoints every 64 rows and replays ranges on demand. `GITTI_PROFILE=1` prints phase timings.

### Askpass

The `gitti` and `gitty` binaries double as their own `GIT_ASKPASS`/`SSH_ASKPASS` helper:
`askpass::run_client_if_requested()` must stay the first thing in `main`. The helper forwards
the prompt (+ random token, + the asking git command) over a unix socket in a 0700 dir
(Windows: localhost TCP) to the running app, which shows a modal. git runs
under `setsid`, so it never prompts on the terminal — also not in gitty.

## Verifying TUI changes (gitty)

`GITTY_SCRIPT="key:enter;key:n;key:space;key:s;dump;state"` runs headless on ratatui's
`TestBackend` (`GITTY_SIZE=WxH`, default 160x45); steps are listed in
`crates/gitty/src/script.rs` (`dump` = screen as text, `cells:Y` = colour runs of a row,
`state` = focus/selection/status, `cmds` = git commands run since the last `cmds`). Each
key step waits until no job is running. Use
`GITTI_CONFIG_DIR=<tmp>` so the persisted command log stays separate.

## Verifying UI changes

**Prefer checks over screenshots.** Named elements carry layout probes (`.probe(name)`,
`src/probe.rs`); `src/layout_check.rs` holds the invariants (no overlap, rows inside their
list, …). `tests/ui/layout.rs` renders the real views headless in gpui's `TestAppContext` with the
OS text system and sweeps window sizes × font sizes × messages × scroll positions — extend it
(a probe on the new element, an invariant or a scenario expectation) when changing layout.
**Speed is a feature**: navigation and scrolling
run no git at all; stage / unstage / discard go through `work_op` (op + path-limited
re-read in one job, no full refresh). Tests check git calls per action (gitti
`Ui::git_commands()`, gitty `cmds`) — `tests/ui/speed.rs`, `tests/tui/speed.rs`.
In scripts, `check` / `layout` report the probes of the live window and `clickon:NAME` clicks
an element by name instead of coordinates. Screenshots are for a human look only.

The app is scriptable via `GITTI_SCRIPT` (`crates/gitti-app/src/script.rs` lists all steps, e.g.
`sleep:MS;select:ROW;lines:A-B;stage;click:X,Y;key:KEYSTROKE;dump;shot:PNG;quit`).

- To look at the UI yourself: build with `--features screenshot`, run with a `shot:` step and Read
  the PNG. Screenshot px = 2 × logical px used by
  click steps.
- Scripted runs open the window off-screen and unfocused and don't save the open tabs; the
  runner draws a frame itself before each step (a locked screen gets no OS frames).
- Settings and open tabs live in SQLite (`<config dir>/gitti.db`, `store.rs`, one row per
  settings field, `PRAGMA user_version` migrations); smoke tests read/patch it with `sqlite3`.
- Use `GITTI_CONFIG_DIR=<tmp>` (or `HOME=<tmp>`) so your real settings stay untouched; settings
  such as font size also change click coordinates.
- `GITTI_SCRIPT_PLACEMENT=1` makes a scripted run open at the saved window placement (e.g.
  maximized, on screen) instead of off-screen — for the restore path; use a copy of the db.
- `GITTI_MENU_BAR=1` forces the in-window (Windows/Linux) menu bar on macOS;
  `GITTI_WINDOW_FRAME=1` forces the self-drawn title bar controls (Linux client-side
  decorations).
- Script click coordinates are relative to the window content (Linux CSD shadow/border is
  added automatically). `shot:` does not work on Linux (`render_to_image` unimplemented).
  Linux screenshots instead: run under XWayland (`env -u WAYLAND_DISPLAY`, the window opens
  on screen with a mutter frame) and grab it with ImageMagick: `import -window $(xwininfo
  -root -tree | grep '"io.github.sneffets.gitti"' | awk '{print $1}') out.png` (1:1 px).
- Several heavy cargo commands at once (e.g. builds in parallel worktree slots) can exhaust
  memory: `scripts/heavy.sh` runs them one at a time (4 jobs; `--all` for the full suite).
- Key bindings use `secondary-` (cmd on macOS, ctrl elsewhere), not `cmd-`.
