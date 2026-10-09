//! Search in Changes in gitty: `/` opens the query line, typing runs one scan per
//! query (debounced), the list shows the hits with why they matched, ⏎ selects the best
//! one and its diff shows only the matching hunks; alt-s strict, alt-a all hunks, esc
//! closes.

use crate::common;
use crate::support;

use common::*;
use support::*;

fn repo() -> tempfile::TempDir {
    let lines = |n: usize| (0..n).map(|i| format!("line {i}\n")).collect::<String>();
    let base = lines(40);
    let v2 = format!("fn parse(token)\n{base}");
    let v3 = format!("{v2}parse more\n");
    let v4 = v3
        .replace("line 20\n", "line twenty\n")
        .replace("line 35\n", "line thirty-five\n");
    import(&[
        Commit { branch: "main", from: None, merge: None, msg: "Initial", files: &[("a.txt", &base), ("b.txt", "x\n")] },
        Commit { branch: "main", from: None, merge: None, msg: "Add parse", files: &[("a.txt", &v2)] },
        Commit {
            branch: "main",
            from: None,
            merge: None,
            msg: "Spread",
            files: &[("a.txt", &v3), ("b.txt", "x\ntoken here\n")],
        },
        Commit { branch: "main", from: None, merge: None, msg: "Unrelated", files: &[("a.txt", &v4)] },
    ])
}

#[gitti_test::test]
fn typing_filters_the_list_and_the_hunks() {
    let dir = repo();
    let out = gitty(
        dir.path(),
        "wait;cmds;press:/;type:parse token;cmds;changes;key:enter;state;dump;\
         key:alt-a;dump;key:alt-a;key:/;key:alt-s;changes;key:esc;changes;state",
    );
    let blocks = cmds(&out);
    // One scan for the whole typed query (one chunk: one diff-tree), nothing else.
    let scans: Vec<&String> = blocks[1].iter().filter(|c| c.contains("diff-tree")).collect();
    assert_eq!(scans.len(), 1, "{out}");
    assert!(scans[0].contains("--stdin") && scans[0].contains("-G"), "{out}");
    let ch: Vec<&str> = out.lines().filter(|l| l.starts_with("changes:")).collect();
    assert!(
        ch[0].contains("jobs=1")
            && ch[0].contains("hits=[Add parse [both words in one line], Spread [both words across 2 files]]"),
        "{out}"
    );
    // ⏎: the best hit selected, focus on the list.
    let st = states(&out);
    assert!(st[0].contains("Add parse]") && word(st[0], "focus") == "Commits", "{out}");
    let screens = screens(&out);
    assert!(screens[0].contains("/ parse token") && screens[0].contains("one line"), "{out}");
    assert!(screens[0].contains("experimental ·"), "{out}");
    // The diff: only the hunk with the words (a.txt's first line), not the whole file;
    // alt-a shows every hunk (the same here: one hunk) — the file stays.
    assert!(screens[0].contains("fn parse(token)"), "{out}");
    assert!(screens[1].contains("fn parse(token)"), "{out}");
    // Strict: both words were added whole, so the same hits.
    assert!(ch[1].contains("strict=true") && ch[1].contains("Add parse"), "{out}");
    // esc closes; the list is whole again.
    assert_eq!(ch[2], "changes: closed", "{out}");
}

#[gitti_test::test]
fn hunks_without_the_words_are_left_out() {
    let dir = repo();
    // "Unrelated" changes two places of a.txt: only the hunk with the word shows, alt-a
    // shows the other one too.
    let out = gitty(dir.path(), "wait;press:/;type:twenty;key:enter;changes;dump;key:alt-a;dump");
    let ch: Vec<&str> = out.lines().filter(|l| l.starts_with("changes:")).collect();
    assert!(ch[0].contains("hits=[Unrelated [1 line]]"), "{out}");
    let screens = screens(&out);
    assert!(screens[0].contains("line twenty"), "{out}");
    // The second hunk (line 35 → thirty-five) only with alt-a.
    assert!(!screens[0].contains("@@ -34,7"), "{out}");
    assert!(screens[1].contains("@@ -34,7"), "{out}");
}
