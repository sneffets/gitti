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
    let marks_repo = make_marks_repo(&tmp.path().join("marks"));
    let marks_sweep_repo = make_marks_repo(&tmp.path().join("marks-sweep"));
    let refs_repo = make_refs_repo(&tmp.path().join("refs"));
    let delete_conflict_repo = make_delete_conflict_repo(&tmp.path().join("delete-conflict"));
    let tests: &[(&str, Scenario)] = &[
        ("merge_sync_scroll", merge_sync_scroll),
        ("merge_long_lines", merge_long_lines),
        ("stash_conflict_abort", stash_conflict_abort),
        ("merge_preview_graph", merge_preview_graph),
        ("merge_commit_flow", merge_commit_flow),
        ("askpass_key_icon", askpass_key_icon),
        ("right_click_selects", right_click_selects),
        ("square_corners", square_corners),
        ("drag_autoscroll", drag_autoscroll),
        ("sidebar_toggle", sidebar_toggle),
        ("sidebar_worktrees_sweep", sidebar_worktrees_sweep),
        ("spell_marks", spell_marks),
        ("panes_restore_after_resize", panes_restore_after_resize),
        ("panes_first_frame_after_resize", panes_first_frame_after_resize),
        ("worktree_sweep", worktree_sweep),
        ("file_tabs_sweep", file_tabs_sweep),
        ("repo_tabs_reorder_and_list", repo_tabs_reorder_and_list),
        ("repo_tab_marks_sweep", repo_tab_marks_sweep),
        ("repo_tab_marks_follow", repo_tab_marks_follow),
        ("commit_box_collapse", commit_box_collapse),
        ("commit_view_sweep", commit_view_sweep),
        ("compare_view_sweep", compare_view_sweep),
        ("spell_marks_dialogs", spell_marks_dialogs),
        ("folder_picker_sweep", folder_picker_sweep),
        ("go_to_commit_picker", go_to_commit_picker),
        ("clone_clipboard_note", clone_clipboard_note),
        ("modal_sizing_sweep", modal_sizing_sweep),
        ("toolbar_command_bar", toolbar_command_bar),
        ("palette_keys_column", palette_keys_column),
        ("commit_list_layouts", commit_list_layouts),
        ("ref_badges_sweep", ref_badges_sweep),
        ("delete_conflict_choices", delete_conflict_choices),
    ];
    common::run(tests, |name| {
        if name == "merge_sync_scroll" {
            merge_repo.clone()
        } else if name == "merge_long_lines" {
            long_merge_repo.clone()
        } else if name == "stash_conflict_abort" {
            stash_conflict_repo.clone()
        } else if name == "merge_commit_flow" || name == "merge_preview_graph" {
            merge_conflict_repo.clone()
        } else if name == "sidebar_worktrees_sweep" {
            wt_repo.clone()
        } else if name == "folder_picker_sweep" {
            folder_repo.clone()
        } else if name == "toolbar_command_bar" {
            remotes_repo.clone()
        } else if name == "repo_tab_marks_follow" {
            marks_repo.clone()
        } else if name == "repo_tab_marks_sweep" {
            marks_sweep_repo.clone()
        } else if name == "ref_badges_sweep" {
            refs_repo.clone()
        } else if name == "delete_conflict_choices" {
            delete_conflict_repo.clone()
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
/// branch name (the merge editor drew a too long line over the other side's text).
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
    // Latin-1 (line 3 is `café` in one byte per char): the editor names the encoding.
    let file = |swap: &dyn Fn(usize) -> Option<String>| -> Vec<u8> {
        let text: String = (1..=40)
            .map(|i| match (i, swap(i)) {
                (_, Some(s)) => s,
                (6 | 14, None) => long("stable"),
                (3, None) => "caf\u{e9}\n".into(),
                _ => format!("line {i}\n"),
            })
            .collect();
        text.chars().map(|c| c as u8).collect()
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

/// Merge editor with long lines: each side's line stays inside its half, the toolbar's and
/// the single view bar's texts stay on one line inside their bar — at several window sizes,
/// split and single view. The file is Latin-1: the toolbar names the encoding.
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
            if !single && find(&p, "merge-encoding").is_none() {
                ui.fail(format!("{ctx}: the Latin-1 file's encoding isn't named"));
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
    // Commits a minute apart: same-second commits would sort by their (random) ids, and
    // MERGE_HEAD's row — so the rows `merge_preview_graph` expects — would vary.
    let time = std::cell::Cell::new(1_700_000_000);
    let commit = |args: &[&str]| {
        time.set(time.get() + 60);
        let date = format!("@{} +0000", time.get());
        let out = std::process::Command::new("git")
            .args(args)
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    write("a.txt", "base\n");
    write("b.txt", "b\n");
    git(dir, &["add", "."]);
    commit(&["commit", "-qm", "init"]);
    let conflicting = if merge {
        // The feature commit is the newest: MERGE_HEAD on row 1, so the rows below the
        // dashed lane show also in the smallest window.
        write("a.txt", "main\n");
        commit(&["commit", "-qam", "main change"]);
        git(dir, &["switch", "-qc", "feature", "HEAD~1"]);
        write("a.txt", "feature\n");
        write("b.txt", "b from feature\n");
        commit(&["commit", "-qam", "feature change"]);
        git(dir, &["switch", "-q", "main"]);
        ["merge", "feature"]
    } else {
        write("a.txt", "stashed\n");
        write("b.txt", "b stashed\n");
        git(dir, &["stash", "push", "-q", "-m", "wip"]);
        write("a.txt", "committed\n");
        commit(&["commit", "-qam", "head change"]);
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

/// `<dir>`: a merge where `feature` deleted `gone.txt` that `main` changed (a
/// modify/delete conflict, `UD`).
fn make_delete_conflict_repo(dir: &std::path::Path) -> std::path::PathBuf {
    init_repo(dir);
    let mut s = Stream::new();
    let base = s.commit("refs/heads/main", "Base", None, &[("gone.txt", Some("base\n"))]);
    s.commit("refs/heads/feature", "Delete it", Some(base), &[("gone.txt", None)]);
    s.commit("refs/heads/main", "Change it", Some(base), &[("gone.txt", Some("main\n"))]);
    s.import(dir);
    git(dir, &["reset", "-q", "--hard"]);
    let out = std::process::Command::new("git")
        .args(["merge", "feature"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(!out.status.success(), "expected a conflict");
    dir.to_path_buf()
}

/// A file one side deleted: its header names the conflict ("deleted by feature, changed by
/// main") and offers Keep File / Delete File instead of the merge editor, both whole at the
/// default and the most cramped sweep point; Keep File stages it (`git add`: as main has
/// it, nothing left to show).
fn delete_conflict_choices(ui: &mut Ui) {
    ui.step("select:0");
    let mut checked = 0;
    for pt in points() {
        let ctx = set_point(ui, pt);
        let p = ui.check(&ctx);
        // The most cramped point leaves the diff no room under the commit box.
        if !p.iter().any(|q| q.kind() == "diff-row" && q.detail() == "file" && q.visible().is_some())
        {
            continue;
        }
        checked += 1;
        if !p.iter().any(|q| q.kind() == "conflict-kind") {
            ui.fail(format!("{ctx}: no conflict kind in the file header"));
        }
        if p.iter().any(|q| q.kind() == "resolve-split") {
            ui.fail(format!("{ctx}: a deleted file offers the merge editor"));
        }
        for want in ["Keep", "Delete"] {
            match p.iter().find(|q| q.kind() == "conflict-choice" && q.detail() == want) {
                // Whole in width (a ghost button may overhang the row by half a pixel).
                Some(b) if b.visible().is_some_and(|v| v.size.width == b.bounds.size.width) => {}
                _ => ui.fail(format!("{ctx}: {want} is not (fully) visible")),
            }
        }
    }
    if checked == 0 {
        ui.fail("the conflicted file's header was never in view".into());
    }
    reset_point(ui);
    let p = ui.frame();
    let Some(keep) = p
        .iter()
        .find(|q| q.kind() == "conflict-choice" && q.detail() == "Keep")
        .map(|q| q.name.clone())
    else {
        ui.fail("no Keep File button".into());
        return;
    };
    ui.click_on(&keep, MouseButton::Left);
    let dir = ui.workdir();
    if !ui.wait_real(1500, |ui| ui.is_idle() && git_out(&dir, &["status", "--porcelain"]).is_empty())
    {
        let st = git_out(&dir, &["status", "--porcelain"]);
        ui.fail(format!("Keep File: status {st:?}, want gone.txt resolved as main has it"));
    }
}

/// During a merge the working-directory row is the pending merge commit — a dashed edge
/// from it (row 0) down a lane to MERGE_HEAD's commit (`feature change`), every dash inside
/// its row (layout invariant) at the default and the most cramped sweep point; the rows
/// below have none. MERGE_HEAD's row has the "MERGE_HEAD" badge, no other row has one.
fn merge_preview_graph(ui: &mut Ui) {
    for pt in points() {
        let ctx = set_point(ui, pt);
        let p = ui.check(&ctx);
        let dashes: Vec<&Probe> = p.iter().filter(|q| q.kind() == "graph-dash").collect();
        let rows: std::collections::BTreeSet<usize> = dashes.iter().filter_map(|q| q.index()).collect();
        let last = rows.last().copied().unwrap_or(0);
        if !dashes.iter().any(|q| q.index() == Some(0) && q.detail() != "0-0") {
            ui.fail(format!("{ctx}: no dashed edge out of the working-directory row: {rows:?}"));
        } else if last == 0 || rows.len() != last + 1 {
            ui.fail(format!("{ctx}: dashed rows {rows:?}, want 0..=MERGE_HEAD's row"));
        }
        let below = format!("commit-row:{}", last + 1);
        if find(&p, &below).is_none() {
            // Cramped (Linux: the menu bar in the window, 800x500 at 28 pt: two rows fit):
            // the row below comes into view one row down — and has no dash either.
            let pitch = ui.metrics().commit_row_h;
            ui.wheel("commit-list", pitch);
            let q = ui.check(&format!("{ctx}, a row down"));
            if find(&q, &below).is_none() {
                ui.fail(format!("{ctx}: no row below the dashed lane ({rows:?})"));
            } else if q.iter().any(|d| d.kind() == "graph-dash" && d.index() == Some(last + 1)) {
                ui.fail(format!("{ctx}: the dashed lane goes on below MERGE_HEAD's row {last}"));
            }
            ui.wheel("commit-list", -pitch);
        }
        let badges: Vec<usize> = p
            .iter()
            .filter(|q| q.kind() == "merge-head-badge")
            .filter_map(|q| q.index())
            .collect();
        if badges != [last] {
            ui.fail(format!("{ctx}: MERGE_HEAD badges on rows {badges:?}, want [{last}]"));
        }
    }
    reset_point(ui);
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

/// A credential prompt from the askpass helper shows the key icon (gitti showed
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
    // A prompt whose helper went away (operation cancelled or over) closes by itself; one
    // cancel refuses the prompts queued behind it as well.
    ui.step("ask:Password: ");
    ui.step("askgone");
    if !ui.wait_real(1000, |ui| find(&ui.frame(), "modal").is_none()) {
        ui.fail("askpass dialog of a gone helper still open".into());
    }
    ui.step("ask:Password: ");
    ui.step("ask:Password: ");
    ui.step("cancel");
    if find(&ui.check("askpass cancel drains"), "modal").is_some() {
        ui.fail("cancel left the queued askpass prompt".into());
    }
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

/// Menus, buttons, inputs, tooltips and dialogs are square — the component theme
/// (which the kit's buttons, menus, inputs and tooltips draw with) has no radius; only the
/// ref badges keep `theme::BADGE_RADIUS`. A context menu open, so its frame is checked too.
fn square_corners(ui: &mut Ui) {
    let (radius, radius_lg, radius_full) = ui.cx.update(|cx| {
        let t = gpui_kit::component::Theme::global(cx);
        (t.radius, t.radius_lg, t.radius_full())
    });
    for (name, r) in [("radius", radius), ("radius_lg", radius_lg), ("radius_full", radius_full)] {
        if r != gpui_kit::px(0.) {
            ui.fail(format!("component theme {name} is {r:?}, menus and buttons must be square"));
        }
    }
    ui.click_on("commit-row:1", MouseButton::Right);
    ui.check("context menu open");
    ui.step("key:escape");
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

/// The first frame after a window resize already has the panes at their new sizes — the old
/// ones scaled to the new width / height, as every later frame — with and without the
/// sidebar, and the git log panel's height. (gpui-base's resize state rescales only in
/// prepaint, after the layout: one frame had the extra width split evenly between the
/// panes, e.g. a 300 px sidebar drawn 466 px wide.)
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
                // A narrow row folds badges into its "+N".
                let folded = row.strip_prefix("commit-row:").is_some_and(|i| {
                    let more = format!("ref-more:{i}:");
                    p.iter().any(|q| q.name.starts_with(&more) && q.visible().is_some())
                });
                match (shown(mark), shown(row)) {
                    (Some(m), Some(r)) if testing::contains(&r, &m) => {}
                    (None, None) => {}
                    (None, Some(_)) if folded && mark.starts_with("wt-branch-badge:") => {}
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
    right_click_menu(ui, "spell-mark:0", SpellTarget::Message, "commit editor");
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

/// A real right click on the marked word `mark` (the input builds the menu while its own
/// state is being updated; reading the editor there aborted the app): the menu of
/// `target` offers "teh".
fn right_click_menu(ui: &mut Ui, mark: &str, target: SpellTarget, ctx: &str) {
    testing::take_editor_menu();
    ui.click_on(mark, MouseButton::Right);
    let menu = testing::take_editor_menu();
    if menu != Some((target, Some("teh".into()))) {
        ui.fail(format!("{ctx}: right click on {mark} showed {menu:?}"));
    }
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

/// [`make_repo`] with two (bare) remotes and no upstream: Push… lists both, a plain Pull
/// fails (no tracking information).
fn make_remotes_repo(dir: &std::path::Path) -> std::path::PathBuf {
    let repo = make_repo(dir);
    for name in ["origin", "backup"] {
        let bare = dir.with_file_name(format!("{name}.git"));
        git(dir, &["init", "-q", "--bare", bare.to_str().unwrap()]);
        git(&repo, &["remote", "add", name, bare.to_str().unwrap()]);
    }
    repo
}


/// HEAD's refs in [`make_refs_repo`] (row 1; `main` is checked out), in badge order.
const HEAD_REFS: &[&str] = &[
    "main",
    "feature/a-rather-long-branch-name-for-the-badges",
    "hotfix",
    "next",
    "release/2026.10",
    "origin/feature/a-rather-long-branch-name-for-the-badges",
    "origin/main",
    "upstream/main",
    "stable-release-candidate",
    "v1.0",
    "v1.0.0",
];

/// [`make_repo`] with many refs written as files (no git): HEAD carries [`HEAD_REFS`]
/// (branches, remote branches, three tags), its parent four branches and one tag.
fn make_refs_repo(dir: &std::path::Path) -> std::path::PathBuf {
    let ids = make_repo_ids(dir);
    let (head, parent) = (&ids[5], &ids[4]);
    for name in &HEAD_REFS[1..] {
        let full = if name.starts_with("origin/") || name.starts_with("upstream/") {
            format!("refs/remotes/{name}")
        } else if name.starts_with("v1") || name.starts_with("stable") {
            format!("refs/tags/{name}")
        } else {
            format!("refs/heads/{name}")
        };
        write_ref(dir, &full, head);
    }
    for b in ["b1", "b2", "b3", "b4"] {
        write_ref(dir, &format!("refs/heads/{b}"), parent);
    }
    write_ref(dir, "refs/tags/only-tag", parent);
    dir.to_path_buf()
}

/// Many refs on one commit: the commit header shows every one of them — wrapping
/// inside the header, which keeps its rows (the invariants: no badge overlaps another,
/// all inside the header, the header over its `HeaderSpace` rows only) — at every window
/// and font size; the list row shows at most three plus "+N", the commit's tag always
/// among them (a single tag behind four branches too), not clipped away.
fn ref_badges_sweep(ui: &mut Ui) {
    ui.step("select:1");
    for pt in points() {
        let ctx = set_point(ui, pt);
        let default = pt == (SIZES[0], FONTS[1]);
        let p = ui.check(&format!("{ctx}, many refs"));
        let header = find(&p, "commit-header").map(|h| h.bounds);
        let shown: Vec<&str> = p
            .iter()
            .filter(|q| q.kind() == "header-ref")
            .map(|q| &q.name["header-ref:".len()..])
            .collect();
        if shown != HEAD_REFS {
            ui.fail(format!("{ctx}: header refs {shown:?}, want {HEAD_REFS:?}"));
        }
        for q in p.iter().filter(|q| q.kind() == "header-ref") {
            if !header.is_some_and(|h| testing::contains(&h, &q.bounds)) {
                ui.fail(format!("{ctx}: {} ({:?}) outside the commit header {header:?}", q.name, q.bounds));
            }
            if default && q.visible() != Some(q.bounds) {
                ui.fail(format!("{ctx}: {} not whole on screen", q.name));
            }
        }
        let rows = |name: &str| find(&p, name).map(|q| f32::from(q.bounds.size.height));
        if let (Some(refs_h), Some(badge_h)) = (rows("header-refs"), rows("header-ref:main"))
            && default
            && refs_h < 2. * badge_h
        {
            ui.fail(format!("{ctx}: the refs did not wrap ({refs_h} px for badges {badge_h} px high)"));
        }
        // The list: three badges (fewer in a narrow row), the tag always among them, the
        // "+N" counting the rest.
        for (row, tag, total) in [(1, "stable-release-candidate", HEAD_REFS.len()), (2, "only-tag", 5)] {
            let badge = format!("ref-badge:{row}:{tag}");
            let shown: Vec<&str> = p
                .iter()
                .filter(|q| q.kind() == "ref-badge" && q.index() == Some(row))
                .map(|q| q.detail())
                .collect();
            if !(1..=3).contains(&shown.len()) || (default && shown.len() != 3) || !shown.contains(&tag) {
                ui.fail(format!("{ctx}: row {row} badges {shown:?}, want up to three with {tag}"));
            }
            let more = total - shown.len();
            match find(&p, &badge).and_then(|b| Some((b.bounds, b.visible()?))) {
                Some((b, v)) if default && v != b => {
                    ui.fail(format!("{ctx}: {badge} clipped ({v:?} of {b:?})"))
                }
                Some(_) => {}
                None => ui.fail(format!("{ctx}: {badge} not visible")),
            }
            if find(&p, &format!("ref-more:{row}:{more}")).and_then(Probe::visible).is_none() {
                ui.fail(format!("{ctx}: row {row} shows no +{more}"));
            }
        }
    }
    reset_point(ui);
}

// ------------------------------------------------------------------ lean scenarios

/// The representative (window size, fonts) points of the sweeps: the default and the most
/// cramped window with the biggest fonts (the whole matrix with `GITTI_TEST_FULL=1`).
fn points() -> Vec<((f32, f32), (f32, f32))> {
    let all: Vec<((f32, f32), (f32, f32))> =
        SIZES.iter().flat_map(|s| FONTS.iter().map(move |f| (*s, *f))).collect();
    let default = all.iter().position(|p| *p == (SIZES[0], FONTS[1])).unwrap();
    common::sweep(&all, &[default, all.len() - 1])
}

/// Sets a sweep point; returns its name.
fn set_point(ui: &mut Ui, (size, (font_ui, font_diff)): ((f32, f32), (f32, f32))) -> String {
    ui.resize(size.0, size.1);
    ui.step(&format!("fonts:{font_ui},{font_diff}"));
    format!("{}x{} fonts {font_ui}/{font_diff}", size.0, size.1)
}

/// Back to the default window and fonts (settings persist between the scenarios).
fn reset_point(ui: &mut Ui) {
    ui.resize(SIZES[0].0, SIZES[0].1);
    ui.step("fonts:13,13");
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

fn picker(ui: &mut Ui) -> Option<testing::PickerState> {
    ui.ui_state().picker
}

/// The worktree view at the sweep points × a short and a long message keeps the invariants:
/// the file tabs above the commit box; the editor as tall as its text (whole lines, at most
/// half the panel); the Commit button whole; the identity line one line; scrolled down and
/// back.
fn worktree_sweep(ui: &mut Ui) {
    for pt in points() {
        let ctx = set_point(ui, pt);
        for lines in common::sweep(&[0, 1, 5, 30], &[1, 3]) {
            let ctx = format!("{ctx}, {lines}-line message");
            ui.step(&format!("msg:{}", message(lines)));
            ui.step("focusmsg");
            let p = ui.check(&format!("{ctx}, editing"));
            if find(&p, "file-tabs").is_none() || find(&p, "file-tab:3").is_none() {
                ui.fail(format!("{ctx}: no file tabs above the commit box"));
            }
            let m = ui.metrics();
            match (find(&p, "commit-editor"), find(&p, "commit-box"), find(&p, "detail")) {
                (Some(ed), Some(bx), Some(d)) => {
                    let h = f32::from(ed.bounds.size.height);
                    let chrome = (h - m.message_line_h).rem_euclid(m.message_line_h);
                    let rows = ((h - chrome) / m.message_line_h).round();
                    let (box_h, panel_h) = (f32::from(bx.bounds.size.height), f32::from(d.bounds.size.height));
                    if rows < lines.clamp(1, 2) as f32 || chrome > 20. || (rows > 2. && box_h > panel_h / 2. + 1.) {
                        ui.fail(format!(
                            "{ctx}: editor is {h} px tall ({rows} lines of {} px + {chrome}), box {box_h} of {panel_h} px",
                            m.message_line_h
                        ));
                    }
                }
                _ => ui.fail(format!("{ctx}: no commit editor / box / panel")),
            }
            match (find(&p, "commit-button"), find(&p, "detail")) {
                (Some(b), Some(d)) if b.visible() == Some(b.bounds) && testing::contains(&d.bounds, &b.bounds) => {}
                _ => ui.fail(format!("{ctx}: the Commit button is not (fully) visible")),
            }
            match find(&p, "commit-identity") {
                Some(id) if f32::from(id.bounds.size.height) < 1.5 * m.message_line_h => {}
                Some(id) => ui.fail(format!("{ctx}: the identity line wraps ({} px tall)", f32::from(id.bounds.size.height))),
                None => ui.fail(format!("{ctx}: no identity line")),
            }
            ui.step("focuslist");
            for dy in [90., 270., -1e5] {
                ui.wheel("diff-list", dy);
                ui.check(&format!("{ctx}, scrolled {dy}"));
            }
        }
    }
    ui.step("msg:");
    reset_point(ui);
}

/// A file tab of the working directory active, at the sweep points and scrolled: the active
/// tab drawn inside its strip, the diff only the tab's file; the ▾ at the strip's right
/// edge opens the list of every tab under it (right-aligned with it, inside the window);
/// typing narrows it, Enter shows that tab, reopened it selects the active one — no git;
/// the commit view's list.
fn file_tabs_sweep(ui: &mut Ui) {
    ui.step("select:0");
    ui.click_on("file-tab:1", MouseButton::Left);
    let ws = ui.ws.clone();
    let files = |ui: &mut Ui| ui.ui_state().rows.into_iter().filter(|r| r.starts_with("file ")).collect::<Vec<_>>();
    let active = ui.cx.update(|cx| testing::worktree_tabs(&ws, cx)).1;
    if active.as_deref() != Some("big.txt") || files(ui) != ["file Unstaged big.txt"] {
        let f = files(ui);
        ui.fail(format!("file-tab:1: active {active:?}, shows {f:?}"));
    }
    for pt in points() {
        let ctx = set_point(ui, pt);
        let p = ui.check(&format!("{ctx}, big.txt tab"));
        match (find(&p, "file-tabs"), find(&p, "file-tab:1")) {
            (Some(strip), Some(tab))
                if tab.visible().is_some()
                    && strip.visible().is_some_and(|s| testing::contains(&s, &tab.visible().unwrap())) => {}
            _ => ui.fail(format!("{ctx}: the active tab is not drawn in its strip")),
        }
        match (find(&p, "file-tabs-menu").and_then(Probe::visible), find(&p, "file-tabs")) {
            (Some(m), Some(bar)) if (f32::from(m.right()) - f32::from(bar.bounds.right())).abs() < 2. => {}
            other => ui.fail(format!("{ctx}: no ▾ at the right edge of the file tabs: {other:?}")),
        }
        ui.wheel("diff-list", 240.);
        ui.check(&format!("{ctx}, big.txt tab scrolled"));
        ui.wheel("diff-list", -1e5);
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
        let window = (pt.0).0;
        let height = (pt.0).1;
        if f32::from(panel.top()) + 1.5 < f32::from(menu.bottom()) && f32::from(panel.bottom()) + 8.5 < height {
            ui.fail(format!("{ctx}: the list starts above the ▾ ({panel:?}, ▾ {menu:?})"));
        }
        if (f32::from(panel.right()) - f32::from(menu.right())).abs() > 2. && f32::from(panel.left()) > 8.5 {
            ui.fail(format!("{ctx}: the list is not right-aligned with the ▾ ({panel:?}, ▾ {menu:?})"));
        }
        if f32::from(panel.left()) < -0.5 || f32::from(panel.right()) > window + 0.5 {
            ui.fail(format!("{ctx}: the list leaves the window ({panel:?})"));
        }
        ui.step("key:escape");
    }
    reset_point(ui);
    // Type to narrow, Enter shows the tab; no git; reopened, the active tab is selected.
    let n = ui.n_git();
    ui.click_on("file-tabs-menu", MouseButton::Left);
    ui.quick("pick:new");
    match picker(ui) {
        Some(p) if p.labels.first().map(String::as_str) == Some("new.txt") => {}
        other => ui.fail(format!("typing `new` into the file list: {other:?}")),
    }
    ui.quick("pickok");
    let active = ui.cx.update(|cx| testing::worktree_tabs(&ws, cx)).1;
    if active.as_deref() != Some("new.txt") || picker(ui).is_some() {
        ui.fail(format!("Enter on new.txt: active tab {active:?}"));
    }
    ui.click_on("file-tabs-menu", MouseButton::Left);
    match picker(ui) {
        Some(p) if p.labels.get(p.selected).map(String::as_str) == Some("new.txt") => {}
        other => ui.fail(format!("reopened list does not select the active tab: {other:?}")),
    }
    ui.quick("key:escape");
    ui.no_git_since(n, "the file list");
    ui.click_on("file-tab:0", MouseButton::Left);
    if ui.cx.update(|cx| testing::worktree_tabs(&ws, cx)).1.is_some() {
        ui.fail("All Changes left a tab active".into());
    }
    // The commit view (newest commit: f0, f1, f2).
    ui.step("select:1");
    ui.click_on("file-tabs-menu", MouseButton::Left);
    match picker(ui) {
        Some(p) if p.labels.len() == 4 && p.labels[0] == "All Files (3)" => {}
        other => ui.fail(format!("commit file list: {other:?}")),
    }
    ui.quick("pick:f1");
    ui.quick("pickok");
    if files(ui) != ["file Commit f1.txt"] {
        let f = files(ui);
        ui.fail(format!("commit file list → f1.txt shows {f:?}"));
    }
}

/// Repository tabs move by drag and drop (the dragged tab takes the place of the one it is
/// dropped on; the active one stays active), the order is saved; the ▾ at the right edge of
/// the tab bar lists every repository and switches.
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
    let center = |p: &[Probe], n: &str| find(p, n).and_then(Probe::visible).map(|b| b.center());
    let (Some(from), Some(to)) = (center(&p, "repo-tab:0"), center(&p, "repo-tab:2")) else {
        ui.fail("no repo tabs drawn".into());
        return;
    };
    ui.mouse(MouseButton::Left, Some(true), from);
    ui.mouse(MouseButton::Left, None, point(from.x + px(12.), from.y));
    ui.mouse(MouseButton::Left, None, to);
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
    ui.click_on("repo-tabs-menu", MouseButton::Left);
    ui.check("repository list open");
    ui.quick("key:down");
    ui.step("key:enter");
    let (_, active) = tabs(ui);
    if active != 0 {
        ui.fail(format!("the ▾ list's first entry made tab {active} active"));
    }
}

/// `main` one commit ahead of its upstream `origin/main`, the working tree clean. Three git
/// processes.
fn make_marks_repo(dir: &std::path::Path) -> std::path::PathBuf {
    init_repo(dir);
    let mut s = Stream::new();
    let one = s.commit("refs/heads/main", "One", None, &[("a.txt", Some("one\n"))]);
    s.reset("refs/remotes/origin/main", Some(one));
    let two = s.commit("refs/heads/main", "Two", Some(one), &[("a.txt", Some("two\n"))]);
    let ids = s.import(dir);
    read_tree(dir, &ids[two]);
    append_config(
        dir,
        "[remote \"origin\"]\n\turl = ../origin.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n\
         [branch \"main\"]\n\tremote = origin\n\tmerge = refs/heads/main\n",
    );
    dir.to_path_buf()
}

/// Per repository tab: its marks and their tooltip.
fn tab_marks(ui: &mut Ui) -> Vec<(String, String)> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::tab_marks(&ws, cx))
}

/// Where tab `ix`'s name starts inside its tab, and its vertical centre.
fn title_place(p: &[Probe], ix: usize) -> Option<(f32, f32)> {
    let tab = find(p, &format!("repo-tab:{ix}"))?;
    let t = find(p, &format!("repo-tab-title:{ix}"))?;
    Some((
        f32::from(t.bounds.left()) - f32::from(tab.bounds.left()),
        f32::from(t.bounds.center().y),
    ))
}

/// A tab's mark right of its name: a hollow circle for commits not pushed, a filled one
/// for uncommitted changes — only the filled one when both hold — centred on the name's
/// line, inside the tab, never over the name (the layout invariants check every frame), the
/// name where it is in a tab without a mark; at the default and the biggest fonts in the
/// default and the most cramped window, with a second tab open.
fn repo_tab_marks_sweep(ui: &mut Ui) {
    let other = ui.repo.parent().unwrap().join("marks-sweep-other");
    if !other.exists() {
        git(ui.repo.parent().unwrap(), &["init", "-q", "-b", "main", "marks-sweep-other"]);
    }
    ui.step(&format!("open:{}", other.display()));
    ui.step("tab:0");
    let marks = tab_marks(ui);
    if marks.iter().map(|m| m.0.as_str()).collect::<Vec<_>>() != ["○", ""] {
        ui.fail(format!("ahead of origin/main, clean / an empty repository: {marks:?}"));
    }
    sweep_tab_mark(ui, "tab-unpushed:0", "tab-dirty:0");
    std::fs::write(ui.repo.join("a.txt"), "changed\n").unwrap();
    if !ui.wait_real(1500, |ui| ui.is_idle() && tab_marks(ui)[0].0 == "●") {
        let marks = tab_marks(ui);
        ui.fail(format!("after an outside change: {marks:?}"));
        return;
    }
    // Uncommitted outranks unpushed: the filled circle alone.
    sweep_tab_mark(ui, "tab-dirty:0", "tab-unpushed:0");
}

/// Over the sweep's points: tab 0 shows `shown` (and not `hidden`) on its name's line, its
/// name where tab 1's is.
fn sweep_tab_mark(ui: &mut Ui, shown: &str, hidden: &str) {
    for point in points() {
        let ctx = set_point(ui, point);
        let p = ui.check(&format!("{ctx}, {shown}"));
        let vis = |n: &str| find(&p, n).and_then(Probe::visible);
        match (vis(shown), vis(hidden), title_place(&p, 0), title_place(&p, 1)) {
            (Some(m), None, Some((x0, cy)), Some((x1, _)))
                if (f32::from(m.center().y) - cy).abs() <= 2. && (x0 - x1).abs() < 0.5 => {}
            other => {
                let marks = tab_marks(ui);
                ui.fail(format!(
                    "{ctx}: only {shown} on the name's line, the name in place: {other:?}, marks {marks:?}"
                ))
            }
        }
    }
    reset_point(ui);
}

/// The marks follow what happens: an outside change (the file watcher) adds the filled
/// circle, staging keeps it, a commit takes it away and the hollow one counts two commits;
/// a change in a background tab's folder shows on its tab. The empty repository next to it
/// has none. Tooltips say it in words.
fn repo_tab_marks_follow(ui: &mut Ui) {
    let marks = tab_marks(ui);
    if marks != [("○".to_string(), "1 commit not pushed to origin/main".to_string())] {
        ui.fail(format!("ahead of origin/main, clean: {marks:?}"));
    }
    std::fs::write(ui.repo.join("outside.txt"), "x\n").unwrap();
    let seen = ui.wait_real(1500, |ui| ui.is_idle() && tab_marks(ui)[0].0 == "●");
    let marks = tab_marks(ui);
    let want = "Uncommitted changes in 1 file: 1 untracked\n1 commit not pushed to origin/main";
    if !seen || marks[0].1 != want {
        ui.fail(format!("after an outside change: {marks:?}"));
    }
    ui.step("stagefile:outside.txt");
    let marks = tab_marks(ui);
    if marks[0].0 != "●" || !marks[0].1.starts_with("Uncommitted changes in 1 file: 1 staged") {
        ui.fail(format!("after staging: {marks:?}"));
    }
    ui.step("msg:Add outside");
    ui.step("commit");
    let marks = tab_marks(ui);
    if marks[0] != ("○".to_string(), "2 commits not pushed to origin/main".to_string()) {
        ui.fail(format!("after the commit: {marks:?}"));
    }
    let other = ui.repo.parent().unwrap().join("marks-other");
    if !other.exists() {
        git(ui.repo.parent().unwrap(), &["init", "-q", "-b", "main", "marks-other"]);
    }
    ui.step(&format!("open:{}", other.display()));
    // The first tab in the background: its watcher still reports.
    std::fs::write(ui.repo.join("later.txt"), "x\n").unwrap();
    let seen = ui.wait_real(1500, |ui| tab_marks(ui)[0].0 == "●");
    let marks = tab_marks(ui);
    if !seen || marks[1] != (String::new(), String::new()) {
        ui.fail(format!("a change in the background tab: {marks:?}"));
    }
}

/// Scrolling down shrinks the commit box to exactly one bold line (the summary), scrolling
/// back up at the top restores it; focusing the editor brings the whole box back. Default
/// and biggest fonts.
fn commit_box_collapse(ui: &mut Ui) {
    for (font_ui, font_diff) in fonts() {
        ui.step(&format!("fonts:{font_ui},{font_diff}"));
        let ctx = format!("fonts {font_ui}/{font_diff}");
        ui.step(&format!("msg:{}", message(5)));
        ui.step("focuslist");
        ui.step("select:0");
        let full_h = find(&ui.frame(), "commit-box").map(|b| f32::from(b.bounds.size.height));
        let collapse = |ui: &mut Ui| {
            for _ in 0..3 {
                ui.wheel("diff-list", 240.);
            }
        };
        collapse(ui);
        let p = ui.check(&format!("{ctx}, collapsed"));
        let m = ui.metrics();
        match (find(&p, "commit-box"), find(&p, "commit-summary")) {
            (Some(b), Some(s)) => {
                let (bh, sh) = (f32::from(b.bounds.size.height), f32::from(s.bounds.size.height));
                if (bh - m.commit_box_min_h).abs() > 2. {
                    ui.fail(format!("{ctx}: collapsed box is {bh} px, expected {}", m.commit_box_min_h));
                }
                if sh < m.message_line_h {
                    ui.fail(format!("{ctx}: summary {sh} px is lower than a message line ({})", m.message_line_h));
                }
            }
            (b, s) => ui.fail(format!("{ctx}: collapsed: commit-box {}, commit-summary {}", b.is_some(), s.is_some())),
        }
        ui.wheel("diff-list", -1e5);
        ui.wheel("diff-list", -1e5);
        let p = ui.check(&format!("{ctx}, expanded again"));
        let back_h = find(&p, "commit-box").map(|b| f32::from(b.bounds.size.height));
        if find(&p, "commit-summary").is_some() || back_h != full_h {
            ui.fail(format!("{ctx}: box after scrolling back {back_h:?}, before {full_h:?}"));
        }
        collapse(ui);
        ui.step("focusmsg");
        let h = find(&ui.check(&format!("{ctx}, focused after collapse")), "commit-box").map(|b| f32::from(b.bounds.size.height));
        if h != full_h {
            ui.fail(format!("{ctx}: focused box {h:?}, full {full_h:?}"));
        }
        ui.step("focuslist");
    }
    ui.step("msg:");
    reset_point(ui);
}

/// The commit view's header with a long message scrolls under the file tabs, never over
/// them; message lines have the message line pitch; switching between a long and a short
/// header keeps every frame valid.
fn commit_view_sweep(ui: &mut Ui) {
    for pt in points() {
        let ctx = set_point(ui, pt);
        ui.step("select:1");
        let p = ui.check(&format!("{ctx}, commit view top"));
        let m = ui.metrics();
        if find(&p, "file-tabs").is_none() {
            ui.fail(format!("{ctx}: no file tabs"));
        }
        let lines: Vec<&Probe> = p.iter().filter(|p| p.kind() == "msg-line").collect();
        if lines.len() < 10 {
            ui.fail(format!("{ctx}: only {} message lines", lines.len()));
        }
        for l in &lines {
            let h = f32::from(l.bounds.size.height);
            let n = (h / m.message_line_h).round().max(1.);
            if (h - n * m.message_line_h).abs() > 1. {
                ui.fail(format!("{ctx}: {} is {h} px, message line height is {}", l.name, m.message_line_h));
                break;
            }
        }
        for dy in [70., 210., 420.] {
            ui.wheel("diff-list", dy);
            ui.check(&format!("{ctx}, scrolled {dy}"));
        }
        for row in [2, 1] {
            ui.step(&format!("select:{row}"));
        }
        ui.step("select:0");
    }
    reset_point(ui);
}

/// The compare banner sits above the file tabs and the diff list, its swap / end buttons
/// whole inside it, the totals left of them, no commit header; it stays put while
/// scrolling; the base's own diff shows the base hint and mark; the working directory
/// against the base: banner, no commit box; Esc ends it.
fn compare_view_sweep(ui: &mut Ui) {
    for pt in points() {
        let ctx = set_point(ui, pt);
        let wide = (pt.0).0 >= 1400. && (pt.1).0 <= 16.;
        ui.step("select:5");
        ui.step("focuslist");
        ui.step("key:shift-c");
        let p = ui.check(&format!("{ctx}, base selected"));
        if find(&p, "compare-banner").is_none() || find(&p, "compare-mark:5").is_none() {
            ui.fail(format!("{ctx}: no base hint / base mark"));
        }
        ui.step("select:1");
        let p = ui.check(&format!("{ctx}, comparing"));
        let Some(banner) = find(&p, "compare-banner").and_then(Probe::visible) else {
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
                Some(b) if b.visible().is_some_and(|v| v == b.bounds) && testing::contains(&banner, &b.bounds) => {}
                _ => ui.fail(format!("{ctx}: {button} not whole inside the banner")),
            }
        }
        match (find(&p, "compare-totals"), find(&p, "compare-swap")) {
            (Some(t), Some(swap)) => {
                if t.bounds.origin.x + t.bounds.size.width > swap.bounds.origin.x + px(0.5) {
                    ui.fail(format!("{ctx}: the totals run under the swap button"));
                }
                if wide && t.visible() != Some(t.bounds) {
                    ui.fail(format!("{ctx}: the totals are cut in a wide window"));
                }
            }
            _ => ui.fail(format!("{ctx}: no compare totals")),
        }
        ui.wheel("diff-list", 180.);
        let p = ui.check(&format!("{ctx}, compare scrolled"));
        if find(&p, "compare-banner").and_then(Probe::visible) != Some(banner) {
            ui.fail(format!("{ctx}: the banner moved while scrolling"));
        }
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
    reset_point(ui);
}

/// Spell checking in the dialogs' text areas and fields at the biggest fonts: the reword
/// dialog marks `teh` and `wrnog` (not in code spans), its right-click offers the
/// suggestions and choosing one fixes the word; Create Tag checks the message, not the tag
/// name; marks drawn inside their editor; off, nothing is marked.
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
    let (font_ui, font_diff) = FONTS[FONTS.len() - 1];
    ui.step(&format!("fonts:{font_ui},{font_diff}"));
    ui.step("select:1");
    ui.step("action:gitti::EditCommitMessage");
    let title = ui.cx.update(|cx| testing_menus::modal(&ws, cx)).map(|m| m.0);
    if !title.as_deref().is_some_and(|t| t.starts_with("Edit message of")) {
        ui.fail(format!("Edit Commit Message dialog: {title:?}"));
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
        ui.fail(format!("reword dialog marked {got:?}, expected teh, wrnog"));
    }
    inside(ui, "reword dialog", "modal-text", 2);
    right_click_menu(ui, "modal-spell-mark:0", SpellTarget::ModalText, "reword dialog");
    let offer = ui.cx.update(|cx| testing::spell_at(&ws, SpellTarget::ModalText, 5, cx));
    if offer != Some(("teh".into(), vec!["the".into(), "ten".into()])) {
        ui.fail(format!("reword menu at teh offers {offer:?}"));
    }
    ui.cx
        .update_window(ui.window, |_, window, cx| testing::spell_choose(&ws, SpellTarget::ModalText, 0, window, cx))
        .unwrap();
    ui.settle();
    let got = marks(ui, SpellTarget::ModalText);
    if got != ["wrnog"] {
        ui.fail(format!("after choosing \"the\" the reword dialog marks {got:?}"));
    }
    ui.step("cancel");
    ui.step("action:gitti::CreateTag");
    ui.step("field:0=tehh-wrnog");
    ui.step("field:1=Release teh build");
    let (name, msg) = (marks(ui, SpellTarget::ModalField(0)), marks(ui, SpellTarget::ModalField(1)));
    if !name.is_empty() || msg != ["teh"] {
        ui.fail(format!("tag dialog marked name {name:?}, message {msg:?}"));
    }
    inside(ui, "tag message", "modal-field:1", 1);
    right_click_menu(ui, "modal-spell-mark:0", SpellTarget::ModalField(1), "tag message");
    ui.cx.update(|cx| testing::spell_at(&ws, SpellTarget::ModalField(1), 9, cx));
    ui.cx
        .update_window(ui.window, |_, window, cx| testing::spell_choose(&ws, SpellTarget::ModalField(1), 0, window, cx))
        .unwrap();
    ui.settle();
    let fields = ui.cx.update(|cx| testing_menus::modal(&ws, cx)).map(|m| m.1);
    if fields.as_ref().and_then(|f| f.get(1)).map(String::as_str) != Some("Release the build") {
        ui.fail(format!("tag message after the suggestion: {fields:?}"));
    }
    ui.step("cancel");
    // Off: the dialogs aren't checked either.
    ui.step("palette:Toggle Spell Checking");
    ui.step("action:gitti::CreateTag");
    ui.step("field:1=Release teh build");
    if !marks(ui, SpellTarget::ModalField(1)).is_empty() {
        ui.fail("tag message marked while spell checking is off".into());
    }
    inside(ui, "spell checking off", "modal-field:1", 0);
    ui.step("cancel");
    ui.step("palette:Toggle Spell Checking");
    reset_point(ui);
}

/// The folder picker over Add Worktree at the sweep points: "Choose…" right of the folders
/// field inside the dialog; the picker, its OK and summary whole in the window, its rows
/// inside its list (invariants) after expanding and scrolling to the end.
fn folder_picker_sweep(ui: &mut Ui) {
    for pt in points() {
        let ctx = set_point(ui, pt);
        ui.step("focuslist");
        ui.step("action:gitti::AddWorktree");
        let p = ui.check(&format!("{ctx}, add worktree"));
        match (find(&p, "modal-choice:1"), find(&p, "modal-field:3"), find(&p, "modal")) {
            (Some(c), Some(f), Some(m))
                if c.bounds.left() > f.bounds.left()
                    && c.bounds.right() <= m.bounds.right()
                    && c.bounds.top() >= f.bounds.top()
                    && c.bounds.bottom() <= f.bounds.bottom() + px(1.) => {}
            other => ui.fail(format!("{ctx}: Choose… not right of the folders field: {:?}", other.0.map(|c| c.bounds))),
        }
        ui.step("choice:1");
        ui.quick("folders:expand,dir01");
        ui.quick("folders:expand,dir01/sub");
        ui.quick("folders:toggle,dir01/sub");
        ui.step("key:end");
        let p = ui.check(&format!("{ctx}, folder picker"));
        for name in ["folder-picker", "folder-picker-ok", "folder-summary"] {
            match find(&p, name) {
                Some(r) if r.visible() == Some(r.bounds) => {}
                other => ui.fail(format!("{ctx}: {name} not (fully) visible: {:?}", other.map(|r| (r.bounds, r.visible())))),
            }
        }
        let rows = p.iter().filter(|r| r.kind() == "folder-row").count();
        if rows < 3 {
            ui.fail(format!("{ctx}: only {rows} folder rows drawn"));
        }
        ui.step("key:escape");
        ui.step("cancel");
    }
    reset_point(ui);
}

/// The Clone dialog with a URL from the clipboard at the sweep points: "From the
/// clipboard" whole under the URL field, inside the dialog, above the next field.
fn clone_clipboard_note(ui: &mut Ui) {
    for pt in points() {
        let ctx = set_point(ui, pt);
        ui.step("focuslist");
        ui.quick("clip:git@github.com:team/a-rather-long-repository-name-for-the-dialog.git");
        ui.quick("action:gitti::CloneRepository");
        let p = ui.check(&format!("{ctx}, clone dialog"));
        let get = |n: &str| find(&p, n);
        // (At the most cramped point the note may be below the dialog's scrolled part.)
        let below = get("modal-body").is_some_and(|b| {
            get("modal-note:0").is_some_and(|n| n.bounds.bottom() > b.bounds.bottom())
        });
        match (get("modal-note:0"), get("modal-field:0"), get("modal-field:1"), get("modal")) {
            (Some(n), Some(f), Some(next), Some(m))
                if (n.visible() == Some(n.bounds) || below)
                    && n.bounds.bottom() <= f.bounds.bottom() + px(1.)
                    && n.bounds.bottom() <= next.bounds.top()
                    && n.bounds.right() <= m.bounds.right() => {}
            other => ui.fail(format!(
                "{ctx}: clipboard note not whole under the URL field: {:?}",
                other.0.map(|n| (n.bounds, n.visible()))
            )),
        }
        ui.quick("cancel");
    }
    reset_point(ui);
}

/// Dialogs at the sweep points (the most cramped: 800x500 with fonts 28/32): the dialog
/// whole in the window, its title and buttons whole, the part between them scrolling —
/// every field and check (or the text box) reachable by the wheel, nothing of it under
/// the buttons (invariant 9); at the default point the first and last shown at once.
/// Representative dialogs: Clone (fields + a check), Add Worktree (options, many fields,
/// a button beside one), Edit Commit Message (a text box).
fn modal_sizing_sweep(ui: &mut Ui) {
    let dialogs: [(&str, &str, &str); 3] = [
        ("clone", "action:gitti::CloneRepository", "modal-check:0"),
        ("add worktree", "action:gitti::AddWorktree", "modal-field:3"),
        ("edit message", "action:gitti::EditCommitMessage", "modal-text"),
    ];
    for pt in points() {
        let ctx = set_point(ui, pt);
        for (what, open, last) in dialogs {
            let ctx = format!("{ctx}, {what}");
            ui.step("select:1");
            ui.step("focuslist");
            ui.quick("clip:git@github.com:team/a-rather-long-repository-name-for-the-dialog.git");
            ui.step(open);
            let p = ui.check(&format!("{ctx}, opened"));
            modal_whole(ui, &p, &ctx);
            let first = if what == "edit message" { "modal-text" } else { "modal-field:0" };
            // Shown (enough of it to see what it is): whole, or 40 px of a bigger one.
            let shown = |p: &[Probe], name: &str| {
                find(p, name).is_some_and(|l| {
                    l.visible().is_some_and(|v| v.size.height + px(0.5) >= l.bounds.size.height.min(px(40.)))
                })
            };
            if pt == points()[0] && !(shown(&p, first) && shown(&p, last)) {
                ui.fail(format!("{ctx}: {first} / {last} not shown at once"));
            }
            let Some(b) = find(&p, "modal-body").map(|b| b.bounds) else {
                ui.fail(format!("{ctx}: no modal-body"));
                ui.step("cancel");
                continue;
            };
            let (x, y) = (f32::from(b.center().x), f32::from(b.center().y));
            // Wheel down a page at a time: every field / check / the text box gets shown.
            let parts: Vec<String> = p
                .iter()
                .filter(|q| matches!(q.kind(), "modal-field" | "modal-check") || q.name == "modal-text")
                .map(|q| q.name.clone())
                .collect();
            let mut seen: Vec<String> = Vec::new();
            let page = (f32::from(b.size.height) * 0.6).max(20.);
            for step in 0..12 {
                let p = if step == 0 { p.clone() } else {
                    ui.step(&format!("wheel:{x},{y},{page}"));
                    ui.check(&format!("{ctx}, scrolled {step}"))
                };
                modal_whole(ui, &p, &ctx);
                seen.extend(parts.iter().filter(|n| shown(&p, n)).cloned());
                if parts.iter().all(|n| seen.contains(n)) {
                    break;
                }
            }
            let missed: Vec<&String> = parts.iter().filter(|n| !seen.contains(n)).collect();
            if !missed.is_empty() || !parts.iter().any(|n| n == last) {
                ui.fail(format!("{ctx}: not reachable by scrolling: {missed:?} (of {parts:?})"));
            }
            ui.step("cancel");
        }
    }
    reset_point(ui);
}

/// The open dialog whole in the window; its OK button whole; nothing of its scrolling part
/// drawn over the buttons.
fn modal_whole(ui: &mut Ui, p: &[Probe], ctx: &str) {
    let (Some(m), Some(body)) = (find(p, "modal"), find(p, "modal-body")) else {
        ui.fail(format!("{ctx}: no dialog"));
        return;
    };
    if m.visible() != Some(m.bounds) {
        ui.fail(format!("{ctx}: dialog {:?} not whole in the window ({:?})", m.bounds, m.visible()));
    }
    match find(p, "modal-ok") {
        Some(ok) if ok.visible() == Some(ok.bounds) && body.bounds.bottom() <= ok.bounds.top() => {}
        other => ui.fail(format!(
            "{ctx}: OK not whole below the scrolling part {:?}: {:?}",
            body.bounds,
            other.map(|o| (o.bounds, o.visible()))
        )),
    }
}

/// Go to Commit with a file search: the note above the results and the match reasons in the
/// rows stay in place at the sweep points.
fn go_to_commit_picker(ui: &mut Ui) {
    for pt in points() {
        let ctx = set_point(ui, pt);
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
    reset_point(ui);
}

/// The palette's panel lies over the command bar: covers it completely, its query field
/// centred on the bar's height, and — in a wide window — centred on it.
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
    let above = f(bar.top() - panel.top());
    if above > f(bar.size.height) + 16. {
        ui.fail(format!("{ctx}: palette starts {above} px above the command bar"));
    }
    let half = f(panel.size.width) / 2.;
    let cx = f(bar.center().x);
    let fits = cx - half >= 8. && cx + half <= width - 8.;
    let off = f(panel.center().x - bar.center().x).abs();
    if fits && off > 2. {
        ui.fail(format!("{ctx}: palette off the bar's centre by {off} px"));
    }
}

/// The command bar sits between branch and buttons, with the error mark of a failed command
/// among the buttons; the palette (by key and by click) lies over it, centred while the
/// window is wide enough, also its second step (Push… → the remotes).
fn toolbar_command_bar(ui: &mut Ui) {
    ui.step("cmd:no-such-command");
    ui.step("focuslist");
    for pt in points() {
        let ctx = set_point(ui, pt);
        let (w, h) = pt.0;
        let p = ui.check(&format!("{ctx}, toolbar"));
        for n in ["command-bar", "error-mark", "push-split", "toolbar-branch"] {
            if n == "toolbar-branch" && w < 1000. {
                continue;
            }
            if find(&p, n).and_then(Probe::visible).is_none() {
                ui.fail(format!("{ctx}: no {n} in the toolbar"));
            }
        }
        ui.step("focuslist");
        ui.step("action:gitti::CommandPalette");
        let p = ui.check(&format!("{ctx}, palette by key"));
        over_bar(ui, &p, &ctx, w);
        ui.quick("pick:push…");
        ui.step("pickok");
        let p = ui.check(&format!("{ctx}, push step"));
        over_bar(ui, &p, &ctx, w);
        let rows = p.iter().filter(|q| q.kind() == "picker-item").count();
        if rows < 4 && h >= 640. && (pt.1).0 <= 20. {
            ui.fail(format!("{ctx}: push step shows {rows} entries, expected 4"));
        }
        ui.step("action:gitti::ModalCancel");
    }
    reset_point(ui);
    ui.click_on("command-bar", MouseButton::Left);
    let p = ui.check("palette by click");
    over_bar(ui, &p, "click", SIZES[0].0);
    ui.step("action:gitti::ModalCancel");
}

/// The palette's keys are a right-aligned column of their own (`layout_check` 3a') at the
/// sweep points, for a few queries (long labels with long keys, entries with a
/// description): every entry has the column, the labels keep room and stay left of it, the
/// keys are not in the labels; Go to File… has `t`.
fn palette_keys_column(ui: &mut Ui) {
    for pt in points() {
        let ctx = set_point(ui, pt);
        let font_ui = (pt.1).0;
        ui.step("focuslist");
        ui.step("action:gitti::CommandPalette");
        for query in ["", "sidebar", "from…"] {
            ui.quick(&format!("pick:{query}"));
            let ctx = format!("{ctx}, query `{query}`");
            let p = ui.check(&ctx);
            let labels = picker(ui).map(|p| p.labels).unwrap_or_default();
            let rows: Vec<&Probe> = p.iter().filter(|q| q.kind() == "picker-item").collect();
            if rows.is_empty() {
                ui.fail(format!("{ctx}: no entries"));
            }
            for row in &rows {
                let i = row.index().unwrap_or(usize::MAX);
                let (Some(keys), Some(label)) = (find(&p, &format!("picker-keys:{i}")), find(&p, &format!("picker-label:{i}"))) else {
                    ui.fail(format!("{ctx}: entry {i} without label / keys column"));
                    continue;
                };
                let chars = labels.get(i).map_or(0, |l| l.chars().count()) as f32;
                if f32::from(label.bounds.size.width) < (font_ui * 3.).min(chars * font_ui * 0.3) {
                    ui.fail(format!("{ctx}: label {i} squeezed to {:?}", label.bounds));
                }
                if label.bounds.right() > keys.bounds.left() {
                    ui.fail(format!("{ctx}: label {i} reaches into the keys column"));
                }
            }
        }
        if let Some(pk) = picker(ui) {
            for (label, keys) in pk.labels.iter().zip(&pk.keys) {
                if keys.chars().count() > 1 && label.contains(keys.as_str()) {
                    ui.fail(format!("{ctx}: `{label}` has its keys `{keys}` in the label"));
                }
            }
        }
        ui.step("action:gitti::ModalCancel");
    }
    reset_point(ui);
    ui.step("focuslist");
    ui.step("action:gitti::CommandPalette");
    ui.quick("pick:go to file");
    let want = ui.cx.update(|cx| testing::key_hint("gitti::FileList", cx));
    match picker(ui) {
        Some(pk) => match pk.labels.iter().position(|l| l == "Go to File…") {
            Some(i) if pk.keys[i] == want && !want.is_empty() => {}
            Some(i) => ui.fail(format!("Go to File… keys `{}`, expected `{want}`", pk.keys[i])),
            None => ui.fail(format!("no Go to File… in {:?}", pk.labels)),
        },
        None => ui.fail("no palette".into()),
    }
    ui.step("action:gitti::ModalCancel");
}

/// View ▸ Commit List — Auto / One Line / Two Lines at three window widths. The
/// rows' height follows the choice (Auto: two lines below 40 em of list width); subject,
/// author and date never overlap (invariant 3c) and stay in their order; the date is
/// whole; the one-line info columns leave the subject more room than the old 8 + 8 em;
/// the choice is stored (shared with gitty).
fn commit_list_layouts(ui: &mut Ui) {
    use gitti_core::list_layout::CommitListLayout as L;
    use gitti_core::store::{SharedSettings, load_settings};
    let em = 13.;
    let f = |p: Pixels| f32::from(p);
    for (w, h) in [(1500., 950.), (1000., 640.), (800., 500.)] {
        ui.resize(w, h);
        for (l, action) in [
            (L::OneLine, "CommitListOneLine"),
            (L::TwoLines, "CommitListTwoLines"),
            (L::Auto, "CommitListAuto"),
        ] {
            ui.step(&format!("action:gitti::{action}"));
            let ctx = format!("{w}x{h} {l:?}");
            let p = ui.check(&ctx);
            let Some(list) = find(&p, "commit-list") else {
                ui.fail(format!("{ctx}: no commit list"));
                continue;
            };
            let two = l.two_lines(f(list.bounds.size.width) < em * 40.);
            let parts: Vec<Option<Probe>> = ["commit-row:2", "commit-subject:2", "commit-author:2", "commit-date:2"]
                .iter()
                .map(|n| find(&p, n).cloned())
                .collect();
            let [Some(row), Some(subject), Some(author), Some(date)] = &parts[..] else {
                ui.fail(format!("{ctx}: row 2 parts {:?}", parts.iter().map(Option::is_some).collect::<Vec<_>>()));
                continue;
            };
            let want_h = if two { (em * 3.2).round() } else { (em * 1.85).round() };
            if (f(row.bounds.size.height) - want_h).abs() > 1.5 {
                ui.fail(format!("{ctx}: row height {:?}, want {want_h}", row.bounds.size.height));
            }
            if date.visible() != Some(date.bounds) {
                ui.fail(format!("{ctx}: date cut: {:?} of {:?}", date.visible(), date.bounds));
            }
            if two {
                if f(author.bounds.top()) + 1. < f(subject.bounds.bottom()) {
                    ui.fail(format!("{ctx}: author not below the subject"));
                }
                continue;
            }
            let (sv, av) = (subject.visible().unwrap_or(subject.bounds), author.visible());
            if av.is_some_and(|a| f(a.left()) + 1. < f(sv.right()))
                || f(date.bounds.left()) + 1. < f(author.bounds.right())
            {
                ui.fail(format!("{ctx}: columns out of order: {sv:?} {av:?} {:?}", date.bounds));
            }
            // Author, gap, date, padding: the old columns took 16 em of the subject's room.
            let info = f(row.bounds.right()) - f(author.bounds.left());
            // (macOS: 160 px; fonts elsewhere differ a little.)
            if w == 1500. && info > em * 13. {
                ui.fail(format!("{ctx}: info columns {info} px, want at most {}", em * 13.));
            }
            if w == 1500. && f(author.bounds.size.width) < em * 5. {
                ui.fail(format!("{ctx}: author column only {:?}", author.bounds.size.width));
            }
        }
        if w == 1500. {
            ui.step("action:gitti::CommitListTwoLines");
            let stored = load_settings::<SharedSettings>().commit_list_layout;
            if stored != L::TwoLines {
                ui.fail(format!("stored layout {stored:?}"));
            }
            ui.step("action:gitti::CommitListAuto");
        }
    }
    reset_point(ui);
}
