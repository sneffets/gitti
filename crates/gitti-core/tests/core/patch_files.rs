//! Patches and snapshots (`patch_files`): commits saved / copied as format-patch output
//! and applied in another clone with `git am`, a commit's files as an archive, a plain
//! diff applied with and without the index, and a `git am` that stops on a conflict
//! (Continue, Abort).

use gitti_core::Repo;
use gitti_core::ops::RepoState;
use gitti_core::patch_files::{ArchiveFormat, PatchCommit, PatchCommits};

use crate::fixture_kit::{C, History, Ids, f, repo_with, sh, sh_ok};

fn commits(ids: &Ids, list: &[(C, &str)]) -> PatchCommits {
    PatchCommits {
        commits: list
            .iter()
            .map(|(c, subject)| PatchCommit {
                id: ids[*c].to_string(),
                subject: subject.to_string(),
                is_merge: false,
            })
            .collect(),
        marked: list.len() > 1,
    }
}

/// `main`: c1 (a.txt, dir/b.txt), c2 (a.txt changed), c3 (n.txt new).
fn three_commits() -> (History, [C; 3]) {
    let mut h = History::new();
    let c1 = h.commit("main", &[], "first", &[("a.txt", f("1\n2\n3\n")), ("dir/b.txt", f("b\n"))]);
    let c2 = h.commit("main", &[c1], "change a", &[("a.txt", f("1\ntwo\n3\n"))]);
    let c3 = h.commit("main", &[c2], "add n", &[("n.txt", f("new\n"))]);
    (h, [c1, c2, c3])
}

/// Two commits saved one file per commit (folder, oldest first) and as one mbox; the mbox
/// applied with `git am` in a clone that has only the first commit gives the same two
/// commits (subjects, tree); Copy as Patch holds both.
#[gitti_test::test]
fn format_patch_of_two_commits_then_am_in_another_clone() {
    let (h, [c1, c2, c3]) = three_commits();
    let (tmp, dir, ids) = repo_with(&h);
    let repo = Repo::open(&dir).unwrap();
    let two = commits(&ids, &[(c2, "change a"), (c3, "add n")]);
    // One file per commit, into a folder git creates.
    let folder = tmp.path().join("out");
    let line = repo.save_patches(&two, &folder, false).unwrap();
    assert!(line.starts_with("Saved 2 patch files into") && line.contains("0001-change-a.patch"), "{line}");
    let mut names: Vec<String> =
        std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["0001-change-a.patch", "0002-add-n.patch"]);
    // All in one file.
    let mbox = tmp.path().join("both.patch");
    let line = repo.save_patches(&two, &mbox, true).unwrap();
    assert!(line.starts_with("Saved 2 commits as patches into"), "{line}");
    let (text, line) = repo.patch_for_clipboard(&two).unwrap();
    assert!(line.starts_with("Copied 2 commits as patches"), "{line}");
    assert_eq!(text.as_bytes(), std::fs::read(&mbox).unwrap());
    // The other clone: only the first commit.
    let mut first = History::new();
    first.commit("main", &[], "first", &[("a.txt", f("1\n2\n3\n")), ("dir/b.txt", f("b\n"))]);
    let (_t2, other_dir, other_ids) = repo_with(&first);
    assert_eq!(other_ids[C(1)], ids[c1], "the same first commit");
    let other = Repo::open(&other_dir).unwrap();
    let preview = other.preview_patch_file(&mbox).unwrap();
    assert_eq!(preview.mbox.as_deref(), Some(&["change a".to_string(), "add n".to_string()][..]));
    assert_eq!(preview.files.len(), 2);
    assert_eq!(preview.applies, Ok(()));
    assert_eq!(preview.blocked(false, None), None);
    assert_eq!(preview.ok_label(false), "Apply 2 Commits");
    let line = other.am_patch_file(&mbox).unwrap();
    assert!(line.starts_with("Applied 2 patches as 2 new commits on main"), "{line}");
    assert_eq!(sh(&other_dir, &["log", "--format=%s", "-3"]), "add n\nchange a\nfirst\n");
    assert_eq!(sh(&other_dir, &["rev-parse", "HEAD^{tree}"]), sh(&dir, &["rev-parse", "HEAD^{tree}"]));
    assert_eq!(other.state(), RepoState::Clean);
}

/// The names in a zip archive (its local file headers).
fn zip_names(bytes: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let mut i = 0;
    while i + 30 <= bytes.len() {
        if bytes[i..i + 4] != *b"PK\x03\x04" {
            break;
        }
        let le16 = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize;
        let le32 = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let (size, name_len, extra) = (le32(i + 18), le16(i + 26), le16(i + 28));
        names.push(String::from_utf8_lossy(&bytes[i + 30..i + 30 + name_len]).into_owned());
        i += 30 + name_len + extra + size;
    }
    names
}

/// Export Snapshot of the first commit: a zip with a top folder named like the archive
/// (its files, not the later commits'), a .tar.gz without one; another extension refused.
#[gitti_test::test]
fn snapshot_as_zip_and_tar_gz() {
    let (h, [c1, ..]) = three_commits();
    let (tmp, dir, ids) = repo_with(&h);
    let repo = Repo::open(&dir).unwrap();
    let rev = ids[c1].to_string();
    let zip = tmp.path().join("snap-1.zip");
    let line = repo.export_snapshot(&rev, &zip, true).unwrap();
    assert!(line.starts_with(&format!("Exported the files of {}", &rev[..7])), "{line}");
    let mut names = zip_names(&std::fs::read(&zip).unwrap());
    names.retain(|n| !n.ends_with('/'));
    names.sort();
    assert_eq!(names, ["snap-1/a.txt", "snap-1/dir/b.txt"]);
    let tgz = tmp.path().join("snap.tar.gz");
    repo.export_snapshot(&rev, &tgz, false).unwrap();
    let bytes = std::fs::read(&tgz).unwrap();
    assert_eq!(bytes[..2], [0x1f, 0x8b], "gzip");
    let listed = std::process::Command::new("tar").arg("-tzf").arg(&tgz).output().unwrap();
    let mut names: Vec<String> = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .filter(|l| !l.ends_with('/'))
        .map(str::to_string)
        .collect();
    names.sort();
    assert_eq!(names, ["a.txt", "dir/b.txt"]);
    let err = repo.export_snapshot(&rev, &tmp.path().join("snap.rar"), false).unwrap_err().to_string();
    assert!(err.contains("ends in .rar"), "{err}");
    assert_eq!(ArchiveFormat::of(&tgz).unwrap().0, ArchiveFormat::TarGz);
}

/// A plain diff: the preview lists its files and says it applies; applied without the
/// index it changes only the working tree, with it the index too. One that doesn't
/// match is refused in plain words — which file, which line — and nothing changes.
#[gitti_test::test]
fn plain_diff_with_and_without_the_index() {
    let (h, _) = three_commits();
    let (tmp, dir, _) = repo_with(&h);
    let repo = Repo::open(&dir).unwrap();
    let diff = tmp.path().join("change.diff");
    std::fs::write(
        &diff,
        "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,3 +1,3 @@\n 1\n-two\n+TWO\n 3\n\
         diff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+x\n",
    )
    .unwrap();
    let preview = repo.preview_patch_file(&diff).unwrap();
    assert_eq!(preview.mbox, None);
    assert_eq!(preview.file_lines(10), ["a.txt (+1 −1)", "new.txt (+1 −0)"]);
    assert_eq!((preview.applies.clone(), preview.applies_staged.clone()), (Ok(()), Ok(())));
    assert!(preview.describe("main").contains("It applies cleanly."));
    let line = repo.apply_patch_file(&diff, false).unwrap();
    assert!(line.contains("to the working tree (2 files) — not staged"), "{line}");
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "1\nTWO\n3\n");
    assert!(sh_ok(&dir, &["diff", "--cached", "--quiet"]), "nothing staged");
    // Back, then with the index.
    sh(&dir, &["reset", "-q", "--hard"]);
    std::fs::remove_file(dir.join("new.txt")).unwrap();
    let line = repo.apply_patch_file(&diff, true).unwrap();
    assert!(line.contains("the working tree and the index") && line.contains("staged"), "{line}");
    assert_eq!(sh(&dir, &["diff", "--cached", "--name-only"]), "a.txt\nnew.txt\n");
    // Applied already: it doesn't match any more.
    let preview = repo.preview_patch_file(&diff).unwrap();
    let why = preview.applies.clone().unwrap_err();
    assert!(why.iter().any(|w| w.starts_with("a.txt: the change at line 1")), "{why:?}");
    assert!(preview.blocked(false, None).is_some_and(|b| b.starts_with("It doesn't apply")));
    let err = repo.apply_patch_file(&diff, false).unwrap_err().to_string();
    assert!(err.contains("a.txt: the change at line 1 doesn't match the file") && err.ends_with("Nothing was changed."), "{err}");
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "1\nTWO\n3\n");
}

/// `main` changed line 2 one way, a patch of `side` the other: `git am --3way` stops on the
/// conflict — an operation in progress (Applying), said with the patch's number. Abort
/// puts `main` back; again, resolved and Continue commits the patch.
#[gitti_test::test]
fn am_conflict_stops_then_abort_or_continue() {
    let mut h = History::new();
    let c1 = h.commit("main", &[], "first", &[("a.txt", f("1\n2\n3\n"))]);
    let side = h.commit("side", &[c1], "side change", &[("a.txt", f("1\nside\n3\n"))]);
    let mine = h.commit("main", &[c1], "main change", &[("a.txt", f("1\nmain\n3\n"))]);
    let (tmp, dir, ids) = repo_with(&h);
    let repo = Repo::open(&dir).unwrap();
    let mbox = tmp.path().join("side.patch");
    repo.save_patches(&commits(&ids, &[(side, "side change")]), &mbox, true).unwrap();
    let stop = |repo: &Repo| {
        let line = repo.am_patch_file(&mbox).unwrap();
        assert!(line.starts_with("Applying patch 1 of 1 (side change) stopped on 1 conflict"), "{line}");
        assert_eq!(repo.state(), RepoState::Applying);
        assert_eq!(repo.conflict_sides().theirs, "patch side change");
        assert!(gitti_core::skip::can_skip(repo.state()));
    };
    stop(&repo);
    // A second am is refused while this one waits.
    let err = repo.am_patch_file(&mbox).unwrap_err().to_string();
    assert!(err.contains("applying patches (git am) is in progress"), "{err}");
    repo.abort_operation().unwrap();
    assert_eq!(repo.state(), RepoState::Clean);
    assert_eq!(crate::fixture_kit::rev(&dir, "HEAD"), ids[mine]);
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "1\nmain\n3\n");
    // Again: resolved, then Continue.
    stop(&repo);
    std::fs::write(dir.join("a.txt"), "1\nboth\n3\n").unwrap();
    repo.mark_resolved(&["a.txt".to_string()]).unwrap();
    repo.continue_operation().unwrap();
    assert_eq!(repo.state(), RepoState::Clean);
    assert_eq!(sh(&dir, &["log", "--format=%s", "-2"]), "side change\nmain change\n");
    assert_eq!(sh(&dir, &["show", "HEAD:a.txt"]), "1\nboth\n3\n");
}
