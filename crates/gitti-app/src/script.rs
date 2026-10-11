//! `GITTI_SCRIPT` — drive the UI without a human, for smoke tests and screenshots.
//!
//! Steps separated by `;`:
//! `sleep:MS`, `shot:PATH` (needs feature `screenshot`), `select:ROW|SHA` (commit list),
//! `lines:A-B` (select diff rows A..=B like click + shift-click), `stage`, `unstage`,
//! `stagehunk:PATH,N` / `unstagehunk:PATH,N` / `stagefile:PATH` / `unstagefile:PATH` (like
//! the hunk / file buttons),
//! `toggle:ROW` (expand/collapse diff file row), `msg:TEXT` (commit message), `commit`,
//! `click:X,Y` / `rclick:X,Y` / `dclick:X,Y` / `tclick:X,Y` (triple click) / `down:X,Y` / `move:X,Y` / `up:X,Y` (logical px
//! relative to the window content, i.e. inside a client-drawn frame's shadow and border, so
//! the same coordinates work on every platform), `wheel:X,Y,DY` (scroll DY px down at X,Y),
//! `answer:TEXT` / `cancel` (current dialog), `fetch`, `zoom:DELTA`, `palette:QUERY` (run best match), `cmd:GIT ARGS` (command line), `config` (repo settings), `settings:SECTION`,
//! `key:KEYSTROKE` (e.g. `key:cmd-p`), `dump` (print diff rows), `open:PATH` (repository tab),
//! `tab:N` (activate tab), `win:merge` / `win:main` (window for the following `shot`, mouse,
//! `wheel`, `key` and `clipboard` steps; `merge` = the last opened merge editor),
//! `mergeeditor:N` (open the merge editor for conflicted file N), `clipboard` (print the
//! clipboard text; merge window), `clip:TEXT` (the clipboard text the Clone dialog takes —
//! a scripted run never reads the system clipboard for it), `fonts:UI,DIFF` (font sizes), `focusmsg` / `focuslist` /
//! `focusdiff` (commit message editor / commit list / diff), `focusnext` / `focusprev` (what
//! Tab / Shift-Tab do: the next / previous focusable element), `clickon:NAME[@X,Y]` /
//! `rclickon:` / `dclickon:` (the probe NAME of a fresh frame — `NAME*` = first one
//! starting with NAME — at its centre, or X,Y from its top left; a negative X / Y counts
//! from its right / bottom edge), `clickrel:DX,DY`
//! (relative to the last click, e.g. a context menu item), `menu:LABEL` (choose the entry
//! LABEL — exactly as shown — of the context menu opened last, e.g. after
//! `rclickon:commit-row:3`; entries made with `menus::menu_action`), `layout` (probes of the next frame
//! as JSON lines, see `probe.rs`), `check` (layout invariants, `layout_check.rs`), `quit`.
//! Dialogs: `field:N=TEXT` (type into field N), `choice:N` (its Nth fill-in button),
//! `checkbox:N` (flip checkbox N; `check:N` with an argument is an alias, `check` alone is
//! the layout step), `option:N` (choose option N), `modalok` (confirm),
//! `ask:PROMPT[|COMMAND]` (a credential prompt as from the askpass helper, optionally with
//! the git command that asked);
//! `worktree:ACTION,NAME` (open / remove / lock / unlock a worktree);
//! `folders:ACTION[,PATH]` (the folder picker: expand / toggle / ok / cancel);
//! `submodule:ACTION,PATH` (update / sync / open a submodule, `*` = all; `dismiss` the
//! out-of-date banner); `subtree:ACTION,PREFIX` (the add / pull / merge / push / split
//! dialog for a subtree); `custom:ACTION[,NAME]` (custom commands: `new`, `manage`,
//! `edit,NAME`, `run,NAME`); `ignore:PATH` ("Add to .gitignore…"
//! for a working-directory file), `exttool:PATH` ("Open in External Diff / Merge Tool"
//! for the first diff file row of PATH), `lfstrack:PATH` ("Track with Git LFS…" for it; the
//! dialog opens after a background check of git-lfs), `lfs:ACTION[,ARG]` (Git LFS
//! health: `fix,setup|hooks|download`, `dismiss` the banner, `check` again),
//! `fspaths:hide|show|dismiss` (paths this file system can't hold), `action:NAME` (dispatch e.g. `gitti::CloneRepository`, as
//! its menu item does). Pickers: `pick:TEXT` (type into the open picker — filters, or
//! starts its search), `pickok` (run the selected entry, as Enter). Search in Changes:
//! `changes:TEXT` (open the bar, type), `changehit:N` (select hit N), `changetoggle:strict
//! |order|all`. Reflog: `reflog:NAME` (open HEAD's or a branch's; `reflog:close`),
//! `reflogentry:N` (select entry N).
//! Steps act on the active tab (the start page while no tab is open).
//! Scripted runs record no start-up timings unless `GITTI_SCRIPT_STARTUP=1`.

use std::time::Duration;

use gpui_kit::*;

use crate::app::GittiApp;
use crate::workspace::Workspace;

pub fn run(script: String, window: AnyWindowHandle, ws: Entity<Workspace>, cx: &mut App) {
    // Nothing a script triggers opens windows on the user's desktop (editor, Finder, …).
    gitti_core::desktop::sandbox();
    cx.spawn(async move |cx| {
        // `win:merge` / `win:main` switch the window that mouse and `shot` steps go to.
        let mut target = window;
        for step in script.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            let (cmd, arg) = step.split_once(':').unwrap_or((step, ""));
            eprintln!("[script] {step}");
            // Steps act on what is on screen: draw it now (a hidden window or a locked
            // screen gets no frames from the OS, then clicks would hit stale hitboxes).
            if !matches!(cmd, "sleep" | "quit" | "win") {
                cx.update_window(target, |_, window, cx| force_frame(window, cx))
                    .ok();
            }
            match cmd {
                "sleep" => {
                    let ms = arg.parse().unwrap_or(500);
                    cx.background_executor()
                        .timer(Duration::from_millis(ms))
                        .await;
                }
                "quit" => {
                    cx.update(|cx| cx.quit());
                }
                "win" => {
                    target = match arg {
                        "merge" => crate::merge_window::LAST_WINDOW
                            .with(|w| *w.borrow())
                            .unwrap_or_else(|| {
                                eprintln!("[script] no merge window");
                                window
                            }),
                        _ => window,
                    };
                }
                // `clickon:NAME[@X,Y]` (also `rclickon`, `dclickon`): the probe NAME of a
                // fresh frame of the target window, its centre or X,Y from its top left;
                // `NAME*` = the first element whose name starts with NAME.
                "clickon" | "rclickon" | "dclickon" => {
                    let Some(mut probes) = fresh_probes(cx, target).await else {
                        continue;
                    };
                    let (name, offset) = match arg.split_once('@') {
                        Some((n, o)) => (n, Some(o)),
                        None => (arg, None),
                    };
                    let matches = |p: &crate::probe::Probe| match name.strip_suffix('*') {
                        Some(prefix) => p.name.starts_with(prefix),
                        None => p.name == name,
                    };
                    probes.sort_by_key(|p| (p.kind().to_string(), p.index()));
                    let Some(probe) = probes.into_iter().find(matches) else {
                        eprintln!("[script] {cmd}: no element {name} in the frame");
                        continue;
                    };
                    let pos = match offset.and_then(parse_xy) {
                        // Negative = from the right / bottom edge.
                        Some((x, y)) => {
                            let b = probe.bounds;
                            let from =
                                |o: f32, size: Pixels| if o < 0. { size + px(o) } else { px(o) };
                            b.origin + point(from(x, b.size.width), from(y, b.size.height))
                        }
                        None => probe.visible().unwrap_or(probe.bounds).center(),
                    };
                    let kind = cmd.trim_end_matches("on").to_string();
                    cx.update_window(target, move |_, window, cx| click_at(&kind, pos, window, cx))
                        .ok();
                }
                _ if is_layout_step(cmd, arg) => {
                    if let Some(probes) = fresh_probes(cx, target).await {
                        report(cmd, &probes);
                    }
                }
                _ if target != window => {
                    let (arg, cmd) = (arg.to_string(), cmd.to_string());
                    let result = cx.update_window(target, move |_, window, cx| {
                        if !window_step(&cmd, &arg, window, cx) {
                            eprintln!("[script] {cmd} is not supported in this window");
                        }
                    });
                    if let Err(e) = result {
                        eprintln!("[script] {step} failed: {e}");
                    }
                }
                _ => {
                    let ws = ws.clone();
                    let (cmd, arg) = (cmd.to_string(), arg.to_string());
                    let result = cx.update_window(window, move |_, window, cx| {
                        main_window_step(&ws, &cmd, &arg, window, cx)
                    });
                    if let Err(e) = result {
                        eprintln!("[script] {step} failed: {e}");
                    }
                }
            }
        }
    })
    .detach();
}

/// `layout` / `check` (probes of a fresh frame) — `check:N` with an argument is the dialog
/// checkbox step (alias of `checkbox:N`), which goes to the window like the other steps.
fn is_layout_step(cmd: &str, arg: &str) -> bool {
    matches!(cmd, "layout" | "check") && arg.is_empty()
}

/// Lays out and paints the window now instead of waiting for the OS's next frame.
fn force_frame(window: &mut Window, cx: &mut App) {
    window.refresh();
    window.draw(cx).clear(cx);
}

/// The probes of a freshly drawn frame of `target`.
async fn fresh_probes(cx: &mut AsyncApp, target: AnyWindowHandle) -> Option<Vec<crate::probe::Probe>> {
    cx.update_window(target, |_, window, cx| {
        crate::probe::clear();
        force_frame(window, cx);
    })
    .ok()?;
    Some(crate::probe::snapshot())
}

/// `layout`: every probe as a JSON line (`[layout] {…}`); `check`: the violated layout
/// invariants (`[check] FAIL …`, or `[check] ok`).
fn report(cmd: &str, probes: &[crate::probe::Probe]) {
    if cmd == "layout" {
        for p in probes {
            println!("[layout] {}", p.to_json());
        }
        return;
    }
    let fails = crate::layout_check::check(probes);
    if fails.is_empty() {
        println!("[check] ok ({} probes)", probes.len());
    }
    for f in fails {
        println!("[check] FAIL {f}");
    }
}

/// A step in the main window: tab steps go to the workspace, the rest to the active tab.
pub(crate) fn main_window_step(
    ws: &Entity<Workspace>,
    cmd: &str,
    arg: &str,
    window: &mut Window,
    cx: &mut App,
) {
    match cmd {
        "open" => ws.update(cx, |w, cx| w.open(arg.into(), window, cx)),
        "tab" => ws.update(cx, |w, cx| w.script_activate(arg.parse().unwrap_or(0), window, cx)),
        // A real scripted run keeps its own clipboard; the headless tests have the test
        // platform's.
        "clip" if std::env::var_os("GITTI_SCRIPT").is_some() => GittiApp::script_set_clipboard(arg),
        "clip" => cx.write_to_clipboard(ClipboardItem::new_string(arg.to_string())),
        "action" => {
            if let Err(e) = crate::testing_menus::dispatch(arg, window, cx) {
                eprintln!("[script] action {arg}: {e:#}");
            }
        }
        _ => match ws.read(cx).script_app() {
            Some(app) => step_in_window(cmd, arg, &app, window, cx),
            None => eprintln!("[script] no repository tab"),
        },
    }
}

/// Steps that work on any window (main or merge editor): screenshots and mouse input.
fn window_step(cmd: &str, arg: &str, window: &mut Window, cx: &mut App) -> bool {
    match cmd {
        "shot" => shot(arg, window),
        "down" | "move" | "up" => {
            // Raw left-button events; a drag is `down:…;sleep:50;move:…;sleep:50;up:…`.
            let position = arg_point(window, arg);
            let event = match cmd {
                "down" => PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                }),
                "move" => PlatformInput::MouseMove(MouseMoveEvent {
                    position,
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Modifiers::default(),
                }),
                _ => PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                }),
            };
            window.dispatch_event(event, cx);
        }
        // In the merge window; the main window's `key` also gives the list focus first.
        "key" => {
            if let Ok(k) = Keystroke::parse(arg) {
                window.dispatch_keystroke(k, cx);
            }
        }
        "clipboard" => eprintln!(
            "[clipboard] {:?}",
            cx.read_from_clipboard().and_then(|c| c.text())
        ),
        "hover" => {
            let (x, y) = arg.split_once(',').unwrap_or(("0", "0"));
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: point(
                        px(x.trim().parse().unwrap_or(0.)),
                        px(y.trim().parse().unwrap_or(0.)),
                    ),
                    ..Default::default()
                }),
                cx,
            );
        }
        "wheel" => {
            // `wheel:X,Y,DY` scrolls by DY logical px (positive = content moves up).
            let v: Vec<f32> = arg
                .split(',')
                .map(|n| n.trim().parse().unwrap_or(0.))
                .collect();
            let (x, y, dy) = (v[0], *v.get(1).unwrap_or(&0.), *v.get(2).unwrap_or(&0.));
            let position = content_point(window, x, y);
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position,
                    ..Default::default()
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position,
                    delta: ScrollDelta::Pixels(point(px(0.), px(-dy))),
                    modifiers: Modifiers::default(),
                    touch_phase: TouchPhase::Moved,
                }),
                cx,
            );
        }
        "click" | "rclick" | "dclick" | "tclick" => {
            click_at(cmd, arg_point(window, arg), window, cx)
        }
        // `clickrel:DX,DY` — relative to the last click (menu items below a right click).
        "clickrel" => {
            let (dx, dy) = parse_xy(arg).unwrap_or_default();
            let pos = LAST_CLICK.with(|c| c.get()) + point(px(dx), px(dy));
            click_at("click", pos, window, cx);
        }
        _ => return false,
    }
    true
}

thread_local! {
    static LAST_CLICK: std::cell::Cell<Point<Pixels>> = std::cell::Cell::new(Point::default());
}

fn parse_xy(arg: &str) -> Option<(f32, f32)> {
    let (x, y) = arg.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/// `click` / `rclick` / `dclick` at a window position: move there, press and release.
fn click_at(kind: &str, pos: Point<Pixels>, window: &mut Window, cx: &mut App) {
    LAST_CLICK.with(|c| c.set(pos));
    let button = if kind == "rclick" {
        MouseButton::Right
    } else {
        MouseButton::Left
    };
    let clicks = match kind {
        "dclick" => 2,
        "tclick" => 3,
        _ => 1,
    };
    window.dispatch_event(
        PlatformInput::MouseMove(MouseMoveEvent {
            position: pos,
            ..Default::default()
        }),
        cx,
    );
    for click_count in 1..=clicks {
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button,
                position: pos,
                modifiers: Modifiers::default(),
                click_count,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button,
                position: pos,
                modifiers: Modifiers::default(),
                click_count,
            }),
            cx,
        );
    }
}

fn step_in_window(cmd: &str, arg: &str, app: &Entity<GittiApp>, window: &mut Window, cx: &mut App) {
    if window_step(cmd, arg, window, cx) {
        return;
    }
    match cmd {
        "mergeeditor" => {
            let file = arg.parse().unwrap_or(0);
            app.update(cx, |a, cx| a.open_merge_editor(file, window, cx));
        }
        "menu" => {
            // As a click on the entry: the menu closes, then the entry acts.
            if app.read(cx).script_state(window, cx).focus == "other"
                && let Ok(k) = Keystroke::parse("escape")
            {
                window.dispatch_keystroke(k, cx);
            }
            if let Err(e) = crate::app::run_menu_entry(arg, window, cx) {
                eprintln!("[script] menu: {e}");
            }
        }
        "select" => {
            // A row number or a commit id prefix.
            let row = arg
                .parse()
                .ok()
                .or_else(|| app.read(cx).script_row_of(arg))
                .unwrap_or(0);
            app.update(cx, |a, cx| a.select_row(row, window, cx));
        }
        "lines" => {
            let (a, b) = arg.split_once('-').unwrap_or((arg, arg));
            let (a, b) = (a.parse().unwrap_or(0), b.parse().unwrap_or(0));
            app.update(cx, |app, cx| app.script_select_lines(a, b, cx));
        }
        "toggle" => {
            let row = arg.parse().unwrap_or(0);
            app.update(cx, |app, cx| app.script_toggle(row, cx));
        }
        "stage" => app.update(cx, |a, cx| a.stage_selection(window, cx)),
        "stagehunk" | "unstagehunk" | "stagefile" | "unstagefile" => {
            let (path, hunk) = arg.split_once(',').unwrap_or((arg, ""));
            let hunk = hunk.parse().ok();
            let stage = cmd.starts_with("stage");
            app.update(cx, |a, cx| a.script_stage(stage, path, hunk, window, cx));
        }
        "msg" => app.update(cx, |a, cx| a.script_set_message(arg, window, cx)),
        "commit" => app.update(cx, |a, cx| a.commit(window, cx)),
        "unstage" => app.update(cx, |a, cx| a.unstage_selection(window, cx)),
        "dump" => {
            eprintln!(
                "window: {:?} {:?} {:?} maximized={} frame={}",
                window.window_decorations(),
                window.viewport_size(),
                window.window_bounds(),
                window.is_maximized(),
                crate::draws_window_frame(window),
            );
            eprintln!(
                "placement: {:?} setup: {}",
                crate::window_placement::current(window, cx),
                crate::window_placement::setup_key(cx),
            );
            app.read(cx).script_dump()
        }
        "answer" => app.update(cx, |a, cx| a.script_answer_modal(arg, window, cx)),
        "cancel" => app.update(cx, |a, cx| a.script_cancel_modal(window, cx)),
        "zoom" => {
            let d: f32 = arg.parse().unwrap_or(1.);
            app.update(cx, |a, cx| a.script_zoom(d, cx));
        }
        "palette" => app.update(cx, |a, cx| a.script_palette(arg, window, cx)),
        "key" => {
            // Scripted windows are not focused by the OS; make sure a view has focus so
            // keystrokes reach the action handlers.
            if window.focused(cx).is_none() {
                app.update(cx, |a, cx| a.script_focus_list(window, cx));
            }
            if let Ok(k) = Keystroke::parse(arg) {
                window.dispatch_keystroke(k, cx);
            }
        }
        "cmd" => app.update(cx, |a, cx| a.script_run_command(arg, window, cx)),
        "settings" => app.update(cx, |a, cx| a.script_open_settings(arg, window, cx)),
        "config" => app.update(cx, |a, cx| a.script_config_dialog(window, cx)),
        "fetch" => app.update(cx, |a, cx| a.script_fetch(window, cx)),
        // `fonts:UI,DIFF` — absolute font sizes (pt).
        "fonts" => {
            let (ui, diff) = arg.split_once(',').unwrap_or((arg, arg));
            let (ui, diff) = (ui.trim().parse().ok(), diff.trim().parse().ok());
            app.update(cx, |a, cx| a.script_fonts(ui, diff, cx));
        }
        "focusmsg" => app.update(cx, |a, cx| a.script_focus_message(window, cx)),
        "focuslist" => app.update(cx, |a, cx| a.script_focus_list(window, cx)),
        "focusdiff" => app.update(cx, |a, cx| a.script_focus_diff(window, cx)),
        "focuslog" => app.update(cx, |a, cx| a.script_focus_log(window, cx)),
        // What Tab / Shift-Tab do (gpui-base's `Root`; the headless tests' window has none).
        "focusnext" => window.focus_next(cx),
        "focusprev" => window.focus_prev(cx),
        "discardall" => app.update(cx, |a, cx| a.script_discard_all(window, cx)),
        "field" => {
            let (n, text) = arg.split_once('=').unwrap_or((arg, ""));
            let n = n.parse().unwrap_or(0);
            app.update(cx, |a, cx| a.script_modal_field(n, text, window, cx));
        }
        "choice" => {
            let n = arg.parse().unwrap_or(0);
            app.update(cx, |a, cx| a.modal_choice(n, window, cx));
        }
        "checkbox" | "check" => {
            let n = arg.parse().unwrap_or(0);
            app.update(cx, |a, cx| a.script_modal_check(n, cx));
        }
        "option" => {
            let n = arg.parse().unwrap_or(0);
            app.update(cx, |a, cx| a.modal_option(n, window, cx));
        }
        // `worktree:ACTION,NAME` — open / remove / lock / unlock the worktree in folder NAME.
        "worktree" => {
            let (action, name) = arg.split_once(',').unwrap_or((arg, ""));
            app.update(cx, |a, cx| a.script_worktree(action, name, window, cx));
        }
        // `submodule:ACTION,PATH` — update / sync / open (PATH `*` = all), dismiss the banner.
        "submodule" => {
            let (action, path) = arg.split_once(',').unwrap_or((arg, ""));
            app.update(cx, |a, cx| a.script_submodule(action, path, window, cx));
        }
        // `folders:ACTION[,PATH]` — the folder picker over a dialog: `expand,PATH`,
        // `toggle,PATH`, `ok`, `cancel`.
        "folders" => {
            let (action, path) = arg.split_once(',').unwrap_or((arg, ""));
            app.update(cx, |a, cx| a.script_folder_picker(action, path, window, cx));
        }
        // `subtree:ACTION,PREFIX` — the dialog of add / pull / merge / push / split (for the
        // subtree at PREFIX, else with PREFIX as its folder).
        "subtree" => {
            let (action, prefix) = arg.split_once(',').unwrap_or((arg, ""));
            app.update(cx, |a, cx| a.script_subtree(action, prefix, window, cx));
        }
        // `custom:ACTION[,NAME]` — custom commands: `new` / `manage` / `edit,NAME` /
        // `run,NAME`.
        "custom" => {
            let (action, name) = arg.split_once(',').unwrap_or((arg, ""));
            app.update(cx, |a, cx| a.script_custom(action, name, window, cx));
        }
        "modalok" => app.update(cx, |a, cx| a.script_confirm_modal(window, cx)),
        // `ask:PROMPT[|COMMAND]` — a credential prompt as the askpass helper forwards it,
        // optionally with the git command that asked (the answer goes nowhere).
        "ask" => {
            let (reply, _) = std::sync::mpsc::channel();
            let (text, command) = match arg.split_once('|') {
                Some((t, c)) => (t, Some(c.to_string())),
                None => (arg, None),
            };
            let req = crate::app::AskRequest {
                prompt: gitti_core::askpass::Prompt {
                    text: text.to_string(),
                    command,
                    ..Default::default()
                },
                reply,
            };
            app.update(cx, |a, cx| a.enqueue_ask(req, window, cx));
        }
        // `askgone` — the helper of the shown / queued prompts went away (its operation
        // was cancelled or ended): their dialogs close themselves.
        "askgone" => app.update(cx, |a, _| a.script_asks_gone()),
        "ignore" => app.update(cx, |a, cx| a.script_ignore_dialog(arg, window, cx)),
        "lfstrack" => app.update(cx, |a, cx| a.lfs_track_dialog(arg.to_string(), window, cx)),
        "exttool" => app.update(cx, |a, cx| a.script_external_tool(arg, window, cx)),
        // `lfs:ACTION[,ARG]` — Git LFS health: `fix,KEY`, `dismiss`, `check`.
        "lfs" => {
            let (action, rest) = arg.split_once(',').unwrap_or((arg, ""));
            app.update(cx, |a, cx| a.script_lfs(action, rest, window, cx));
        }
        // `fspaths:ACTION` — paths this file system can't hold: `hide`, `show`, `dismiss`.
        "fspaths" => app.update(cx, |a, cx| a.script_fs_paths(arg, window, cx)),
        "pick" => app.update(cx, |a, cx| a.script_picker_query(arg, window, cx)),
        "changes" => app.update(cx, |a, cx| a.script_change_search(arg, window, cx)),
        "changehit" => {
            let i = arg.parse().unwrap_or(0);
            app.update(cx, |a, cx| a.script_change_hit(i, window, cx))
        }
        "changetoggle" => app.update(cx, |a, cx| a.script_change_toggle(arg, cx)),
        "reflog" => app.update(cx, |a, cx| a.script_reflog(arg, window, cx)),
        "reflogentry" => {
            let i = arg.parse().unwrap_or(0);
            app.update(cx, |a, cx| a.script_reflog_entry(i, cx))
        }
        "pickok" => app.update(cx, |a, cx| a.script_picker_accept(window, cx)),
        other => eprintln!("[script] unknown step {other}"),
    }
}

/// Window position of content coordinates `x`,`y`: with client-side decorations (Linux) the
/// window surface also holds the frame's shadow and 1 px border around the content.
fn content_point(window: &Window, x: f32, y: f32) -> Point<Pixels> {
    let pad = gpui_kit::component::window_paddings(window);
    let border = |tiled: bool| match window.window_decorations() {
        Decorations::Client { .. } if !tiled => px(1.),
        _ => px(0.),
    };
    let tiling = match window.window_decorations() {
        Decorations::Client { tiling } => tiling,
        Decorations::Server => Tiling::default(),
    };
    point(
        pad.left + border(tiling.left) + px(x),
        pad.top + border(tiling.top) + px(y),
    )
}

/// `X,Y` step argument → window position (see [`content_point`]).
fn arg_point(window: &Window, arg: &str) -> Point<Pixels> {
    let (x, y) = arg.split_once(',').unwrap_or(("0", "0"));
    let num = |n: &str| n.trim().parse().unwrap_or(0.);
    content_point(window, num(x), num(y))
}

#[cfg(feature = "screenshot")]
fn shot(path: &str, window: &mut Window) {
    match window.render_to_image() {
        Ok(img) => match img.save(path) {
            Ok(()) => eprintln!("[script] saved {path} ({}x{})", img.width(), img.height()),
            Err(e) => eprintln!("[script] save failed: {e}"),
        },
        Err(e) => eprintln!("[script] render_to_image failed: {e}"),
    }
}

#[cfg(not(feature = "screenshot"))]
fn shot(_: &str, _: &mut Window) {
    eprintln!("[script] shot needs `--features screenshot`");
}

#[cfg(test)]
mod tests {
    use super::is_layout_step;

    #[gitti_test::test]
    fn check_step_vs_checkbox_step() {
        assert!(is_layout_step("check", ""));
        assert!(is_layout_step("layout", ""));
        assert!(!is_layout_step("check", "0"));
        assert!(!is_layout_step("checkbox", "2"));
    }
}
