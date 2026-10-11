//! gitti-core's integration tests: one test binary (one link) instead of one per file.
//! `cargo test --workspace --test core -- staging::` runs one module's tests (`--workspace`,
//! not `-p`: cargo would resolve dependency features differently and rebuild them).
//! `askpass_env` stays its own binary (own `main`, environment set before any thread).

mod allowed_signers;
mod change_search;
mod commit_box;
mod config_safety;
mod conflicts;
mod custom_commands;
mod discard;
mod fixture_kit;
mod fs_paths;
mod fx;
mod history_diffs;
mod hostile_config;
mod lfs;
mod lfs_health;
mod linux_env;
mod merge_preview;
mod navigate;
mod non_utf8;
mod pull;
mod push;
mod reflog;
mod repo_ops;
mod repo_setup;
mod repo_shapes;
mod restore;
mod runner;
mod search;
mod sha256;
mod sidebar_ops;
mod signature_keys;
mod staging;
mod stash;
mod submodules;
mod subtrees;
mod switching;
mod tab_marks;
mod unix_fs;
mod worktrees;

/// The test repositories' settings — identity, no signing, contents as written whatever
/// the user's global config says, no `git maintenance run --auto` after every commit /
/// fetch / merge (a process each) — appended to `<dir>/.git/config` instead of four or
/// five `git config` processes (~0.5 s on Windows).
pub fn test_config(dir: &std::path::Path) {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(dir.join(".git").join("config"))
        .unwrap();
    f.write_all(
        b"[user]\n\tname = T\n\temail = t@example.com\n[commit]\n\tgpgsign = false\n\
          [tag]\n\tgpgsign = false\n[core]\n\tautocrlf = false\n[maintenance]\n\tauto = false\n",
    )
    .unwrap();
}

/// Every test of the workspace has the time limit (2 s, `crates/gitti-test`): a plain
/// `#[test]` would not.
#[gitti_test::test]
fn every_test_has_the_time_limit() {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut plain = Vec::new();
    let mut dirs = vec![crates];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                for (i, line) in text.lines().enumerate() {
                    if line.trim() == "#[test]" {
                        plain.push(format!("{}:{}", path.display(), i + 1));
                    }
                }
            }
        }
    }
    assert!(plain.is_empty(), "`#[test]` instead of `#[gitti_test::test]`:\n{}", plain.join("\n"));
}
