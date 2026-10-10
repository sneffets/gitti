//! Repository operations against real temporary repositories (fast-import fixtures):
//! branches / tags / stashes / remotes, cherry-pick / revert / reset / merge and their
//! options, conflicts from a stash and a merge, the 3-way merge document, switching with
//! blocking files, rewording, config / upstream / file versions and SSH-signed commits.

use std::fs;
use std::path::Path;

use gitti_core::ops::{RepoState, ResetMode};
use gitti_core::Repo;

use crate::fixture_kit::{History, append_config, f, repo_with, sh, sh_ok};

fn read(p: &Path, name: &str) -> String {
    fs::read_to_string(p.join(name)).unwrap()
}

fn index(p: &Path, name: &str) -> String {
    sh(p, &["show", &format!(":{name}")])
}

/// A repository with one commit (`a.txt` = `1`).
fn one_commit(files: &[(&str, &str)]) -> (tempfile::TempDir, std::path::PathBuf, Repo) {
    let mut h = History::new();
    let files: Vec<_> = files.iter().map(|(n, t)| (*n, f(t))).collect();
    h.commit("main", &[], "init", &files);
    let (tmp, p, _) = repo_with(&h);
    let repo = Repo::open(&p).unwrap();
    (tmp, p, repo)
}

#[gitti_test::test]
fn branch_tag_stash_remote_ops() {
    let (_tmp, p, r) = one_commit(&[("a.txt", "1\n")]);
    r.create_branch("topic", "HEAD", false).unwrap();
    r.rename_branch("topic", "topic2").unwrap();
    r.create_tag("v1", "HEAD", Some("release")).unwrap();
    r.create_tag("light", "HEAD", None).unwrap();
    let refs = r.refs().unwrap();
    assert!(refs.iter().any(|x| x.short_name == "topic2"));
    let tags = refs.iter().filter(|x| x.kind == gitti_core::RefKind::Tag).count();
    assert_eq!(tags, 2);
    r.delete_branch("topic2", false).unwrap();
    r.delete_tag("light").unwrap();
    let refs = r.refs().unwrap();
    let names: Vec<&str> = refs.iter().map(|x| x.short_name.as_str()).collect();
    assert!(!names.contains(&"topic2") && !names.contains(&"light"), "{names:?}");

    fs::write(p.join("a.txt"), "2\n").unwrap();
    r.stash_save("wip", false).unwrap();
    assert_eq!(read(&p, "a.txt"), "1\n");
    let st = r.stashes().unwrap();
    assert_eq!(st.len(), 1);
    assert!(st[0].message.contains("wip"));
    r.stash_apply(0, true).unwrap();
    assert_eq!(read(&p, "a.txt"), "2\n");
    assert!(r.stashes().unwrap().is_empty());

    r.add_remote("origin", "https://example.com/x.git").unwrap();
    r.set_remote_url("origin", "https://example.com/y.git").unwrap();
    r.rename_remote("origin", "up").unwrap();
    let rem = r.remotes().unwrap();
    assert_eq!(rem.len(), 1);
    assert_eq!((rem[0].name.as_str(), rem[0].fetch_url.as_str()), ("up", "https://example.com/y.git"));
    r.remove_remote("up").unwrap();
    assert!(r.remotes().unwrap().is_empty());
}

#[gitti_test::test]
fn cherry_pick_revert_reset_and_conflict_state() {
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[("a.txt", f("1\n"))]);
    let side = h.commit("side", &[root], "side b", &[("b.txt", f("b\n"))]);
    h.commit("c1", &[root], "c1", &[("a.txt", f("c1\n"))]);
    h.commit("main", &[root], "main", &[("a.txt", f("main\n"))]);
    let (_tmp, p, ids) = repo_with(&h);
    let r = Repo::open(&p).unwrap();
    r.cherry_pick(&ids[side].to_string(), false).unwrap();
    assert_eq!(read(&p, "b.txt"), "b\n");
    r.revert("HEAD", false).unwrap();
    assert!(!p.join("b.txt").exists());
    r.reset("HEAD~1", ResetMode::Hard).unwrap();
    assert_eq!(read(&p, "b.txt"), "b\n");

    // A conflicting merge: state detection, abort.
    assert!(r.merge("c1").is_err());
    assert_eq!(r.state(), RepoState::Merging);
    r.abort_operation().unwrap();
    assert_eq!(r.state(), RepoState::Clean);
    assert_eq!(read(&p, "a.txt"), "main\n");
}

#[gitti_test::test]
fn merge_and_cherry_pick_options() {
    use gitti_core::ops::{CherryPickOptions, FastForward, MergeOptions};
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[("a.txt", f("1\n"))]);
    let side = h.commit("side", &[root], "side b", &[("b.txt", f("b\n"))]);
    h.commit("diverged", &[root], "main 2", &[("a.txt", f("2\n"))]);
    let (_tmp, p, ids) = repo_with(&h);
    let r = Repo::open(&p).unwrap();
    let side = ids[side].to_string();
    // Parents and message of HEAD in one git call.
    let head = || {
        let out = sh(&p, &["log", "-1", "--format=%H%n%P%n%B"]);
        let mut l = out.lines();
        let id = l.next().unwrap().to_string();
        let parents = l.next().unwrap().split_whitespace().count();
        (id, parents, l.collect::<Vec<_>>().join("\n"))
    };
    let root = ids[root].to_string();

    // -x records the source; --no-commit leaves the change staged.
    r.cherry_pick_with(&side, false, CherryPickOptions { record_origin: true, no_commit: false })
        .unwrap();
    let (_, _, msg) = head();
    assert!(msg.contains(&format!("(cherry picked from commit {side})")), "{msg}");
    r.reset("HEAD~1", ResetMode::Hard).unwrap();
    r.cherry_pick_with(&side, false, CherryPickOptions { record_origin: false, no_commit: true })
        .unwrap();
    assert_eq!(index(&p, "b.txt"), "b\n");
    r.reset("HEAD", ResetMode::Hard).unwrap();
    assert_eq!(head().0, root, "no commit");

    // --no-ff forces a merge commit with the given message.
    let no_ff = MergeOptions { ff: FastForward::Never, message: Some("merge side".into()), ..Default::default() };
    r.merge_with("side", &no_ff).unwrap();
    let (_, parents, msg) = head();
    assert_eq!((parents, msg.trim()), (2, "merge side"));
    r.reset("HEAD~1", ResetMode::Hard).unwrap();

    // --squash: changes staged, no commit, no MERGE_HEAD.
    r.merge_with("side", &MergeOptions { squash: true, ..Default::default() }).unwrap();
    assert_eq!(index(&p, "b.txt"), "b\n");
    assert_eq!(r.state(), RepoState::Clean);

    // --ff-only on a diverged history fails; a plain merge makes a merge commit.
    r.reset("diverged", ResetMode::Hard).unwrap();
    let ff_only = MergeOptions { ff: FastForward::Only, ..Default::default() };
    assert!(r.merge_with("side", &ff_only).is_err());
    r.merge_with("side", &MergeOptions::default()).unwrap();
    assert_eq!(head().1, 2);
}

/// `git stash apply` that conflicts: no MERGE_HEAD, but an unmerged file shown as a
/// conflict (diff with markers), resolved with the stash's side.
#[gitti_test::test]
fn conflict_from_stash_apply() {
    let (_tmp, p, r) = one_commit(&[("a.txt", "1\n")]);
    fs::write(p.join("a.txt"), "stashed\n").unwrap();
    sh(&p, &["stash", "-q"]);
    fs::write(p.join("a.txt"), "committed\n").unwrap();
    sh(&p, &["commit", "-q", "-am", "c"]);
    assert!(r.stash_apply(0, false).is_err());
    assert_eq!(r.state(), RepoState::Clean);
    assert_eq!(r.conflict_sides().theirs, "stash");
    let st = r.working_state().unwrap();
    assert_eq!(st.conflicted.len(), 1);
    assert!(st.unstaged.iter().all(|f| f.path() != "a.txt"));
    let d = st.conflicted[0].diff.as_ref().expect("diff vs HEAD");
    assert!(d.hunks[0].lines.iter().any(|l| l.text.starts_with("<<<<<<<")));
    assert!(r.has_conflict_markers("a.txt"));
    r.resolve_with_side(&["a.txt".into()], false).unwrap();
    assert_eq!(read(&p, "a.txt"), "stashed\n");
    assert!(r.working_state().unwrap().conflicted.is_empty());
}

/// A merge with a content conflict and a modify/delete conflict, resolved per side and
/// concluded.
#[gitti_test::test]
fn conflicts_from_a_merge_resolved_per_side() {
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[("a.txt", f("1\n")), ("b.txt", f("b\n"))]);
    h.commit("side", &[root], "side", &[("a.txt", f("side\n")), ("b.txt", crate::fixture_kit::E::Delete)]);
    h.commit("main", &[root], "main", &[("a.txt", f("main\n")), ("b.txt", f("b2\n"))]);
    let (_tmp, p, _) = repo_with(&h);
    let r = Repo::open(&p).unwrap();
    assert!(r.merge("side").is_err());
    assert_eq!(r.working_state().unwrap().conflicted.len(), 2);
    let sides = r.conflict_sides();
    assert_eq!((sides.ours.as_str(), sides.theirs.as_str()), ("main", "side"));
    r.resolve_with_side(&["a.txt".into()], true).unwrap();
    assert_eq!(read(&p, "a.txt"), "main\n");
    r.resolve_with_side(&["b.txt".into()], false).unwrap();
    assert!(!p.join("b.txt").exists());
    assert!(r.working_state().unwrap().conflicted.is_empty());
    r.continue_operation().unwrap();
    assert_eq!(r.state(), RepoState::Clean);
}

#[gitti_test::test]
fn merge_doc_from_conflict_stages() {
    use gitti_core::merge::{ChunkKind, Resolution};
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[("f.txt", f("head\nsep\na\nmid\nb\ntail\n"))]);
    h.commit("side", &[root], "side", &[("f.txt", f("head\nsep\nA-side\nmid\nb\ntail\nadded by side\n"))]);
    h.commit("main", &[root], "main", &[("f.txt", f("HEAD-main\nsep\nA-main\nmid\nb\ntail\n"))]);
    let (_tmp, p, _) = repo_with(&h);
    let r = Repo::open(&p).unwrap();
    assert!(r.merge("side").is_err());

    let doc = r.merge_doc("f.txt").unwrap();
    let kinds: Vec<_> = doc.chunks.iter().map(|c| c.kind).collect();
    use ChunkKind::*;
    assert_eq!(kinds, vec![Ours, Stable, Conflict, Stable, Theirs]);
    assert_eq!(doc.chunks[2].ours, vec!["A-main\n"]);
    assert_eq!(doc.chunks[2].theirs, vec!["A-side\n"]);

    let text = doc.result_text(&[None, None, Some(Resolution::TheirsThenOurs)], ("main", "side"));
    assert_eq!(text, "HEAD-main\nsep\nA-side\nA-main\nmid\nb\ntail\nadded by side\n");
    r.write_worktree_file("f.txt", &text).unwrap();
    r.mark_resolved(&["f.txt".into()]).unwrap();
    assert!(r.working_state().unwrap().conflicted.is_empty());
    assert_eq!(index(&p, "f.txt"), text);
}

/// `main` (`a.txt` = 1 2 3) and `feat` (`a.txt` = 1 2 feat, plus `x.txt`).
fn switch_fixture() -> (tempfile::TempDir, std::path::PathBuf, Repo) {
    let mut h = History::new();
    let root = h.commit("main", &[], "init", &[("a.txt", f("1\n2\n3\n"))]);
    h.commit("feat", &[root], "feat", &[("a.txt", f("1\n2\nfeat\n")), ("x.txt", f("theirs\n"))]);
    let (tmp, p, _) = repo_with(&h);
    let repo = Repo::open(&p).unwrap();
    (tmp, p, repo)
}

fn branch(p: &Path) -> String {
    sh(p, &["branch", "--show-current"]).trim().to_string()
}

/// Unrelated untracked files never block a switch; one the target tracks does, and is
/// moved into a stash (kept there) when switching anyway.
#[gitti_test::test]
fn switch_moves_blocking_untracked_files_aside() {
    use gitti_core::switch::{SwitchOutcome, SwitchTarget};
    let (_tmp, p, repo) = switch_fixture();
    fs::write(p.join("keep.txt"), "mine\n").unwrap();
    let target = SwitchTarget::Local("feat".into());
    assert!(matches!(repo.switch(&target).unwrap(), SwitchOutcome::Done(_)));
    sh(&p, &["switch", "-q", "main"]);

    fs::write(p.join("x.txt"), "ours\n").unwrap();
    let SwitchOutcome::Blocked(block) = repo.switch(&target).unwrap() else {
        panic!("switch should be blocked");
    };
    assert_eq!(block.untracked, vec!["x.txt"]);
    assert!(block.local.is_empty(), "{block:?}");
    let msg = repo.switch_moving_aside(&target, &block).unwrap();
    assert!(msg.contains("stash"), "{msg}");
    assert_eq!(branch(&p), "feat");
    assert_eq!(read(&p, "x.txt"), "theirs\n");
    assert_eq!(read(&p, "keep.txt"), "mine\n", "other untracked files stay");
    let stashed = sh(&p, &["show", "stash@{0}^3:x.txt"]);
    assert_eq!(stashed, "ours\n", "our untracked version is kept in the stash");
}

/// A local change to a file the target changes too: stashed, switched, popped back —
/// or, overlapping with the target's change, left conflicted with the stash kept.
#[gitti_test::test]
fn switch_moves_local_changes_aside() {
    use gitti_core::switch::{SwitchOutcome, SwitchTarget};
    let (_tmp, p, repo) = switch_fixture();
    let target = SwitchTarget::Local("feat".into());
    fs::write(p.join("a.txt"), "local\n2\n3\n").unwrap();
    let SwitchOutcome::Blocked(block) = repo.switch(&target).unwrap() else {
        panic!("switch should be blocked");
    };
    assert_eq!(block.local, vec!["a.txt"]);
    let msg = repo.switch_moving_aside(&target, &block).unwrap();
    assert!(msg.contains("re-applied"), "{msg}");
    assert_eq!(read(&p, "a.txt"), "local\n2\nfeat\n");

    sh(&p, &["checkout", "-q", "-f", "main"]);
    fs::write(p.join("a.txt"), "1\n2\nmain-local\n").unwrap();
    let SwitchOutcome::Blocked(block) = repo.switch(&target).unwrap() else {
        panic!("switch should be blocked");
    };
    let msg = repo.switch_moving_aside(&target, &block).unwrap();
    assert!(msg.contains("conflicted"), "{msg}");
    assert_eq!(branch(&p), "feat");
    assert!(read(&p, "a.txt").contains("<<<<<<<"));
    assert_eq!(repo.stashes().unwrap().len(), 1, "the stash is kept");
}

/// "Edit Commit Message": HEAD is amended (`--only`: the staged file is not committed),
/// an older commit reworded by a rebase; staged and unstaged work survives both.
#[gitti_test::test]
fn reword_head_and_older_commit_keeps_staged_and_unstaged_changes() {
    let mut h = History::new();
    let c1 = h.commit("main", &[], "first", &[("a.txt", f("1\n"))]);
    let c2 = h.commit("main", &[c1], "second", &[("a.txt", f("2\n"))]);
    let c3 = h.commit("main", &[c2], "third", &[("a.txt", f("3\n"))]);
    let (_tmp, p, ids) = repo_with(&h);
    let repo = Repo::open(&p).unwrap();
    // Work in progress: one staged file, one unstaged change.
    fs::write(p.join("b.txt"), "new\n").unwrap();
    repo.stage_paths(&["b.txt"]).unwrap();
    fs::write(p.join("a.txt"), "wip\n").unwrap();

    repo.reword(ids[c3], "third, reworded\n\nwith body").unwrap();
    let out = sh(&p, &["log", "-1", "--format=%B", "--name-only"]);
    assert!(out.starts_with("third, reworded\n\nwith body\n"), "{out}");
    assert!(!out.contains("b.txt"), "staged file not committed: {out}");

    repo.reword(ids[c2], "second, Gr\u{fc}\u{df}e").unwrap();
    assert_eq!(sh(&p, &["log", "--format=%s"]), "third, reworded\nsecond, Gr\u{fc}\u{df}e\nfirst\n");
    assert_eq!(read(&p, "a.txt"), "wip\n", "unstaged change kept");
    assert_eq!(sh(&p, &["diff", "--cached", "--name-only"]), "b.txt\n", "staged kept");
    assert!(!p.join(".git/rebase-merge").exists());
}

/// The root commit is reworded with `rebase --root`; pushed commits are refused.
#[gitti_test::test]
fn reword_the_root_commit_and_not_pushed_ones() {
    let mut h = History::new();
    let c1 = h.commit("main", &[], "first", &[("a.txt", f("1\n"))]);
    h.commit("main", &[c1], "second", &[("a.txt", f("2\n"))]);
    let (_tmp, p, ids) = repo_with(&h);
    let repo = Repo::open(&p).unwrap();
    repo.reword(ids[c1], "the root").unwrap();
    let log = sh(&p, &["log", "--format=%H %s"]);
    let subjects: Vec<&str> = log.lines().map(|l| &l[41..]).collect();
    assert_eq!(subjects, ["second", "the root"]);

    // Pushed (a remote-tracking branch contains it): refused.
    let head = crate::fixture_kit::oid(&log[..40]);
    fs::create_dir_all(p.join(".git/refs/remotes/origin")).unwrap();
    fs::write(p.join(".git/refs/remotes/origin/main"), format!("{head}\n")).unwrap();
    let why = repo.reword_blocker(head).unwrap().unwrap();
    assert!(why.contains("pushed"), "{why}");
    assert!(repo.reword(head, "nope").is_err());
}

/// Rewording an older commit needs no sed / cp (a minimal Git for Windows has
/// neither): git's PATH holds git alone, the message (backslash, indented line, no last
/// newline) arrives as typed.
#[cfg(unix)]
#[gitti_test::test]
fn reword_needs_no_tools_but_git() {
    let mut h = History::new();
    let c1 = h.commit("main", &[], "first", &[("a.txt", f("1\n"))]);
    let c2 = h.commit("main", &[c1], "second", &[("a.txt", f("2\n"))]);
    h.commit("main", &[c2], "third", &[("a.txt", f("3\n"))]);
    let (tmp, p, ids) = repo_with(&h);
    let bin = tmp.path().join("only-git");
    fs::create_dir(&bin).unwrap();
    let git = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|d| d.join("git"))
        .find(|g| g.is_file())
        .unwrap();
    std::os::unix::fs::symlink(&git, bin.join("git")).unwrap();
    let mut repo = Repo::open(&p).unwrap();
    repo.set_env(vec![("PATH".into(), bin.to_string_lossy().into_owned())]);
    repo.reword(ids[c2], "second, new\n\n  indented \\ body").unwrap();
    assert_eq!(
        sh(&p, &["log", "--format=%B|", "-2"]),
        "third\n|\nsecond, new\n\n  indented \\ body\n|\n"
    );
}

#[gitti_test::test]
fn config_upstream_and_blobs() {
    use gitti_core::blobs::Version;
    use gitti_core::ops::ConfigScope;
    let mut h = History::new();
    let one = h.commit("main", &[], "init", &[("a.txt", f("one\n"))]);
    h.commit("main", &[one], "second", &[("a.txt", f("two\n"))]);
    h.set_ref("refs/remotes/origin/main", one);
    let (_tmp, p, _) = repo_with(&h);
    append_config(&p, "[remote \"origin\"]\n\turl = /nowhere\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n");
    let r = Repo::open(&p).unwrap();
    // The global file as git itself reports it (no $HOME assumption, see Windows).
    let global = r.config_path(ConfigScope::Global).unwrap();
    assert!(global.is_absolute(), "{}", global.display());
    assert!(
        global.ends_with(".gitconfig") || global.ends_with("git/config"),
        "{}",
        global.display()
    );
    r.config_set(ConfigScope::Local, "user.name", "Local Name").unwrap();
    assert_eq!(r.config_get(ConfigScope::Local, "user.name").as_deref(), Some("Local Name"));
    assert_eq!(r.config_effective("user.name").as_deref(), Some("Local Name"));
    r.config_set(ConfigScope::Local, "pull.rebase", "true").unwrap();
    r.config_set(ConfigScope::Local, "pull.rebase", "").unwrap();
    assert_eq!(r.config_get(ConfigScope::Local, "pull.rebase"), None);
    // Unsetting a missing key is fine.
    r.config_set(ConfigScope::Local, "pull.rebase", "").unwrap();

    r.set_upstream("main", "origin/main").unwrap();
    let t = r.tracking().unwrap();
    assert_eq!(t[0].upstream.as_deref(), Some("origin/main"));
    assert_eq!((t[0].ahead, t[0].behind), (1, 0));
    r.unset_upstream("main").unwrap();
    assert_eq!(r.tracking().unwrap()[0].upstream, None);

    // File versions.
    fs::write(p.join("a.txt"), "three\n").unwrap();
    let v = r
        .read_versions(&[
            (Version::Worktree, "a.txt".into()),
            (Version::Index, "a.txt".into()),
            (Version::Rev("HEAD~1".into()), "a.txt".into()),
            (Version::Rev("HEAD".into()), "missing.txt".into()),
        ])
        .unwrap();
    assert_eq!(v[0].as_deref(), Some("three\n"));
    assert_eq!(v[1].as_deref(), Some("two\n"));
    assert_eq!(v[2].as_deref(), Some("one\n"));
    assert_eq!(v[3], None);

    let out = r.run_user_command(&["log".into(), "--format=%s".into()]).unwrap();
    assert_eq!(out, "second\ninit");
}

#[gitti_test::test]
fn ssh_signed_commit_verification() {
    use gitti_core::signature::{SigKind, SigStatus};
    let (_tmp, p, mut r) = one_commit(&[("a.txt", "1\n")]);
    // The user's global config may name an allowed signers file: not here.
    r.set_env(vec![
        ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ]);
    let unsigned = crate::fixture_kit::rev(&p, "HEAD");
    assert_eq!(r.verify_signature(unsigned).unwrap(), None);

    let keys = tempfile::tempdir().unwrap();
    let key = keys.path().join("id");
    let st = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "t@example.com", "-f"])
        .arg(&key)
        .status()
        .unwrap();
    assert!(st.success());
    let key_path = key.to_str().unwrap().replace('\\', "/");
    append_config(&p, &format!("[gpg]\n\tformat = ssh\n[user]\n\tsigningkey = {key_path}\n"));
    fs::write(p.join("a.txt"), "2\n").unwrap();
    assert!(sh_ok(&p, &["commit", "-q", "-S", "-am", "signed"]));
    let signed = crate::fixture_kit::rev(&p, "HEAD");

    // Without an allowed signers file git cannot check SSH signatures.
    let info = r.verify_signature(signed).unwrap().unwrap();
    assert_eq!(info.kind, SigKind::Ssh);
    assert_eq!(info.status, SigStatus::CannotCheck);
    assert!(info.hint().unwrap().contains("allowedSignersFile"), "{info:?}");

    // Non-ASCII principals survive (git's `LC_ALL=C` made ssh-keygen mangle the "ß"), and
    // the file / line that verified it is named.
    let pubkey = fs::read_to_string(key.with_extension("pub")).unwrap();
    let other = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGb8Y3Z1rHxkK6c0dX5JbQpJ4vC1mWZ0T5o8U0aY2Yxq other";
    let allowed = keys.path().join("allowed");
    let principal = "j\u{fc}rgen@gr\u{f6}\u{df}e.de";
    fs::write(&allowed, format!("# team\n\nother@x {other}\n{principal},j@x namespaces=\"git\" {pubkey}")).unwrap();
    let allowed_path = allowed.to_str().unwrap().replace('\\', "/");
    append_config(&p, &format!("[gpg \"ssh\"]\n\tallowedSignersFile = {allowed_path}\n"));
    let info = r.verify_signature(signed).unwrap().unwrap();
    assert_eq!(info.status, SigStatus::Good, "{info:?}");
    assert_eq!(info.signer, principal, "{info:?}");
    assert!(info.key.starts_with("SHA256:"), "{info:?}");
    assert!(info.output.contains(principal), "{info:?}");
    let src = info.sources.join("\n");
    assert!(src.contains(&format!("line 4: {principal},j@x namespaces=\"git\"")), "{src}");
    assert!(info.details().contains("Verified with:"), "{}", info.details());
}

/// With nothing staged Commit asks — "Stage All and Commit" takes modified,
/// deleted and untracked files in (one `add -A` + the commit); "Stage Tracked Files and
/// Commit" only the tracked ones (`add -u`, offered when untracked files would come along);
/// "Create Empty Commit" makes a commit without changes; the plan says which applies, the
/// question says in plain words what each one stages.
#[gitti_test::test]
fn nothing_staged_stage_all_or_empty_commit() {
    use gitti_core::ops::{
        CommitKind, CommitPlan, UnstagedCounts, empty_commit_blocker, nothing_staged_prompt,
        plan_commit,
    };
    let (_tmp, p, r) = one_commit(&[("a.txt", "1\n"), ("gone.txt", "x\n")]);
    let plan = |staged, unstaged, amend, msg| {
        plan_commit(RepoState::Clean, 0, staged, unstaged, amend, msg)
    };
    assert_eq!(plan(0, 0, false, "m"), CommitPlan::NothingStaged { can_stage_all: false });
    assert_eq!(plan(0, 2, false, "m"), CommitPlan::NothingStaged { can_stage_all: true });
    assert_eq!(plan(1, 2, false, "m"), CommitPlan::Run(CommitKind::Staged));
    assert_eq!(plan(0, 0, true, "m"), CommitPlan::Run(CommitKind::Amend));
    assert_eq!(plan(0, 2, false, " \n"), CommitPlan::NoMessage);
    assert_eq!(
        plan_commit(RepoState::Merging, 0, 0, 0, false, ""),
        CommitPlan::ConcludeMerge
    );
    assert_eq!(
        plan_commit(RepoState::Merging, 0, 0, 0, false, "merge it"),
        CommitPlan::Run(CommitKind::Staged)
    );
    assert!(matches!(plan_commit(RepoState::Merging, 1, 0, 0, false, "m"), CommitPlan::Blocked(_)));
    let kinds = |c: &UnstagedCounts| -> Vec<CommitKind> {
        nothing_staged_prompt(c).2.into_iter().map(|(_, k)| k).collect()
    };
    let tracked = UnstagedCounts { tracked: 2, ..Default::default() };
    let (title, detail, _) = nothing_staged_prompt(&tracked);
    assert_eq!(title, "Nothing staged");
    assert!(detail.contains("2 tracked files with changes"), "{detail}");
    assert!(detail.contains("until you push it"), "{detail}");
    assert!(detail.contains("stays staged"), "{detail}");
    assert_eq!(kinds(&tracked), [CommitKind::StageAll, CommitKind::Empty]);
    let both = UnstagedCounts { tracked: 1, untracked_files: 2, untracked_folders: 1, ..Default::default() };
    let (_, detail, choices) = nothing_staged_prompt(&both);
    assert!(
        detail.contains("1 tracked file with changes (modified or deleted) and 2 untracked files and 1 untracked folder"),
        "{detail}"
    );
    assert!(detail.contains("git add -u"), "{detail}");
    assert_eq!(choices[1], ("Stage Tracked Files and Commit", CommitKind::StageTracked));
    assert_eq!(kinds(&both).len(), 3);
    // Untracked only: nothing tracked to stage alone.
    let new = UnstagedCounts { untracked_files: 1, ..Default::default() };
    assert_eq!(kinds(&new), [CommitKind::StageAll, CommitKind::Empty]);
    // Hidden by `status.showUntrackedFiles=no`: said, and the tracked-only way offered.
    let hidden = UnstagedCounts { tracked: 1, untracked_hidden: true, ..Default::default() };
    let (_, detail, _) = nothing_staged_prompt(&hidden);
    assert!(detail.contains("showUntrackedFiles"), "{detail}");
    assert_eq!(kinds(&hidden).len(), 3);
    let (title, _, choices) = nothing_staged_prompt(&UnstagedCounts::default());
    assert_eq!(title, "Nothing to commit");
    assert_eq!(choices, [("Create Empty Commit", CommitKind::Empty)]);
    // Only changes inside a submodule: nothing to stage here, said so.
    let sub = UnstagedCounts { inside_submodules: 1, ..Default::default() };
    let (title, detail, _) = nothing_staged_prompt(&sub);
    assert_eq!(title, "Nothing to commit");
    assert!(detail.contains("1 submodule has changed only inside"), "{detail}");
    // The palette's "Create Empty Commit" with files staged / amend on: refused.
    assert!(empty_commit_blocker(1, false).is_some_and(|w| w.contains("1 file(s) staged")));
    assert!(empty_commit_blocker(0, true).is_some());
    assert_eq!(empty_commit_blocker(0, false), None);

    fs::write(p.join("a.txt"), "2\n").unwrap();
    fs::remove_file(p.join("gone.txt")).unwrap();
    fs::write(p.join("new.txt"), "n\n").unwrap();
    let counts = UnstagedCounts::of(&r.working_state().unwrap());
    let want = UnstagedCounts { tracked: 2, untracked_files: 1, ..Default::default() };
    assert_eq!(counts, want);
    r.commit_as("Everything", CommitKind::StageAll).unwrap();
    assert_eq!(sh(&p, &["status", "--porcelain"]), "");
    assert_eq!(
        sh(&p, &["show", "--name-status", "--format=%s", "HEAD"]).trim(),
        "Everything\n\nM\ta.txt\nD\tgone.txt\nA\tnew.txt"
    );
    r.commit_as("Nothing", CommitKind::Empty).unwrap();
    assert_eq!(sh(&p, &["log", "--format=%s"]), "Nothing\nEverything\ninit\n");
    assert_eq!(sh(&p, &["diff", "HEAD~1", "HEAD", "--stat"]), "");
    // Tracked only: the untracked file stays out.
    fs::write(p.join("a.txt"), "3\n").unwrap();
    fs::write(p.join("later.txt"), "l\n").unwrap();
    r.commit_as("Tracked", CommitKind::StageTracked).unwrap();
    assert_eq!(sh(&p, &["show", "--name-status", "--format=", "HEAD"]).trim(), "M\ta.txt");
    assert_eq!(sh(&p, &["status", "--porcelain"]), "?? later.txt\n");
}
