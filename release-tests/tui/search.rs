//! End-to-end: Go to Commit (palette) — the shared smart search of gitti: a file name
//! lists the commits that changed the file (through a rename), each row says why, the line
//! above says what was searched; Enter selects the commit and puts the diff cursor on the
//! file.

use crate::common;

use common::*;

fn renamed_repo() -> tempfile::TempDir {
    let dir = repo();
    let d = dir.path();
    let body: String = (0..20).map(|i| format!("parser line {i}\n")).collect();
    std::fs::create_dir_all(d.join("src/core")).unwrap();
    std::fs::write(d.join("src/foo.rs"), &body).unwrap();
    git(d, &["add", "-A"]);
    git(d, &["commit", "-qm", "Add the parser"]);
    git(d, &["mv", "src/foo.rs", "src/core/bar.rs"]);
    git(d, &["commit", "-qm", "Move the parser"]);
    std::fs::write(d.join("src/core/bar.rs"), format!("{body}more\n")).unwrap();
    std::fs::write(d.join("zzz.txt"), "z\n").unwrap();
    git(d, &["add", "-A"]);
    git(d, &["commit", "-qm", "Edit things"]);
    dir
}

#[test]
fn go_to_commit_finds_file_history_and_selects_the_file() {
    let dir = renamed_repo();
    let d = dir.path();
    let out = gitty(
        d,
        "key:f1;type:go to commit;key:enter;type:bar.rs;dump;key:down;key:enter;state",
    );
    let screen = screens(&out)[0];
    if std::env::var_os("SHOW").is_some() {
        println!("{out}");
    }
    // Every commit of the file, through its rename, each with the path it had.
    assert!(screen.contains("Edit things"), "{screen}");
    assert!(screen.contains("Move the parser"), "{screen}");
    assert!(screen.contains("Add the parser"), "{screen}");
    assert!(screen.contains("src/foo.rs → src/core/bar.rs"), "{screen}");
    assert!(!screen.contains("Initial commit"), "{screen}");
    // What was searched.
    assert!(
        screen.contains("No commit message or author contains 'bar.rs'"),
        "{screen}"
    );
    // The second hit ("Move the parser"), its file under the diff cursor.
    assert!(out.contains("Move the parser]"), "{out}");
    assert!(out.contains("focus=Commits"), "{out}");
    let state = out.lines().find(|l| l.starts_with("focus=")).unwrap();
    assert!(state.contains("File {"), "diff cursor on the file header: {state}");

    // Plain words search messages (no git run); nothing found is said.
    let out = gitty(d, "key:f1;type:go to commit;key:enter;cmds;type:parser;cmds;dump;type:qq;dump");
    assert!(out.contains("cmds: 0\n"), "text search runs no git: {out}");
    let s = screens(&out);
    assert!(s[0].contains("Move the parser") && s[0].contains("Add the parser"), "{}", s[0]);
    assert!(s[1].contains("Nothing matches 'parserqq'"), "{}", s[1]);
}
