//! The user's git config honoured, a repository's own config kept from running programs:
//! `status.showUntrackedFiles` (`no` hides untracked files and says so, `normal` lists an
//! untracked folder as one entry that stages and discards as a whole, unset keeps every
//! file), `core.fsmonitor` hooks and `gpg.program`s from the
//! repository's config never run on gitti's reads, and git's `safe.directory` refusal is
//! recognised and fixed by trusting the folder.
//!
//! Not pinned, deliberately: a repository's own clean / smudge filters. git runs them on
//! status and working-tree diffs as it does for any repository its owner opens
//! (`safe.directory` guards foreign ones); `git-crypt unlock` and `git lfs install --local`
//! write theirs into the repository's config, and without them status and diffs would
//! compare the plain file with the stored ciphertext / pointer. textconv and external
//! diff drivers never run on gitti's diffs (`--no-textconv --no-ext-diff`,
//! `hostile_config.rs`).

use std::fs;
use std::path::Path;

use gitti_core::safe_directory::{self, DubiousOwnership};
use gitti_core::status::UntrackedFiles;
use gitti_core::{DiffOptions, Repo};

use crate::fixture_kit::{append_config, init, sh, write_objects};

fn untracked(repo: &Repo) -> Vec<String> {
    let status = repo.status().unwrap();
    status.entries.iter().filter(|e| e.untracked).map(|e| e.path.clone()).collect()
}

/// `top.txt`, `d/a`, `d/e/b` untracked, `d/x.o` ignored (one git process).
fn untracked_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("r");
    init(&dir);
    fs::create_dir_all(dir.join("d/e")).unwrap();
    fs::write(dir.join("top.txt"), "top\n").unwrap();
    fs::write(dir.join("d/a"), "a\n").unwrap();
    fs::write(dir.join("d/e/b"), "b\n").unwrap();
    fs::write(dir.join("d/x.o"), "obj\n").unwrap();
    fs::write(dir.join(".git/info/exclude"), "*.o\n").unwrap();
    (tmp, dir)
}

#[gitti_test::test]
fn show_untracked_files_config_is_honoured() {
    let (_tmp, dir) = untracked_repo();
    let repo = Repo::open(&dir).unwrap();
    // Unset: every file, as before.
    assert_eq!(repo.untracked_files(), UntrackedFiles::All);
    assert_eq!(untracked(&repo), ["d/a", "d/e/b", "top.txt"]);

    append_config(&dir, "[status]\n\tshowUntrackedFiles = normal\n");
    assert_eq!(repo.untracked_files(), UntrackedFiles::Normal);
    assert_eq!(untracked(&repo), ["d/", "top.txt"]);

    append_config(&dir, "[status]\n\tshowUntrackedFiles = no\n");
    let status = repo.status().unwrap();
    assert!(status.untracked_hidden());
    assert!(status.entries.is_empty(), "{:?}", status.entries);
    // The working state carries it to the apps.
    assert!(repo.working_state().unwrap().status.untracked_hidden());
}

/// `normal`: the folder entry `d/` stages as a whole, a path-limited re-read after
/// unstaging folds the files back into it like a full read, and discarding it deletes the
/// untracked files but keeps the ignored one.
#[gitti_test::test]
fn untracked_folder_entry_stages_and_discards_as_a_whole() {
    let (_tmp, dir) = untracked_repo();
    append_config(&dir, "[status]\n\tshowUntrackedFiles = normal\n");
    let repo = Repo::open(&dir).unwrap();
    let opts = DiffOptions::default();
    let ws = repo.working_state().unwrap();
    let paths = |files: &[gitti_core::WorkFile]| -> Vec<String> {
        files.iter().map(|f| f.path().to_string()).collect()
    };
    assert_eq!(paths(&ws.unstaged), ["d/", "top.txt"]);
    assert!(ws.unstaged[0].diff.is_none(), "a folder has no preview");

    repo.stage_paths(&["d/"]).unwrap();
    let ws = repo.working_state_after(&opts, &ws, &["d/".into()]).unwrap();
    assert_eq!(paths(&ws.staged), ["d/a", "d/e/b"]);
    assert_eq!(paths(&ws.unstaged), ["top.txt"]);

    repo.unstage_paths(&["d/a", "d/e/b"]).unwrap();
    let after = repo
        .working_state_after(&opts, &ws, &["d/a".into(), "d/e/b".into()])
        .unwrap();
    assert_eq!(paths(&after.unstaged), ["d/", "top.txt"], "folded like a full read");
    assert!(after.staged.is_empty());

    let entry = after.unstaged[0].entry.clone();
    repo.discard_paths(&[&entry]).unwrap();
    assert!(!dir.join("d/a").exists() && !dir.join("d/e").exists());
    assert!(dir.join("d/x.o").exists(), "ignored files stay");
    assert!(dir.join("top.txt").exists());
}

/// A marker script (`<marker name>.sh` in `dir`): running it creates `marker`.
#[cfg(unix)]
fn marker_script(dir: &Path, marker: &Path) -> String {
    use std::os::unix::fs::PermissionsExt as _;
    let name = marker.file_name().unwrap().to_string_lossy();
    let script = dir.join(format!("{name}.sh"));
    fs::write(&script, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    script.display().to_string()
}

/// A repository whose config names an fsmonitor hook: gitti's status and diffs never run
/// it (plain `git status` does).
#[cfg(unix)]
#[gitti_test::test]
fn reads_never_run_a_repository_fsmonitor_hook() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("r");
    init(&dir);
    let marker = tmp.path().join("fsmonitor-ran");
    let script = marker_script(tmp.path(), &marker);
    append_config(&dir, &format!("[core]\n\tfsmonitor = {script}\n"));
    fs::write(dir.join("f.txt"), "x\n").unwrap();
    let repo = Repo::open(&dir).unwrap();
    let ws = repo.working_state().unwrap();
    assert_eq!(ws.unstaged.len(), 1);
    assert!(!marker.exists(), "gitti's reads ran the repository's fsmonitor hook");
    // The fixture is real: git itself runs it.
    sh(&dir, &["status", "--porcelain"]);
    assert!(marker.exists(), "the hook did not run under plain git either");
}

/// Selecting a signed commit verifies it with the user's programs, never ones the
/// repository's config names — `gpg.program` (OpenPGP), `gpg.ssh.program` (SSH, with the
/// repository's allowed signers file, which git needs before it runs it) and
/// `gpg.x509.program` (X.509); plain `git log %G?` runs each of them.
#[cfg(unix)]
#[gitti_test::test]
fn signature_check_never_runs_a_repository_gpg_program() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("r");
    init(&dir);
    let kinds = [("openpgp", "PGP SIGNATURE"), ("ssh", "SSH SIGNATURE"), ("x509", "SIGNED MESSAGE")];
    let markers: Vec<_> = kinds.iter().map(|(k, _)| tmp.path().join(format!("{k}-ran"))).collect();
    let scripts: Vec<String> = markers.iter().map(|m| marker_script(tmp.path(), m)).collect();
    fs::write(dir.join("allowed_signers"), "t@example.com ssh-ed25519 AAAA\n").unwrap();
    append_config(
        &dir,
        &format!(
            "[gpg]\n\tprogram = {}\n[gpg \"ssh\"]\n\tprogram = {}\n\tallowedSignersFile = allowed_signers\n\
             [gpg \"x509\"]\n\tprogram = {}\n",
            scripts[0], scripts[1], scripts[2]
        ),
    );
    let commits: Vec<Vec<u8>> = kinds
        .iter()
        .map(|(kind, armor)| {
            format!(
                "tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904\n\
                 author T <t@example.com> 1700000000 +0000\n\
                 committer T <t@example.com> 1700000000 +0000\n\
                 gpgsig -----BEGIN {armor}-----\n \n iQEzBAABCAAdFiEE\n -----END {armor}-----\n\n\
                 signed {kind}\n"
            )
            .into_bytes()
        })
        .collect();
    let ids = write_objects(&dir, "commit", &commits);
    let repo = Repo::open(&dir).unwrap();
    let programs = repo.verify_programs();
    for program in [&programs.openpgp, &programs.ssh, &programs.x509] {
        assert!(!scripts.contains(program), "{programs:?}");
    }
    for (id, (kind, _)) in ids.iter().zip(kinds) {
        let info = repo.verify_signature(*id).unwrap().expect("signed");
        assert_ne!(info.status, gitti_core::signature::SigStatus::Good, "{kind}");
    }
    for (marker, (kind, _)) in markers.iter().zip(kinds) {
        assert!(!marker.exists(), "the repository's {kind} program ran");
    }
    // The fixture is real: git itself runs each.
    let hexes: Vec<String> = ids.iter().map(ToString::to_string).collect();
    let mut args = vec!["log", "--no-walk", "--format=%G?"];
    args.extend(hexes.iter().map(String::as_str));
    sh(&dir, &args);
    for (marker, (kind, _)) in markers.iter().zip(kinds) {
        assert!(marker.exists(), "plain git did not run the repository's {kind} program");
    }
}

/// git's "dubious ownership" refusal (simulated with git's own test switch) is
/// recognised; trusting the folder adds it to the global `safe.directory` and git works.
#[gitti_test::test]
fn dubious_ownership_is_recognised_and_trusted() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("r");
    init(&dir);
    let global = tmp.path().join("global.gitconfig");
    let mut repo = Repo::open(&dir).unwrap();
    repo.set_env(vec![
        ("GIT_TEST_ASSUME_DIFFERENT_OWNER".into(), "1".into()),
        ("GIT_CONFIG_GLOBAL".into(), global.display().to_string()),
    ]);
    let err = repo.working_state().unwrap_err();
    let d: DubiousOwnership = safe_directory::dubious_ownership(&err).expect("classified");
    assert!(
        gitti_core::paths::canonical(Path::new(&d.dir)) == gitti_core::paths::canonical(&dir),
        "{} vs {}",
        d.dir,
        dir.display()
    );
    assert!(d.describe().contains("safe.directory"));
    repo.trust_directory(&d).unwrap();
    assert!(fs::read_to_string(&global).unwrap().contains("directory"));
    repo.working_state().unwrap();
}
