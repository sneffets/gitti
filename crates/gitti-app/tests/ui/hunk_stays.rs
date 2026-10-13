//! Staging hunk by hunk with the mouse at one spot: after Stage / Unstage / Discard on a
//! hunk header (also on its sticky copy at the top) the next hunk's header — of the same
//! file, else the next file's first hunk — is where the clicked one was, so the next click
//! on the same spot takes the next hunk; a file header's button brings the next file's
//! header; staged lines leave the lines around them in place. At the end of the section
//! the hunk before comes there; at the list's end as close as the list allows. Flat, tree
//! view, wrapped lines (`app/op_anchor.rs`, `gitti_core::rows::follow_op`).
//!
//! `cargo test --workspace --test ui -- hunk_stays[::FILTER]`.

use crate::common;

use std::path::{Path, PathBuf};

use common::*;

pub fn main() {
    let tmp = common::setup();
    let base = make_repo(&tmp.path().join("base"));
    let tests: &[(&str, Scenario)] = &[
        ("clicks_on_one_spot_walk_the_hunks", clicks_on_one_spot_walk_the_hunks),
        ("sticky_hunk_copy", sticky_hunk_copy),
        ("wrapped_lines", wrapped_lines),
        ("tree_view_next_folder", tree_view_next_folder),
        ("file_headers", file_headers),
        ("unstage_and_discard", unstage_and_discard),
        ("staged_lines_keep_the_lines_around", staged_lines_keep_the_lines_around),
        ("list_end", list_end),
    ];
    let copies = std::cell::Cell::new(0);
    common::run(tests, |_| {
        copies.set(copies.get() + 1);
        let dir = tmp.path().join(format!("copy{}", copies.get()));
        copy_dir(&base, &dir).expect("copying the test repository failed");
        dir
    });
}

/// The changed lines of `src/a.txt` (200 lines): six hunks; the one at 110 changes 110
/// and 113 (two groups of a hunk); 50 and 140 are long (they wrap).
const A_CHANGES: &[usize] = &[20, 50, 80, 110, 113, 140, 170];

/// `src/a.txt` (6 hunks), `src/b.txt` (2), `docs/c.txt` (1), `z.txt` (1), all unstaged.
fn make_repo(dir: &Path) -> PathBuf {
    init_repo(dir);
    let text = |tag: &str, n: usize, changed: &[usize]| -> String {
        (1..=n)
            .map(|i| {
                if !changed.contains(&i) {
                    format!("{tag}{i}\n")
                } else if i == 50 || i == 140 {
                    format!("{} {i} {}\n", tag.to_uppercase(), "long words ".repeat(40))
                } else {
                    format!("{}{i}\n", tag.to_uppercase())
                }
            })
            .collect()
    };
    let files: [(&str, &str, usize, &[usize]); 4] = [
        ("src/a.txt", "a", 200, A_CHANGES),
        ("src/b.txt", "b", 120, &[30, 70]),
        ("docs/c.txt", "c", 80, &[40]),
        ("z.txt", "z", 80, &[40]),
    ];
    let head: Vec<(String, String)> =
        files.iter().map(|(p, t, n, _)| (p.to_string(), text(t, *n, &[]))).collect();
    let mut st = Stream::new();
    let refs: Vec<(&str, Option<&str>)> =
        head.iter().map(|(p, t)| (p.as_str(), Some(t.as_str()))).collect();
    let h = st.commit("refs/heads/main", "Base", None, &refs);
    let ids = st.import(dir);
    read_tree(dir, &ids[h]);
    for (p, t, n, changed) in files {
        let path = dir.join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text(t, n, changed)).unwrap();
    }
    dir.to_path_buf()
}

// ------------------------------------------------------------------ helpers

fn row_ids(ui: &mut Ui) -> Vec<String> {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::diff_row_ids(&ws, cx))
}

fn list_top(ui: &mut Ui) -> f32 {
    let p = ui.frame();
    find(&p, "diff-list").map_or(0., |l| f32::from(l.bounds.top()))
}

/// Scrolls the diff so row `ix` is `y` px below the list's top (as far as it goes).
fn put(ui: &mut Ui, ix: usize, y: f32) {
    let row_h = ui.metrics().diff_row_h;
    let want = ix as f32 * row_h - y;
    for _ in 0..8 {
        let at = ui.scroll_info().diff_y;
        if (at - want).abs() < 0.5 {
            break;
        }
        ui.wheel("diff-list", want - at);
    }
}

/// Where row `ix` is drawn in the list (its own place, px below the list's top).
fn row_y(ui: &mut Ui, ix: usize) -> f32 {
    ix as f32 * ui.metrics().diff_row_h - ui.scroll_info().diff_y
}

/// The rows' ids and kinds.
fn rows(ui: &mut Ui) -> (Vec<String>, Vec<&'static str>) {
    let ids = row_ids(ui);
    let kinds = ui.metrics().diff_rows;
    (ids, kinds)
}

/// The header rows of the hunks of `path` in section `src` ("Unstaged" / "Staged").
fn hunks_of(ui: &mut Ui, src: &str, path: &str) -> Vec<usize> {
    let (ids, kinds) = rows(ui);
    let Some(file) = ids.iter().position(|i| *i == format!("file {src} {path}")) else {
        return Vec::new();
    };
    let end = (file + 1..kinds.len()).find(|&i| kinds[i] == "file" || kinds[i] == "section").unwrap_or(kinds.len());
    (file + 1..end).filter(|&i| kinds[i] == "hunk").collect()
}

/// The unstaged hunks' header rows in the order they are shown, with their file's path.
fn unstaged_hunks(ui: &mut Ui) -> Vec<(usize, String)> {
    let (ids, kinds) = rows(ui);
    let mut out = Vec::new();
    let mut path: Option<String> = None;
    for (i, id) in ids.iter().enumerate() {
        match kinds[i] {
            "section" => path = None,
            "file" => path = id.strip_prefix("file Unstaged ").map(str::to_string),
            "hunk" => out.extend(path.clone().map(|p| (i, p))),
            _ => {}
        }
    }
    out
}

/// What identifies the hunk at header row `h` across rebuilds: its first line's id.
fn hunk_id(ui: &mut Ui, h: usize) -> String {
    row_ids(ui).get(h + 1).cloned().unwrap_or_default()
}

/// The header row of the hunk `id` ([`hunk_id`]) now.
fn hunk_row(ui: &mut Ui, id: &str) -> Option<usize> {
    let (ids, kinds) = rows(ui);
    ids.iter().position(|i| i == id).filter(|&i| i > 0 && kinds[i - 1] == "hunk").map(|i| i - 1)
}

/// Clicks button `name` (a probe) where it is drawn inside the probe `within` (its row or
/// a sticky copy); returns the top of `within` below the list's top.
fn click_in(ui: &mut Ui, name: &str, within: &str) -> Option<f32> {
    let lt = list_top(ui);
    let p = ui.frame();
    let w = p.iter().filter(|q| q.name == within).filter_map(|q| q.visible()).next_back()?;
    let b = p
        .iter()
        .filter(|q| q.name == name)
        .filter_map(|q| q.visible())
        .find(|b| w.contains(&b.center()))?;
    let top = f32::from(w.top()) - lt;
    ui.clicks(b.center(), 1, None);
    ui.settle();
    Some(top)
}

/// Clicks the button `button` (`hunk-btn`, `hunk-unstage`, …) of the hunk header at row
/// `h` on its own row; returns that row's place.
fn click_header(ui: &mut Ui, button: &str, h: usize) -> Option<f32> {
    click_in(ui, &format!("{button}:{h}"), &format!("diff-row:{h}:hunk"))
}

/// After an operation: hunk `next` ([`hunk_id`]) has its header where the clicked one was
/// (`y`), or the list is scrolled as far as it goes towards that.
fn expect_at(ui: &mut Ui, what: &str, next: &str, y: f32) -> Option<usize> {
    let Some(h) = hunk_row(ui, next) else {
        ui.fail(format!("{what}: the next hunk ({next}) is gone"));
        return None;
    };
    let got = row_y(ui, h);
    let s = ui.scroll_info();
    // The list can't scroll above its top / below its end.
    let clamped = (got < y && s.diff_y <= 0.5) || (got > y && s.diff_y >= s.diff_max - 0.5);
    if (got - y).abs() > 1.5 && !clamped {
        ui.fail(format!("{what}: the next hunk's header is at {got}, the clicked one was at {y}"));
    }
    Some(h)
}

/// Stage Hunk on the header at `y` px below the list's top (the one drawn there), expecting
/// hunk `next` there afterwards; returns `next`'s row.
fn stage_at(ui: &mut Ui, what: &str, y: f32, next: &str) -> Option<usize> {
    let row_h = ui.metrics().diff_row_h;
    let h = ((ui.scroll_info().diff_y + y) / row_h).round() as usize;
    if ui.metrics().diff_rows.get(h) != Some(&"hunk") {
        ui.fail(format!("{what}: no hunk header at {y} (row {h})"));
        return None;
    }
    let git = ui.git_commands().len();
    let Some(clicked) = click_header(ui, "hunk-btn", h) else {
        ui.fail(format!("{what}: no Stage button on row {h}"));
        return None;
    };
    // The operation and its path-limited re-read, nothing else.
    let ran = ui.git_commands().len() - git;
    if ran > 4 {
        let cmds = ui.git_commands();
        ui.fail(format!("{what}: {ran} git commands: {:?}", &cmds[git..]));
    }
    expect_at(ui, what, next, clicked)
}

// ------------------------------------------------------------------ scenarios

/// Stage Hunk clicked again and again on one spot in the middle of the view: each time the
/// next hunk comes there — `src/a.txt`'s one by one, then `src/b.txt`'s first (the next
/// file); b's last takes the file out of the section: `z.txt`'s comes. That one is the
/// last of the section: the hunk before it (`docs/c.txt`'s, the first) comes back, as
/// near to the spot as the list's top allows.
/// Workflow: ST-22
fn clicks_on_one_spot_walk_the_hunks(ui: &mut Ui) {
    ui.step("select:0");
    walk(ui, "flat", 9.);
}

/// From the second unstaged hunk on, Stage Hunk on the spot `rows` rows below the list's
/// top until one is left: the next hunk in the shown order each time, the first at last.
fn walk(ui: &mut Ui, what: &str, rows: f32) {
    let row_h = ui.metrics().diff_row_h;
    let hunks = unstaged_hunks(ui);
    let paths: Vec<&str> = hunks.iter().map(|(_, p)| p.as_str()).collect();
    if paths != ["docs/c.txt", "src/a.txt", "src/a.txt", "src/a.txt", "src/a.txt", "src/a.txt", "src/a.txt", "src/b.txt", "src/b.txt", "z.txt"] {
        ui.fail(format!("{what}: unstaged hunks {hunks:?}"));
        return;
    }
    let order: Vec<String> = hunks.iter().map(|&(h, _)| hunk_id(ui, h)).collect();
    let y = rows * row_h;
    put(ui, hunks[1].0, y);
    for (i, next) in order.iter().enumerate().skip(2) {
        stage_at(ui, &format!("{what}, click {} ({} → {})", i - 1, paths[i - 1], paths[i]), y, next);
        if !ui.fails.is_empty() {
            return;
        }
    }
    stage_at(ui, &format!("{what}, the section's last hunk"), y, &order[0]);
}

/// Scrolled into a hunk, its header shown as the sticky copy at the top: Stage on the copy
/// brings the next hunk's real header to where the copy was — the next file's too.
/// Workflow: ST-22
fn sticky_hunk_copy(ui: &mut Ui) {
    ui.step("select:0");
    let row_h = ui.metrics().diff_row_h;
    let a = hunks_of(ui, "Unstaged", "src/a.txt");
    let b = hunks_of(ui, "Unstaged", "src/b.txt");
    for (k, next) in [(1, hunk_id(ui, a[2])), (usize::MAX, hunk_id(ui, b[0]))] {
        let a = hunks_of(ui, "Unstaged", "src/a.txt");
        let h = a[k.min(a.len() - 1)];
        // Two rows into the hunk: its header is the sticky copy.
        put(ui, h + 2, 0.);
        let Some(y) = click_in(ui, &format!("hunk-btn:{h}"), "sticky-hunk") else {
            ui.fail(format!("hunk {k}: no Stage button on the sticky copy of row {h}"));
            return;
        };
        if (y - row_h).abs() > 0.5 {
            ui.fail(format!("hunk {k}: the sticky copy is at {y}, expected under the file row"));
        }
        expect_at(ui, &format!("sticky copy of hunk {k}"), &next, y);
    }
}

/// Wrapped lines (the long ones of `a.txt` take several rows): the same.
/// Workflow: ST-22
fn wrapped_lines(ui: &mut Ui) {
    ui.step("select:0");
    ui.step("action:gitti::ToggleWrap");
    let row_h = ui.metrics().diff_row_h;
    let a = hunks_of(ui, "Unstaged", "src/a.txt");
    let (ids, _) = rows(ui);
    if !ids.iter().any(|i| i.contains("long words") && i.ends_with("#1")) {
        ui.fail("wrap on: no wrapped line".into());
    }
    let ids: Vec<String> = a.iter().map(|&h| hunk_id(ui, h)).collect();
    let y = 7. * row_h;
    // The hunk at 20 → the long one at 50 → 80 → (110 / 113) → the long one at 140.
    put(ui, a[0], y);
    for (i, next) in ids.iter().enumerate().skip(1).take(4) {
        stage_at(ui, &format!("wrapped, click {i}"), y, next);
    }
}

/// Tree view (folders `docs`, `src`, then `z.txt` at the top level): the same walk — the
/// next hunk in another file of another folder, one without a folder; and from the sticky
/// hunk copy under the sticky rows.
/// Workflow: ST-22
fn tree_view_next_folder(ui: &mut Ui) {
    ui.step("select:0");
    ui.step("action:gitti::ToggleChangesTree");
    if !ui.metrics().diff_rows.contains(&"folder") {
        ui.fail("tree view: no folder rows".into());
        return;
    }
    // `docs/c.txt`'s hunk → `src/a.txt`'s first (another folder) …
    let hunks = unstaged_hunks(ui);
    let row_h = ui.metrics().diff_row_h;
    let next = hunk_id(ui, hunks[1].0);
    put(ui, hunks[0].0 + 1, 0.);
    match click_in(ui, &format!("hunk-btn:{}", hunks[0].0), "sticky-hunk") {
        Some(y) => {
            expect_at(ui, "tree view, sticky copy", &next, y);
        }
        None => ui.fail("tree view: no Stage button on the sticky hunk copy".into()),
    }
    // … and the walk on a spot in the middle (`src/b.txt`'s last → `z.txt`'s).
    let hunks = unstaged_hunks(ui);
    let order: Vec<String> = hunks.iter().map(|&(h, _)| hunk_id(ui, h)).collect();
    let y = 10. * row_h;
    put(ui, hunks[5].0, y);
    for (i, next) in order.iter().enumerate().skip(6) {
        stage_at(ui, &format!("tree view, click {} → {}", i - 5, hunks[i].1), y, next);
    }
}

/// Stage File on a file header (also its sticky copy): the next file's header comes there
/// (`src/b.txt` → `z.txt`; `src/a.txt` → `z.txt`).
/// Workflow: ST-22
fn file_headers(ui: &mut Ui) {
    ui.step("select:0");
    let row_h = ui.metrics().diff_row_h;
    let file_row = |ui: &mut Ui, path: &str| row_ids(ui).iter().position(|i| *i == format!("file Unstaged {path}"));
    let (Some(b), Some(_)) = (file_row(ui, "src/b.txt"), file_row(ui, "z.txt")) else {
        ui.fail("no file rows".into());
        return;
    };
    put(ui, b, 6. * row_h);
    let Some(y) = click_in(ui, &format!("file-stage:{b}"), &format!("diff-row:{b}:file")) else {
        ui.fail("no Stage File button on src/b.txt".into());
        return;
    };
    match file_row(ui, "z.txt") {
        Some(z) if (row_y(ui, z) - y).abs() <= 1.5 => {}
        z => {
            let at = z.map(|z| row_y(ui, z));
            ui.fail(format!("Stage File: z.txt's header at {at:?}, the clicked one was at {y}"));
        }
    }
    // The sticky file row: scrolled into src/a.txt, Stage File on its copy → z at the top.
    let a = file_row(ui, "src/a.txt").unwrap_or(0);
    put(ui, a + 3, 0.);
    match click_in(ui, &format!("file-stage:{a}"), "sticky-file") {
        Some(y) => match file_row(ui, "z.txt") {
            Some(z) if (row_y(ui, z) - y).abs() <= 1.5 => {}
            z => {
                let at = z.map(|z| row_y(ui, z));
                ui.fail(format!("Stage File on the sticky copy: z.txt at {at:?}, clicked at {y}"));
            }
        },
        None => ui.fail("no Stage File button on the sticky file row".into()),
    }
}

/// Unstage Hunk in the staged section and Discard Hunk (asks) in the unstaged one: the
/// same.
/// Workflow: ST-22
fn unstage_and_discard(ui: &mut Ui) {
    ui.step("select:0");
    let row_h = ui.metrics().diff_row_h;
    ui.step("stagefile:src/a.txt");
    let a = hunks_of(ui, "Staged", "src/a.txt");
    if a.len() != 6 {
        ui.fail(format!("staged a.txt hunks: {a:?}"));
        return;
    }
    let ids: Vec<String> = a.iter().map(|&h| hunk_id(ui, h)).collect();
    let y = 8. * row_h;
    put(ui, a[1], y);
    for (i, next) in ids.iter().enumerate().skip(2).take(2) {
        let h = ((ui.scroll_info().diff_y + y) / row_h).round() as usize;
        match click_header(ui, "hunk-unstage", h) {
            Some(at) => {
                expect_at(ui, &format!("unstage, click {i}"), next, at);
            }
            None => {
                ui.fail(format!("unstage: no Unstage button on row {h}"));
                return;
            }
        }
    }
    let b = hunks_of(ui, "Unstaged", "src/b.txt");
    let next = hunk_id(ui, b[1]);
    put(ui, b[0], y);
    let p = ui.frame();
    let Some(btn) = find(&p, &format!("hunk-discard:{}", b[0])).and_then(|q| q.visible()) else {
        ui.fail("no Discard Hunk button".into());
        return;
    };
    ui.clicks(btn.center(), 1, None);
    match ui.cx.pending_prompt() {
        Some((t, _)) if t.starts_with("Discard") => ui.cx.simulate_prompt_answer("Discard"),
        other => {
            ui.fail(format!("discard: question {other:?}"));
            return;
        }
    }
    ui.settle();
    expect_at(ui, "discard", &next, y);
}

/// The first change of the hunk at 110 (lines 110 and 113) selected and staged with the
/// header's button: `A110` (context now) comes where the selection began, the lines after
/// it right below — the hunk's header moves down by the context lines it lost (a107–a109,
/// now hidden between the hunks) and stays on screen.
/// Workflow: ST-22
fn staged_lines_keep_the_lines_around(ui: &mut Ui) {
    ui.step("select:0");
    let row_h = ui.metrics().diff_row_h;
    let a = hunks_of(ui, "Unstaged", "src/a.txt");
    let h = a[3];
    let (ids, _) = rows(ui);
    let Some(first) = (h + 1..ids.len()).find(|&i| ids[i].contains("\"a110\"")) else {
        ui.fail(format!("no line a110 after row {h}"));
        return;
    };
    put(ui, h, 10. * row_h);
    ui.quick(&format!("lines:{first}-{}", first + 1));
    let (header_y, line_y) = (row_y(ui, h), row_y(ui, first));
    if click_header(ui, "hunk-btn", h).is_none() {
        ui.fail("no Stage Lines button".into());
        return;
    }
    let (ids, kinds) = rows(ui);
    let at = |y: f32, ui: &mut Ui| ((ui.scroll_info().diff_y + y) / row_h).round() as usize;
    let ll = at(line_y, ui);
    match (ids.get(ll), ids.get(ll + 1)) {
        (Some(i), Some(j)) if i.contains("Context \"A110\"") && j.contains("Context \"a111\"") => {}
        other => ui.fail(format!("staged lines: rows at the selection's place {line_y}: {other:?}")),
    }
    let header = (0..ll).rev().find(|&i| kinds[i] == "hunk");
    match header.map(|h| row_y(ui, h)) {
        Some(y) if y >= header_y && y < line_y => {}
        y => ui.fail(format!("staged lines: the header went from {header_y} to {y:?}")),
    }
}

/// The list's end: scrolled to the bottom, Unstage on the staged section's last hunk — the
/// hunk before it comes as near to the spot as the list allows.
/// Workflow: ST-22
fn list_end(ui: &mut Ui) {
    ui.step("select:0");
    ui.step("stagefile:src/a.txt");
    let a = hunks_of(ui, "Staged", "src/a.txt");
    let Some(&last) = a.last() else {
        ui.fail("no staged hunks".into());
        return;
    };
    let next = hunk_id(ui, a[a.len() - 2]);
    let (s, rows) = (ui.scroll_info(), ui.metrics().diff_rows.len());
    put(ui, rows, s.diff_viewport_h);
    // Again: the first wheel steps shrink the commit box (a taller viewport).
    let s = ui.scroll_info();
    put(ui, rows, s.diff_viewport_h);
    let s = ui.scroll_info();
    if s.diff_y < s.diff_max - 0.5 {
        ui.fail(format!("could not scroll to the end: {} of {}", s.diff_y, s.diff_max));
    }
    let Some(y) = click_header(ui, "hunk-unstage", last) else {
        let s = ui.scroll_info();
        let p: Vec<String> = ui.frame().iter().filter(|q| q.name.contains(&format!(":{last}"))).map(|q| q.to_json()).collect();
        ui.fail(format!("no Unstage button on the last hunk (row {last}, scroll {} of {}): {p:?}", s.diff_y, s.diff_max));
        return;
    };
    expect_at(ui, "list end", &next, y);
}
