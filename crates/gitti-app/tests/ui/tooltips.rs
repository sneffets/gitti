//! Headless tests of tooltips: no tooltips while a context menu or a split button's
//! dropdown is open — not over the menu (also not one whose hover delay was already running
//! when the menu opened), not over other elements the mouse passes meanwhile — and tooltips
//! as before once the menu is closed. Covers gpui's own tooltips (sidebar worktree rows,
//! the commit list's worktree labels) and the "managed" button tooltips (split button). No
//! tooltip after the pointer left the window or the window was deactivated while the delay
//! ran, and a tooltip that is up goes away then. A remote's sidebar row shows its URLs.
//!
//! Own `main` like `layout.rs`: `cargo test --workspace --test ui -- tooltips[::FILTER]`.
// Some helpers are used only by the unix-only scenarios.
#![cfg_attr(not(unix), allow(dead_code, unused_macros))]

use crate::common;

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Duration;

use common::*;

thread_local! {
    static BASE: RefCell<PathBuf> = const { RefCell::new(PathBuf::new()) };
}

macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

fn base() -> PathBuf {
    BASE.with(|b| b.borrow().clone())
}

pub fn main() {
    let tmp = common::setup();
    BASE.with(|b| *b.borrow_mut() = gitti_core::paths::canonical(tmp.path()));
    let tests: &[(&str, Scenario)] = &[
        ("split_button_dropdown", split_button_dropdown),
        ("context_menu", context_menu),
        ("left_window", left_window),
        ("remote_url", remote_url),
    ];
    common::run_with(tests, |name| {
        let dir = base().join(name);
        let repo = if name == "split_button_dropdown" {
            make_merge_repo(&dir)
        } else if name == "remote_url" {
            let d = make_repo(&dir);
            append_config(&d, REMOTES);
            d
        } else {
            // In a subdir: the worktrees beside it stay this scenario's own.
            let d = make_repo(&dir.join("repo"));
            git(&d, &["worktree", "add", "-q", "../wt-one", "-b", "one"]);
            git(&d, &["worktree", "add", "-q", "../wt-two", "-b", "two"]);
            d
        };
        rooted_ui(&repo)
    });
}

/// `remote_url`'s remotes, written into `.git/config`: one URL; a separate push URL;
/// credentials in the URL; two URLs (git fetches from the first, pushes to both).
const REMOTES: &str = "[remote \"origin\"]\n\turl = https://example.com/team/app.git\n\
    [remote \"fork\"]\n\turl = https://example.com/me/app.git\n\tpushurl = git@example.com:me/app.git\n\
    [remote \"secret\"]\n\turl = https://jane:s3cret-pw@example.com/team/app.git\n\
    [remote \"mirror\"]\n\turl = https://a.example/app.git\n\turl = https://b.example/app.git\n";

/// [`Ui::new`], but the window's root is gpui-kit's `Root` as in the app
/// (`gpui_kit::open_window`): it holds the overlay that shows the button tooltips.
fn rooted_ui(repo: &std::path::Path) -> Ui {
    let text = gpui_kit::platform::current_platform(true).text_system();
    let cx = TestAppContext::build_with_text_system(TestDispatcher::new(0), None, text);
    cx.update(gitti_app::init);
    let mut ws = None;
    let window = cx.update(|cx| {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(1500.), px(950.)));
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            focus: false,
            ..Default::default()
        };
        let repo = repo.to_path_buf();
        let (window, w) = gpui_kit::open_window(options, cx, |window, cx| {
            testing::workspace(repo, window, cx)
        })
        .unwrap();
        ws = Some(w);
        window
    });
    let mut ui = Ui {
        cx,
        window,
        ws: ws.unwrap(),
        repo: repo.to_path_buf(),
        fails: Vec::new(),
        frames: 0,
        last_step: "open".into(),
        dump: false,
        last_frame: Vec::new(),
    };
    ui.settle();
    ui
}

/// (tooltip views made visible so far, visible ones up now, a menu is open)
fn tooltips(ui: &mut Ui) -> (usize, usize, bool) {
    ui.cx.update(|cx| testing::tooltips(cx))
}

/// Moves the mouse (no button) to `at`, then lets `ms` of (test) time pass.
fn hover_at(ui: &mut Ui, at: Point<Pixels>, ms: u64) {
    ui.cx
        .update_window(ui.window, |_, window, cx| {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: at,
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                cx,
            );
        })
        .unwrap();
    wait(ui, ms);
}

/// Lets `ms` of test time pass in a few chunks (not a frame per 50 ms — the timers
/// fire either way, a frame after each chunk shows what they changed).
fn wait(ui: &mut Ui, ms: u64) {
    let mut left = ms;
    let mut chunk = 50;
    while left > 0 {
        let step = chunk.min(left);
        ui.cx.executor().advance_clock(Duration::from_millis(step));
        ui.cx.run_until_parked();
        ui.render();
        left -= step;
        chunk = (chunk * 3).max(150);
    }
}

/// The pointer leaves the window (the platform's exit event; no last move), from `at`.
fn exit_window(ui: &mut Ui, at: Point<Pixels>) {
    ui.cx
        .update_window(ui.window, |_, window, cx| {
            window.dispatch_event(
                PlatformInput::MouseExited(MouseExitEvent {
                    position: at,
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                cx,
            );
        })
        .unwrap();
}

/// The conflicted file's "Resolve Conflicts" split button: its main part has a button
/// tooltip (gpui-component's managed overlay), ▾ opens a dropdown menu.
fn split_button_dropdown(ui: &mut Ui) {
    let p = ui.frame();
    let first = |prefix: &str| {
        p.iter()
            .find(|q| q.name.starts_with(prefix))
            .and_then(Probe::visible)
    };
    let (Some(main), Some(split)) = (first("resolve:"), first("resolve-split:")) else {
        let names: Vec<String> = p.into_iter().map(|p| p.name).collect();
        fail!(ui, "no Resolve Conflicts split button: {names:?}");
        return;
    };
    let main_at = main.center();
    // ▾: the split button's part right of the main button.
    let arrow_at = point((main.right() + split.right()) / 2., split.center().y);
    hover_at(ui, main_at, 800);
    let (shown, _, open) = tooltips(ui);
    if shown == 0 || open {
        fail!(ui, "no button tooltip on hover: shown {shown}, menu {open}");
    }
    hover_at(ui, point(px(700.), px(400.)), 600);
    // Open the dropdown, then hover the main button (and wait) while it is open.
    hover_at(ui, arrow_at, 50);
    ui.cx
        .update_window(ui.window, |_, window, cx| {
            for down in [true, false] {
                let event = if down {
                    PlatformInput::MouseDown(MouseDownEvent {
                        button: MouseButton::Left,
                        position: arrow_at,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                        first_mouse: false,
                    })
                } else {
                    PlatformInput::MouseUp(MouseUpEvent {
                        button: MouseButton::Left,
                        position: arrow_at,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                    })
                };
                window.dispatch_event(event, cx);
            }
        })
        .unwrap();
    wait(ui, 300);
    let (before, _, open) = tooltips(ui);
    if !open {
        fail!(ui, "▾ opened no dropdown menu");
    }
    hover_at(ui, main_at, 900);
    hover_at(ui, arrow_at, 900);
    let (after, live, open) = tooltips(ui);
    if !open || after != before || live != 0 {
        fail!(
            ui,
            "button tooltip while the dropdown is open: shown {before} → {after}, up {live}"
        );
    }
    ui.step("key:escape");
    hover_at(ui, point(px(700.), px(400.)), 600);
    let (_, _, open) = tooltips(ui);
    if open {
        fail!(ui, "the dropdown is still open after Escape");
    }
    hover_at(ui, main_at, 800);
    let (again, _, _) = tooltips(ui);
    if again == after {
        fail!(ui, "no button tooltip after the dropdown closed");
    }
    // Away, back on the button, out of the window while the delay runs.
    hover_at(ui, point(px(700.), px(400.)), 600);
    hover_at(ui, main_at, 100);
    exit_window(ui, main_at);
    let again = tooltips(ui).0;
    wait(ui, 900);
    let (later, live, _) = tooltips(ui);
    if later != again || live != 0 {
        fail!(
            ui,
            "button tooltip after the pointer left the window: shown {again} → {later}, up {live}"
        );
    }
}

/// The centre of the probe `target` in the current frame.
fn center(ui: &mut Ui, target: &str) -> Option<Point<Pixels>> {
    let p = ui.frame();
    find(&p, target)
        .and_then(Probe::visible)
        .map(|b| b.center())
}

/// A right click at `at` (press + release), without waiting.
fn right_click_at(ui: &mut Ui, at: Point<Pixels>) {
    ui.cx
        .update_window(ui.window, |_, window, cx| {
            for down in [true, false] {
                let event = if down {
                    PlatformInput::MouseDown(MouseDownEvent {
                        button: MouseButton::Right,
                        position: at,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                        first_mouse: false,
                    })
                } else {
                    PlatformInput::MouseUp(MouseUpEvent {
                        button: MouseButton::Right,
                        position: at,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                    })
                };
                window.dispatch_event(event, cx);
            }
        })
        .unwrap();
}

/// Sidebar worktree rows have gpui tooltips; a right click on a worktree row while the
/// delay runs opens its context menu and no tooltip comes up over it, nor over other
/// elements while it is open; closed, tooltips as before; one that is up goes when a menu
/// opens; every context menu counts.
fn context_menu(ui: &mut Ui) {
    let (Some(one), Some(two)) = (center(ui, "side:wt-wt-one"), center(ui, "side:wt-wt-two"))
    else {
        let names: Vec<String> = ui.frame().into_iter().map(|p| p.name).collect();
        let side: Vec<&String> = names.iter().filter(|n| n.starts_with("side:")).collect();
        fail!(ui, "no worktree rows in the sidebar: {side:?}");
        return;
    };
    // Normal: a hover shows the tooltip after its delay.
    hover_at(ui, one, 700);
    let (shown, live, open) = tooltips(ui);
    if shown == 0 || live != 1 || open {
        fail!(
            ui,
            "hover without a menu: shown {shown}, up {live}, menu {open}"
        );
    }
    // Away, back, and a right click while the delay is still running: the menu opens and
    // the tooltip must not come up over it.
    hover_at(ui, point(px(700.), px(400.)), 300);
    hover_at(ui, one, 100);
    let before = tooltips(ui).0;
    right_click_at(ui, one);
    wait(ui, 900);
    let (shown, live, open) = tooltips(ui);
    if !open {
        fail!(ui, "right click opened no context menu");
    }
    if shown != before || live != 0 {
        fail!(
            ui,
            "tooltip over the open context menu: shown {before} → {shown}, up {live}"
        );
    }
    // Moving over other elements with tooltips while the menu is open: none either.
    hover_at(ui, two, 900);
    hover_at(ui, one, 900);
    let (shown2, live, open) = tooltips(ui);
    if !open || shown2 != shown || live != 0 {
        fail!(
            ui,
            "tooltip while the menu is open: shown {shown} → {shown2}, up {live}, menu {open}"
        );
    }
    // Closed: tooltips as before (a fresh hover, its delay).
    ui.step("key:escape");
    let (_, _, open) = tooltips(ui);
    if open {
        fail!(ui, "the menu is still open after Escape");
    }
    hover_at(ui, point(px(700.), px(400.)), 300);
    hover_at(ui, two, 100);
    if tooltips(ui).1 != 0 {
        fail!(ui, "a tooltip without its delay after the menu closed");
    }
    wait(ui, 600);
    let (shown3, live, _) = tooltips(ui);
    if shown3 == shown2 || live != 1 {
        fail!(
            ui,
            "no tooltip after the menu closed: shown {shown2} → {shown3}, up {live}"
        );
    }
    // A tooltip that is up when a menu opens goes away.
    right_click_at(ui, two);
    wait(ui, 300);
    let (_, live, open) = tooltips(ui);
    if !open || live != 0 {
        fail!(
            ui,
            "tooltip stayed up when the menu opened: up {live}, menu {open}"
        );
    }
    ui.step("key:escape");
    // Every other context menu counts as open too (same gate): branches, commits, file
    // tabs, diff rows, sidebar sections.
    let file_row = ui
        .frame()
        .into_iter()
        .find(|p| p.name.starts_with("diff-row:") && p.name.ends_with(":file"))
        .map_or_else(String::new, |p| p.name);
    for target in [
        "file-tab:1",
        file_row.as_str(),
        "side:ref-refs/heads/main",
        "side-section:branches",
        "commit-row:1",
    ] {
        let Some(at) = center(ui, target) else {
            fail!(ui, "no {target} to right click");
            continue;
        };
        right_click_at(ui, at);
        wait(ui, 300);
        if !tooltips(ui).2 {
            fail!(ui, "right click on {target}: no menu counted as open");
        }
        ui.step("key:escape");
        wait(ui, 100);
        if tooltips(ui).2 {
            fail!(ui, "the menu of {target} still counts as open after Escape");
        }
    }
}

/// No tooltip after the pointer left the window, on a gpui tooltip (sidebar worktree
/// row): the pointer leaves the window while
/// the delay runs → none comes up; one that is up goes away when it leaves; the same for a
/// deactivated window; back inside, tooltips as before.
fn left_window(ui: &mut Ui) {
    let (Some(one), Some(two)) = (center(ui, "side:wt-wt-one"), center(ui, "side:wt-wt-two"))
    else {
        fail!(ui, "no worktree rows in the sidebar");
        return;
    };
    let away = point(px(700.), px(400.));
    // Leaves while the delay runs.
    hover_at(ui, one, 100);
    let before = tooltips(ui).0;
    exit_window(ui, one);
    wait(ui, 900);
    let (shown, live, _) = tooltips(ui);
    if shown != before || live != 0 {
        fail!(
            ui,
            "tooltip after the pointer left the window: shown {before} → {shown}, up {live}"
        );
    }
    // Back inside: a fresh hover shows one after its delay.
    hover_at(ui, away, 300);
    hover_at(ui, two, 700);
    let (shown2, live, _) = tooltips(ui);
    if shown2 == shown || live != 1 {
        fail!(
            ui,
            "no tooltip after coming back into the window: shown {shown} → {shown2}, up {live}"
        );
    }
    // A tooltip that is up goes away when the pointer leaves.
    exit_window(ui, two);
    wait(ui, 300);
    if tooltips(ui).1 != 0 {
        fail!(ui, "the tooltip stayed up after the pointer left the window");
    }
    // Window deactivated while the delay runs: none either.
    hover_at(ui, away, 300);
    ui.cx
        .update_window(ui.window, |_, window, _| window.activate_window())
        .unwrap();
    wait(ui, 100);
    hover_at(ui, one, 100);
    let before = tooltips(ui).0;
    VisualTestContext::from_window(ui.window, &ui.cx).deactivate_window();
    wait(ui, 900);
    let (shown3, live, _) = tooltips(ui);
    if shown3 != before || live != 0 {
        fail!(
            ui,
            "tooltip in the deactivated window: shown {before} → {shown3}, up {live}"
        );
    }
    // The pointer moves in it again: tooltips as before.
    hover_at(ui, away, 300);
    hover_at(ui, two, 700);
    let (shown4, live, _) = tooltips(ui);
    if shown4 == shown3 || live != 1 {
        fail!(
            ui,
            "no tooltip after moving in the window again: shown {shown3} → {shown4}, up {live}"
        );
    }
}

/// A remote's sidebar row shows its URL on hover — fetch and push URLs apart when they
/// differ, every push URL, credentials masked — and the hover runs no git.
fn remote_url(ui: &mut Ui) {
    let cases = [
        ("origin", "https://example.com/team/app.git"),
        ("fork", "Fetch: https://example.com/me/app.git\nPush: git@example.com:me/app.git"),
        ("secret", "https://jane:***@example.com/team/app.git"),
        (
            "mirror",
            "Fetch: https://a.example/app.git\nPush: https://a.example/app.git\nPush: https://b.example/app.git",
        ),
    ];
    let away = point(px(700.), px(400.));
    let git = ui.n_git();
    for (name, want) in cases {
        let Some(at) = center(ui, &format!("side:remote:{name}")) else {
            let names: Vec<String> = ui.frame().into_iter().map(|p| p.name).collect();
            let side: Vec<&String> = names.iter().filter(|n| n.starts_with("side:")).collect();
            fail!(ui, "no sidebar row of the remote {name}: {side:?}");
            return;
        };
        hover_at(ui, away, 300);
        hover_at(ui, at, 700);
        let (_, live, _) = tooltips(ui);
        let text = ui.cx.update(|cx| testing::last_tooltip_text(cx));
        if live != 1 || text.as_deref() != Some(want) {
            fail!(ui, "hover on {name}: up {live}, tooltip {text:?}, want {want:?}");
        }
    }
    ui.no_git_since(git, "hovering the remotes");
}
