//! Selecting text with the mouse: a double click selects a word, a triple click the line —
//! in the committed message, the diff (a word broken by line wrapping too), the commit
//! message editor, a dialog's field and the merge editor's sides; a drag from a double
//! click extends by words. And
//! the commit header's ids: a right click offers copying the commit's id / each parent's.
//!
//! `cargo test --workspace --test ui -- selection[::FILTER]`.

use crate::common;

use std::path::{Path, PathBuf};

use common::*;

macro_rules! fail {
    ($ui:expr, $($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $ui.fail(msg);
    }};
}

/// The word big.txt's changed line ends in: longer than a wrapped row, without spaces.
fn long_word() -> String {
    (0..400).map(|i| (b'a' + (i % 26) as u8) as char).collect()
}

pub fn main() {
    let tmp = common::setup();
    let base = gitti_core::paths::canonical(tmp.path());
    let tests: &[(&str, Scenario)] = &[
        ("message_word_line_and_drag", message_word_line_and_drag),
        ("diff_word_across_a_wrap", diff_word_across_a_wrap),
        ("editor_and_dialog_field_words", editor_and_dialog_field_words),
        ("header_ids_copy", header_ids_copy),
        ("merge_editor_sides", merge_editor_sides),
    ];
    common::run(tests, |name| {
        let dir = base.join(name);
        match name {
            "header_ids_copy" => merge_repo(&dir),
            "merge_editor_sides" => make_merge_repo(&dir),
            "diff_word_across_a_wrap" => {
                let repo = make_repo(&dir);
                // The only change: line 3 gets "changed" and the long word.
                let big: String = (0..400)
                    .map(|i| {
                        if i == 3 {
                            format!("line 3 changed {}\n", long_word())
                        } else {
                            format!("line {i} \n")
                        }
                    })
                    .collect();
                std::fs::write(repo.join("big.txt"), big).unwrap();
                repo
            }
            _ => make_repo(&dir),
        }
    });
}

/// `main`: "base" → "main side" and "side" → the merge "Merge side" (HEAD, row 1).
fn merge_repo(dir: &Path) -> PathBuf {
    init_repo(dir);
    let mut s = Stream::new();
    let base = s.commit("refs/heads/main", "base", None, &[("a.txt", Some("a\n"))]);
    let side = s.commit("refs/heads/side", "side", Some(base), &[("b.txt", Some("b\n"))]);
    let main = s.commit("refs/heads/main", "main side", None, &[("c.txt", Some("c\n"))]);
    let merge = s.merge("refs/heads/main", "Merge side", &[main, side], &[("b.txt", Some("b\n"))]);
    let ids = s.import(dir);
    read_tree(dir, &ids[merge]);
    dir.to_path_buf()
}

fn point(ui: &mut Ui, area: &str, line: usize, byte: usize) -> Option<Point<Pixels>> {
    ui.frame();
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::text_point(&ws, area, line, byte, cx))
}

fn selected(ui: &mut Ui) -> String {
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::selected_text(&ws, cx)).unwrap_or_default()
}

/// `count` clicks on byte `byte` of `area`'s line `line`; the selected text after it.
fn clicks_on(ui: &mut Ui, area: &str, line: usize, byte: usize, count: usize) -> Option<String> {
    let Some(p) = point(ui, area, line, byte) else {
        fail!(ui, "{area} line {line} not laid out");
        return None;
    };
    ui.clicks(p, count, None);
    Some(selected(ui))
}

/// The commit view's message: line 2 is "Paragraph line 0: explains why the change was
/// needed." A double click selects the word / the punctuation under the pointer, a triple
/// click the line; double click and drag extends by whole words; ⌘C copies the word.
fn message_word_line_and_drag(ui: &mut Ui) {
    ui.step("select:1");
    let line = "Paragraph line 0: explains why the change was needed.";
    let at = |s: &str| line.find(s).unwrap();
    for (byte, count, want) in [
        (at("explains") + 3, 2, "explains"),
        (at(": ex"), 2, ":"),
        (at("line 0") + 1, 3, line),
    ] {
        let got = clicks_on(ui, "message", 2, byte, count);
        if got.as_deref() != Some(want) {
            fail!(ui, "{count} clicks at byte {byte} of the message line selected {got:?}, want {want:?}");
        }
    }
    let got = clicks_on(ui, "message", 0, 13, 2);
    if got.as_deref() != Some("frobnicator") {
        fail!(ui, "double click on the subject selected {got:?}");
    }
    // Double click on "explains", drag into "change": whole words from one to the other.
    let (Some(from), Some(to)) = (point(ui, "message", 2, at("explains") + 2), point(ui, "message", 2, at("change ") + 2)) else {
        return fail!(ui, "message line 2 not laid out");
    };
    ui.clicks(from, 2, Some(to));
    let got = selected(ui);
    if got != "explains why the change" {
        fail!(ui, "double click and drag selected {got:?}");
    }
    ui.cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
    ui.stroke("secondary-c");
    let copied = ui.cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
    if copied != "explains why the change" {
        fail!(ui, "⌘C copied {copied:?}");
    }
}

/// Wrapped diff lines: a double click on the long word's second row selects the whole
/// word (both rows, copied without a line break), on "changed" just that word; a triple
/// click the whole line. The line selection for staging stays the one line.
fn diff_word_across_a_wrap(ui: &mut Ui) {
    ui.step("action:gitti::ToggleWrap");
    let rows = ui.ui_state().rows;
    let Some(first) = rows.iter().position(|r| r == "line Unstaged big.txt Added") else {
        return fail!(ui, "no added line in big.txt: {rows:?}");
    };
    let parts = rows[first..].iter().take_while(|r| *r == "line Unstaged big.txt Added").count();
    if parts < 2 {
        return fail!(ui, "the long line is not wrapped ({parts} rows)");
    }
    let word = long_word();
    let full = format!("line 3 changed {word}");
    let checks = [
        (first, "line 3 ch".len(), 2, "changed".to_string()),
        (first + 1, 3, 2, word.clone()),
        (first, 1, 2, "line".to_string()),
        (first + 1, 3, 3, full.clone()),
    ];
    for (row, byte, count, want) in checks {
        let got = clicks_on(ui, "diff", row, byte, count);
        if got.as_deref() != Some(want.as_str()) {
            fail!(ui, "{count} clicks on diff row {row} byte {byte}: {got:?}, want {want:?}");
        }
    }
    if ui.metrics().selected_lines != 1 {
        fail!(ui, "line selection after the clicks: {} lines", ui.metrics().selected_lines);
    }
}

fn input(ui: &mut Ui, field: Option<usize>, byte: usize) -> (Option<Point<Pixels>>, String) {
    ui.frame();
    let ws = ui.ws.clone();
    ui.cx.update(|cx| testing::input(&ws, field, byte, cx)).unwrap_or_default()
}

/// The commit message editor and a dialog's field (the tag message): a double click
/// selects the word, a triple click (editor) the line.
fn editor_and_dialog_field_words(ui: &mut Ui) {
    let text = "Fix the frobnicator\n\nIt broke on_save handlers.";
    ui.quick(&format!("msg:{}", text.replace('\n', "\\n")));
    let at = |s: &str| text.find(s).unwrap();
    for (byte, count, want) in [
        (at("frob") + 2, 2, "frobnicator"),
        (at("on_save") + 4, 2, "on_save"),
        (at("broke"), 3, "It broke on_save handlers."),
    ] {
        let (Some(p), _) = input(ui, None, byte) else {
            fail!(ui, "editor byte {byte} not laid out");
            continue;
        };
        ui.clicks(p, count, None);
        let (_, got) = input(ui, None, byte);
        if got != want {
            fail!(ui, "{count} clicks in the editor at byte {byte}: {got:?}, want {want:?}");
        }
    }
    ui.step("action:gitti::CreateTag");
    let msg = "Release the frobnicator build";
    ui.quick(&format!("field:1={msg}"));
    let byte = msg.find("frob").unwrap() + 3;
    match input(ui, Some(1), byte) {
        (Some(p), _) => {
            ui.clicks(p, 2, None);
            let (_, got) = input(ui, Some(1), byte);
            if got != "frobnicator" {
                fail!(ui, "double click in the tag message field selected {got:?}");
            }
        }
        (None, _) => fail!(ui, "tag message field not laid out"),
    }
    ui.step("cancel");
}

/// Opens `target`'s context menu and chooses the entry `label` by keyboard (the entries
/// after it are enabled); returns the labels and the clipboard after it (emptied before).
fn copy_from_menu(ui: &mut Ui, target: &str, label: &str) -> (Vec<String>, String) {
    ui.cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
    testing::menu_labels();
    ui.click_on(target, MouseButton::Right);
    let labels = testing::menu_labels();
    let Some(ix) = labels.iter().position(|l| l == label) else {
        fail!(ui, "{target}: no {label:?} in {labels:?}");
        ui.quick("key:escape");
        return (labels, String::new());
    };
    // The first ↑ selects the last entry.
    for _ in ix..labels.len() {
        ui.quick("key:up");
    }
    ui.quick("key:enter");
    let copied = ui.cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
    (labels, copied)
}

/// The merge's header: the commit id and each parent are elements of their own
/// (`header-id`, `header-parent:N`) whose right-click menu copies that id (only that menu,
/// not the diff's), full or short; no git for it.
fn header_ids_copy(ui: &mut Ui) {
    ui.step("select:1");
    let dir = ui.workdir();
    let ids = git_out(&dir, &["log", "-1", "--format=%H %P"]);
    let ids: Vec<&str> = ids.split_whitespace().collect();
    let [head, p1, p2] = ids[..] else {
        return fail!(ui, "merge ids {ids:?}");
    };
    let n = ui.n_git();
    let (labels, copied) = copy_from_menu(ui, "header-id", "Copy SHA");
    if labels != ["Copy SHA", "Copy Short SHA"] {
        fail!(ui, "commit id menu: {labels:?}");
    }
    if copied != head {
        fail!(ui, "Copy SHA copied {copied:?}, want {head}");
    }
    let (labels, copied) = copy_from_menu(ui, "header-parent:0", "Copy Short Parent 1 SHA");
    if labels != ["Copy Parent 1 SHA", "Copy Short Parent 1 SHA"] {
        fail!(ui, "first parent's menu: {labels:?}");
    }
    if copied != p1[..7] {
        fail!(ui, "Copy Short Parent 1 SHA copied {copied:?}, want {}", &p1[..7]);
    }
    let (_, copied) = copy_from_menu(ui, "header-parent:1", "Copy Parent 2 SHA");
    if copied != p2 {
        fail!(ui, "Copy Parent 2 SHA copied {copied:?}, want {p2}");
    }
    ui.no_git_since(n, "copying the header's ids");
}

/// The merge editor's ours / theirs sides: a double click selects the word on that side, a
/// triple click the line.
fn merge_editor_sides(ui: &mut Ui) {
    ui.step("select:0");
    ui.step("mergeeditor:0");
    let Some(win) = testing::merge_window() else {
        return fail!(ui, "no merge window");
    };
    let merge_selected = |ui: &mut Ui| ui.cx.update(testing::merge_state).and_then(|s| s.selected);
    for (ours, needle, offset, count, want) in [
        (false, "theirs 20", 2, 2, "theirs"),
        (true, "ours 20", 6, 2, "20"),
        (false, "line 19", 1, 3, "line 19"),
    ] {
        ui.cx.update_window(win, |_, window, cx| window.draw(cx).clear(cx)).unwrap();
        let Some(p) = ui.cx.update(|cx| testing::merge_text_point(ours, needle, offset, cx)) else {
            fail!(ui, "merge editor: {needle:?} not laid out");
            continue;
        };
        ui.clicks_in(win, p, count, None);
        let got = merge_selected(ui);
        if got.as_deref() != Some(want) {
            fail!(ui, "{count} clicks on {needle:?} in the merge editor selected {got:?}, want {want:?}");
        }
    }
}
