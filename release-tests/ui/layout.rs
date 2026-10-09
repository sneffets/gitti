//! Headless UI layout tests: the real gitti views, drawn by gpui's test platform with the
//! OS text system (real font metrics), checked through layout probes (`src/probe.rs`)
//! against the invariants in `src/layout_check.rs` plus per-scenario expectations.
//!
//! Own `main` (no libtest harness): the platform text system must be created on the main
//! thread on macOS. `cargo test --workspace --test ui -- layout[::FILTER]`; `LAYOUT_DUMP=1`
//! prints the probes of every checked frame.

use crate::common;

use common::*;
use gitti_app::testing::SpellTarget;
use gitti_app::testing_menus;

pub fn main() {
    let tmp = common::setup();
    // Deterministic spell checking (`spell_marks`), whatever dictionaries the OS has.
    gitti_app::spell::use_word_list(&[("teh", &["the", "ten"]), ("wrnog", &["wrong"])]);
    let repo = make_repo(&tmp.path().join("repo"));
    let merge_repo = make_merge_repo(&tmp.path().join("merge"));
    let long_merge_repo = make_long_line_merge_repo(&tmp.path().join("merge-long"));
    let stash_conflict_repo = make_conflict_repo(&tmp.path().join("stash-conflict"), false);
    let merge_conflict_repo = make_conflict_repo(&tmp.path().join("merge-conflict"), true);
    let wt_repo = make_worktrees_repo(&tmp.path().join("wt"));
    let folder_repo = make_folder_repo(&tmp.path().join("folders"));
    let remotes_repo = make_remotes_repo(&tmp.path().join("remotes"));
    let tests: &[(&str, Scenario)] = &[
        ("worktree_sweep", worktree_sweep),
        ("worktree_file_tabs", worktree_file_tabs),
        ("file_tab_list", file_tab_list),
        ("repo_tabs_reorder_and_list", repo_tabs_reorder_and_list),
        ("commit_box_collapse", commit_box_collapse),
        ("commit_view_sweep", commit_view_sweep),
        ("compare_view_sweep", compare_view_sweep),
        ("merge_sync_scroll", merge_sync_scroll),
        ("merge_long_lines", merge_long_lines),
        ("stash_conflict_abort", stash_conflict_abort),
        ("merge_commit_flow", merge_commit_flow),
        ("askpass_key_icon", askpass_key_icon),
        ("right_click_selects", right_click_selects),
        ("drag_autoscroll", drag_autoscroll),
        ("sidebar_toggle", sidebar_toggle),
        ("sidebar_worktrees_sweep", sidebar_worktrees_sweep),
        ("spell_marks", spell_marks),
        ("spell_marks_dialogs", spell_marks_dialogs),
        ("go_to_commit_picker", go_to_commit_picker),
        ("folder_picker_sweep", folder_picker_sweep),
        ("panes_restore_after_resize", panes_restore_after_resize),
        ("panes_first_frame_after_resize", panes_first_frame_after_resize),
        ("toolbar_command_bar", toolbar_command_bar),
        ("palette_keys_column", palette_keys_column),
    ];
    common::run(tests, |name| {
        if name == "merge_sync_scroll" {
            merge_repo.clone()
        } else if name == "merge_long_lines" {
            long_merge_repo.clone()
        } else if name == "stash_conflict_abort" {
            stash_conflict_repo.clone()
        } else if name == "merge_commit_flow" {
            merge_conflict_repo.clone()
        } else if name == "sidebar_worktrees_sweep" {
            wt_repo.clone()
        } else if name == "folder_picker_sweep" {
            folder_repo.clone()
        } else if name == "toolbar_command_bar" {
            remotes_repo.clone()
        } else {
            repo.clone()
        }
    });
}

// ------------------------------------------------------------------ scenarios

/// Window sizes (logical px).
const SIZES: [(f32, f32); 3] = [(1500., 950.), (1000., 640.), (800., 500.)];

/// UI / diff font sizes, up to the maximum (`Settings::zoom` clamps at 28 / 32).
const FONTS: &[(f32, f32)] = &[(11., 11.), (13., 13.), (16., 17.), (20., 22.), (28., 32.)];

/// The sweeps' window sizes: default and the most cramped (all with `GITTI_TEST_FULL=1`).
fn sizes() -> Vec<(f32, f32)> {
    common::sweep(&SIZES, &[0, SIZES.len() - 1])
}

/// The sweeps' font sizes: default and the biggest (all with `GITTI_TEST_FULL=1`).
fn fonts() -> Vec<(f32, f32)> {
    common::sweep(FONTS, &[1, FONTS.len() - 1])
}

fn message(lines: usize) -> String {
    match lines {
        0 => String::new(),
        1 => "Fix the thing".into(),
        // Subject, blank line, body: `n` lines (`\\n` = newline in a `msg:` step).
        n => {
            let mut s = String::from("Subject line of a longer message");
            for i in 1..n {
                if i == 1 {
                    s.push_str("\\n");
                } else {
                    s.push_str(&format!("\\nbody line {i} with some words in it"));
                }
            }
            s
        }
    }
}

/// Worktree: every font size × message length × scroll position keeps the invariants; the
/// commit editor is as tall as its text (up to 12 lines) while editing.
fn worktree_sweep(ui: &mut Ui) {
    for size in sizes() {
        ui.resize(size.0, size.1);
        for (font_ui, font_diff) in fonts() {
            ui.step(&format!("fonts:{font_ui},{font_diff}"));
            for lines in common::sweep(&[0, 1, 5, 30], &[1, 3]) {
                let ctx = format!(
                    "{}x{} fonts {font_ui}/{font_diff}, {lines}-line message",
                    size.0, size.1
                );
                ui.step(&format!("msg:{}", message(lines)));
                ui.step("focusmsg");
                let p = ui.check(&format!("{ctx}, editing"));
                // The working directory's file tabs: all + big.txt, new.txt, small.rs
                // (their order against the commit box is an invariant).
                if find(&p, "file-tabs").is_none() || find(&p, "file-tab:3").is_none() {
                    ui.fail(format!("{ctx}: no file tabs above the commit box"));
                }
                let m = ui.metrics();
                // Whole lines of text (long ones wrap at big fonts) plus the editor's border
                // and padding: at least two lines for a longer message, more only while the
                // commit box takes at most half the panel. The Commit button stays reachable
                // however long the text.
                match (
                    find(&p, "commit-editor"),
                    find(&p, "commit-box"),
                    find(&p, "detail"),
                ) {
                    (Some(ed), Some(bx), Some(d)) => {
                        let h = f32::from(ed.bounds.size.height);
                        let chrome = (h - m.message_line_h).rem_euclid(m.message_line_h);
                        let rows = ((h - chrome) / m.message_line_h).round();
                        let (box_h, panel_h) = (
                            f32::from(bx.bounds.size.height),
                            f32::from(d.bounds.size.height),
                        );
                        if rows < lines.clamp(1, 2) as f32
                            || chrome > 20.
                            || (rows > 2. && box_h > panel_h / 2. + 1.)
                        {
                            ui.fail(format!(
                                "{ctx}: editor is {h} px tall ({rows} lines of {} px + {chrome}), box {box_h} of {panel_h} px",
                                m.message_line_h
                            ));
                        }
                    }
                    _ => ui.fail(format!("{ctx}: no commit editor / box / panel")),
                }
                match (find(&p, "commit-button"), find(&p, "detail")) {
                    (Some(b), Some(d))
                        if b.visible() == Some(b.bounds)
                            && testing::contains(&d.bounds, &b.bounds) => {}
                    _ => ui.fail(format!("{ctx}: the Commit button is not (fully) visible")),
                }
                // The identity line stays one line (cut with an ellipsis): wrapped, it took
                // the Commit button's room (800x500 at the biggest fonts on Windows).
                match find(&p, "commit-identity") {
                    Some(id) if f32::from(id.bounds.size.height) < 1.5 * m.message_line_h => {}
                    Some(id) => ui.fail(format!(
                        "{ctx}: the identity line wraps ({} px tall)",
                        f32::from(id.bounds.size.height)
                    )),
                    None => ui.fail(format!("{ctx}: no identity line")),
                }
                ui.step("focuslist");
                ui.step("select:0");
                for i in 0..6 {
                    ui.wheel("diff-list", 90.);
                    ui.check(&format!("{ctx}, scrolled down {}", i + 1));
                }
                for i in 0..6 {
                    ui.wheel("diff-list", -90.);
                    ui.check(&format!("{ctx}, scrolled back {}", i + 1));
                }
            }
        }
    }
}

/// A file tab of the working directory active, at every window size × font size and
/// scrolled: tabs, commit box and the filtered diff stay in that order without overlap
/// (invariants), the active tab is drawn whole inside its strip, and the diff shows only
/// the tab's file.
fn worktree_file_tabs(ui: &mut Ui) {
    ui.step("select:0");
    ui.click_on("file-tab:1", MouseButton::Left);
    let ws = ui.ws.clone();
    let active = ui.cx.update(|cx| testing::worktree_tabs(&ws, cx)).1;
    if active.as_deref() != Some("big.txt") {
        ui.fail(format!("clicking file-tab:1 made {active:?} active, expected big.txt"));
    }
    let files = ui
        .cx
        .update_window(ui.window, |_, window, cx| testing::state(&ws, window, cx))
        .unwrap()
        .rows
        .into_iter()
        .filter(|r| r.starts_with("file "))
        .collect::<Vec<_>>();
    if files != ["file Unstaged big.txt"] {
        ui.fail(format!("big.txt tab shows {files:?}"));
    }
    for size in sizes() {
        ui.resize(size.0, size.1);
        for (font_ui, font_diff) in fonts() {
            ui.step(&format!("fonts:{font_ui},{font_diff}"));
            let ctx = format!("{}x{} fonts {font_ui}/{font_diff}, big.txt tab", size.0, size.1);
            let p = ui.check(&ctx);
            // (Narrow panels scroll the strip sideways; the tab is in it, at least partly.)
            match (find(&p, "file-tabs"), find(&p, "file-tab:1")) {
                (Some(strip), Some(tab))
                    if tab.visible().is_some()
                        && strip.visible().is_some_and(|s| testing::contains(&s, &tab.visible().unwrap())) => {}
                _ => ui.fail(format!("{ctx}: the active tab is not drawn in its strip")),
            }
            for i in 0..4 {
                ui.wheel("diff-list", 120.);
                ui.check(&format!("{ctx}, scrolled down {}", i + 1));
            }
            for i in 0..4 {
                ui.wheel("diff-list", -120.);
                ui.check(&format!("{ctx}, scrolled back {}", i + 1));
            }
        }
    }
    ui.click_on("file-tab:0", MouseButton::Left);
    let active = ui.cx.update(|cx| testing::worktree_tabs(&ws, cx)).1;
    if active.is_some() {
        ui.fail(format!("All Changes left {active:?} active"));
    }
}

/// The ▾ at the right edge of the file tabs opens the list of every tab under
/// it (not under the command bar), at every size × font inside the window; typing narrows
/// it, Enter shows that tab. Same in the commit view.
fn file_tab_list(ui: &mut Ui) {
    let ws = ui.ws.clone();
    let picker = |ui: &mut Ui| {
        ui.cx
            .update_window(ui.window, |_, window, cx| testing::state(&ws, window, cx))
            .unwrap()
            .picker
    };
    ui.step("select:0");
    for size in sizes() {
        ui.resize(size.0, size.1);
        for (font_ui, font_diff) in fonts() {
            ui.step(&format!("fonts:{font_ui},{font_diff}"));
            let ctx = format!("{}x{} fonts {font_ui}/{font_diff}", size.0, size.1);
            let p = ui.check(&format!("{ctx}, file tabs"));
            match (find(&p, "file-tabs-menu").and_then(Probe::visible), find(&p, "file-tabs")) {
                (Some(m), Some(bar))
                    if (f32::from(m.right()) - f32::from(bar.bounds.right())).abs() < 2. => {}
                other => ui.fail(format!("{ctx}: no ▾ at the right edge of the file tabs: {other:?}")),
            }
            ui.click_on("file-tabs-menu", MouseButton::Left);
            let p = ui.check(&format!("{ctx}, file list open"));
            let labels = picker(ui).map(|p| p.labels).unwrap_or_default();
            if labels.len() != 4 || !labels[0].starts_with("All Changes") {
                ui.fail(format!("{ctx}: the ▾ list: {labels:?}"));
            }
            let (Some(panel), Some(menu)) = (
                find(&p, "picker").and_then(Probe::visible),
                find(&p, "file-tabs-menu").and_then(Probe::visible),
            ) else {
                ui.fail(format!("{ctx}: no file list drawn"));
                ui.step("key:escape");
                continue;
            };
            let window = size_of_window(&p);
            // Under the ▾ (or moved up only as far as the window needs), right-aligned
            // with it unless the window pushed it in, and always inside the window.
            if f32::from(panel.top()) + 1.5 < f32::from(menu.bottom())
                && f32::from(panel.bottom()) + 8.5 < window.1
            {
                ui.fail(format!("{ctx}: the list starts above the ▾ ({panel:?}, ▾ {menu:?})"));
            }
            if (f32::from(panel.right()) - f32::from(menu.right())).abs() > 2.
                && f32::from(panel.left()) > 8.5
            {
                ui.fail(format!("{ctx}: the list is not right-aligned with the ▾ ({panel:?}, ▾ {menu:?})"));
            }
            if f32::from(panel.left()) < -0.5 || f32::from(panel.right()) > window.0 + 0.5 {
                ui.fail(format!("{ctx}: the list leaves the window ({panel:?})"));
            }
            ui.step("key:escape");
        }
    }
    // Type to narrow, Enter shows the tab; no git.
    ui.resize(1500., 950.);
    ui.step("fonts:13,13");
    let before = ui.git_commands().len();
    ui.click_on("file-tabs-menu", MouseButton::Left);
    ui.step("pick:new");
    match picker(ui) {
        Some(p) if p.labels.first().map(String::as_str) == Some("new.txt") => {}
        other => ui.fail(format!("typing `new` into the file list: {other:?}")),
    }
    ui.step("pickok");
    let active = ui.cx.update(|cx| testing::worktree_tabs(&ws, cx)).1;
    if active.as_deref() != Some("new.txt") || picker(ui).is_some() {
        ui.fail(format!("Enter on new.txt: active tab {active:?}"));
    }
    let cmds = ui.git_commands();
    if cmds.len() != before {
        ui.fail(format!("the file list ran git: {:?}", &cmds[before..]));
    }
    // Opened again, the active tab is selected.
    ui.click_on("file-tabs-menu", MouseButton::Left);
    match picker(ui) {
        Some(p) if p.labels.get(p.selected).map(String::as_str) == Some("new.txt") => {}
        other => ui.fail(format!("reopened list does not select the active tab: {other:?}")),
    }
    ui.step("key:escape");
    ui.click_on("file-tab:0", MouseButton::Left);

    // The commit view (newest commit: f0, f1, f2).
    ui.step("select:1");
    ui.click_on("file-tabs-menu", MouseButton::Left);
    match picker(ui) {
        Some(p) if p.labels.len() == 4 && p.labels[0] == "All Files (3)" => {}
        other => ui.fail(format!("commit file list: {other:?}")),
    }
    ui.step("pick:f1");
    ui.step("pickok");
    let rows = ui
        .cx
        .update_window(ui.window, |_, window, cx| testing::state(&ws, window, cx))
        .unwrap()
        .rows
        .into_iter()
        .filter(|r| r.starts_with("file "))
        .collect::<Vec<_>>();
    if rows != ["file Commit f1.txt"] {
        ui.fail(format!("commit file list → f1.txt shows {rows:?}"));
    }
}

/// The window's (width, height) from the probes (the tab bar spans it).
fn size_of_window(p: &[Probe]) -> (f32, f32) {
    let w = find(p, "tab-bar").map_or(0., |t| f32::from(t.bounds.right()));
    let h = find(p, "status-bar").map_or(0., |t| f32::from(t.bounds.bottom()));
    (w, h)
}

/// Repository tabs move by drag and drop (the dragged tab takes the place of
/// the one it is dropped on; the active one stays active), the order is saved with the
/// open tabs; the ▾ at the right edge of the tab bar lists every repository and switches.
fn repo_tabs_reorder_and_list(ui: &mut Ui) {
    let base = ui.repo.parent().unwrap().to_path_buf();
    let mut paths = vec![gitti_core::paths::canonical(&ui.repo)];
    for name in ["tabs-b", "tabs-c"] {
        let dir = base.join(name);
        if !dir.exists() {
            git(&base, &["init", "-q", "-b", "main", name]);
        }
        paths.push(gitti_core::paths::canonical(&dir));
        ui.step(&format!("open:{}", dir.display()));
    }
    let ws = ui.ws.clone();
    let tabs = |ui: &mut Ui| ui.cx.update(|cx| (testing::tab_paths(&ws, cx), testing::tabs(&ws, cx).1));
    let (now, active) = tabs(ui);
    if now != paths || active != 2 {
        ui.fail(format!("three tabs opened: {now:?}, active {active}"));
    }
    let p = ui.check("three tabs");
    match (find(&p, "repo-tabs-menu").and_then(Probe::visible), find(&p, "tab-bar")) {
        (Some(m), Some(bar)) if f32::from(bar.bounds.right()) - f32::from(m.right()) < 12. => {}
        other => ui.fail(format!("no ▾ at the right edge of the tab bar: {other:?}")),
    }
    // Drag the first tab onto the last.
    let center = |p: &[Probe], n: &str| find(p, n).and_then(Probe::visible).map(|b| b.center());
    let (Some(from), Some(to)) = (center(&p, "repo-tab:0"), center(&p, "repo-tab:2")) else {
        ui.fail("no repo tabs drawn".into());
        return;
    };
    ui.mouse(MouseButton::Left, Some(true), from);
    ui.mouse(MouseButton::Left, None, point(from.x + px(12.), from.y));
    ui.mouse(MouseButton::Left, None, to);
    ui.check("dragging a tab");
    ui.mouse(MouseButton::Left, Some(false), to);
    let (now, active) = tabs(ui);
    let want = vec![paths[1].clone(), paths[2].clone(), paths[0].clone()];
    if now != want || active != 1 {
        ui.fail(format!("after dragging tab 0 onto tab 2: {now:?}, active {active} (want {want:?}, 1)"));
    }
    let (saved, saved_active) = gitti_core::store::load_tabs();
    if saved != want || saved_active != 1 {
        ui.fail(format!("saved order {saved:?}, active {saved_active}"));
    }
    ui.check("after the drag");
    // The ▾: the first entry (now tabs-b) switches.
    ui.click_on("repo-tabs-menu", MouseButton::Left);
    ui.check("repository list open");
    ui.step("key:down");
    ui.step("key:enter");
    let (_, active) = tabs(ui);
    if active != 0 {
        ui.fail(format!("the ▾ list's first entry made tab {active} active"));
    }
}

/// Scrolling down shrinks the commit box to exactly one bold line (the summary), scrolling
/// back up at the top restores it; focusing the editor brings the whole box back.
fn commit_box_collapse(ui: &mut Ui) {
    for (font_ui, font_diff) in fonts() {
        ui.step(&format!("fonts:{font_ui},{font_diff}"));
        let ctx = format!("fonts {font_ui}/{font_diff}");
        ui.step(&format!("msg:{}", message(5)));
        ui.step("focuslist");
        ui.step("select:0");
        let full = ui.frame();
        let full_h = find(&full, "commit-box").map(|b| f32::from(b.bounds.size.height));
        for _ in 0..12 {
            ui.wheel("diff-list", 60.);
        }
        let p = ui.check(&format!("{ctx}, collapsed"));
        let m = ui.metrics();
        match (find(&p, "commit-box"), find(&p, "commit-summary")) {
            (Some(b), Some(s)) => {
                let (bh, sh) = (
                    f32::from(b.bounds.size.height),
                    f32::from(s.bounds.size.height),
                );
                if (bh - m.commit_box_min_h).abs() > 2. {
                    ui.fail(format!(
                        "{ctx}: collapsed box is {bh} px, expected {}",
                        m.commit_box_min_h
                    ));
                }
                if sh < m.message_line_h {
                    ui.fail(format!(
                        "{ctx}: summary {sh} px is lower than a message line ({})",
                        m.message_line_h
                    ));
                }
            }
            (b, s) => ui.fail(format!(
                "{ctx}: collapsed state: commit-box {}, commit-summary {}",
                b.is_some(),
                s.is_some()
            )),
        }
        for _ in 0..40 {
            ui.wheel("diff-list", -60.);
        }
        let p = ui.check(&format!("{ctx}, expanded again"));
        let back_h = find(&p, "commit-box").map(|b| f32::from(b.bounds.size.height));
        if find(&p, "commit-summary").is_some() || back_h != full_h {
            ui.fail(format!(
                "{ctx}: box after scrolling back {back_h:?}, before {full_h:?}"
            ));
        }
        // Collapse again, then focus the editor: the whole box is back.
        for _ in 0..12 {
            ui.wheel("diff-list", 60.);
        }
        ui.step("focusmsg");
        let p = ui.check(&format!("{ctx}, focused after collapse"));
        let h = find(&p, "commit-box").map(|b| f32::from(b.bounds.size.height));
        if h != full_h {
            ui.fail(format!("{ctx}: focused box {h:?}, full {full_h:?}"));
        }
        ui.step("focuslist");
    }
}

/// Commit view: the header with a long message scrolls under the file tabs, never over
/// them; message lines have the message line pitch at every font size.
fn commit_view_sweep(ui: &mut Ui) {
    for size in sizes() {
        ui.resize(size.0, size.1);
        for (font_ui, font_diff) in fonts() {
            ui.step(&format!("fonts:{font_ui},{font_diff}"));
            // Row 0 is the worktree, row 1 the newest commit (long message, several files).
            ui.step("select:1");
            let ctx = format!("{}x{} fonts {font_ui}/{font_diff}", size.0, size.1);
            let p = ui.check(&format!("{ctx}, commit view top"));
            let m = ui.metrics();
            if find(&p, "file-tabs").is_none() {
                ui.fail(format!("{ctx}: no file tabs"));
            }
            let lines: Vec<&Probe> = p.iter().filter(|p| p.kind() == "msg-line").collect();
            if lines.len() < 10 {
                ui.fail(format!("{ctx}: only {} message lines", lines.len()));
            }
            // Long lines wrap: a whole number of message lines.
            for l in &lines {
                let h = f32::from(l.bounds.size.height);
                let n = (h / m.message_line_h).round().max(1.);
                if (h - n * m.message_line_h).abs() > 1. {
                    ui.fail(format!(
                        "{ctx}: {} is {h} px, message line height is {}",
                        l.name, m.message_line_h
                    ));
                    break;
                }
            }
            for i in 0..15 {
                ui.wheel("diff-list", 70.);
                ui.check(&format!("{ctx}, scrolled {}", i + 1));
            }
            // Long header ↔ short header (row 2 = "Add f2"): every frame of the switch is
            // checked (`settle`), the header never lies over diff rows.
            for row in [2, 1, 2] {
                ui.step(&format!("select:{row}"));
            }
            ui.step("select:0");
        }
    }
}

/// Compare mode: the banner sits above the file tabs and the diff list at every
/// size, its swap / end buttons stay whole, no commit header is drawn; the base's own diff
/// and the working directory show the base hint instead.
fn compare_view_sweep(ui: &mut Ui) {
    for size in sizes() {
        ui.resize(size.0, size.1);
        for (font_ui, font_diff) in fonts() {
            ui.step(&format!("fonts:{font_ui},{font_diff}"));
            let ctx = format!("{}x{} fonts {font_ui}/{font_diff}", size.0, size.1);
            // Row 5 = "Initial import" is the base, row 1 the newest commit.
            ui.step("select:5");
            ui.step("focuslist");
            ui.step("key:shift-c");
            let p = ui.check(&format!("{ctx}, base selected"));
            if find(&p, "compare-banner").is_none() || find(&p, "compare-mark:5").is_none() {
                ui.fail(format!("{ctx}: no base hint / base mark"));
            }
            ui.step("select:1");
            let p = ui.check(&format!("{ctx}, comparing"));
            let banner = find(&p, "compare-banner").and_then(Probe::visible);
            let Some(banner) = banner else {
                ui.fail(format!("{ctx}: no compare banner"));
                ui.step("key:escape");
                continue;
            };
            if find(&p, "file-tabs").is_none() {
                ui.fail(format!("{ctx}: no file tabs for the compared files"));
            }
            if find(&p, "commit-header").is_some() || p.iter().any(|q| q.kind() == "msg-line") {
                ui.fail(format!("{ctx}: a commit header is drawn in compare mode"));
            }
            for button in ["compare-swap", "compare-end"] {
                match find(&p, button) {
                    Some(b) if b.visible().is_some_and(|v| v == b.bounds)
                        && testing::contains(&banner, &b.bounds) => {}
                    _ => ui.fail(format!("{ctx}: {button} not whole inside the banner")),
                }
            }
            // The totals: inside the banner, left of the buttons; whole while the
            // window is wide (a narrow one clips them first).
            match (find(&p, "compare-totals"), find(&p, "compare-swap")) {
                (Some(t), Some(swap)) => {
                    let right = t.bounds.origin.x + t.bounds.size.width;
                    if right > swap.bounds.origin.x + px(0.5) {
                        ui.fail(format!("{ctx}: the totals run under the swap button"));
                    }
                    if size.0 >= 1400. && font_ui <= 16. && t.visible() != Some(t.bounds) {
                        ui.fail(format!("{ctx}: the totals are cut in a wide window"));
                    }
                }
                _ => ui.fail(format!("{ctx}: no compare totals")),
            }
            for i in 0..4 {
                ui.wheel("diff-list", 90.);
                let p = ui.check(&format!("{ctx}, compare scrolled {}", i + 1));
                if find(&p, "compare-banner").and_then(Probe::visible) != Some(banner) {
                    ui.fail(format!("{ctx}: the banner moved while scrolling"));
                }
            }
            // The working directory against the base: banner above the diff, no commit box.
            ui.step("select:0");
            let p = ui.check(&format!("{ctx}, comparing with the working directory"));
            if find(&p, "compare-banner").is_none() || find(&p, "commit-box").is_some() {
                ui.fail(format!("{ctx}: working directory compared: banner / commit box"));
            }
            ui.step("focuslist");
            ui.step("key:escape");
            let p = ui.check(&format!("{ctx}, ended"));
            if find(&p, "compare-banner").is_some() || find(&p, "commit-box").is_none() {
                ui.fail(format!("{ctx}: Esc did not end compare mode"));
            }
        }
    }
}

/// Merge editor: scrolling ours | theirs moves the result to the matching line and
/// the other way round; scrolled to the end, the leading pane stays at its end.
fn merge_sync_scroll(ui: &mut Ui) {
    ui.step("select:0");
    ui.step("mergeeditor:0");
    let Some(merge) = testing::merge_window() else {
        ui.fail("no merge window".into());
        return;
    };
    let expect_sync = |ui: &mut Ui, ctx: &str| {
        let Some((row, line, want, at_end)) = ui.cx.update(|cx| testing::merge_scroll(cx)) else {
            ui.fail(format!("{ctx}: no merge editor"));
            return;
        };
        // At the end the follower may stop early (clamped); otherwise lines must match.
        if !at_end && (line - want).abs() > 0.05 {
            ui.fail(format!(
                "{ctx}: top row {row} ↔ result line {line}, expected {want}"
            ));
        }
    };
    for (pane, dy) in [
        ("merge-top-pane", 300.),
        ("merge-top-pane", 470.),
        ("merge-result-pane", 600.),
        ("merge-result-pane", -250.),
        ("merge-top-pane", -2000.),
    ] {
        ui.wheel_in(merge, pane, dy);
        expect_sync(ui, &format!("wheel {dy} over {pane}"));
    }
    // To the end of the top: it stays there (the result clamps, no tug of war).
    for _ in 0..3 {
        ui.wheel_in(merge, "merge-top-pane", 5000.);
    }
    match ui.cx.update(|cx| testing::merge_scroll(cx)) {
        Some((_, _, _, true)) => {}
        other => ui.fail(format!("top pane left its end: {other:?}")),
    }
}

/// A merge conflict with long lines on both sides and around it, a long path and a long
/// branch name (the merge editor once drew a too long line over the other side's text).
fn make_long_line_merge_repo(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "Layout Test"]);
    git(dir, &["config", "user.email", "layout@example.com"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    let path = "src/a_rather_long_directory_name/nested_deeper/merge_conflict_with_long_lines.rs";
    std::fs::create_dir_all(dir.join(path).parent().unwrap()).unwrap();
    let long = |who: &str| {
        let words: Vec<String> = (0..60).map(|i| format!("{who}_word_{i}")).collect();
        format!("let {who} = call(\"{}\");\n", words.join(" "))
    };
    let file = |swap: &dyn Fn(usize) -> Option<String>| -> String {
        (1..=40)
            .map(|i| match (i, swap(i)) {
                (_, Some(s)) => s,
                (6 | 14, None) => long("stable"),
                _ => format!("line {i}\n"),
            })
            .collect()
    };
    std::fs::write(dir.join(path), file(&|_| None)).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "init"]);
    let branch = "feature/a-very-long-branch-name-for-the-incoming-side-of-this-merge";
    git(dir, &["switch", "-qc", branch]);
    std::fs::write(dir.join(path), file(&|i| (i == 10).then(|| long("theirs")))).unwrap();
    git(dir, &["commit", "-qam", "theirs"]);
    git(dir, &["switch", "-q", "main"]);
    std::fs::write(dir.join(path), file(&|i| (i == 10).then(|| long("ours")))).unwrap();
    git(dir, &["commit", "-qam", "ours"]);
    let merged = std::process::Command::new("git")
        .args(["merge", "-q", branch])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(!merged.status.success(), "expected a merge conflict");
    dir.to_path_buf()
}

/// The probes of a fresh frame of another window (the merge editor).
fn frame_of(ui: &mut Ui, win: AnyWindowHandle) -> Vec<Probe> {
    testing::clear_probes();
    ui.cx
        .update_window(win, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
    testing::probes()
}

/// Merge editor with long lines: each side's line stays inside its half, the
/// toolbar's and the single view bar's texts stay on one line inside their bar — at
/// several window sizes, split and single view.
fn merge_long_lines(ui: &mut Ui) {
    ui.step("select:0");
    ui.step("mergeeditor:0");
    let Some(merge) = testing::merge_window() else {
        ui.fail("no merge window".into());
        return;
    };
    ui.cx.update(testing::merge_focus_top);
    ui.settle();
    for single in [false, true] {
        for (w, h) in [(1400., 900.), (900., 600.), (560., 420.)] {
            ui.cx.simulate_window_resize(merge, size(px(w), px(h)));
            ui.settle();
            let ctx = format!("{w}x{h}{}", if single { " single view" } else { "" });
            let p = frame_of(ui, merge);
            if ui.dump {
                println!("--- merge {ctx}");
                for q in &p {
                    println!("{}", q.to_json());
                }
            }
            for f in testing::check(&p) {
                ui.fail(format!("{ctx}: {f}"));
            }
            let bar = if single { "merge-folded" } else { "merge-toolbar" };
            if find(&p, bar).is_none() {
                ui.fail(format!("{ctx}: no {bar}"));
            }
            // The long lines are on screen and reach their half's right edge.
            if !single {
                let long_cells = p
                    .iter()
                    .filter(|t| t.kind() == "merge-text")
                    .filter(|t| {
                        let cell =
                            format!("merge-cell:{}:{}", t.index().unwrap_or(0), t.detail());
                        find(&p, &cell).is_some_and(|c| {
                            f32::from(t.bounds.right()) >= f32::from(c.bounds.right()) - 1.
                        })
                    })
                    .count();
                if long_cells < 2 {
                    ui.fail(format!("{ctx}: expected long lines in both halves, {long_cells} found"));
                }
            }
        }
        if !single {
            // ⌥V: the single view with its folded sides bar.
            let ks = Keystroke::parse("alt-v").unwrap();
            ui.cx
                .update_window(merge, |_, window, cx| window.dispatch_keystroke(ks, cx))
                .unwrap();
            ui.settle();
        }
    }
}

/// `<dir>`: `main` and `feature` both changed `a.txt` (→ a merge conflict), `feature` also
/// changed `b.txt` (merges cleanly). With `merge`, `git merge feature` is left conflicted;
/// otherwise a stash of a.txt / b.txt changes popped onto a conflicting HEAD (no MERGE_HEAD).
fn make_conflict_repo(dir: &std::path::Path, merge: bool) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "Layout Test"]);
    git(dir, &["config", "user.email", "layout@example.com"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
    write("a.txt", "base\n");
    write("b.txt", "b\n");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "init"]);
    let conflicting = if merge {
        git(dir, &["switch", "-qc", "feature"]);
        write("a.txt", "feature\n");
        write("b.txt", "b from feature\n");
        git(dir, &["commit", "-qam", "feature change"]);
        git(dir, &["switch", "-q", "main"]);
        write("a.txt", "main\n");
        git(dir, &["commit", "-qam", "main change"]);
        ["merge", "feature"]
    } else {
        write("a.txt", "stashed\n");
        write("b.txt", "b stashed\n");
        git(dir, &["stash", "push", "-q", "-m", "wip"]);
        write("a.txt", "committed\n");
        git(dir, &["commit", "-qam", "head change"]);
        ["stash", "pop"]
    };
    let out = std::process::Command::new("git")
        .args(conflicting)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(!out.status.success(), "expected a conflict");
    dir.to_path_buf()
}

fn git_out(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Conflicts from a `git stash pop`: the banner has Abort, Commit says why it is
/// off; Abort asks, then puts the files back to HEAD and the stash entry stays.
fn stash_conflict_abort(ui: &mut Ui) {
    let ws = ui.ws.clone();
    let dir = ui.cx.update(|cx| testing::workdir(&ws, cx));
    let p = ui.check("stash conflict");
    for name in ["op-abort", "commit-blocker"] {
        if find(&p, name).is_none() {
            ui.fail(format!("stash conflict: no {name}"));
        }
    }
    if !ui.cx.update(|cx| testing::commit_message(&ws, cx)).is_empty() {
        ui.fail("stash conflict: the message box got a merge message".into());
    }
    // The banner's text wraps, its buttons stay whole at every font size (at 28 pt the
    // unwrapped text pushed Abort out of the window — Windows fonts). Independent of the
    // fonts the scenarios before left in the shared settings.
    for (font_ui, font_diff) in fonts() {
        ui.step(&format!("fonts:{font_ui},{font_diff}"));
        let p = ui.check(&format!("stash conflict, fonts {font_ui}/{font_diff}"));
        match find(&p, "op-abort") {
            Some(b) if b.visible() == Some(b.bounds) => {}
            _ => ui.fail(format!("fonts {font_ui}/{font_diff}: Abort is not (fully) visible")),
        }
    }
    ui.click_on("op-abort", MouseButton::Left);
    if !ui.cx.has_pending_prompt() {
        ui.fail("Abort did not ask first".into());
        return;
    }
    ui.cx.simulate_prompt_answer("Abort");
    ui.settle();
    let working = ui.cx.update(|cx| testing::working(&ws, cx));
    if !working.is_empty() {
        ui.fail(format!("after Abort: still changes {working:?}"));
    }
    if !git_out(&dir, &["stash", "list"]).contains("wip") {
        ui.fail("after Abort: the stash entry is gone".into());
    }
    let p = ui.check("after abort");
    if find(&p, "op-abort").is_some() || find(&p, "commit-blocker").is_some() {
        ui.fail("after Abort: banner / commit blocker still shown".into());
    }
}

/// A merge with a conflict: the message box gets git's merge message, Commit is
/// off with the reason, the banner has Abort; the main part of "Resolve Conflicts" opens
/// the merge editor; once resolved and staged, Commit concludes the merge.
fn merge_commit_flow(ui: &mut Ui) {
    let ws = ui.ws.clone();
    let dir = ui.cx.update(|cx| testing::workdir(&ws, cx));
    let msg = ui.cx.update(|cx| testing::commit_message(&ws, cx));
    if msg != "Merge branch 'feature'" {
        ui.fail(format!("merge message not prefilled: {msg:?}"));
    }
    // A conflicted file's tab says `C` (git's `U` read as untracked / unstaged).
    let (labels, _) = ui.cx.update(|cx| testing::worktree_tabs(&ws, cx));
    if !labels.iter().any(|l| l == "C a.txt") || labels.iter().any(|l| l.starts_with("U ")) {
        ui.fail(format!("conflicted file tabs: {labels:?}"));
    }
    let p = ui.check("merge conflict");
    for name in ["op-abort", "commit-blocker", "resolve-split:"] {
        if find(&p, name).is_none() && !p.iter().any(|q| q.name.starts_with(name)) {
            ui.fail(format!("merge conflict: no {name}"));
        }
    }
    // Commit while conflicted: refused with the reason, nothing committed.
    let head = git_out(&dir, &["rev-parse", "HEAD"]);
    ui.step("commit");
    if git_out(&dir, &["rev-parse", "HEAD"]) != head {
        ui.fail("Commit ran with open conflicts".into());
    }
    match ui.cx.update(|cx| testing::error(&ws, cx)) {
        Some(e) if e.contains("conflicted") => {}
        other => ui.fail(format!("Commit with conflicts: error {other:?}")),
    }
    // The split button's main part opens the merge editor right away.
    // (Window handles of earlier scenarios' apps can compare equal: count this app's.)
    let before = ui.cx.windows().len();
    if let Some(name) = p
        .iter()
        .find(|q| q.kind() == "resolve")
        .map(|q| q.name.clone())
    {
        ui.click_on(&name, MouseButton::Left);
        if ui.cx.windows().len() != before + 1 {
            ui.fail("Resolve Conflicts did not open the merge editor".into());
        }
    } else {
        ui.fail("no Resolve Conflicts button".into());
    }
    // Resolved outside (a `cmd:` step would leave the git command panel open in the shared
    // settings for the later scenarios), then F5.
    git(&dir, &["checkout", "--ours", "--", "a.txt"]);
    git(&dir, &["add", "a.txt"]);
    ui.step("key:f5");
    let p = ui.check("merge resolved");
    if find(&p, "commit-blocker").is_some() {
        ui.fail("resolved merge: Commit still blocked".into());
    }
    ui.step("commit");
    let parents = git_out(&dir, &["rev-list", "--parents", "-n1", "HEAD"]);
    if parents.split_whitespace().count() != 3 {
        ui.fail(format!("Commit did not conclude the merge: {parents}"));
    }
    let subject = git_out(&dir, &["log", "-1", "--format=%s"]);
    if subject != "Merge branch 'feature'" {
        ui.fail(format!("merge commit message {subject:?}"));
    }
    let p = ui.check("merge committed");
    if find(&p, "op-abort").is_some() {
        ui.fail("merge committed: banner still shown".into());
    }
    if !ui.cx.update(|cx| testing::commit_message(&ws, cx)).is_empty() {
        ui.fail("merge committed: message box not cleared".into());
    }
}

/// A credential prompt from the askpass helper shows the key icon (gitti once showed
/// none on Windows).
fn askpass_key_icon(ui: &mut Ui) {
    ui.step("ask:Enter passphrase for key '/home/u/.ssh/id_ed25519': ");
    let p = ui.check("askpass");
    if find(&p, "modal").is_none() || find(&p, "modal-key-icon").is_none() {
        ui.fail("askpass dialog without key icon".into());
    }
    ui.step("cancel");
    // The dialog names the git command that asked.
    ui.step("ask:Password for 'https://user@example.com': |git fetch origin");
    ui.check("askpass with command");
    let ws = ui.ws.clone();
    let message = ui.cx.update(|cx| testing::modal_message(&ws, cx));
    if message.as_deref()
        != Some("Password for 'https://user@example.com':\n\nAsked by: git fetch origin")
    {
        ui.fail(format!("askpass dialog message {message:?}"));
    }
    ui.step("cancel");
}

/// A right click selects what it hits (like gitty) before the menu opens: a
/// commit row, or a branch in the sidebar (its commit).
fn right_click_selects(ui: &mut Ui) {
    ui.click_on("commit-row:3", MouseButton::Right);
    let m = ui.metrics();
    if m.selected_row != 3 {
        ui.fail(format!("right click on commit row 3 selected row {}", m.selected_row));
    }
    ui.step("key:escape");
    // `main` points at the newest commit, row 1 (row 0 is the worktree).
    ui.click_on("side:ref-refs/heads/main", MouseButton::Right);
    let m = ui.metrics();
    if m.selected_row != 1 {
        ui.fail(format!("right click on branch main selected row {}", m.selected_row));
    }
}

/// Dragging a selection past the diff list's bottom scrolls it and extends the selection,
/// as long as the mouse is held there; released, it stops. Lines (worktree) and text
/// (from the commit message down into the diff).
fn drag_autoscroll(ui: &mut Ui) {
    for select in ["select:0", "select:1"] {
        ui.step(select);
        let p = ui.frame();
        let Some(list) = find(&p, "diff-list").and_then(Probe::visible) else {
            ui.fail(format!("{select}: no diff list"));
            continue;
        };
        let before = ui.metrics();
        // Worktree: press on the first changed line (a line selection starts there);
        // commit view: on the message's first line.
        let mut start = None;
        if select == "select:1" {
            if let Some(m) = find(&p, "msg-line:0").and_then(Probe::visible) {
                ui.mouse(MouseButton::Left, Some(true), m.center());
                start = Some("msg-line:0".to_string());
            }
        } else {
            for r in p.iter().filter(|r| r.kind() == "diff-row" && r.detail() == "line") {
                let Some(v) = r.visible() else { continue };
                ui.mouse(MouseButton::Left, Some(true), v.center());
                if ui.metrics().selected_lines == 1 {
                    start = Some(r.name.clone());
                    break;
                }
                ui.mouse(MouseButton::Left, Some(false), v.center());
            }
        }
        let Some(start) = start else {
            ui.fail(format!("{select}: nothing to start a drag on"));
            continue;
        };
        let ctx = format!("{select}, drag from {start}");
        let below = point(list.center().x, list.bottom() + px(80.));
        ui.mouse(MouseButton::Left, None, below);
        let held = ui.metrics();
        if held.diff_scroll_y < before.diff_scroll_y + 100. {
            ui.fail(format!(
                "{ctx}: held below the list, scrolled {} → {}",
                before.diff_scroll_y, held.diff_scroll_y
            ));
        }
        let grown = if select == "select:0" {
            held.selected_lines > 1
        } else {
            held.text_selected > 200
        };
        if !grown {
            ui.fail(format!(
                "{ctx}: selection did not follow ({} lines, {} bytes)",
                held.selected_lines, held.text_selected
            ));
        }
        ui.mouse(MouseButton::Left, Some(false), below);
        let released = ui.metrics();
        ui.settle();
        if ui.metrics().diff_scroll_y != released.diff_scroll_y {
            ui.fail(format!("{ctx}: still scrolling after the release"));
        }
    }
}

/// View ▸ Toggle Sidebar: without it the commit list starts at the window's left edge and
/// list + detail fill the row (both views, every window size); with it back the sidebar
/// is left of the list again. Every frame keeps the invariants.
fn sidebar_toggle(ui: &mut Ui) {
    for size in sizes() {
        ui.resize(size.0, size.1);
        for (row, view) in [(0, "worktree"), (1, "commit")] {
            ui.step(&format!("select:{row}"));
            let ctx = format!("{}x{} {view}", size.0, size.1);
            ui.step("action:gitti::ToggleSidebar");
            let p = ui.check(&format!("{ctx}, sidebar hidden"));
            let list = find(&p, "commit-list").and_then(Probe::visible);
            let detail = find(&p, "detail").and_then(Probe::visible);
            match (find(&p, "sidebar"), list, detail) {
                (None, Some(l), Some(d)) => {
                    if f32::from(l.origin.x) > 1. {
                        ui.fail(format!("{ctx}: commit list starts at x {:?}", l.origin.x));
                    }
                    if f32::from(d.origin.x) < f32::from(l.right()) - 1. {
                        ui.fail(format!("{ctx}: detail overlaps the list"));
                    }
                    if (f32::from(d.right()) - size.0).abs() > 2. {
                        ui.fail(format!("{ctx}: detail ends at {:?}", d.right()));
                    }
                }
                (s, l, d) => ui.fail(format!(
                    "{ctx}: hidden sidebar: sidebar {:?}, list {l:?}, detail {d:?}",
                    s.map(|s| s.bounds)
                )),
            }
            ui.step("action:gitti::ToggleSidebar");
            let p = ui.check(&format!("{ctx}, sidebar shown"));
            let side = find(&p, "sidebar").and_then(Probe::visible);
            let list = find(&p, "commit-list").and_then(Probe::visible);
            match (side, list) {
                (Some(s), Some(l)) if f32::from(s.right()) <= f32::from(l.origin.x) + 1. => {}
                other => ui.fail(format!("{ctx}: sidebar back: {other:?}")),
            }
        }
    }
}

/// The stored pane fractions apply to the window's real size even if it changed between
/// the body's first measurement and the first frame with panes: Windows draws a restored
/// maximized window once at its normal size, then maximizes it (otherwise the panels shrink
/// by the normal / maximized width difference on every start).
fn panes_restore_after_resize(ui: &mut Ui) {
    use gitti_core::store::{self, PaneLayout};
    let unset = PaneLayout::default();
    let stored = PaneLayout { pane_sidebar: 0.2, pane_list: 0.3, pane_log: 0. };
    store::save_changed_settings(&unset, &stored);
    // A fresh window (reads the stored fractions), now 2000 wide, whose panes start in a
    // frame whose body was measured while the window was 1500 wide.
    *ui = Ui::new(&ui.repo.clone(), 1500., 950.);
    ui.resize(2000., 1100.);
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::restart_panes(&ws, 1500., 1500., cx));
    ui.settle();
    let p = ui.check("2000x1100, first panes frame with the body measured at 1500");
    for (name, want) in [("sidebar", 400.), ("commit-list", 600.)] {
        match find(&p, name) {
            Some(probe) if (f32::from(probe.bounds.size.width) - want).abs() < 2. => {}
            other => ui.fail(format!(
                "{name}: want width {want}, got {:?}",
                other.map(|p| p.bounds.size.width)
            )),
        }
    }
    store::save_changed_settings(&stored, &unset);
}

/// The first frame after a window resize already has the panes at their
/// new sizes — the old ones scaled to the new width / height, as every later frame —
/// with and without the sidebar, and the git log panel's height. (gpui-base's resize
/// state rescales only in prepaint, after the layout: one frame had the extra width split
/// evenly between the panes, e.g. a 300 px sidebar drawn 466 px wide.)
fn panes_first_frame_after_resize(ui: &mut Ui) {
    ui.step("action:gitti::ToggleGitLog");
    for sidebar in [true, false] {
        let mut size = (1500., 950.);
        ui.resize(size.0, size.1);
        for next in [(2000., 1100.), (1100., 700.), (1600., 900.)] {
            let panes = |p: &[Probe]| -> Vec<(&'static str, f32)> {
                let names = [("sidebar", true), ("commit-list", true), ("detail", true)];
                names
                    .into_iter()
                    .chain([("git-log-panel", false)])
                    .filter_map(|(name, horizontal)| {
                        let b = find(p, name)?.bounds.size;
                        Some((name, f32::from(if horizontal { b.width } else { b.height })))
                    })
                    .collect()
            };
            let before = panes(&ui.check("before the resize"));
            let ctx =
                format!("sidebar {sidebar}, {}x{} -> {}x{}", size.0, size.1, next.0, next.1);
            if before.len() != if sidebar { 4 } else { 3 } {
                ui.fail(format!("{ctx}: panes missing: {before:?}"));
            }
            // (`take_frames` would hand the checked frame out again as the first one.)
            testing::clear_probes();
            ui.cx
                .simulate_window_resize(ui.window, gpui_kit::size(px(next.0), px(next.1)));
            let mut frames = Vec::new();
            for _ in 0..4 {
                let tick = std::time::Duration::from_millis(50);
                ui.cx.executor().advance_clock(tick);
                ui.cx.run_until_parked();
                ui.render();
                frames.extend(testing::take_frames());
                testing::clear_probes();
            }
            let last = frames.last().map(|p| panes(p)).unwrap_or_default();
            for (k, p) in frames.iter().enumerate() {
                for f in testing::check(p) {
                    ui.fail(format!("{ctx}: frame {k}: {f}"));
                }
                let now = panes(p);
                if now != last {
                    ui.fail(format!("{ctx}: frame {k} has panes {now:?}, the last {last:?}"));
                }
            }
            // The panes follow the window (scaled with it).
            for ((name, old), (_, new)) in before.iter().zip(&last) {
                if (new - old).abs() < 0.5 {
                    ui.fail(format!("{ctx}: {name} kept {old} px"));
                }
            }
            size = next;
        }
        ui.step("action:gitti::ToggleSidebar");
    }
    ui.step("action:gitti::ToggleGitLog");
}

/// `make_repo` in `<dir>/repo` plus two linked worktrees next to it (one with long names,
/// one detached at `HEAD~1`, sparse and locked — the most words on a sidebar row).
fn make_worktrees_repo(dir: &std::path::Path) -> std::path::PathBuf {
    let repo = make_repo(&dir.join("repo"));
    for (name, branch) in [
        ("repo-a-rather-long-folder-name-for-the-sidebar", "feature/with-a-long-branch-name"),
        ("repo-b", "b"),
    ] {
        let path = dir.join(name);
        git(&repo, &["worktree", "add", "-q", "-b", branch, path.to_str().unwrap()]);
    }
    let b = dir.join("repo-b");
    git(&b, &["switch", "-q", "--detach", "HEAD~1"]);
    git(&b, &["sparse-checkout", "set", "--cone", "docs"]);
    git(&repo, &["worktree", "lock", b.to_str().unwrap()]);
    repo
}

/// The sidebar with a WORKTREES section (three entries, one locked + sparse) and branch
/// marks, across window and font sizes: entries inside the sidebar, one below the other,
/// the section between BRANCHES and REMOTES, the marks and `in <folder>` inside their
/// rows; the commit list's worktree labels inside their commit rows.
fn sidebar_worktrees_sweep(ui: &mut Ui) {
    for size in sizes() {
        ui.resize(size.0, size.1);
        for (font_ui, font_diff) in fonts() {
            ui.step(&format!("fonts:{font_ui},{font_diff}"));
            let ctx = format!("{}x{} fonts {font_ui}/{font_diff}", size.0, size.1);
            let p = ui.check(&ctx);
            let shown = |name: &str| find(&p, name).and_then(Probe::visible);
            if shown("side-section:worktrees").is_none() {
                ui.fail(format!("{ctx}: no WORKTREES section"));
                continue;
            }
            let mut rows: Vec<&Probe> = p
                .iter()
                .filter(|r| matches!(r.kind(), "side" | "side-section") && r.visible().is_some())
                .collect();
            rows.sort_by(|a, b| a.bounds.top().partial_cmp(&b.bounds.top()).unwrap());
            for w in rows.windows(2) {
                if f32::from(w[0].bounds.bottom()) > f32::from(w[1].bounds.top()) + 0.5 {
                    ui.fail(format!("{ctx}: {} overlaps {}", w[0].name, w[1].name));
                }
            }
            let wt_rows = rows.iter().filter(|r| r.name.starts_with("side:wt-")).count();
            if wt_rows != 3 && size.1 > 600. {
                ui.fail(format!("{ctx}: {wt_rows} worktree entries visible, expected 3"));
            }
            if size.0 >= 1500. && font_ui <= 16. {
                for name in [
                    "wt-sparse:repo-b",
                    "wt-branch-badge:feature/with-a-long-branch-name",
                    "wt-badge:repo-b",
                ] {
                    if shown(name).is_none() {
                        ui.fail(format!("{ctx}: {name} not shown"));
                    }
                }
            }
            if shown("wt-mark:b").is_some() {
                ui.fail(format!("{ctx}: b is checked out nowhere (repo-b is detached)"));
            }
            for (mark, row) in [
                ("wt-sparse:repo-b", "side:wt-repo-b"),
                ("wt-branch-badge:feature/with-a-long-branch-name", "commit-row:1"),
                ("wt-badge:repo-b", "commit-row:2"),
                (
                    "wt-mark:feature/with-a-long-branch-name",
                    "side:ref-refs/heads/feature/with-a-long-branch-name",
                ),
                (
                    "wt-where:feature/with-a-long-branch-name",
                    "side:ref-refs/heads/feature/with-a-long-branch-name",
                ),
            ] {
                match (shown(mark), shown(row)) {
                    (Some(m), Some(r)) if testing::contains(&r, &m) => {}
                    (None, None) => {}
                    other => ui.fail(format!("{ctx}: {mark} not inside {row}: {other:?}")),
                }
            }
            if shown("wt-mark:main").is_some() {
                ui.fail(format!("{ctx}: the checked-out branch is marked"));
            }
        }
    }
}

/// Spell checking (the word list `main` installs instead of the OS checker): exactly the
/// misspelled prose words get a wavy underline — under the word (its width and place in
/// the line), inside the editor's text area, also when the line wraps, at every font
/// size — code-like tokens stay unmarked; a suggestion replaces the word; switching it
/// off removes the marks.
fn spell_marks(ui: &mut Ui) {
    const MSG: &str = "Fix teh wrnog thing in src/teh.rs and `teh` here";
    let ws = ui.ws.clone();
    for size in [SIZES[0], SIZES[2]] {
        ui.resize(size.0, size.1);
        for (font_ui, font_diff) in fonts() {
            ui.step(&format!("fonts:{font_ui},{font_diff}"));
            let ctx = format!("{}x{} fonts {font_ui}/{font_diff}", size.0, size.1);
            ui.step(&format!("msg:{MSG}"));
            ui.step("focuslist");
            let marked = ui.cx.update(|cx| testing::spell_marks(&ws, SpellTarget::Message, cx));
            if marked != ["teh", "wrnog"] {
                ui.fail(format!("{ctx}: marked {marked:?}, expected teh, wrnog"));
                continue;
            }
            let p = ui.check(&ctx);
            let marks: Vec<&Probe> = p.iter().filter(|q| q.kind() == "spell-mark").collect();
            let Some(editor) = find(&p, "commit-editor").and_then(Probe::visible) else {
                ui.fail(format!("{ctx}: no commit editor"));
                continue;
            };
            let (teh, wrnog) = match marks.as_slice() {
                [a, b] if a.index() == Some(0) && b.index() == Some(1) => (a.bounds, b.bounds),
                _ => {
                    let names: Vec<_> = marks.iter().map(|m| m.name.clone()).collect();
                    ui.fail(format!("{ctx}: spell marks drawn {names:?}"));
                    continue;
                }
            };
            for m in &marks {
                if !m.visible().is_some_and(|v| testing::contains(&editor, &v)) {
                    ui.fail(format!(
                        "{ctx}: {} {:?} not inside the editor {editor:?}",
                        m.name, m.bounds
                    ));
                }
            }
            // Monospace: "teh" is 3 advances wide, "wrnog" 5, and starts 4 after "teh".
            let adv = f32::from(teh.size.width) / 3.;
            let (w1, dx) = (
                f32::from(wrnog.size.width),
                f32::from(wrnog.left() - teh.left()),
            );
            if (w1 - 5. * adv).abs() > 1.5
                || (dx - 4. * adv).abs() > 1.5
                || teh.top() != wrnog.top()
            {
                ui.fail(format!(
                    "{ctx}: marks not under their words: teh {teh:?}, wrnog {wrnog:?} (advance {adv})"
                ));
            }
        }
    }
    // A wrapped message: the misspelled word on a later visual line is marked there.
    // (Window big enough for the editor to show all of it: it opens scrolled to the top.)
    ui.resize(SIZES[0].0, SIZES[0].1);
    ui.step("fonts:20,22");
    let long = format!("{}teh end", "word ".repeat(25));
    ui.step(&format!("msg:{long}"));
    ui.step("focuslist");
    let p = ui.check("wrapped message");
    let line_h = ui.metrics().message_line_h;
    match (
        find(&p, "spell-mark:0").and_then(Probe::visible),
        find(&p, "commit-editor").and_then(Probe::visible),
    ) {
        (Some(m), Some(e))
            if testing::contains(&e, &m) && f32::from(m.top() - e.top()) > line_h => {}
        other => ui.fail(format!("wrapped message: mark / editor {other:?}")),
    }
    // The right-click menu's suggestion replaces the word.
    ui.step("fonts:13,13");
    ui.step("msg:Fix teh bug");
    ui.step("focusmsg");
    let offer = ui.cx.update(|cx| testing::spell_at(&ws, SpellTarget::Message, 5, cx));
    if offer != Some(("teh".into(), vec!["the".into(), "ten".into()])) {
        ui.fail(format!("menu at teh offers {offer:?}"));
    }
    let chosen = ui
        .cx
        .update_window(ui.window, |_, window, cx| {
            testing::spell_choose(&ws, SpellTarget::Message, 0, window, cx)
        })
        .unwrap();
    ui.settle();
    let text = ui
        .cx
        .update_window(ui.window, |_, window, cx| testing::state(&ws, window, cx))
        .unwrap()
        .message;
    if !chosen || text != "Fix the bug" {
        ui.fail(format!("choosing \"the\" left {text:?} (chosen {chosen})"));
    }
    if !ui.cx.update(|cx| testing::spell_marks(&ws, SpellTarget::Message, cx)).is_empty() {
        ui.fail("marks left after the replacement".into());
    }
    // Off: no marks; on again: back.
    ui.step("focuslist");
    ui.step("msg:Fix teh bug");
    ui.step("palette:Toggle Spell Checking");
    let p = ui.check("spell checking off");
    if p.iter().any(|q| q.kind() == "spell-mark") {
        ui.fail("spell marks drawn while spell checking is off".into());
    }
    ui.step("palette:Toggle Spell Checking");
    let p = ui.check("spell checking on again");
    if !p.iter().any(|q| q.kind() == "spell-mark") {
        ui.fail("no spell marks after switching spell checking on again".into());
    }
}

/// Spell checking in dialogs: the Edit Commit Message text area and a dialog's
/// message field (Create Tag) get the same marks as the commit message — under the
/// misspelled words, inside their editor — the menu's suggestion replaces the word there,
/// fields that aren't messages (the tag name) stay unchecked, and the setting switches the
/// dialogs off too.
fn spell_marks_dialogs(ui: &mut Ui) {
    let ws = ui.ws.clone();
    let marks = |ui: &mut Ui, t: SpellTarget| ui.cx.update(|cx| testing::spell_marks(&ws, t, cx));
    let inside = |ui: &mut Ui, ctx: &str, editor: &str, n: usize| {
        let p = ui.check(ctx);
        let Some(e) = find(&p, editor).and_then(Probe::visible) else {
            ui.fail(format!("{ctx}: no {editor}"));
            return;
        };
        let drawn: Vec<&Probe> = p.iter().filter(|q| q.kind() == "modal-spell-mark").collect();
        if drawn.len() != n {
            let names: Vec<_> = drawn.iter().map(|m| m.name.clone()).collect();
            ui.fail(format!("{ctx}: {n} marks expected, drawn {names:?}"));
        }
        for m in drawn {
            if !m.visible().is_some_and(|v| testing::contains(&e, &v)) {
                ui.fail(format!("{ctx}: {} {:?} not inside {editor} {e:?}", m.name, m.bounds));
            }
        }
        if p.iter().any(|q| q.kind() == "spell-mark") {
            ui.fail(format!("{ctx}: the commit editor's marks drawn"));
        }
    };
    ui.resize(SIZES[0].0, SIZES[0].1);
    for &(font_ui, font_diff) in &[FONTS[1], FONTS[4]] {
        let ctx = format!("fonts {font_ui}/{font_diff}");
        ui.step(&format!("fonts:{font_ui},{font_diff}"));
        // Edit Commit Message… of a commit: its text area.
        ui.step("select:1");
        ui.step("action:gitti::EditCommitMessage");
        ui.settle();
        let title = ui.cx.update(|cx| testing_menus::modal(&ws, cx)).map(|m| m.0);
        if !title.as_deref().is_some_and(|t| t.starts_with("Edit message of")) {
            ui.fail(format!("{ctx}: Edit Commit Message dialog: {title:?}"));
            return;
        }
        ui.cx
            .update_window(ui.window, |_, window, cx| {
                testing::modal_text(&ws, "Fix teh wrnog thing\n\nUse `teh` in src/teh.rs", window, cx)
            })
            .unwrap();
        ui.settle();
        let got = marks(ui, SpellTarget::ModalText);
        if got != ["teh", "wrnog"] {
            ui.fail(format!("{ctx}: reword dialog marked {got:?}, expected teh, wrnog"));
        }
        inside(ui, &format!("{ctx}, reword dialog"), "modal-text", 2);
        let offer = ui.cx.update(|cx| testing::spell_at(&ws, SpellTarget::ModalText, 5, cx));
        if offer != Some(("teh".into(), vec!["the".into(), "ten".into()])) {
            ui.fail(format!("{ctx}: reword menu at teh offers {offer:?}"));
        }
        ui.cx
            .update_window(ui.window, |_, window, cx| {
                testing::spell_choose(&ws, SpellTarget::ModalText, 0, window, cx)
            })
            .unwrap();
        ui.settle();
        let got = marks(ui, SpellTarget::ModalText);
        if got != ["wrnog"] {
            ui.fail(format!("{ctx}: after choosing \"the\" the reword dialog marks {got:?}"));
        }
        ui.step("cancel");
        // Create Tag…: the message field is checked, the tag name isn't.
        ui.step("action:gitti::CreateTag");
        ui.step("field:0=tehh-wrnog");
        ui.step("field:1=Release teh build");
        ui.settle();
        let (name, message) = (
            marks(ui, SpellTarget::ModalField(0)),
            marks(ui, SpellTarget::ModalField(1)),
        );
        if !name.is_empty() || message != ["teh"] {
            ui.fail(format!("{ctx}: tag dialog marked name {name:?}, message {message:?}"));
        }
        inside(ui, &format!("{ctx}, tag message"), "modal-field:1", 1);
        let offer = ui.cx.update(|cx| testing::spell_at(&ws, SpellTarget::ModalField(1), 9, cx));
        if offer.as_ref().map(|o| o.0.as_str()) != Some("teh") {
            ui.fail(format!("{ctx}: tag message menu offers {offer:?}"));
        }
        ui.cx
            .update_window(ui.window, |_, window, cx| {
                testing::spell_choose(&ws, SpellTarget::ModalField(1), 0, window, cx)
            })
            .unwrap();
        ui.settle();
        let fields = ui.cx.update(|cx| testing_menus::modal(&ws, cx)).map(|m| m.1);
        if fields.as_ref().and_then(|f| f.get(1)).map(String::as_str) != Some("Release the build") {
            ui.fail(format!("{ctx}: tag message after the suggestion: {fields:?}"));
        }
        ui.step("cancel");
    }
    // Off: the dialogs aren't checked either.
    ui.step("palette:Toggle Spell Checking");
    ui.step("action:gitti::CreateTag");
    ui.step("field:1=Release teh build");
    ui.settle();
    if !marks(ui, SpellTarget::ModalField(1)).is_empty() {
        ui.fail("tag message marked while spell checking is off".into());
    }
    inside(ui, "spell checking off", "modal-field:1", 0);
    ui.step("cancel");
    ui.step("palette:Toggle Spell Checking");
}

/// Go to Commit with a file search: the note above the results (wrapping at big fonts)
/// and the match reasons in the rows stay in place at every window and font size.
/// `make_repo` plus many folders (some nested, one with a long name) for the folder picker.
fn make_folder_repo(dir: &std::path::Path) -> std::path::PathBuf {
    let d = make_repo(dir);
    for i in 0..30 {
        let f = d.join(format!("dir{i:02}/sub/deeper/x.txt"));
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, "x\n").unwrap();
    }
    let long = d.join(format!("{}/y.txt", "a-folder-with-a-really-long-name-".repeat(4)));
    std::fs::create_dir_all(long.parent().unwrap()).unwrap();
    std::fs::write(&long, "y\n").unwrap();
    git(&d, &["add", "."]);
    git(&d, &["commit", "-qm", "Folders"]);
    d
}

/// The folder picker over Add Worktree at every size / font: whole inside the
/// window, its rows inside its list at one pitch (`layout_check`), the buttons reachable;
/// "Choose…" sits right of the folders field, inside the dialog.
fn folder_picker_sweep(ui: &mut Ui) {
    for size in sizes() {
        ui.resize(size.0, size.1);
        for (font_ui, font_diff) in fonts() {
            ui.step(&format!("fonts:{font_ui},{font_diff}"));
            let ctx = format!("{}x{} fonts {font_ui}/{font_diff}", size.0, size.1);
            ui.step("focuslist");
            ui.step("action:gitti::AddWorktree");
            let p = ui.check(&format!("{ctx}, add worktree"));
            // (At big fonts in small windows the dialog's fields scroll: compare with the
            // field, whether scrolled into view or not.)
            match (find(&p, "modal-choice:1"), find(&p, "modal-field:3"), find(&p, "modal")) {
                (Some(c), Some(f), Some(m))
                    if c.bounds.left() > f.bounds.left()
                        && c.bounds.right() <= m.bounds.right()
                        && c.bounds.top() >= f.bounds.top()
                        && c.bounds.bottom() <= f.bounds.bottom() + px(1.) => {}
                other => ui.fail(format!(
                    "{ctx}: Choose… not right of the folders field: {:?}",
                    other.0.map(|c| c.bounds)
                )),
            }
            ui.step("choice:1");
            ui.step("folders:expand,dir01");
            ui.step("folders:expand,dir01/sub");
            ui.step("folders:toggle,dir01/sub");
            ui.step("key:end");
            let p = ui.check(&format!("{ctx}, folder picker"));
            for name in ["folder-picker", "folder-picker-ok", "folder-summary"] {
                match find(&p, name) {
                    Some(r) if r.visible() == Some(r.bounds) => {}
                    other => ui.fail(format!(
                        "{ctx}: {name} not (fully) visible: {:?}",
                        other.map(|r| (r.bounds, r.visible()))
                    )),
                }
            }
            let rows = p.iter().filter(|r| r.kind() == "folder-row").count();
            if rows < 3 {
                ui.fail(format!("{ctx}: only {rows} folder rows drawn"));
            }
            ui.step("key:escape");
            ui.step("cancel");
        }
    }
}

fn go_to_commit_picker(ui: &mut Ui) {
    for size in sizes() {
        ui.resize(size.0, size.1);
        for (font_ui, font_diff) in fonts() {
            ui.step(&format!("fonts:{font_ui},{font_diff}"));
            let ctx = format!("{}x{} fonts {font_ui}/{font_diff}", size.0, size.1);
            ui.step("focuslist");
            ui.step("action:gitti::GoToCommit");
            ui.step("pick:f1.txt");
            let p = ui.check(&format!("{ctx}, file search"));
            let has = |n: &str| p.iter().any(|q| q.name == n && q.visible().is_some());
            if !has("picker-note") || !has("picker-tag:0") || !has("picker-item:1") {
                ui.fail(format!("{ctx}: no note / reason tag / two results in the picker"));
            }
            ui.step("cancel");
        }
    }
}

/// [`make_repo`] with two (bare) remotes and an upstream that doesn't exist: Push… lists
/// both, a plain Pull fails (a branch *without* an upstream runs no git and
/// shows no ⚠, it opens Pull from… instead).
fn make_remotes_repo(dir: &std::path::Path) -> std::path::PathBuf {
    let repo = make_repo(dir);
    for name in ["origin", "backup"] {
        let bare = dir.with_file_name(format!("{name}.git"));
        git(dir, &["init", "-q", "--bare", bare.to_str().unwrap()]);
        git(&repo, &["remote", "add", name, bare.to_str().unwrap()]);
    }
    let branch = String::from_utf8(
        std::process::Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(&repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let branch = branch.trim();
    git(&repo, &["config", &format!("branch.{branch}.remote"), "origin"]);
    git(&repo, &["config", &format!("branch.{branch}.merge"), "refs/heads/nowhere"]);
    repo
}

/// The command bar sits between branch and buttons at every size and font;
/// the palette (by key and by click) lies over it, centred on it while the window
/// is wide enough, also its second step (Push… → the remotes); a failed command leaves the
/// error mark among the buttons (a Pull without upstream runs no git).
fn toolbar_command_bar(ui: &mut Ui) {
    ui.step("cmd:no-such-command");
    ui.step("focuslist");
    for size in sizes() {
        ui.resize(size.0, size.1);
        for (font_ui, font_diff) in fonts() {
            ui.step(&format!("fonts:{font_ui},{font_diff}"));
            let ctx = format!("{}x{} fonts {font_ui}/{font_diff}", size.0, size.1);
            let p = ui.check(&format!("{ctx}, toolbar"));
            for n in ["command-bar", "error-mark", "push-split", "toolbar-branch"] {
                // A narrow window with a big font has no room left for the branch name.
                if n == "toolbar-branch" && size.0 < 1000. {
                    continue;
                }
                if find(&p, n).and_then(Probe::visible).is_none() {
                    ui.fail(format!("{ctx}: no {n} in the toolbar"));
                }
            }
            ui.step("focuslist");
            ui.step("action:gitti::CommandPalette");
            let p = ui.check(&format!("{ctx}, palette by key"));
            over_bar(ui, &p, &ctx, size.0);
            ui.step("pick:push…");
            ui.step("pickok");
            let p = ui.check(&format!("{ctx}, push step"));
            over_bar(ui, &p, &ctx, size.0);
            // origin, backup, all remotes, Options… (fewer rows where the window is low).
            let rows = p.iter().filter(|q| q.kind() == "picker-item").count();
            if rows < 4 && size.1 >= 640. && font_ui <= 20. {
                ui.fail(format!("{ctx}: push step shows {rows} entries, expected 4"));
            }
            ui.step("action:gitti::ModalCancel");
        }
    }
    // A click on the bar opens it the same way.
    ui.resize(1500., 950.);
    ui.step("fonts:13,13");
    ui.click_on("command-bar", MouseButton::Left);
    let p = ui.check("palette by click");
    over_bar(ui, &p, "click", 1500.);
    ui.step("action:gitti::ModalCancel");
}

/// The palette's keys are a right-aligned column of their own (`layout_check`
/// 3a': one right edge, inside the row, never overlapped — labels truncate before it) at
/// every window size × font size; every entry has the column, the labels keep room, and
/// the keys are not in the labels. Long labels with long keys (Toggle Sidebar …) and
/// entries with a description (Fetch from… — a remote) checked on their own.
fn palette_keys_column(ui: &mut Ui) {
    let ws = ui.ws.clone();
    let picker = |ui: &mut Ui| {
        ui.cx
            .update_window(ui.window, |_, window, cx| testing::state(&ws, window, cx))
            .unwrap()
            .picker
            .expect("palette")
    };
    for size in sizes() {
        ui.resize(size.0, size.1);
        for (font_ui, font_diff) in fonts() {
            ui.step(&format!("fonts:{font_ui},{font_diff}"));
            let ctx = format!("{}x{} fonts {font_ui}/{font_diff}", size.0, size.1);
            ui.step("focuslist");
            ui.step("action:gitti::CommandPalette");
            for query in ["", "toggle", "sidebar", "from…", "file list"] {
                ui.step(&format!("pick:{query}"));
                let ctx = format!("{ctx}, query `{query}`");
                let p = ui.check(&ctx);
                let labels = picker(ui).labels;
                let rows: Vec<&Probe> = p.iter().filter(|q| q.kind() == "picker-item").collect();
                if rows.is_empty() {
                    ui.fail(format!("{ctx}: no entries"));
                }
                for row in &rows {
                    let i = row.index().unwrap_or(usize::MAX);
                    let keys = find(&p, &format!("picker-keys:{i}"));
                    let label = find(&p, &format!("picker-label:{i}"));
                    let (Some(keys), Some(label)) = (keys, label) else {
                        ui.fail(format!("{ctx}: entry {i} without label / keys column"));
                        continue;
                    };
                    // The label keeps a few characters (the keys don't take the row);
                    // short ones ("Pull") are whole anyway (narrow glyphs: "Pull" is 1.6 em
                    // wide in the macOS UI font).
                    let chars = labels.get(i).map_or(0, |l| l.chars().count()) as f32;
                    if f32::from(label.bounds.size.width) < (font_ui * 3.).min(chars * font_ui * 0.3) {
                        ui.fail(format!("{ctx}: label {i} squeezed to {:?}", label.bounds));
                    }
                    if label.bounds.right() > keys.bounds.left() {
                        ui.fail(format!("{ctx}: label {i} reaches into the keys column"));
                    }
                }
            }
            let picker = picker(ui);
            for (label, keys) in picker.labels.iter().zip(&picker.keys) {
                if keys.chars().count() > 1 && label.contains(keys.as_str()) {
                    ui.fail(format!("{ctx}: `{label}` has its keys `{keys}` in the label"));
                }
            }
            ui.step("action:gitti::ModalCancel");
        }
    }
    ui.resize(1500., 950.);
    ui.step("fonts:13,13");
    // The file list is in the palette, with the key of the action (`t`).
    ui.step("focuslist");
    ui.step("action:gitti::CommandPalette");
    ui.step("pick:file list");
    let picker = picker(ui);
    let want = ui.cx.update(|cx| testing::key_hint("gitti::FileList", cx));
    match picker.labels.iter().position(|l| l == "File List…") {
        Some(i) if picker.keys[i] == want && !want.is_empty() => {}
        Some(i) => ui.fail(format!("File List… keys `{}`, expected `{want}`", picker.keys[i])),
        None => ui.fail(format!("no File List… in {:?}", picker.labels)),
    }
    ui.step("action:gitti::ModalCancel");
}

/// The palette's panel lies over the command bar: covers it completely, its
/// query field centred on the bar's height, and — in a wide window — centred on it.
fn over_bar(ui: &mut Ui, p: &[Probe], ctx: &str, width: f32) {
    let (Some(bar), Some(panel)) = (
        find(p, "command-bar").and_then(Probe::visible),
        find(p, "picker").and_then(Probe::visible),
    ) else {
        ui.fail(format!("{ctx}: no command bar / palette"));
        return;
    };
    let f = |x: Pixels| f32::from(x);
    if f(panel.top()) > f(bar.top())
        || f(panel.left()) > f(bar.left())
        || f(panel.right()) < f(bar.right())
        || f(panel.bottom()) < f(bar.bottom())
    {
        ui.fail(format!("{ctx}: palette {panel:?} does not cover the command bar {bar:?}"));
    }
    // Starts at most a field's height above the bar (it lies over it, not somewhere else).
    let above = f(bar.top() - panel.top());
    if above > f(bar.size.height) + 16. {
        ui.fail(format!("{ctx}: palette starts {above} px above the command bar"));
    }
    // Centred on the bar unless that would leave the window (then kept inside).
    let half = f(panel.size.width) / 2.;
    let cx = f(bar.center().x);
    let fits = cx - half >= 8. && cx + half <= width - 8.;
    let off = f(panel.center().x - bar.center().x).abs();
    if fits && off > 2. {
        ui.fail(format!("{ctx}: palette off the bar's centre by {off} px"));
    }
}
