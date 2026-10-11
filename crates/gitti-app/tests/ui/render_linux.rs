//! Rendering tests beyond `layout.rs` (Linux test push): the Linux window chrome
//! (in-window menu bar, self-drawn title bar controls) across window × font sizes and its
//! menus' dropdowns, the fonts the OS text system resolves, Unicode (CJK, emoji with ZWJ
//! sequences, combining marks, RTL, tabs, zero-width characters, very long lines) in every
//! list, and scale factors 1 / 1.5 / 2. Linux-only scenarios are behind
//! `cfg(target_os = "linux")`; the others run on every OS.
//!
//! Own `main` like `layout.rs`: `cargo test --workspace --test ui -- render_linux[::FILTER]`;
//! `LAYOUT_DUMP=1` prints the probes of every checked frame.
// Some helpers are used only by the unix-only scenarios.
#![cfg_attr(not(unix), allow(dead_code, unused_macros))]

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
        ("menu_bar_dropdowns", menu_bar_dropdowns),
        ("unicode_worktree_and_wrap", unicode_worktree_and_wrap),
        ("unicode_commits", unicode_commits),
        ("scale_factors", scale_factors),
        (
            "commit_view_rescaled_scrolled",
            commit_view_rescaled_scrolled,
        ),
        ("unicode_sidebar", unicode_sidebar),
        ("unicode_message", unicode_message),
        ("unicode_staging", unicode_staging),
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


fn f(p: Pixels) -> f32 {
    f32::from(p)
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

// ------------------------------------------------------------------ fonts

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
                        // Its strip scrolls: at least a good part of it is on screen (40 px:
                        // 800 px at UI font 28 leave 45 next to the tab list's ▾, which
                        // also reaches every tab).
                        let shown = q.visible().map_or(0., |v| f(v.size.width));
                        if shown < f(b.size.width).min(40.) {
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


// ------------------------------------------------------------------ Unicode

/// A commit view scrolled past its header while the window size and the fonts change:
/// every frame keeps the invariants. Before the fix, `HeaderSpaced` shifted the list by the
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
/// Workflow: CM-20
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
/// Workflow: ST-21
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

/// Rounding slack (as `layout_check::TOL`).
const TOL: f32 = 1.5;

fn window_rect(size: (f32, f32)) -> Bounds<Pixels> {
    Bounds::new(
        point(px(0.), px(0.)),
        gpui_kit::size(px(size.0), px(size.1)),
    )
}

// ------------------------------------------------------------------ lean scenarios

/// The representative (window size, fonts) points of a sweep: the default and the most
/// cramped with the biggest fonts (the whole matrix with `GITTI_TEST_FULL=1`).
fn points() -> Vec<((f32, f32), (f32, f32))> {
    let all: Vec<((f32, f32), (f32, f32))> = SIZES
        .iter()
        .flat_map(|s| FONTS.iter().map(move |f| (*s, *f)))
        .collect();
    let default = all.iter().position(|p| *p == (SIZES[0], FONTS[1])).unwrap();
    common::sweep(&all, &[default, all.len() - 1])
}

/// Back to the default window and fonts (settings persist between the scenarios).
fn reset_point(ui: &mut Ui) {
    ui.resize(SIZES[0].0, SIZES[0].1);
    ui.step("fonts:13,13");
}

fn set_point(ui: &mut Ui, (size, (font_ui, font_diff)): ((f32, f32), (f32, f32))) -> String {
    ui.resize(size.0, size.1);
    ui.step(&format!("fonts:{font_ui},{font_diff}"));
    format!("{}x{} fonts {font_ui}/{font_diff}", size.0, size.1)
}

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
            if family.as_deref() != Some(mono) {
                out.push(format!("mono font {mono:?} resolves to {family:?}"));
            }
            for size in [11., 32.] {
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
            match ts.advance(ui_id, px(13.), 'M') {
                Ok(a) if f(a.width) > 0. => {}
                other => out.push(format!("UI font {ui_family:?}: advance of M {other:?}")),
            }
            // A family that does not exist (a typo in a theme / settings) still draws.
            let missing = ts.resolve_font(&font("gitti-no-such-font-family"));
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

/// The open menu: its index in the menu bar (the `AppMenu` element id after
/// `app-menu-bar` in the popup's element path), its popup and its items (gpui-component
/// observes both: role `Menu`, role `MenuItem`).
fn open_popup(ui: &mut Ui) -> Option<(usize, Bounds<Pixels>, Vec<Bounds<Pixels>>)> {
    ui.cx
        .update_window(ui.window, |_, window, _| {
            let all = gpui_kit::base::test_support::snapshots(window);
            let popup = all.iter().find(|s| s.role() == Some(Role::Menu) && s.visible())?;
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

/// Every menu of the in-window menu bar (forced on macOS with `GITTI_MENU_BAR`) opens — a
/// click on the first, then → walks the bar as a user does — and its dropdown with all its
/// items lies inside the window; a menu scrolls only when the window has no room for all of
/// it. Default and the cramped window with the biggest fonts.
fn menu_bar_dropdowns(ui: &mut Ui) {
    let mut expected: Vec<(String, usize)> = Vec::new();
    for (menu, _, _) in gitti_app::testing_menus::menu_items() {
        match expected.last_mut() {
            Some((m, n)) if *m == menu => *n += 1,
            _ => expected.push((menu, 1)),
        }
    }
    for pt in points() {
        let ctx = set_point(ui, pt);
        let win = window_rect(pt.0);
        let p = ui.frame();
        let Some(bar) = find(&p, "menu-bar").map(|q| q.bounds) else {
            fail!(ui, "{ctx}: no menu bar");
            continue;
        };
        let start = point(bar.left() + px(12.), bar.center().y);
        ui.mouse(MouseButton::Left, Some(true), start);
        ui.mouse(MouseButton::Left, Some(false), start);
        let mut seen: Vec<(usize, Bounds<Pixels>, Vec<Bounds<Pixels>>)> = Vec::new();
        for _ in 0..expected.len() {
            match open_popup(ui) {
                Some(open) if !seen.iter().any(|s| s.0 == open.0) => seen.push(open),
                Some(_) => {}
                None => {
                    fail!(ui, "{ctx}: no menu open after {} menus", seen.len());
                    break;
                }
            }
            ui.stroke("right");
        }
        ui.stroke("escape");
        if open_popup(ui).is_some() {
            fail!(ui, "{ctx}: Escape left the menu open");
            ui.stroke("escape");
        }
        if seen.len() != expected.len() {
            fail!(ui, "{ctx}: {} menus opened along the bar, {} expected", seen.len(), expected.len());
        }
        for (i, popup, items) in &seen {
            let name = expected.get(*i).map_or("?", |e| e.0.as_str());
            let want = expected.get(*i).map_or(0, |e| e.1);
            if !testing::contains(&win, popup) {
                fail!(ui, "{ctx}: menu {name} {popup:?} does not fit the window");
            }
            if items.len() < want {
                fail!(ui, "{ctx}: menu {name} shows {} items, {want} expected", items.len());
            }
            // A menu scrolls only when the window has no room for all of it.
            let content = items.iter().map(|b| f(b.size.height)).sum::<f32>();
            let clipped = items.iter().filter(|b| !testing::contains(popup, b)).count();
            if clipped > 0 && content + 120. < f(win.size.height) {
                fail!(ui, "{ctx}: menu {name} scrolls ({clipped} items cut) although the window has room");
            }
            let out = items
                .iter()
                .filter(|b| testing::contains(popup, b) && !testing::contains(&win, b))
                .count();
            if out > 0 {
                fail!(ui, "{ctx}: menu {name}: {out} of {} items outside the window", items.len());
            }
        }
    }
    reset_point(ui);
}

/// The diff text of row `N` (`diff-text:N`) ends inside its row: a wrapped part fits the
/// panel. CJK and emoji glyphs are wider than an advance of the diff font;
/// `gitti_core::rows::wrap_ranges` counts them as two columns.
fn check_text_fits(ui: &mut Ui, p: &[Probe], ctx: &str) {
    for t in p.iter().filter(|q| q.kind() == "diff-text") {
        let Some(ix) = t.index() else { continue };
        let Some(row) = p.iter().find(|q| q.kind() == "diff-row" && q.index() == Some(ix)) else {
            continue;
        };
        if row.visible().is_none() {
            continue;
        }
        let over = f(t.bounds.right()) - f(row.bounds.right());
        if over > TOL {
            fail!(ui, "{ctx}: {} text ends {over} px right of its row", row.name);
        }
    }
}

/// Columns of a commit row (subject, author, date) are drawn inside their row.
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
        if f(c.bounds.right()) > f(r.bounds.right()) + TOL || f(c.bounds.left()) + TOL < f(r.bounds.left()) {
            fail!(ui, "{ctx}: {} {:?} is not inside {row} {:?}", c.name, c.bounds, r.bounds);
        }
    }
}

/// The working directory of the Unicode repository (CJK, emoji, RTL, tabs, zero-width
/// characters), wrapped and not, keeps the invariants with the file tabs inside their
/// strip; wrapped, every part of a CJK / emoji line ends inside its row and the long
/// unbroken ASCII line (1190 chars) is cut into as many parts as its length needs.
fn unicode_worktree_and_wrap(ui: &mut Ui) {
    ui.step("select:0");
    let tabs = ui.cx.update(|cx| testing::worktree_tabs(&ui.ws, cx)).0;
    if tabs.len() < 7 {
        fail!(ui, "file tabs {tabs:?}");
    }
    for pt in points() {
        let ctx = set_point(ui, pt);
        for wrap in [false, true] {
            set_wrap(ui, wrap);
            let ctx = format!("{ctx} wrap {wrap}");
            let p = ui.check(&ctx);
            for t in p.iter().filter(|q| q.kind() == "file-tab") {
                match (t.visible(), find(&p, "file-tabs").and_then(Probe::visible)) {
                    (None, _) => {}
                    (Some(v), Some(s)) if testing::contains(&s, &v) => {}
                    other => fail!(ui, "{ctx}: {} outside its strip: {other:?}", t.name),
                }
            }
            if wrap {
                check_text_fits(ui, &p, &ctx);
            }
        }
        // Wrapped: long.txt's added line at the top, its parts counted.
        let rows = diff_rows(ui);
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
        let row_h = ui.metrics().diff_row_h;
        for _ in 0..2 {
            let y = ui.metrics().diff_scroll_y;
            ui.wheel("diff-list", first as f32 * row_h - y);
        }
        let p = ui.check(&format!("{ctx}, long.txt"));
        check_text_fits(ui, &p, &ctx);
        let chars = "x0123456789abcdef".len() * 70;
        let text = find(&p, &format!("diff-text:{first}")).map(|t| f(t.bounds.size.width));
        let font_diff = (pt.1).1;
        let adv = ui
            .cx
            .update(|cx| {
                let ts = cx.text_system().clone();
                ts.advance(ts.resolve_font(&font(testing::mono_font())), px(font_diff), 'm')
            })
            .map(|a| f(a.width))
            .unwrap_or(0.);
        match text {
            Some(w) if adv > 0. => {
                let per_row = (w / adv).round() as usize;
                let want = chars.div_ceil(per_row.max(1));
                if parts.len() != want {
                    fail!(ui, "{ctx}: long.txt in {} parts, {want} expected ({per_row} per row)", parts.len());
                }
            }
            _ => fail!(ui, "{ctx}: no diff-text:{first} for long.txt (advance {adv})"),
        }
        // Down through the jp / emoji lines.
        for i in 0..3 {
            ui.wheel("diff-list", 400.);
            let p = ui.check(&format!("{ctx}, scrolled {i}"));
            check_text_fits(ui, &p, &ctx);
        }
        ui.wheel("diff-list", -1e6);
        set_wrap(ui, false);
    }
    reset_point(ui);
}

/// The commit list with Unicode subjects and authors (RTL, ZWJ emoji, combining marks, a
/// tab, a bidi override, a 250-char subject without spaces): subject, author and date
/// inside their rows; each commit's view (message header) drawn, scrolled.
fn unicode_commits(ui: &mut Ui) {
    for pt in points() {
        let ctx = set_point(ui, pt);
        for row in 1..=3 {
            ui.step(&format!("select:{row}"));
            let ctx = format!("{ctx} commit {row}");
            let p = ui.check(&ctx);
            check_commit_columns(ui, &p, &ctx);
            for i in 1..=3 {
                if find(&p, &format!("commit-subject:{i}")).and_then(Probe::visible).is_none() {
                    fail!(ui, "{ctx}: commit-subject:{i} not shown");
                }
            }
            if find(&p, "msg-line:0").is_none() {
                fail!(ui, "{ctx}: no message header");
            }
        }
        ui.wheel("diff-list", 240.);
        ui.check(&format!("{ctx}, commit 3 scrolled"));
        ui.step("select:0");
    }
    reset_point(ui);
}

/// The same window at scale factors 1, 1.5 and 2 (HiDPI): invariants hold, and the logical
/// layout of the main regions is the same at every scale (within a pixel).
fn scale_factors(ui: &mut Ui) {
    const NAMES: [&str; 7] =
        ["tab-bar", "toolbar", "sidebar", "commit-list", "detail", "diff-list", "status-bar"];
    ui.step("fonts:13,13");
    for (size, select) in [(SIZES[0], "select:0"), (SIZES[2], "select:1")] {
        ui.resize(size.0, size.1);
        ui.step(select);
        let mut base: Option<Vec<Option<Bounds<Pixels>>>> = None;
        for scale in [2.0, 1.0, 1.5] {
            ui.cx.simulate_window_scale_factor_change(ui.window, scale);
            ui.settle();
            let ctx = format!("{}x{} scale {scale} {select}", size.0, size.1);
            let p = ui.check(&ctx);
            let seen = ui.cx.update_window(ui.window, |_, window, _| window.scale_factor()).unwrap();
            if (seen - scale).abs() > 0.01 {
                fail!(ui, "{ctx}: window scale factor is {seen}");
            }
            let now: Vec<Option<Bounds<Pixels>>> = NAMES.iter().map(|n| find(&p, n).map(|q| q.bounds)).collect();
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
            ui.wheel("diff-list", 240.);
            ui.check(&format!("{ctx}, scrolled"));
            // Back to the top (the commit box grows back), the next scale compares the same.
            ui.wheel("diff-list", -1e6);
            ui.wheel("diff-list", -1e6);
        }
    }
    ui.cx.simulate_window_scale_factor_change(ui.window, 2.0);
    ui.settle();
}
