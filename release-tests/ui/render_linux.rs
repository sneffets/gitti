//! Rendering tests beyond `layout.rs` (Linux test push): the Linux window chrome
//! (in-window menu bar, self-drawn title bar controls) across window × font sizes and its
//! menus' dropdowns, the fonts the OS text system resolves, Unicode (CJK, emoji with ZWJ
//! sequences, combining marks, RTL, tabs, zero-width characters, very long lines) in every
//! list, and scale factors 1 / 1.5 / 2. Linux-only scenarios are behind
//! `cfg(target_os = "linux")`; the others run on every OS.
//!
//! Own `main` like `layout.rs`: `cargo test --workspace --test ui -- render_linux[::FILTER]`;
//! `LAYOUT_DUMP=1` prints the probes of every checked frame.

use crate::common;

use std::path::{Path, PathBuf};

use common::*;

pub fn main() {
    // The test platform reports server-side decorations; on GNOME / Wayland gitti draws
    // its own title bar controls, so force that path (`draws_window_frame`). The menu bar
    // is in the window on Linux anyway (`in_window_menu_bar`).
    #[cfg(target_os = "linux")]
    // SAFETY: still single-threaded.
    unsafe {
        std::env::set_var("GITTI_WINDOW_FRAME", "1")
    };
    let tmp = common::setup();
    let repo = make_repo(&tmp.path().join("repo"));
    let uni = make_unicode_repo(&tmp.path().join("unicode"));
    #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
    let mut tests: Vec<(&str, Scenario)> = vec![
        ("fonts_resolve", fonts_resolve),
        ("unicode_worktree", unicode_worktree),
        ("unicode_wrap_fit", unicode_wrap_fit),
        ("unicode_commits", unicode_commits),
        (
            "commit_view_rescaled_scrolled",
            commit_view_rescaled_scrolled,
        ),
        ("unicode_sidebar", unicode_sidebar),
        ("unicode_message", unicode_message),
        ("unicode_staging", unicode_staging),
        ("scale_factors", scale_factors),
        ("menu_bar_dropdowns", menu_bar_dropdowns),
    ];
    #[cfg(target_os = "linux")]
    tests.extend([
        ("linux_chrome_sweep", linux_chrome_sweep as Scenario),
        ("linux_font_fallback", linux_font_fallback),
    ]);
    common::run(&tests, |name| {
        // The in-window menu bar (Windows / Linux) also on macOS, for its menus only.
        #[cfg(not(target_os = "linux"))]
        // SAFETY: only this suite's own threads run (watchdog, settings writer), none
        // reads the environment outside std's lock.
        unsafe {
            if name == "menu_bar_dropdowns" {
                std::env::set_var("GITTI_MENU_BAR", "1");
            } else {
                std::env::remove_var("GITTI_MENU_BAR");
            }
        }
        if name.starts_with("unicode") || name == "scale_factors" {
            uni.clone()
        } else {
            repo.clone()
        }
    });
}

/// Window sizes (logical px), as in `layout.rs`.
const SIZES: [(f32, f32); 3] = [(1500., 950.), (1000., 640.), (800., 500.)];

/// UI / diff font sizes up to the maximum (`Settings::zoom` clamps at 28 / 32).
const FONTS: &[(f32, f32)] = &[(11., 11.), (13., 13.), (20., 22.), (28., 32.)];

/// The sweeps' window sizes: default and the most cramped (all with `GITTI_TEST_FULL=1`).
fn sizes() -> Vec<(f32, f32)> {
    common::sweep(&SIZES, &[0, SIZES.len() - 1])
}

/// The sweeps' font sizes: default and the biggest (all with `GITTI_TEST_FULL=1`).
fn fonts() -> Vec<(f32, f32)> {
    common::sweep(FONTS, &[1, FONTS.len() - 1])
}

/// Rounding slack (as `layout_check::TOL`).
const TOL: f32 = 1.5;

fn f(p: Pixels) -> f32 {
    f32::from(p)
}

fn window_rect(size: (f32, f32)) -> Bounds<Pixels> {
    Bounds::new(
        point(px(0.), px(0.)),
        gpui_kit::size(px(size.0), px(size.1)),
    )
}

/// `ui.fail(format!(…))` with the message built first (its arguments may use `ui`).
macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

// ------------------------------------------------------------------ Unicode repository

/// Lines with CJK, emoji (ZWJ family, skin tone, flag), RTL and combining marks.
const JP: &str = "日本語のテキスト：設計書の第一章です。";
const EMOJI: &str = "👩‍👩‍👧‍👦 family, 👍🏽 tone, 🇩🇪 flag, 🎉 party, 👩‍💻 coder";
const RTL: &str = "שלום עולם — مرحبا بالعالم 123 abc";
/// q with dot below + dot above: no precomposed form, the same in NFC and NFD (macOS file
/// systems normalise names).
const COMB: &str = "q\u{323}\u{307}";

/// A repository whose names, contents, commits (subjects, authors) and branches are full of
/// non-ASCII text, with unstaged, staged and untracked changes in the working directory.
fn make_unicode_repo(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "Layout Test"]);
    git(dir, &["config", "user.email", "layout@example.com"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    git(dir, &["config", "core.autocrlf", "false"]);
    let write = |name: &str, text: &str| {
        let p = dir.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    let lines =
        |prefix: &str, n: usize| -> String { (0..n).map(|i| format!("{prefix} {i}\n")).collect() };
    write("日本語/設計書.txt", &lines(JP, 30));
    write("emoji 👩‍👩‍👧‍👦 🎉.md", &lines(EMOJI, 12));
    write("עברית مرحبا.txt", &lines(RTL, 12));
    write(
        &format!("{COMB} combining zoë.txt"),
        &lines(&format!("{COMB}{COMB} zoë"), 8),
    );
    write(
        "zero\u{200b}width\u{200d}name.txt",
        "zero\u{200b}width\u{200d}joiner\u{feff}bom\u{ad}soft\n",
    );
    write("tabs.tsv", "a\tb\tc\n\tindented\t日本\t👍🏽\n");
    write("long.txt", "short\n");
    git(dir, &["add", "."]);
    let commit = |author: &str, msg: &str| {
        git(dir, &["commit", "-q", "--author", author, "-m", msg]);
    };
    commit(
        "山田 太郎 👩‍💻 <yamada@example.com>",
        "初期コミット：日本語のファイル名 👩‍👩‍👧‍👦 שלום עולם مرحبا",
    );
    for b in [
        "feature/日本語-ブランチ",
        "emoji/🚀-launch-👩‍👩‍👧‍👦",
        "rtl/שלום-مرحبا",
        &format!("combining/{COMB}-zoë"),
        "a-very-long-branch-name-日本語-with-many-words-and-👍🏽-that-never-ends-at-all",
    ] {
        git(dir, &["branch", b]);
    }
    write("emoji 👩‍👩‍👧‍👦 🎉.md", &lines(&format!("{EMOJI} v2"), 12));
    git(dir, &["add", "."]);
    let long_subject = "Ein_sehr_langer_Betreff_ohne_Leerzeichen_".repeat(6);
    commit(
        "مُحَمَّد عَبْدُ ٱللَّٰه <m@example.com>",
        &format!("{long_subject}\n\nBody with 中文 and 👍🏽 and עברית\n\ttabbed line\n"),
    );
    write("tabs.tsv", "a\tb\tc\n\tindented\t日本\t👍🏽 v2\n");
    git(dir, &["add", "."]);
    commit(
        &format!("Zoë Ångström {COMB} <zoe@example.com>"),
        "Tab\tin subject, zero\u{200b}width\u{200d}joiner and \u{202e}RTL override\u{202c} 🇩🇪",
    );
    // Working directory: unstaged changes everywhere (a long CJK line and a long unbroken
    // ASCII line among them), one staged file, one untracked file.
    let mut jp = lines(JP, 30);
    jp = jp.replace(&format!("{JP} 3\n"), &format!("{JP} 3 変更\n"));
    jp = jp.replace(
        &format!("{JP} 17\n"),
        &format!("{}\n", "漢字仮名交じり文".repeat(30)),
    );
    jp.push_str(&format!("{}\n", "👩‍👩‍👧‍👦👍🏽🇩🇪🎉".repeat(25)));
    write("日本語/設計書.txt", &jp);
    write("long.txt", &format!("{}\n", "x0123456789abcdef".repeat(70)));
    write(
        "עברית مرحبا.txt",
        &lines(RTL, 12).replace(&format!("{RTL} 5\n"), &format!("{RTL} 5 שונה تغيير\n")),
    );
    write("tabs.tsv", "a\tb\tc\n\tindented\t日本\t👍🏽 v3\n\t\t\tdeep\n");
    write(
        &format!("{COMB} combining zoë.txt"),
        &lines(&format!("{COMB}{COMB} zoë"), 8).replace(" 2\n", " 2 é\u{301}\n"),
    );
    git(dir, &["add", &format!("{COMB} combining zoë.txt")]);
    write("新しい ファイル 🆕.txt", &lines("新規", 4));
    dir.to_path_buf()
}

// ------------------------------------------------------------------ helpers

/// The diff text of row `N` (`diff-text:N`, recorded by `text_sel::DragTargets::register`)
/// ends inside its row: a wrapped part fits the panel. CJK and emoji glyphs are wider than
/// an advance of the diff font; `gitti_core::rows::wrap_ranges` counts them as two columns
/// (it used to count chars, and wrapped CJK rows ran up to 325 px past the panel).
fn check_text_fits(ui: &mut Ui, p: &[Probe], ctx: &str, rows: &[String]) {
    for t in p.iter().filter(|q| q.kind() == "diff-text") {
        let Some(ix) = t.index() else { continue };
        let Some(row) = p
            .iter()
            .find(|q| q.kind() == "diff-row" && q.index() == Some(ix))
        else {
            continue;
        };
        if row.visible().is_none() {
            continue;
        }
        let over = f(t.bounds.right()) - f(row.bounds.right());
        if over > TOL {
            let what = rows.get(ix).map_or("?", String::as_str);
            fail!(
                ui,
                "{ctx}: {} ({what}) text ends {over} px right of its row",
                row.name
            );
        }
    }
}

/// Columns of a commit row (subject, author, date) are drawn inside their row: none pushed
/// out of the commit list by a long subject or author.
fn check_commit_columns(ui: &mut Ui, p: &[Probe], ctx: &str) {
    for c in p
        .iter()
        .filter(|q| matches!(q.kind(), "commit-subject" | "commit-author" | "commit-date"))
    {
        let row = format!("commit-row:{}", c.index().unwrap_or(0));
        let Some(r) = find(p, &row) else { continue };
        if r.visible().is_none() {
            continue;
        }
        if f(c.bounds.right()) > f(r.bounds.right()) + TOL
            || f(c.bounds.left()) + TOL < f(r.bounds.left())
        {
            fail!(
                ui,
                "{ctx}: {} {:?} is not inside {row} {:?}",
                c.name,
                c.bounds,
                r.bounds
            );
        }
    }
}

/// Every diff row kind in the active tab's diff (`state.rows`), e.g.
/// `line Unstaged long.txt Added`.
fn diff_rows(ui: &mut Ui) -> Vec<String> {
    let ws = ui.ws.clone();
    ui.cx
        .update_window(ui.window, |_, window, cx| testing::state(&ws, window, cx))
        .unwrap()
        .rows
}

fn set_wrap(ui: &mut Ui, on: bool) {
    let ws = ui.ws.clone();
    let now = ui
        .cx
        .update_window(ui.window, |_, window, cx| testing::state(&ws, window, cx))
        .unwrap()
        .wrap_lines;
    if now != on {
        ui.step("action:gitti::ToggleWrap");
    }
}

/// Scrolls the diff list from the top to its end (and back), checking every frame and
/// `extra` on each.
fn scroll_through(ui: &mut Ui, ctx: &str, extra: &dyn Fn(&mut Ui, &[Probe], &str)) {
    for i in 0..40 {
        let before = ui.metrics().diff_scroll_y;
        let p = ui.check(&format!("{ctx}, scrolled {i}"));
        extra(ui, &p, &format!("{ctx}, scrolled {i}"));
        ui.wheel("diff-list", 160.);
        if i > 0 && ui.metrics().diff_scroll_y == before {
            break;
        }
    }
    for _ in 0..40 {
        ui.wheel("diff-list", -400.);
        if ui.metrics().diff_scroll_y <= 0. {
            break;
        }
    }
}

// ------------------------------------------------------------------ fonts

/// The fonts gitti asks for resolve to themselves, the diff font is monospace (equal
/// advances for `i` and `M`, equal widths of `iiii` and `MMMM`), text measures > 0, and an
/// unknown family still resolves to something that draws.
fn fonts_resolve(ui: &mut Ui) {
    let mono = testing::mono_font();
    let report = ui
        .cx
        .update_window(ui.window, |_, window, cx| {
            let mut out: Vec<String> = Vec::new();
            let ts = cx.text_system().clone();
            let mono_id = ts.resolve_font(&font(mono));
            let family = ts.get_font_for_id(mono_id).map(|f| f.family.to_string());
            println!("    mono font {mono:?} resolves to {family:?}");
            if family.as_deref() != Some(mono) {
                out.push(format!("mono font {mono:?} resolves to {family:?}"));
            }
            for size in [11., 13., 32.] {
                let (i, m) = (
                    ts.advance(mono_id, px(size), 'i').map(|s| f(s.width)),
                    ts.advance(mono_id, px(size), 'M').map(|s| f(s.width)),
                );
                match (i, m) {
                    (Ok(i), Ok(m)) if i > 0. && (i - m).abs() < 0.01 => {}
                    other => out.push(format!("{mono} at {size}: advances i / M {other:?}")),
                }
                let run = |len: usize| TextRun {
                    len,
                    font: font(mono),
                    color: Hsla::default(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let width = |s: &str| {
                    f(window
                        .text_system()
                        .shape_line(s.to_string().into(), px(size), &[run(s.len())], None)
                        .width)
                };
                let (thin, wide) = (width("iiii"), width("MMMM"));
                if thin <= 0. || (thin - wide).abs() > 0.05 {
                    out.push(format!("{mono} at {size}: iiii {thin} px, MMMM {wide} px"));
                }
            }
            let ui_family = testing::ui_font_family(cx);
            let ui_id = ts.resolve_font(&font(ui_family.clone()));
            let resolved = ts.get_font_for_id(ui_id).map(|f| f.family.to_string());
            println!("    UI font {ui_family:?} resolves to {resolved:?}");
            match ts.advance(ui_id, px(13.), 'M') {
                Ok(a) if f(a.width) > 0. => {}
                other => out.push(format!("UI font {ui_family:?}: advance of M {other:?}")),
            }
            // A family that does not exist (a typo in a theme / settings) still draws.
            let missing = ts.resolve_font(&font("gitti-no-such-font-family"));
            let fallback = ts.get_font_for_id(missing).map(|f| f.family.to_string());
            println!("    a missing family falls back to {fallback:?}");
            match ts.advance(missing, px(13.), 'M') {
                Ok(a) if f(a.width) > 0. => {}
                other => out.push(format!("missing family: advance of M {other:?}")),
            }
            out
        })
        .unwrap();
    for r in report {
        ui.fail(r);
    }
}

/// Linux (fontconfig): CJK, Hebrew, Arabic and emoji text set in the diff font draws real
/// glyphs — from the diff font (DejaVu Sans Mono has Hebrew and Arabic) or a fallback font
/// (CJK, emoji) — never the missing glyph (glyph 0, a box), for every script whose fonts
/// fontconfig reports installed.
#[cfg(target_os = "linux")]
fn linux_font_fallback(ui: &mut Ui) {
    let installed = |lang: &str| {
        std::process::Command::new("fc-list")
            .args([format!(":lang={lang}"), "family".into()])
            .output()
            .is_ok_and(|o| !o.stdout.is_empty())
    };
    let samples = [
        ("ja", "日本語の漢字"),
        ("he", "שלום עולם"),
        ("ar", "مرحبا بالعالم"),
        ("und-zsye", "👩‍👩‍👧‍👦👍🏽🎉"),
    ];
    let mono = testing::mono_font();
    for (lang, text) in samples {
        if !installed(lang) {
            println!("    no {lang} font installed (fc-list), skipped");
            continue;
        }
        let r = ui
            .cx
            .update_window(ui.window, |_, window, cx| {
                let ts = cx.text_system().clone();
                let mono_id = ts.resolve_font(&font(mono));
                let run = TextRun {
                    len: text.len(),
                    font: font(mono),
                    color: Hsla::default(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let layout = window
                    .text_system()
                    .layout_line(text, px(13.), &[run], None);
                let fonts: Vec<String> = layout
                    .runs
                    .iter()
                    .filter_map(|r| ts.get_font_for_id(r.font_id).map(|f| f.family.to_string()))
                    .collect();
                let tofu = layout
                    .runs
                    .iter()
                    .any(|r| r.glyphs.iter().any(|g| g.id.0 == 0));
                let only_mono = layout.runs.iter().all(|r| r.font_id == mono_id);
                (f(layout.width), fonts, tofu, only_mono)
            })
            .unwrap();
        let (width, fonts, tofu, only_mono) = r;
        let from = if only_mono {
            "the diff font itself"
        } else {
            "a fallback font"
        };
        println!("    {lang}: {width} px, glyphs from {from} {fonts:?}");
        if width <= 0. || tofu {
            fail!(
                ui,
                "{lang} {text:?}: width {width}, fonts {fonts:?}, missing glyphs {tofu}"
            );
        }
    }
}

// ------------------------------------------------------------------ Linux chrome

/// The in-window menu bar and the title bar controls (minimize / maximize / close, the drag
/// area) at every window × font size: present, whole, inside the window and the tab bar,
/// left to right without overlapping the repository tabs, the controls at the right edge;
/// the content below stays inside the window.
#[cfg(target_os = "linux")]
fn linux_chrome_sweep(ui: &mut Ui) {
    for size in sizes() {
        ui.resize(size.0, size.1);
        let win = window_rect(size);
        for (font_ui, font_diff) in fonts() {
            ui.step(&format!("fonts:{font_ui},{font_diff}"));
            for row in [0, 1] {
                ui.step(&format!("select:{row}"));
                let ctx = format!(
                    "{}x{} fonts {font_ui}/{font_diff}, row {row}",
                    size.0, size.1
                );
                let p = ui.check(&ctx);
                let Some(bar) = find(&p, "tab-bar").map(|q| q.bounds) else {
                    fail!(ui, "{ctx}: no tab bar");
                    continue;
                };
                // Left to right; the drag area may shrink, everything else stays whole.
                let order = [
                    "menu-bar",
                    "repo-tab:0",
                    "open-repo",
                    "window-drag",
                    "window-controls",
                ];
                let mut prev: Option<(&str, Bounds<Pixels>)> = None;
                for name in order {
                    let Some(q) = find(&p, name) else {
                        fail!(ui, "{ctx}: no {name}");
                        continue;
                    };
                    let b = q.bounds;
                    if name == "repo-tab:0" {
                        // Its strip scrolls: at least a good part of it is on screen.
                        let shown = q.visible().map_or(0., |v| f(v.size.width));
                        if shown < f(b.size.width).min(48.) {
                            fail!(ui, "{ctx}: only {shown} px of {name} {b:?} on screen");
                        }
                        prev = Some((name, q.visible().unwrap_or(b)));
                        continue;
                    }
                    if name != "window-drag" && q.visible() != Some(b) {
                        fail!(
                            ui,
                            "{ctx}: {name} {b:?} is not whole on screen ({:?})",
                            q.visible()
                        );
                    }
                    if !testing::contains(&win, &b) {
                        fail!(ui, "{ctx}: {name} {b:?} is outside the window");
                    }
                    if !testing::contains(&bar, &b) {
                        fail!(ui, "{ctx}: {name} {b:?} is outside the tab bar {bar:?}");
                    }
                    if let Some((pn, pb)) = prev
                        && f(pb.right()) > f(b.left()) + TOL
                    {
                        fail!(
                            ui,
                            "{ctx}: {pn} (right {}) runs into {name} (left {})",
                            f(pb.right()),
                            f(b.left())
                        );
                    }
                    prev = Some((name, b));
                }
                // The controls end at the window's right edge, the three buttons inside them.
                if let Some(c) = find(&p, "window-controls")
                    && (f(c.bounds.right()) - size.0).abs() > 2.
                {
                    fail!(
                        ui,
                        "{ctx}: the window controls end at {}, not at the right edge",
                        f(c.bounds.right())
                    );
                }
                for button in ["window-minimize", "window-maximize", "window-close"] {
                    match (find(&p, button), find(&p, "window-controls")) {
                        (Some(b), Some(c))
                            if b.visible() == Some(b.bounds)
                                && testing::contains(&c.bounds, &b.bounds)
                                && f(b.bounds.size.width) >= 16. => {}
                        (b, _) => fail!(
                            ui,
                            "{ctx}: {button} not whole in the controls: {:?}",
                            b.map(|b| b.bounds)
                        ),
                    }
                }
                // Something to grab the window by (at least a button's width).
                match find(&p, "window-drag") {
                    Some(d) if f(d.bounds.size.width) >= 32. - TOL => {}
                    d => fail!(
                        ui,
                        "{ctx}: drag area {:?} too small to grab",
                        d.map(|d| d.bounds.size.width)
                    ),
                }
                for name in ["toolbar", "status-bar", "commit-list", "detail"] {
                    match find(&p, name).and_then(Probe::visible) {
                        Some(v) if testing::contains(&win, &v) => {}
                        v => fail!(ui, "{ctx}: {name} {v:?} not inside the window"),
                    }
                }
            }
        }
    }
    ui.step("fonts:13,13");
}

/// Mouse helpers that keep no button pressed.
fn hover(ui: &mut Ui, position: Point<Pixels>) {
    ui.cx
        .update_window(ui.window, |_, window, cx| {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position,
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                cx,
            );
        })
        .unwrap();
    ui.settle();
}

/// The open menu: its index in the menu bar (the `AppMenu` element id after
/// `app-menu-bar` in the popup's element path), its popup and its items (gpui-component
/// observes both: role `Menu`, role `MenuItem`).
fn open_popup(ui: &mut Ui) -> Option<(usize, Bounds<Pixels>, Vec<Bounds<Pixels>>)> {
    ui.render();
    ui.cx
        .update_window(ui.window, |_, window, _| {
            let all = gpui_kit::base::test_support::snapshots(window);
            let popup = all
                .iter()
                .find(|s| s.role() == Some(Role::Menu) && s.visible())?;
            let path = popup.path();
            let bar = path
                .iter()
                .position(|e| matches!(e, ElementId::Name(n) if n == "app-menu-bar"))?;
            let ix = path[bar..].iter().find_map(|e| match e {
                ElementId::Integer(i) => Some(*i as usize),
                _ => None,
            })?;
            let items = all
                .iter()
                .filter(|s| s.role() == Some(Role::MenuItem) && s.path().starts_with(path))
                .map(|s| s.bounds())
                .collect();
            Some((ix, popup.bounds(), items))
        })
        .unwrap()
}

/// Every menu is scrollable with the window's height as its cap (`menu_height.rs`):
/// a menu taller than the window (the View menu at 800×500) scrolls instead of running out.
const TALL_MENUS_SCROLL: bool = true;

/// Every menu of the in-window menu bar (forced on macOS with `GITTI_MENU_BAR`) opens (click on the first, then hovering along the
/// bar switches menus) and its dropdown with all its items lies inside the window — at the
/// biggest and the smallest window, normal and biggest fonts.
fn menu_bar_dropdowns(ui: &mut Ui) {
    let mut expected: Vec<(String, usize)> = Vec::new();
    for (menu, _, _) in gitti_app::testing_menus::menu_items() {
        match expected.last_mut() {
            Some((m, n)) if *m == menu => *n += 1,
            _ => expected.push((menu, 1)),
        }
    }
    for size in [SIZES[0], SIZES[2]] {
        ui.resize(size.0, size.1);
        let win = window_rect(size);
        for (font_ui, font_diff) in [(13., 13.), (28., 32.)] {
            ui.step(&format!("fonts:{font_ui},{font_diff}"));
            let ctx = format!("{}x{} fonts {font_ui}/{font_diff}", size.0, size.1);
            let p = ui.frame();
            let Some(bar) = find(&p, "menu-bar").map(|q| q.bounds) else {
                fail!(ui, "{ctx}: no menu bar");
                continue;
            };
            let y = bar.center().y;
            let mut seen: Vec<(usize, Bounds<Pixels>, Vec<Bounds<Pixels>>)> = Vec::new();
            // Hovering along the bar switches the open menu (as a user does)…
            let start = point(bar.left() + px(12.), y);
            ui.mouse(MouseButton::Left, Some(true), start);
            ui.mouse(MouseButton::Left, Some(false), start);
            let mut x = f(bar.left()) + 4.;
            while x < f(bar.right()) - 2. {
                hover(ui, point(px(x), y));
                if let Some(open) = open_popup(ui)
                    && !seen.iter().any(|s| s.0 == open.0)
                {
                    seen.push(open);
                }
                x += 10.;
            }
            ui.step("key:escape");
            let hovered = seen.len();
            // …unless a menu too tall for the window covers the bar: then a click on each
            // menu with none open.
            let mut x = f(bar.left()) + 4.;
            while seen.len() < expected.len() && x < f(bar.right()) - 2. {
                let at = point(px(x), y);
                ui.mouse(MouseButton::Left, Some(true), at);
                ui.mouse(MouseButton::Left, Some(false), at);
                if let Some(open) = open_popup(ui)
                    && !seen.iter().any(|s| s.0 == open.0)
                {
                    seen.push(open);
                }
                if open_popup(ui).is_some() {
                    ui.step("key:escape");
                }
                x += 6.;
            }
            if hovered < seen.len() {
                println!(
                    "    {ctx}: hovering along the menu bar reached {hovered} of {} menus (a tall one covers the bar)",
                    seen.len()
                );
            }
            seen.sort_by_key(|s| s.0);
            ui.step("key:escape");
            if open_popup(ui).is_some() {
                fail!(ui, "{ctx}: Escape left the menu open");
                ui.step("key:escape");
            }
            if seen.len() != expected.len() {
                fail!(
                    ui,
                    "{ctx}: {} menus opened along the bar, {} expected",
                    seen.len(),
                    expected.len()
                );
            }
            for (i, popup, items) in &seen {
                let i = *i;
                let name = expected.get(i).map_or("?", |e| e.0.as_str());
                let want = expected.get(i).map_or(0, |e| e.1);
                if !TALL_MENUS_SCROLL && popup.size.height > win.size.height {
                    // Known gap: taller than the whole window, cannot fit anywhere.
                    println!(
                        "    known gap: {ctx}: menu {name} is {:?} tall, the window {:?} \
                         (gpui-component scrolls only menus with > 20 entries)",
                        popup.size.height, win.size.height
                    );
                    continue;
                }
                if !testing::contains(&win, popup) {
                    fail!(ui, "{ctx}: menu {name} {popup:?} does not fit the window");
                }
                if items.len() < want {
                    fail!(
                        ui,
                        "{ctx}: menu {name} shows {} items, {want} expected",
                        items.len()
                    );
                }
                // A menu scrolls only when the window has no room for all of it.
                let content = items.iter().map(|b| f(b.size.height)).sum::<f32>();
                let clipped = items.iter().filter(|b| !testing::contains(popup, b)).count();
                if clipped > 0 && content + 120. < f(win.size.height) {
                    fail!(
                        ui,
                        "{ctx}: menu {name} scrolls ({clipped} items cut) although the window has room"
                    );
                }
                let out = items
                    .iter()
                    .filter(|b| testing::contains(popup, b) && !testing::contains(&win, b))
                    .count();
                if out > 0 {
                    fail!(
                        ui,
                        "{ctx}: menu {name}: {out} of {} items outside the window (popup {popup:?})",
                        items.len()
                    );
                }
                if f(popup.top()) + TOL < f(bar.center().y) {
                    // Too tall to open below the bar: moved up over it (it still fits).
                    println!(
                        "    {ctx}: menu {name} opened over the menu bar at {:?}",
                        popup.origin
                    );
                }
            }
        }
    }
    ui.step("fonts:13,13");
}

// ------------------------------------------------------------------ Unicode

/// The working directory of the Unicode repository (file rows, hunks, lines with CJK,
/// emoji, RTL, tabs, zero-width characters), wrapped and unwrapped, at every window × font
/// size, scrolled through: invariants on every frame, file tabs inside their strip.
fn unicode_worktree(ui: &mut Ui) {
    ui.step("select:0");
    let ws = ui.ws.clone();
    let tabs = ui.cx.update(|cx| testing::worktree_tabs(&ws, cx)).0;
    // All Changes + 7 files (staged combining one, 5 unstaged, the untracked one).
    if tabs.len() < 7 {
        fail!(ui, "file tabs {tabs:?}");
    }
    for wrap in [false, true] {
        set_wrap(ui, wrap);
        for size in sizes() {
            ui.resize(size.0, size.1);
            for (font_ui, font_diff) in fonts() {
                ui.step(&format!("fonts:{font_ui},{font_diff}"));
                let ctx = format!(
                    "{}x{} fonts {font_ui}/{font_diff} wrap {wrap}",
                    size.0, size.1
                );
                let p = ui.check(&ctx);
                for t in p.iter().filter(|q| q.kind() == "file-tab") {
                    match (t.visible(), find(&p, "file-tabs").and_then(Probe::visible)) {
                        (None, _) => {}
                        (Some(v), Some(s)) if testing::contains(&s, &v) => {}
                        other => fail!(ui, "{ctx}: {} outside its strip: {other:?}", t.name),
                    }
                }
                if size == SIZES[0] {
                    scroll_through(ui, &ctx, &|_, _, _| {});
                }
            }
        }
    }
    set_wrap(ui, false);
    ui.step("fonts:13,13");
}

/// Line wrap with wide characters: every wrapped part of a CJK / emoji / long ASCII line
/// ends inside its row (the panel width is measured in `m` advances of the diff font, a CJK
/// glyph is wider), and the long unbroken ASCII line is cut into as many parts as its
/// length needs — none lost, none extra.
fn unicode_wrap_fit(ui: &mut Ui) {
    ui.step("select:0");
    set_wrap(ui, true);
    for size in [SIZES[0], SIZES[2]] {
        ui.resize(size.0, size.1);
        for (font_ui, font_diff) in fonts() {
            ui.step(&format!("fonts:{font_ui},{font_diff}"));
            let ctx = format!("{}x{} fonts {font_ui}/{font_diff}", size.0, size.1);
            let rows = diff_rows(ui);
            scroll_through(ui, &ctx, &|ui, p, ctx| check_text_fits(ui, p, ctx, &rows));
            // long.txt: one removed line ("short"), one added line of 1190 chars.
            let parts: Vec<usize> = rows
                .iter()
                .enumerate()
                .filter(|(_, r)| *r == "line Unstaged long.txt Added")
                .map(|(i, _)| i)
                .collect();
            let Some(&first) = parts.first() else {
                fail!(ui, "{ctx}: no added line in long.txt: {rows:?}");
                continue;
            };
            // Bring the first part to the top: the rows are uniform.
            let row_h = ui.metrics().diff_row_h;
            let y = ui.metrics().diff_scroll_y;
            ui.wheel("diff-list", first as f32 * row_h - y);
            let p = ui.check(&format!("{ctx}, long.txt"));
            let chars = "x0123456789abcdef".len() * 70;
            let text = find(&p, &format!("diff-text:{first}")).map(|t| f(t.bounds.size.width));
            let adv = ui
                .cx
                .update(|cx| {
                    let ts = cx.text_system().clone();
                    ts.advance(
                        ts.resolve_font(&font(testing::mono_font())),
                        px(font_diff),
                        'm',
                    )
                })
                .map(|a| f(a.width))
                .unwrap_or(0.);
            match text {
                Some(w) if adv > 0. => {
                    let per_row = (w / adv).round() as usize;
                    let want = chars.div_ceil(per_row.max(1));
                    if parts.len() != want {
                        fail!(
                            ui,
                            "{ctx}: long.txt in {} parts, {want} expected ({per_row} per row)",
                            parts.len()
                        );
                    }
                }
                _ => fail!(
                    ui,
                    "{ctx}: no diff-text:{first} for long.txt (advance {adv})"
                ),
            }
        }
    }
    set_wrap(ui, false);
    ui.step("fonts:13,13");
}

/// The commit list with Unicode subjects and authors (RTL, ZWJ emoji, combining marks, a
/// tab, a bidi override, a 250-char subject without spaces) at every window × font size:
/// subject, author and date inside their rows; each commit's view (message header,
/// files) scrolled through.
fn unicode_commits(ui: &mut Ui) {
    for size in sizes() {
        ui.resize(size.0, size.1);
        for (font_ui, font_diff) in fonts() {
            ui.step(&format!("fonts:{font_ui},{font_diff}"));
            for row in 1..=3 {
                ui.step(&format!("select:{row}"));
                let ctx = format!(
                    "{}x{} fonts {font_ui}/{font_diff} commit {row}",
                    size.0, size.1
                );
                let p = ui.check(&ctx);
                check_commit_columns(ui, &p, &ctx);
                for i in 1..=3 {
                    let name = format!("commit-subject:{i}");
                    if find(&p, &name).and_then(Probe::visible).is_none() {
                        fail!(ui, "{ctx}: {name} not shown");
                    }
                }
                if find(&p, "msg-line:0").is_none() {
                    fail!(ui, "{ctx}: no message header");
                }
                for i in 0..4 {
                    ui.wheel("diff-list", 120.);
                    let p = ui.check(&format!("{ctx}, scrolled {i}"));
                    check_commit_columns(ui, &p, &ctx);
                }
            }
            // Fonts and window size change in the worktree view (see
            // `commit_view_rescaled_scrolled` for a scrolled commit view).
            ui.step("select:0");
        }
    }
    ui.step("fonts:13,13");
}

/// A commit view scrolled past its header while the window size and the fonts change:
/// every frame keeps the invariants. `HeaderSpaced` used to shift the list by the
/// header's row change also when the header was scrolled out of view, so one frame had an
/// empty strip at the top (rows "clipped inside diff-list"); now it scrolls by the change.
fn commit_view_rescaled_scrolled(ui: &mut Ui) {
    for size in sizes() {
        for (font_ui, font_diff) in fonts() {
            ui.resize(size.0, size.1);
            ui.step(&format!("fonts:{font_ui},{font_diff}"));
            ui.step("select:2");
            for _ in 0..6 {
                ui.wheel("diff-list", 120.);
            }
            ui.check(&format!(
                "{}x{} fonts {font_ui}/{font_diff}, scrolled",
                size.0, size.1
            ));
        }
    }
    ui.step("select:0");
    ui.step("fonts:13,13");
}

/// Branch names with CJK, emoji, RTL and combining marks in the sidebar: every one shown,
/// inside the sidebar (invariant), one below the other.
fn unicode_sidebar(ui: &mut Ui) {
    let names = [
        "feature/日本語-ブランチ".to_string(),
        "emoji/🚀-launch-👩‍👩‍👧‍👦".to_string(),
        "rtl/שלום-مرحبا".to_string(),
        format!("combining/{COMB}-zoë"),
    ];
    for size in sizes() {
        ui.resize(size.0, size.1);
        for (font_ui, font_diff) in fonts() {
            ui.step(&format!("fonts:{font_ui},{font_diff}"));
            let ctx = format!("{}x{} fonts {font_ui}/{font_diff}", size.0, size.1);
            let p = ui.check(&ctx);
            let mut rows: Vec<&Probe> = p
                .iter()
                .filter(|r| matches!(r.kind(), "side" | "side-section") && r.visible().is_some())
                .collect();
            rows.sort_by(|a, b| f(a.bounds.top()).total_cmp(&f(b.bounds.top())));
            for w in rows.windows(2) {
                if f(w[0].bounds.bottom()) > f(w[1].bounds.top()) + 0.5 {
                    fail!(ui, "{ctx}: {} overlaps {}", w[0].name, w[1].name);
                }
            }
            if size.1 > 600. && font_ui <= 13. {
                for n in &names {
                    let id = format!("side:ref-refs/heads/{n}");
                    if find(&p, &id).and_then(Probe::visible).is_none() {
                        fail!(ui, "{ctx}: branch {n:?} not shown in the sidebar");
                    }
                }
            }
        }
    }
    ui.step("fonts:13,13");
}

/// A commit message with CJK, emoji, RTL, combining marks and a long unbroken word in the
/// editor at every window × font size: the editor stays inside the commit box (invariant),
/// the Commit button whole, and the text comes back unchanged.
fn unicode_message(ui: &mut Ui) {
    let msg = format!(
        "修正：{JP} 👩‍👩‍👧‍👦\\n\\n{RTL}\\n{COMB}{COMB} zoë é\u{301}\\n{}\\nzero\u{200b}width",
        "長".repeat(120)
    );
    let want = msg.replace("\\n", "\n");
    let ws = ui.ws.clone();
    ui.step("select:0");
    for size in sizes() {
        ui.resize(size.0, size.1);
        for (font_ui, font_diff) in fonts() {
            ui.step(&format!("fonts:{font_ui},{font_diff}"));
            let ctx = format!("{}x{} fonts {font_ui}/{font_diff}", size.0, size.1);
            ui.step(&format!("msg:{msg}"));
            ui.step("focusmsg");
            let p = ui.check(&format!("{ctx}, editing"));
            match find(&p, "commit-button") {
                Some(b) if b.visible() == Some(b.bounds) => {}
                _ => fail!(ui, "{ctx}: the Commit button is not (fully) visible"),
            }
            if find(&p, "commit-editor").and_then(Probe::visible).is_none() {
                fail!(ui, "{ctx}: no commit editor");
            }
            let text = ui.cx.update(|cx| testing::commit_message(&ws, cx));
            if text != want {
                fail!(ui, "{ctx}: message came back as {text:?}");
            }
            ui.step("focuslist");
            ui.check(&format!("{ctx}, not editing"));
        }
    }
    ui.step("msg:");
    ui.step("fonts:13,13");
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// Line staging on Unicode lines (wrapped, so the line has several rows): selecting one
/// row of the long CJK line stages the whole line exactly; unstaging it puts it back; the
/// same for a line in the RTL file.
fn unicode_staging(ui: &mut Ui) {
    let dir = ui.repo.clone();
    ui.step("select:0");
    set_wrap(ui, true);
    let cjk = format!("+{}", "漢字仮名交じり文".repeat(30));
    let rtl = format!("+{RTL} 5 שונה تغيير");
    for (path, line) in [
        ("日本語/設計書.txt", cjk.as_str()),
        ("עברית مرحبا.txt", rtl.as_str()),
    ] {
        let rows = diff_rows(ui);
        let target = format!("line Unstaged {path} Added");
        // The added lines of the file: the long CJK line is the second one ("3 変更" first).
        let added: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| **r == target)
            .map(|(i, _)| i)
            .collect();
        let pick = if path.starts_with("日本語") {
            // A middle row of the wrapped CJK line (after the one-row "3 変更" line).
            added.get(2).copied()
        } else {
            added.first().copied()
        };
        let Some(ix) = pick else {
            fail!(ui, "{path}: no added rows in {rows:?}");
            continue;
        };
        ui.step(&format!("lines:{ix}-{ix}"));
        ui.step("stage");
        let cached = git_out(&dir, &["diff", "--cached", "--", path]);
        let added_cached: Vec<&str> = cached
            .lines()
            .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
            .collect();
        if added_cached != [line] {
            fail!(
                ui,
                "{path}: staged {added_cached:?}, expected exactly {line:?}"
            );
        }
        let unstaged = git_out(&dir, &["diff", "--", path]);
        if unstaged.lines().any(|l| l == line) {
            fail!(ui, "{path}: the staged line is still unstaged");
        }
        ui.check(&format!("{path} staged"));
        let rows = diff_rows(ui);
        let staged = format!("line Staged {path} Added");
        match rows.iter().position(|r| *r == staged) {
            Some(ix) => {
                ui.step(&format!("lines:{ix}-{ix}"));
                ui.step("unstage");
            }
            None => fail!(ui, "{path}: no staged row after staging: {rows:?}"),
        }
        let cached = git_out(&dir, &["diff", "--cached", "--", path]);
        if !cached.is_empty() {
            fail!(ui, "{path}: still staged after unstaging: {cached}");
        }
        ui.check(&format!("{path} unstaged"));
    }
    set_wrap(ui, false);
}

// ------------------------------------------------------------------ scale factors

/// The same window at scale factors 1, 1.5 and 2 (HiDPI): invariants hold, and the logical
/// layout of the main regions is the same at every scale (within a pixel).
fn scale_factors(ui: &mut Ui) {
    const NAMES: [&str; 7] = [
        "tab-bar",
        "toolbar",
        "sidebar",
        "commit-list",
        "detail",
        "diff-list",
        "status-bar",
    ];
    for (size, select) in [(SIZES[0], "select:0"), (SIZES[2], "select:1")] {
        ui.resize(size.0, size.1);
        ui.step(select);
        let mut base: Option<Vec<Option<Bounds<Pixels>>>> = None;
        for scale in [2.0, 1.0, 1.5, 2.0] {
            ui.cx.simulate_window_scale_factor_change(ui.window, scale);
            ui.settle();
            let ctx = format!("{}x{} scale {scale} {select}", size.0, size.1);
            let p = ui.check(&ctx);
            let seen = ui
                .cx
                .update_window(ui.window, |_, window, _| window.scale_factor())
                .unwrap();
            if (seen - scale).abs() > 0.01 {
                fail!(ui, "{ctx}: window scale factor is {seen}");
            }
            let now: Vec<Option<Bounds<Pixels>>> = NAMES
                .iter()
                .map(|n| find(&p, n).map(|q| q.bounds))
                .collect();
            match &base {
                None => base = Some(now),
                Some(b) => {
                    for (i, n) in NAMES.iter().enumerate() {
                        let same = match (b[i], now[i]) {
                            (Some(x), Some(y)) => {
                                (f(x.left()) - f(y.left())).abs() <= 1.
                                    && (f(x.top()) - f(y.top())).abs() <= 1.
                                    && (f(x.size.width) - f(y.size.width)).abs() <= 1.
                                    && (f(x.size.height) - f(y.size.height)).abs() <= 1.
                            }
                            (None, None) => true,
                            _ => false,
                        };
                        if !same {
                            fail!(ui, "{ctx}: {n} {:?}, at scale 2 {:?}", now[i], b[i]);
                        }
                    }
                }
            }
            for _ in 0..3 {
                ui.wheel("diff-list", 120.);
                ui.check(&format!("{ctx}, scrolled"));
            }
            // Back to the top (scrolling down collapsed the commit box; up there restores
            // it), so the next scale compares the same state.
            for _ in 0..6 {
                ui.wheel("diff-list", -200.);
            }
        }
    }
    ui.cx.simulate_window_scale_factor_change(ui.window, 2.0);
    ui.settle();
}
