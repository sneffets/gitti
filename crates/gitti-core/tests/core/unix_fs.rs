//! gitti-core on unix file-system semantics: exec bit, symlinks, type changes,
//! case-sensitive and odd file names, unreadable files and directories, directory ↔ file
//! swaps, empty files, missing trailing newlines, very long lines, binary files. Status,
//! diff and hunk + line stage / unstage / discard against temporary repositories.
//!
//! Cases that need unix semantics are behind `cfg(unix)` (or `target_os = "linux"` where
//! macOS differs: case-insensitive APFS, Unicode normalisation, UTF-8-only names); the
//! rest runs on every platform.
// Some helpers are used only by the unix-only tests.
#![cfg_attr(not(unix), allow(dead_code, unused_macros))]

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use gitti_core::diff::{FileChange, FileDiff, LineKind};
use gitti_core::patch::{PatchMode, Selection};
use gitti_core::status::StatusEntry;
use gitti_core::{DiffOptions, Repo, WorkFile, WorkingState};

// ------------------------------------------------------------------------- helpers

fn git_raw<S: AsRef<OsStr>>(dir: &Path, args: &[S]) -> std::process::Output {
    std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap()
}

fn git_bytes<S: AsRef<OsStr>>(dir: &Path, args: &[S]) -> Vec<u8> {
    let out = git_raw(dir, args);
    let shown: Vec<_> = args
        .iter()
        .map(|a| a.as_ref().to_string_lossy().into_owned())
        .collect();
    assert!(
        out.status.success(),
        "git {shown:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

fn sh(dir: &Path, args: &[&str]) -> String {
    String::from_utf8(git_bytes(dir, args)).unwrap()
}

struct Fx {
    _dir: tempfile::TempDir,
    repo: Repo,
}

fn fixture(files: &[(&str, &str)]) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    sh(p, &["init", "-q", "-b", "main"]);
    crate::test_config(p);
    for (name, content) in files {
        let full = p.join(name);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }
    if !files.is_empty() {
        sh(p, &["add", "-A"]);
        sh(p, &["commit", "-q", "-m", "init"]);
    }
    let repo = Repo::open(p).unwrap();
    Fx { _dir: dir, repo }
}

impl Fx {
    fn p(&self) -> &Path {
        self.repo.workdir()
    }
    fn path(&self, name: &str) -> PathBuf {
        gitti_core::paths::join_repo_path(self.p(), name)
    }
    fn write(&self, name: &str, content: &str) {
        self.write_bytes(name, content.as_bytes());
    }
    fn write_bytes(&self, name: &str, content: &[u8]) {
        let full = self.path(name);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }
    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.path(name)).unwrap()
    }
    fn commit_all(&self, msg: &str) {
        sh(self.p(), &["add", "-A"]);
        sh(self.p(), &["commit", "-q", "-m", msg]);
    }
    /// The staged content of `name` (raw bytes, lossy as text).
    fn index(&self, name: &str) -> String {
        String::from_utf8_lossy(&git_bytes(
            self.p(),
            &["cat-file", "blob", &format!(":{name}")],
        ))
        .into_owned()
    }
    /// `(mode, path)` of every index entry, from `ls-files -s -z` (any byte in names).
    fn index_entries(&self) -> Vec<(String, String)> {
        let out = git_bytes(self.p(), &["ls-files", "-s", "-z"]);
        out.split(|b| *b == 0)
            .filter(|r| !r.is_empty())
            .map(|r| {
                let r = String::from_utf8_lossy(r);
                let (meta, path) = r.split_once('\t').unwrap();
                (
                    meta.split(' ').next().unwrap().to_string(),
                    path.to_string(),
                )
            })
            .collect()
    }
    fn index_mode(&self, name: &str) -> Option<String> {
        self.index_entries()
            .into_iter()
            .find(|(_, p)| p == name)
            .map(|(m, _)| m)
    }
    fn state(&self) -> WorkingState {
        self.repo.working_state().unwrap()
    }
    fn unstaged_file(&self, name: &str) -> WorkFile {
        let s = self.state();
        s.unstaged
            .iter()
            .find(|f| f.path() == name)
            .unwrap_or_else(|| panic!("{name:?} not unstaged: {:#?}", s.status.entries))
            .clone()
    }
    fn staged_file(&self, name: &str) -> WorkFile {
        let s = self.state();
        s.staged
            .iter()
            .find(|f| f.path() == name)
            .unwrap_or_else(|| panic!("{name:?} not staged: {:#?}", s.status.entries))
            .clone()
    }
    fn unstaged(&self, name: &str) -> FileDiff {
        self.unstaged_file(name)
            .diff
            .unwrap_or_else(|| panic!("{name:?} has no unstaged diff"))
    }
    fn staged(&self, name: &str) -> FileDiff {
        self.staged_file(name)
            .diff
            .unwrap_or_else(|| panic!("{name:?} has no staged diff"))
    }
    fn entry(&self, name: &str) -> StatusEntry {
        let s = self.repo.status().unwrap();
        s.entries
            .iter()
            .find(|e| e.path == name)
            .unwrap_or_else(|| panic!("{name:?} not in status: {:#?}", s.entries))
            .clone()
    }
    fn apply(&self, d: &FileDiff, sel: &Selection, mode: PatchMode) {
        self.repo
            .apply_selection(d, sel, mode)
            .unwrap_or_else(|e| panic!("{mode:?} of {:?}: {e:#}", d.path()));
    }
    fn is_clean(&self) -> bool {
        self.repo.status().unwrap().entries.is_empty()
    }
    /// The partial re-read after touching `paths` equals a full read.
    fn check_after(&self, prev: &WorkingState, paths: &[&str], what: &str) {
        let opts = DiffOptions::default();
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        let after = self.repo.working_state_after(&opts, prev, &paths).unwrap();
        let full = self.repo.working_state_with(&opts).unwrap();
        assert_eq!(format!("{after:?}"), format!("{full:?}"), "{what}");
    }
}

/// A selection of the added (`+`) lines only, every hunk.
fn added_only(f: &FileDiff) -> Selection {
    let mut sel = Selection::default();
    for (h, hunk) in f.hunks.iter().enumerate() {
        let picked: Vec<usize> = hunk
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.kind == LineKind::Added)
            .map(|(i, _)| i)
            .collect();
        if !picked.is_empty() {
            sel.add_lines(h, picked);
        }
    }
    sel
}

/// A selection of the removed (`-`) lines only, every hunk.
fn removed_only(f: &FileDiff) -> Selection {
    let mut sel = Selection::default();
    for (h, hunk) in f.hunks.iter().enumerate() {
        let picked: Vec<usize> = hunk
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.kind == LineKind::Removed)
            .map(|(i, _)| i)
            .collect();
        if !picked.is_empty() {
            sel.add_lines(h, picked);
        }
    }
    sel
}

#[cfg(unix)]
fn has_header(d: &FileDiff, prefix: &str) -> bool {
    d.header.iter().any(|h| h.starts_with(prefix))
}

#[cfg(unix)]
fn mode_of(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
}

#[cfg(unix)]
fn chmod(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
}

/// Gives a path its permissions back when dropped (a `chmod 000` directory would keep the
/// temp dir from being deleted, and a failing assert must not leave it behind).
#[cfg(unix)]
struct Restore(PathBuf, u32);

#[cfg(unix)]
impl Drop for Restore {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(self.1));
    }
}

#[cfg(unix)]
fn symlink(target: &str, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(unix)]
fn link_target(p: &Path) -> String {
    fs::read_link(p).unwrap().to_string_lossy().into_owned()
}

// ------------------------------------------------------------------------- exec bit

/// `chmod +x` alone: a diff with `old mode` / `new mode` and no hunks. Staging it (the
/// whole file, `apply_selection` of a hunkless diff) sets 100755 in the index; unstaging
/// takes it back; discarding resets the worktree file's mode.
/// Workflow: EX-12
#[cfg(unix)]
#[gitti_test::test]
fn mode_only_change_stages_unstages_and_discards() {
    let fx = fixture(&[("run.sh", "#!/bin/sh\necho hi\n")]);
    chmod(&fx.path("run.sh"), 0o755);
    let e = fx.entry("run.sh");
    assert_eq!((e.index, e.worktree), (None, Some('M')));
    let d = fx.unstaged("run.sh");
    assert!(d.hunks.is_empty(), "{d:#?}");
    assert!(
        has_header(&d, "old mode 100644") && has_header(&d, "new mode 100755"),
        "{d:#?}"
    );
    assert_eq!(d.change, FileChange::Modified);

    let prev = fx.state();
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    fx.check_after(&prev, &["run.sh"], "stage mode");
    assert_eq!(fx.index_mode("run.sh").as_deref(), Some("100755"));
    assert!(fx.state().unstaged.is_empty());
    let s = fx.staged("run.sh");
    assert!(
        s.hunks.is_empty() && has_header(&s, "new mode 100755"),
        "{s:#?}"
    );

    fx.apply(&s, &Selection::whole_hunk(0), PatchMode::Unstage);
    assert_eq!(fx.index_mode("run.sh").as_deref(), Some("100644"));
    assert_eq!(mode_of(&fx.path("run.sh")), 0o755, "worktree keeps +x");

    let d = fx.unstaged("run.sh");
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Discard);
    assert_eq!(mode_of(&fx.path("run.sh")) & 0o111, 0, "discard drops +x");
    assert!(fx.is_clean());

    // The same through the whole-file operations.
    chmod(&fx.path("run.sh"), 0o755);
    fx.repo.stage_paths(&["run.sh"]).unwrap();
    assert_eq!(fx.index_mode("run.sh").as_deref(), Some("100755"));
    fx.repo.unstage_paths(&["run.sh"]).unwrap();
    assert_eq!(fx.index_mode("run.sh").as_deref(), Some("100644"));
    fx.repo.discard_paths(&[&fx.entry("run.sh")]).unwrap();
    assert!(fx.is_clean());
}

/// 100755 → 100644 (dropping +x) is the same in the other direction.
/// Workflow: EX-12
#[cfg(unix)]
#[gitti_test::test]
fn removing_the_exec_bit_stages() {
    let fx = fixture(&[("tool", "#!/bin/sh\n")]);
    chmod(&fx.path("tool"), 0o755);
    fx.commit_all("exec");
    chmod(&fx.path("tool"), 0o644);
    let d = fx.unstaged("tool");
    assert!(has_header(&d, "old mode 100755") && has_header(&d, "new mode 100644"));
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    assert_eq!(fx.index_mode("tool").as_deref(), Some("100644"));
    let s = fx.staged("tool");
    fx.apply(&s, &Selection::whole_hunk(0), PatchMode::Unstage);
    assert_eq!(fx.index_mode("tool").as_deref(), Some("100755"));
}

/// Mode change + content change in one file. Staging or discarding lines or hunks moves
/// only those lines — the mode change stays where it is and is staged on its own
/// afterwards (a hunkless diff). A discarded line must not cost the file its exec bit
/// (it did: the patch carried `old mode` / `new mode`).
#[cfg(unix)]
#[gitti_test::test]
fn mode_and_content_change_lines_leave_the_mode_alone() {
    let fx = fixture(&[("s.sh", &numbered(30))]);
    let changed = numbered(30)
        .replace("line 2\n", "line two\n")
        .replace("line 25\n", "line 25!\n");
    fx.write("s.sh", &changed);
    chmod(&fx.path("s.sh"), 0o755);
    let d = fx.unstaged("s.sh");
    assert_eq!(d.hunks.len(), 2);
    assert!(has_header(&d, "new mode 100755"));

    // One line of the first hunk.
    fx.apply(&d, &lines(&d, &["line two"]), PatchMode::Stage);
    assert_eq!(
        fx.index("s.sh"),
        numbered(30).replace("line 2\n", "line 2\nline two\n")
    );
    assert_eq!(
        fx.index_mode("s.sh").as_deref(),
        Some("100644"),
        "mode not staged with a line"
    );

    // The second hunk only.
    let d = fx.unstaged("s.sh");
    let h = d
        .hunks
        .iter()
        .position(|h| h.lines.iter().any(|l| l.text == "line 25!"))
        .unwrap();
    fx.apply(&d, &Selection::whole_hunk(h), PatchMode::Stage);
    assert_eq!(
        fx.index_mode("s.sh").as_deref(),
        Some("100644"),
        "mode not staged with a hunk"
    );

    // Discard the last unstaged line (the whole content change): exec bit kept.
    let d = fx.unstaged("s.sh");
    fx.apply(&d, &lines(&d, &["line 2"]), PatchMode::Discard);
    assert_eq!(
        fx.read("s.sh"),
        changed.replace("line two\n", "line 2\nline two\n")
    );
    // (git writes the file anew: 0777 minus the umask.)
    assert_ne!(
        mode_of(&fx.path("s.sh")) & 0o100,
        0,
        "discarding a line keeps +x"
    );

    // What is left of the unstaged side: the mode change alone, stageable on its own.
    let d = fx.unstaged("s.sh");
    assert!(
        d.hunks.is_empty() && has_header(&d, "new mode 100755"),
        "{d:#?}"
    );
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    assert_eq!(fx.index_mode("s.sh").as_deref(), Some("100755"));
    assert!(fx.state().unstaged.is_empty());

    // Unstaging one line, then every hunk, of the staged side keeps the staged mode.
    let s = fx.staged("s.sh");
    fx.apply(&s, &lines(&s, &["line 25!"]), PatchMode::Unstage);
    assert_eq!(fx.index_mode("s.sh").as_deref(), Some("100755"));
    assert!(!fx.index("s.sh").contains("line 25!"));
    let s = fx.staged("s.sh");
    let all: Selection = {
        let mut sel = Selection::default();
        for h in 0..s.hunks.len() {
            sel.hunks.insert(h, None);
        }
        sel
    };
    fx.apply(&s, &all, PatchMode::Unstage);
    assert_eq!(fx.index_mode("s.sh").as_deref(), Some("100755"));
    assert_eq!(fx.index("s.sh"), numbered(30));
    // The mode change alone unstages as well.
    let s = fx.staged("s.sh");
    assert!(s.hunks.is_empty(), "{s:#?}");
    fx.apply(&s, &Selection::whole_hunk(0), PatchMode::Unstage);
    assert_eq!(fx.index_mode("s.sh").as_deref(), Some("100644"));
    assert!(fx.state().staged.is_empty());
}

/// A file's only hunk staged / discarded as a hunk: content only, like `git add -p` (the
/// mode change is its own step); the whole-file operations take both.
#[cfg(unix)]
#[gitti_test::test]
fn mode_and_content_change_single_hunk() {
    let fx = fixture(&[("s.sh", "a\nb\nc\n")]);
    fx.write("s.sh", "a\nB\nc\n");
    chmod(&fx.path("s.sh"), 0o755);
    let d = fx.unstaged("s.sh");
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    assert_eq!(fx.index_mode("s.sh").as_deref(), Some("100644"));
    assert_eq!(fx.index("s.sh"), "a\nB\nc\n");
    let s = fx.staged("s.sh");
    fx.apply(&s, &Selection::whole_hunk(0), PatchMode::Unstage);
    let d = fx.unstaged("s.sh");
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Discard);
    assert_eq!(fx.read("s.sh"), "a\nb\nc\n");
    assert_ne!(
        mode_of(&fx.path("s.sh")) & 0o100,
        0,
        "discarding the hunk keeps +x"
    );

    fx.write("s.sh", "a\nB\nc\n");
    fx.repo.stage_paths(&["s.sh"]).unwrap();
    assert_eq!(fx.index_mode("s.sh").as_deref(), Some("100755"));
    assert_eq!(fx.index("s.sh"), "a\nB\nc\n");
    fx.repo.unstage_paths(&["s.sh"]).unwrap();
    fx.repo.discard_paths(&[&fx.entry("s.sh")]).unwrap();
    assert_eq!(mode_of(&fx.path("s.sh")) & 0o111, 0);
    assert!(fx.is_clean());
}

/// A new executable file: the untracked preview says 100755, a partial stage creates the
/// index entry as 100755, a partial unstage keeps it.
#[cfg(unix)]
#[gitti_test::test]
fn new_executable_file_partial_stage_keeps_mode() {
    let fx = fixture(&[("keep", "k\n")]);
    fx.write("new.sh", "#!/bin/sh\none\ntwo\n");
    chmod(&fx.path("new.sh"), 0o755);
    let d = fx.unstaged("new.sh");
    assert_eq!(d.change, FileChange::Added);
    assert!(has_header(&d, "new file mode 100755"), "{d:#?}");
    fx.apply(&d, &lines(&d, &["#!/bin/sh", "two"]), PatchMode::Stage);
    assert_eq!(fx.index("new.sh"), "#!/bin/sh\ntwo\n");
    assert_eq!(fx.index_mode("new.sh").as_deref(), Some("100755"));
    let s = fx.staged("new.sh");
    fx.apply(&s, &lines(&s, &["two"]), PatchMode::Unstage);
    assert_eq!(fx.index("new.sh"), "#!/bin/sh\n");
    assert_eq!(fx.index_mode("new.sh").as_deref(), Some("100755"));
    // Unstaging the rest removes it from the index; the file is untracked again.
    let s = fx.staged("new.sh");
    fx.apply(&s, &Selection::whole_hunk(0), PatchMode::Unstage);
    assert_eq!(fx.index_mode("new.sh"), None);
    assert!(fx.entry("new.sh").untracked);
    assert_eq!(mode_of(&fx.path("new.sh")), 0o755);
}

/// `core.fileMode=false` (e.g. repositories on vfat / NTFS mounts): `chmod` is no change
/// at all, and line staging keeps the recorded mode whatever the worktree says.
/// Workflow: EX-12
#[cfg(unix)]
#[gitti_test::test]
fn core_filemode_false_ignores_the_exec_bit() {
    let fx = fixture(&[("a.sh", "a\nb\nc\n"), ("x.sh", "x\ny\n")]);
    chmod(&fx.path("x.sh"), 0o755);
    fx.commit_all("x exec");
    sh(fx.p(), &["config", "core.fileMode", "false"]);
    chmod(&fx.path("a.sh"), 0o755);
    chmod(&fx.path("x.sh"), 0o644);
    assert!(fx.is_clean(), "{:?}", fx.repo.status().unwrap().entries);
    fx.write("a.sh", "a\nB\nc\nD\n");
    fx.write("x.sh", "x\nY\n");
    let d = fx.unstaged("a.sh");
    assert!(!has_header(&d, "new mode"), "{d:#?}");
    fx.apply(&d, &lines(&d, &["D"]), PatchMode::Stage);
    assert_eq!(fx.index("a.sh"), "a\nb\nc\nD\n");
    assert_eq!(fx.index_mode("a.sh").as_deref(), Some("100644"));
    let d = fx.unstaged("x.sh");
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    assert_eq!(fx.index_mode("x.sh").as_deref(), Some("100755"));
    fx.repo.stage_paths(&["a.sh"]).unwrap();
    assert_eq!(fx.index_mode("a.sh").as_deref(), Some("100644"));
    let d = fx.staged("x.sh");
    fx.apply(&d, &lines(&d, &["Y"]), PatchMode::Unstage);
    assert_eq!(fx.index("x.sh"), "x\n");
    assert_eq!(fx.index_mode("x.sh").as_deref(), Some("100755"));
}

// ------------------------------------------------------------------------- symlinks

/// A new symlink is an untracked entry, previewed as what `git add` records: the link
/// (mode 120000) with its target as its one line — also a dangling one or one to a
/// folder. Staging its hunk puts the link into the index; staging, unstaging and
/// discarding by path act on the link, never on its target.
/// Workflow: FS-05
#[cfg(unix)]
#[gitti_test::test]
fn untracked_symlink_preview_is_the_link() {
    let fx = fixture(&[("a.txt", "target\n"), ("dir/x", "x\n")]);
    symlink("a.txt", &fx.path("link"));
    symlink("nowhere", &fx.path("dangling"));
    symlink("dir", &fx.path("to_dir"));
    for (name, target) in [("link", "a.txt"), ("dangling", "nowhere"), ("to_dir", "dir")] {
        let d = fx.unstaged(name);
        assert!(has_header(&d, "new file mode 120000"), "{name}: {d:#?}");
        let added: Vec<&str> = d.hunks[0].lines.iter().filter(|l| l.is_change()).map(|l| l.text.as_str()).collect();
        assert_eq!(added, [target], "{name}");
    }
    let d = fx.unstaged("link");
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    assert_eq!(fx.index_mode("link").as_deref(), Some("120000"));
    assert_eq!(fx.index("link"), "a.txt");
    assert_eq!(fx.read("a.txt"), "target\n");
}

/// A new symlink is an untracked entry; staging, unstaging and discarding it act on the
/// link, never on its target.
/// Workflow: FS-05
#[cfg(unix)]
#[gitti_test::test]
fn new_symlink_stage_unstage_discard() {
    let fx = fixture(&[("a.txt", "target\n")]);
    symlink("a.txt", &fx.path("link"));
    let st = fx.state();
    let f = st.unstaged.iter().find(|f| f.path() == "link").unwrap();
    assert!(f.entry.untracked);
    fx.repo.stage_paths(&["link"]).unwrap();
    assert_eq!(fx.index_mode("link").as_deref(), Some("120000"));
    assert_eq!(fx.index("link"), "a.txt");
    let s = fx.staged("link");
    assert_eq!(s.change, FileChange::Added);
    assert!(has_header(&s, "new file mode 120000"), "{s:#?}");
    fx.repo.unstage_paths(&["link"]).unwrap();
    assert!(fx.entry("link").untracked);
    fx.repo.discard_paths(&[&fx.entry("link")]).unwrap();
    assert!(
        fs::symlink_metadata(fx.path("link")).is_err(),
        "link deleted"
    );
    assert_eq!(fx.read("a.txt"), "target\n", "target untouched");
    assert!(fx.is_clean());
}

/// The staged symlink's hunk unstages through a patch (`git apply --cached -R`).
#[cfg(unix)]
#[gitti_test::test]
fn staged_new_symlink_unstages_as_a_hunk() {
    let fx = fixture(&[("a.txt", "target\n")]);
    symlink("a.txt", &fx.path("link"));
    fx.repo.stage_paths(&["link"]).unwrap();
    let s = fx.staged("link");
    assert_eq!(s.hunks.len(), 1);
    fx.apply(&s, &Selection::whole_hunk(0), PatchMode::Unstage);
    assert_eq!(fx.index_mode("link"), None);
    // Selecting the single line is the same as the hunk.
    fx.repo.stage_paths(&["link"]).unwrap();
    let s = fx.staged("link");
    fx.apply(&s, &lines(&s, &["a.txt"]), PatchMode::Unstage);
    assert_eq!(fx.index_mode("link"), None);
}

/// A symlink pointing elsewhere: a one-line `-old` / `+new` diff (both without a trailing
/// newline). Hunk stage / unstage / discard repoint the link; any line selection acts as
/// the whole hunk (a link target is one value — staging "just the added line" would
/// make a target with a newline in it).
/// Workflow: FS-05
#[cfg(unix)]
#[gitti_test::test]
fn changed_symlink_target_hunk_and_lines() {
    let fx = fixture(&[("a.txt", "a\n"), ("b.txt", "b\n")]);
    symlink("a.txt", &fx.path("link"));
    fx.commit_all("link");
    fs::remove_file(fx.path("link")).unwrap();
    symlink("b.txt", &fx.path("link"));
    assert_eq!(fx.entry("link").worktree, Some('M'));
    let d = fx.unstaged("link");
    let texts: Vec<(LineKind, &str)> = d.hunks[0]
        .lines
        .iter()
        .map(|l| (l.kind, l.text.as_str()))
        .collect();
    assert_eq!(texts[0], (LineKind::Removed, "a.txt"));
    assert!(texts.contains(&(LineKind::Added, "b.txt")), "{texts:?}");

    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    assert_eq!(fx.index("link"), "b.txt");
    assert_eq!(fx.index_mode("link").as_deref(), Some("120000"));
    let s = fx.staged("link");
    fx.apply(&s, &Selection::whole_hunk(0), PatchMode::Unstage);
    assert_eq!(fx.index("link"), "a.txt");

    // Only the added line selected: still the whole target.
    let d = fx.unstaged("link");
    fx.apply(&d, &added_only(&d), PatchMode::Stage);
    assert_eq!(fx.index("link"), "b.txt");
    let s = fx.staged("link");
    fx.apply(&s, &removed_only(&s), PatchMode::Unstage);
    assert_eq!(fx.index("link"), "a.txt");

    let d = fx.unstaged("link");
    fx.apply(&d, &added_only(&d), PatchMode::Discard);
    assert_eq!(link_target(&fx.path("link")), "a.txt");
    assert!(fx.is_clean());
}

/// The working-tree version of a symlink (for context expansion / highlighting) is what
/// git stores: the link target, not the content of the file it points to.
#[cfg(unix)]
#[gitti_test::test]
fn worktree_version_of_a_symlink_is_its_target() {
    use gitti_core::blobs::Version;
    let fx = fixture(&[("a.txt", "content of a\n")]);
    symlink("a.txt", &fx.path("link"));
    fx.commit_all("link");
    symlink("missing", &fx.path("dangling"));
    fs::create_dir(fx.path("dir")).unwrap();
    fx.write("dir/x", "x\n");
    symlink("dir", &fx.path("dirlink"));
    let v = fx
        .repo
        .read_versions(&[
            (Version::Worktree, "link".into()),
            (Version::Index, "link".into()),
            (Version::Worktree, "dangling".into()),
            (Version::Worktree, "dirlink".into()),
        ])
        .unwrap();
    assert_eq!(v[0].as_deref(), Some("a.txt"));
    assert_eq!(v[0], v[1]);
    assert_eq!(v[2].as_deref(), Some("missing"));
    assert_eq!(v[3].as_deref(), Some("dir"));
}

/// File versions of names with newlines and spaces come out of the one `cat-file --batch`
/// (NUL-delimited with git ≥ 2.43, `GitCaps`) without shifting the files after them; an
/// older git skips the newline names instead of misreading the rest.
#[cfg(unix)]
#[gitti_test::test]
fn read_versions_of_names_with_newlines() {
    use gitti_core::blobs::Version;
    let fx = fixture(&[("new\nline.txt", "nl\n"), ("plain.txt", "plain\n"), ("sp ace", "s\n")]);
    fx.write("new\nline.txt", "nl 2\n");
    sh(fx.p(), &["add", "--", "new\nline.txt"]);
    let head = || Version::Rev("HEAD".into());
    let v = fx
        .repo
        .read_versions(&[
            (head(), "new\nline.txt".into()),
            (Version::Index, "new\nline.txt".into()),
            (head(), "gone blob 5".into()),
            (head(), "plain.txt".into()),
            (head(), "sp ace".into()),
        ])
        .unwrap();
    let nul = gitti_core::caps::GitCaps::get().cat_file_nul();
    if nul {
        assert_eq!(v[0].as_deref(), Some("nl\n"));
        assert_eq!(v[1].as_deref(), Some("nl 2\n"));
    } else {
        assert_eq!((v[0].as_deref(), v[1].as_deref()), (None, None));
    }
    assert_eq!(v[2], None, "a missing path that looks like a header");
    assert_eq!(v[3].as_deref(), Some("plain\n"));
    assert_eq!(v[4].as_deref(), Some("s\n"));
    let log = fx.repo.command_log().snapshot();
    let batch = log.iter().rfind(|e| e.args.iter().any(|a| a == "cat-file")).unwrap();
    assert_eq!(batch.args.iter().any(|a| a == "-Z"), nul, "{:?}", batch.args);
}

/// A symlink to a directory is one entry (git does not descend into it); discarding the
/// untracked link removes the link only.
/// Workflow: FS-05
#[cfg(unix)]
#[gitti_test::test]
fn symlink_to_directory() {
    let fx = fixture(&[("dir/inner.txt", "inner\n")]);
    symlink("dir", &fx.path("dirlink"));
    let st = fx.repo.status().unwrap();
    let paths: Vec<&str> = st.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, ["dirlink"], "not descended into");
    fx.state();
    fx.repo.stage_paths(&["dirlink"]).unwrap();
    assert_eq!(fx.index_mode("dirlink").as_deref(), Some("120000"));
    assert_eq!(fx.index("dirlink"), "dir");
    fx.repo.unstage_paths(&["dirlink"]).unwrap();
    fx.repo.discard_paths(&[&fx.entry("dirlink")]).unwrap();
    assert!(fs::symlink_metadata(fx.path("dirlink")).is_err());
    assert_eq!(fx.read("dir/inner.txt"), "inner\n");
    assert!(fx.is_clean());

    // Committed, then repointed and deleted: still single entries.
    symlink("dir", &fx.path("dirlink"));
    fx.commit_all("dirlink");
    fs::remove_file(fx.path("dirlink")).unwrap();
    let d = fx.unstaged("dirlink");
    assert_eq!(d.change, FileChange::Deleted);
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Discard);
    assert_eq!(link_target(&fx.path("dirlink")), "dir");
    assert!(fx.is_clean());
}

/// A dangling symlink is listed, staged and discarded like any other.
/// Workflow: FS-05
#[cfg(unix)]
#[gitti_test::test]
fn dangling_symlink() {
    let fx = fixture(&[("a.txt", "a\n")]);
    symlink("nowhere/at/all", &fx.path("dangling"));
    let f = fx.unstaged_file("dangling");
    assert!(f.entry.untracked);
    fx.repo.stage_paths(&["dangling"]).unwrap();
    assert_eq!(fx.index("dangling"), "nowhere/at/all");
    fx.commit_all("dangling");
    // Repoint (still dangling) and stage the hunk.
    fs::remove_file(fx.path("dangling")).unwrap();
    symlink("elsewhere", &fx.path("dangling"));
    let d = fx.unstaged("dangling");
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    assert_eq!(fx.index("dangling"), "elsewhere");
    // Deleted: partial stage of the deletion is the whole deletion.
    fs::remove_file(fx.path("dangling")).unwrap();
    let d = fx.unstaged("dangling");
    assert_eq!(d.change, FileChange::Deleted);
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    assert_eq!(fx.index_mode("dangling"), None);
    let s = fx.staged("dangling");
    fx.apply(&s, &Selection::whole_hunk(0), PatchMode::Unstage);
    assert_eq!(fx.index("dangling"), "nowhere/at/all");
    fx.repo.discard_paths(&[&fx.entry("dangling")]).unwrap();
    assert_eq!(link_target(&fx.path("dangling")), "nowhere/at/all");
}

/// A deleted symlink: hunk stage / unstage / discard.
/// Workflow: FS-05
#[cfg(unix)]
#[gitti_test::test]
fn deleted_symlink_hunks() {
    let fx = fixture(&[("a.txt", "a\n")]);
    symlink("a.txt", &fx.path("link"));
    fx.commit_all("link");
    fs::remove_file(fx.path("link")).unwrap();
    assert_eq!(fx.entry("link").worktree, Some('D'));
    let d = fx.unstaged("link");
    assert_eq!(d.change, FileChange::Deleted);
    assert!(has_header(&d, "deleted file mode 120000"));
    fx.apply(&d, &lines(&d, &["a.txt"]), PatchMode::Stage);
    assert_eq!(fx.index_mode("link"), None);
    let s = fx.staged("link");
    fx.apply(&s, &Selection::whole_hunk(0), PatchMode::Unstage);
    assert_eq!(fx.index_mode("link").as_deref(), Some("120000"));
    let d = fx.unstaged("link");
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Discard);
    assert_eq!(link_target(&fx.path("link")), "a.txt");
    assert_eq!(fx.read("a.txt"), "a\n");
    assert!(fx.is_clean());
}

/// A regular file replaced by a symlink (status `T`): listed once per side, and the
/// hunk / line operations of the UI act on the whole type change.
/// Workflow: EX-13
#[cfg(unix)]
#[gitti_test::test]
fn file_to_symlink_type_change() {
    let fx = fixture(&[("f", "one\ntwo\n"), ("g", "gee\n")]);
    fs::remove_file(fx.path("f")).unwrap();
    symlink("g", &fx.path("f"));
    let e = fx.entry("f");
    assert_eq!(e.worktree, Some('T'));
    let st = fx.state();
    assert_eq!(st.unstaged.iter().filter(|f| f.path() == "f").count(), 1);
    let d = fx.unstaged("f");
    assert_eq!(d.change, FileChange::TypeChanged, "{d:#?}");

    let prev = fx.state();
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    fx.check_after(&prev, &["f"], "stage type change");
    assert_eq!(fx.index_mode("f").as_deref(), Some("120000"));
    assert_eq!(fx.index("f"), "g");
    assert!(fx.state().unstaged.is_empty());
    let s = fx.staged("f");
    assert_eq!(s.change, FileChange::TypeChanged, "{s:#?}");
    assert_eq!(fx.staged_file("f").entry.index, Some('T'));
    fx.apply(&s, &lines(&s, &["g"]), PatchMode::Unstage);
    assert_eq!(fx.index_mode("f").as_deref(), Some("100644"));
    assert_eq!(fx.index("f"), "one\ntwo\n");

    let d = fx.unstaged("f");
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Discard);
    assert!(!fs::symlink_metadata(fx.path("f")).unwrap().is_symlink());
    assert_eq!(fx.read("f"), "one\ntwo\n");
    assert!(fx.is_clean());

    // The whole-file operations.
    fs::remove_file(fx.path("f")).unwrap();
    symlink("g", &fx.path("f"));
    fx.repo.stage_paths(&["f"]).unwrap();
    assert_eq!(fx.index_mode("f").as_deref(), Some("120000"));
    fx.repo.unstage_paths(&["f"]).unwrap();
    fx.repo.discard_paths(&[&fx.entry("f")]).unwrap();
    assert_eq!(fx.read("f"), "one\ntwo\n");
}

/// A symlink replaced by a regular file: the other direction.
/// Workflow: EX-13
#[cfg(unix)]
#[gitti_test::test]
fn symlink_to_file_type_change() {
    let fx = fixture(&[("g", "gee\n")]);
    symlink("g", &fx.path("f"));
    fx.commit_all("link");
    fs::remove_file(fx.path("f")).unwrap();
    fx.write("f", "now\na file\n");
    assert_eq!(fx.entry("f").worktree, Some('T'));
    let d = fx.unstaged("f");
    assert_eq!(d.change, FileChange::TypeChanged);
    fx.apply(&d, &lines(&d, &["now"]), PatchMode::Stage);
    assert_eq!(fx.index_mode("f").as_deref(), Some("100644"));
    assert_eq!(fx.index("f"), "now\na file\n");
    let s = fx.staged("f");
    fx.apply(&s, &Selection::whole_hunk(0), PatchMode::Unstage);
    assert_eq!(fx.index_mode("f").as_deref(), Some("120000"));
    let d = fx.unstaged("f");
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Discard);
    assert_eq!(link_target(&fx.path("f")), "g");
    assert!(fx.is_clean());
}

/// Type change of a staged file, then a further content change: both sides listed, each
/// operable.
#[cfg(unix)]
#[gitti_test::test]
fn type_change_staged_and_then_edited() {
    let fx = fixture(&[("g", "gee\n")]);
    symlink("g", &fx.path("f"));
    fx.commit_all("link");
    fs::remove_file(fx.path("f")).unwrap();
    fx.write("f", "file\n");
    fx.repo.stage_paths(&["f"]).unwrap();
    fx.write("f", "file\nmore\n");
    let st = fx.state();
    assert!(st.staged.iter().any(|f| f.path() == "f"));
    let u = fx.unstaged("f");
    assert_eq!(u.change, FileChange::Modified);
    fx.apply(&u, &lines(&u, &["more"]), PatchMode::Stage);
    assert_eq!(fx.index("f"), "file\nmore\n");
    let s = fx.staged("f");
    fx.apply(&s, &Selection::whole_hunk(0), PatchMode::Unstage);
    assert_eq!(fx.index_mode("f").as_deref(), Some("120000"));
}

// ------------------------------------------------------------------------- names

/// Names only unix allows: tab, newline, double quote, backslash, glob characters,
/// leading / trailing spaces, control characters (exercised with the portable ones in
/// `odd_names_*`, all names in one repository).
#[cfg(unix)]
const UNIX_NAMES: &[&str] = &[
    "tab\there.txt",
    "new\nline.txt",
    "q\"uote.txt",
    "\"",
    "back\\slash.txt",
    "\\",
    "*star?.txt",
    " lead",
    "trail ",
    "ctl\x01x",
    "esc\x1b[31m",
    "cr\rname",
    "bell\x07bs\x08vt\x0bff\x0c",
    "del\x7f",
    ":colon",
    ":(glob)magic",
    ":!not",
    "dir\twith tab/n\"x\\y",
];

/// `Foo` and `foo` are two files on a case-sensitive file system: each is staged on its
/// own, line by line.
/// Workflow: FS-03
#[cfg(target_os = "linux")]
#[gitti_test::test]
fn case_sensitive_names_are_separate_files() {
    let fx = fixture(&[
        ("Foo", "a\nb\nc\n"),
        ("foo", "a\nb\nc\n"),
        ("FOO.txt", "x\n"),
    ]);
    fx.write("Foo", "a\nB\nc\n");
    fx.write("foo", "a\nb\nc\nd\n");
    let st = fx.state();
    let paths: Vec<&str> = st.unstaged.iter().map(|f| f.path()).collect();
    assert_eq!(paths, ["Foo", "foo"]);
    let d = fx.unstaged("foo");
    fx.apply(&d, &lines(&d, &["d"]), PatchMode::Stage);
    assert_eq!(fx.index("foo"), "a\nb\nc\nd\n");
    assert_eq!(fx.index("Foo"), "a\nb\nc\n");
    let d = fx.unstaged("Foo");
    fx.apply(&d, &lines(&d, &["b"]), PatchMode::Stage);
    assert_eq!(fx.index("Foo"), "a\nc\n");
    fx.apply(
        &fx.unstaged("Foo"),
        &Selection::whole_hunk(0),
        PatchMode::Discard,
    );
    assert_eq!(fx.read("Foo"), "a\nc\n");
    assert_eq!(fx.read("foo"), "a\nb\nc\nd\n");
    let s = fx.staged("foo");
    fx.apply(&s, &Selection::whole_hunk(0), PatchMode::Unstage);
    assert_eq!(fx.index("foo"), "a\nb\nc\n");
    assert_eq!(fx.index("Foo"), "a\nc\n");
}

/// A rename that only changes the case: delete + untracked in the worktree, a rename
/// once both are staged; unstaging takes both back.
/// Workflow: RN-01, FS-03
#[cfg(target_os = "linux")]
#[gitti_test::test]
fn rename_only_in_case() {
    let fx = fixture(&[("Readme.md", "# hi\n"), ("other", "o\n")]);
    fs::rename(fx.path("Readme.md"), fx.path("README.md")).unwrap();
    let st = fx.repo.status().unwrap();
    let e: Vec<(&str, Option<char>, bool)> = st
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e.worktree, e.untracked))
        .collect();
    assert_eq!(
        e,
        [("Readme.md", Some('D'), false), ("README.md", None, true)]
    );
    let prev = fx.state();
    fx.repo.stage_paths(&["Readme.md", "README.md"]).unwrap();
    fx.check_after(&prev, &["Readme.md", "README.md"], "stage case rename");
    let st = fx.state();
    let r = st
        .status
        .entries
        .iter()
        .find(|e| e.path == "README.md")
        .unwrap();
    assert_eq!(
        (r.index, r.orig_path.as_deref()),
        (Some('R'), Some("Readme.md"))
    );
    let staged: Vec<(&str, Option<char>)> = st
        .staged
        .iter()
        .map(|f| (f.path(), f.entry.index))
        .collect();
    assert!(staged.contains(&("README.md", Some('R'))), "{staged:?}");
    assert!(staged.contains(&("Readme.md", Some('D'))), "{staged:?}");
    // Each half of the rename is a diff of its own (`--no-renames`).
    let add = fx.staged("README.md");
    assert_eq!(add.change, FileChange::Added);
    fx.apply(&add, &Selection::whole_hunk(0), PatchMode::Unstage);
    assert_eq!(fx.index_mode("README.md"), None);
    assert_eq!(fx.entry("Readme.md").index, Some('D'));
    fx.repo.unstage_paths(&["Readme.md"]).unwrap();
    assert_eq!(fx.index("Readme.md"), "# hi\n");
    // `git mv` to another case.
    fs::rename(fx.path("README.md"), fx.path("Readme.md")).unwrap();
    assert!(fx.is_clean());
    sh(fx.p(), &["mv", "Readme.md", "readme.md"]);
    assert_eq!(
        fx.entry("readme.md").orig_path.as_deref(),
        Some("Readme.md")
    );
    fx.repo.unstage_paths(&["readme.md", "Readme.md"]).unwrap();
    assert_eq!(fx.index("Readme.md"), "# hi\n");
}

/// NFC and NFD spellings of "café" are two different files on Linux.
/// Workflow: FS-04
#[cfg(target_os = "linux")]
#[gitti_test::test]
fn unicode_nfc_and_nfd_names_are_separate_files() {
    let nfc = "caf\u{e9}.txt";
    let nfd = "cafe\u{301}.txt";
    let fx = fixture(&[(nfc, "nfc\n"), (nfd, "nfd\n")]);
    assert_eq!(fx.index_entries().len(), 2);
    fx.write(nfc, "nfc\nchanged\n");
    fx.write(nfd, "nfd\nchanged\n");
    let st = fx.state();
    assert_eq!(st.unstaged.len(), 2);
    let d = fx.unstaged(nfd);
    assert_eq!(d.path(), nfd);
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    assert_eq!(fx.index(nfd), "nfd\nchanged\n");
    assert_eq!(fx.index(nfc), "nfc\n");
    let d = fx.unstaged(nfc);
    fx.apply(&d, &lines(&d, &["changed"]), PatchMode::Discard);
    assert_eq!(fx.read(nfc), "nfc\n");
    assert_eq!(fx.read(nfd), "nfd\nchanged\n");
    // The NFD name alone goes through `odd_names_*` (Linux).
}

/// A file name that is not UTF-8 (Latin-1 "é"): listed, and refresh does not fail.
/// Workflow: NU-04
#[cfg(target_os = "linux")]
#[gitti_test::test]
fn non_utf8_name_is_listed() {
    use std::os::unix::ffi::OsStrExt;
    let fx = fixture(&[("keep.txt", "k\n")]);
    let raw = OsStr::from_bytes(b"caf\xe9.txt");
    fs::write(fx.p().join(raw), "one\ntwo\n").unwrap();
    let st = fx.state();
    assert_eq!(st.unstaged.len(), 1);
    assert!(st.unstaged[0].entry.untracked);
    fx.repo.snapshot(&DiffOptions::default()).unwrap();
}

/// A non-UTF-8 file name: stage, unstage, discard (git gets the name back as bytes).
#[cfg(target_os = "linux")]
#[gitti_test::test]
#[ignore = "paths are Strings (lossy UTF-8): a non-UTF-8 name becomes U+FFFD and \
            git does not find it"]
/// Workflow: NU-04
fn non_utf8_name_stages() {
    use std::os::unix::ffi::OsStrExt;
    let fx = fixture(&[("keep.txt", "k\n")]);
    let raw = OsStr::from_bytes(b"caf\xe9.txt");
    fs::write(fx.p().join(raw), "one\ntwo\n").unwrap();
    git_bytes(fx.p(), &[OsStr::new("add"), raw]);
    git_bytes(fx.p(), &["commit", "-q", "-m", "latin1"]);
    fs::write(fx.p().join(raw), "one\nTWO\n").unwrap();
    let st = fx.state();
    let f = &st.unstaged[0];
    let d = f.diff.clone().expect("a diff");
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    assert!(fx.state().unstaged.is_empty());
    let path = f.path().to_string();
    fx.repo.unstage_paths(&[&path]).unwrap();
    fx.repo.discard_paths(&[&f.entry]).unwrap();
    assert!(fx.is_clean());
}

/// File content that is not UTF-8 (Latin-1 text): hunk and line staging move the exact
/// bytes (more in `non_utf8::`).
/// Workflow: NU-01
#[gitti_test::test]
fn latin1_content_stages_lines() {
    let fx = fixture(&[("l1.txt", "a\n")]);
    fx.write_bytes("l1.txt", b"a\ncaf\xe9\nna\xefve\n");
    let d = fx.unstaged("l1.txt");
    fx.apply(&d, &Selection::lines(0, [1]), PatchMode::Stage);
    let idx = git_bytes(fx.p(), &["cat-file", "blob", ":l1.txt"]);
    assert_eq!(idx, b"a\ncaf\xe9\n");
}

// ------------------------------------------------------------------------- unreadable

/// `chmod 000` on a modified tracked file: `git diff` dies on it ("cannot hash"). The
/// refresh still works: the file is listed without a diff, every other file has its
/// diff, and whole-file operations on the others work.
/// Workflow: EX-23
#[cfg(unix)]
#[gitti_test::test]
fn unreadable_tracked_file_does_not_break_the_refresh() {
    let fx = fixture(&[("secret", "s\n"), ("a.txt", "a\nb\n"), ("z.txt", "z\n")]);
    fx.write("secret", "s\nmore\n");
    fx.write("a.txt", "a\nB\n");
    fx.write("z.txt", "z\nzz\n");
    let _restore = Restore(fx.path("secret"), 0o644);
    chmod(&fx.path("secret"), 0o000);
    if fs::read(fx.path("secret")).is_ok() {
        return; // running as root: nothing is unreadable
    }
    let st = fx
        .repo
        .working_state()
        .expect("refresh with an unreadable file");
    let secret = st
        .unstaged
        .iter()
        .find(|f| f.path() == "secret")
        .expect("listed");
    assert!(secret.diff.is_none());
    for name in ["a.txt", "z.txt"] {
        assert!(
            st.unstaged
                .iter()
                .find(|f| f.path() == name)
                .unwrap()
                .diff
                .is_some(),
            "{name}"
        );
    }
    fx.repo.snapshot(&DiffOptions::default()).unwrap();
    let ws = DiffOptions {
        ignore_whitespace: true,
        ..Default::default()
    };
    fx.repo.working_state_with(&ws).unwrap();
    // Partial re-read of a readable file and of the unreadable one.
    let prev = fx.state();
    let d = fx.unstaged("a.txt");
    fx.apply(&d, &lines(&d, &["B"]), PatchMode::Stage);
    fx.check_after(&prev, &["a.txt"], "stage next to an unreadable file");
    let after = fx
        .repo
        .working_state_after(&DiffOptions::default(), &prev, &["secret".to_string()])
        .unwrap();
    assert!(after.unstaged.iter().any(|f| f.path() == "secret"));
    // Staging the unreadable file fails cleanly.
    assert!(fx.repo.stage_paths(&["secret"]).is_err());
    // Readable again: everything as usual.
    chmod(&fx.path("secret"), 0o644);
    let d = fx.unstaged("secret");
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    assert_eq!(fx.index("secret"), "s\nmore\n");
}

/// An unreadable untracked file: listed, no preview, no error.
#[cfg(unix)]
#[gitti_test::test]
fn unreadable_untracked_file_is_listed_without_preview() {
    let fx = fixture(&[("a.txt", "a\n")]);
    fx.write("locked", "x\n");
    let _restore = Restore(fx.path("locked"), 0o644);
    chmod(&fx.path("locked"), 0o000);
    if fs::read(fx.path("locked")).is_ok() {
        return;
    }
    let f = fx.unstaged_file("locked");
    assert!(f.entry.untracked);
    assert!(f.diff.is_none());
    assert!(fx.repo.stage_paths(&["locked"]).is_err());
    // Deleting it needs only the directory's permissions.
    fx.repo.discard_paths(&[&f.entry]).unwrap();
    assert!(fx.is_clean());
}

/// An unreadable untracked directory: git warns and skips it; the refresh works.
#[cfg(unix)]
#[gitti_test::test]
fn unreadable_untracked_directory() {
    let fx = fixture(&[("a.txt", "a\n")]);
    fx.write("closed/inner.txt", "x\n");
    fx.write("open.txt", "o\n");
    let _restore = Restore(fx.path("closed"), 0o755);
    chmod(&fx.path("closed"), 0o000);
    if fs::read_dir(fx.path("closed")).is_ok() {
        return;
    }
    let st = fx.repo.working_state().expect("refresh");
    assert!(st.unstaged.iter().any(|f| f.path() == "open.txt"));
    fx.repo.snapshot(&DiffOptions::default()).unwrap();
}

/// A directory with tracked files that becomes unreadable (`chmod 000`: not even
/// `lstat` of its entries works): no panic, no error; the files show as deleted.
/// Workflow: EX-23
#[cfg(unix)]
#[gitti_test::test]
fn unreadable_directory_with_tracked_files() {
    let fx = fixture(&[("d/x.txt", "x\n"), ("d/y.txt", "y\n"), ("a.txt", "a\n")]);
    fx.write("a.txt", "a\nA\n");
    let _restore = Restore(fx.path("d"), 0o755);
    chmod(&fx.path("d"), 0o000);
    if fs::read_dir(fx.path("d")).is_ok() {
        return;
    }
    let st = fx.repo.working_state().expect("refresh");
    assert!(st.unstaged.iter().any(|f| f.path() == "a.txt"));
    for f in &st.unstaged {
        if f.path().starts_with("d/") {
            assert_eq!(f.entry.worktree, Some('D'), "{:?}", f.entry);
        }
    }
    let d = fx.unstaged("a.txt");
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    assert_eq!(fx.index("a.txt"), "a\nA\n");
}

// ------------------------------------------------------------------------- dir <-> file

/// Discarding the deleted files of a directory replaced by a file, in the order the UI
/// lists them (tracked first) — and the untracked file alone.
/// Workflow: EX-14
#[gitti_test::test]
fn directory_replaced_by_file_discard_in_steps() {
    let fx = fixture(&[("d/a.txt", "a\n"), ("keep", "k\n")]);
    fs::remove_dir_all(fx.path("d")).unwrap();
    fx.write("d", "file\n");
    fx.repo.discard_paths(&[&fx.entry("d")]).unwrap();
    fx.repo.discard_paths(&[&fx.entry("d/a.txt")]).unwrap();
    assert_eq!(fx.read("d/a.txt"), "a\n");
    assert!(fx.is_clean());
}

// ------------------------------------------------------------------------- empty files

// ------------------------------------------------------------------------- no newline at EOF

/// Discarding one side of an unstaged no-EOL change.
#[gitti_test::test]
fn no_eol_both_sides_discard_one_side() {
    let fx = fixture(&[("f", "x\na")]);
    fx.write("f", "x\nb");
    let d = fx.unstaged("f");
    fx.apply(&d, &added_only(&d), PatchMode::Discard);
    assert_eq!(fx.read("f"), "x\n");
    fx.write("f", "x\nb");
    let d = fx.unstaged("f");
    fx.apply(&d, &removed_only(&d), PatchMode::Discard);
    assert_eq!(fx.read("f"), "x\na\nb");
}

// ------------------------------------------------------------------------- long lines

// ------------------------------------------------------------------------- binary

// ------------------------------------------------------------------------- misc unix

/// A file in a directory reached through a tracked symlink is outside the repository
/// (git refuses `beyond a symbolic link`): staging fails cleanly.
/// Workflow: FS-06
#[cfg(unix)]
#[gitti_test::test]
fn path_beyond_a_symlink_is_refused() {
    let fx = fixture(&[("real/x.txt", "x\n")]);
    symlink("real", &fx.path("alias"));
    fx.commit_all("alias");
    fx.write("real/x.txt", "x\ny\n");
    assert!(fx.repo.stage_paths(&["alias/x.txt"]).is_err());
    let d = fx.unstaged("real/x.txt");
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    assert_eq!(fx.index("real/x.txt"), "x\ny\n");
}

/// Hard links: two tracked names for the same inode; changing one changes both, and they
/// stage independently.
/// Workflow: FS-07
#[cfg(unix)]
#[gitti_test::test]
fn hard_linked_files_stage_independently() {
    let fx = fixture(&[("a", "1\n2\n")]);
    fs::hard_link(fx.path("a"), fx.path("b")).unwrap();
    fx.commit_all("b");
    fx.write("a", "1\n2\n3\n");
    assert_eq!(fx.read("b"), "1\n2\n3\n");
    let st = fx.state();
    assert_eq!(st.unstaged.len(), 2);
    let d = fx.unstaged("b");
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    assert_eq!(fx.index("b"), "1\n2\n3\n");
    assert_eq!(fx.index("a"), "1\n2\n");
    // Discarding `a` rewrites it (git replaces the file, breaking the link).
    let d = fx.unstaged("a");
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Discard);
    assert_eq!(fx.read("a"), "1\n2\n");
}

/// A FIFO / socket-like special file in the worktree is ignored by git (not a regular
/// file, not a link): the refresh does not block or fail on it.
/// Workflow: FS-07
#[cfg(unix)]
#[gitti_test::test]
fn fifo_in_worktree_does_not_block() {
    let fx = fixture(&[("a.txt", "a\n")]);
    let fifo = fx.path("pipe");
    let ok = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return;
    }
    fx.write("a.txt", "a\nb\n");
    let st = fx.repo.working_state().unwrap();
    assert!(st.unstaged.iter().any(|f| f.path() == "a.txt"));
    // git status lists nothing for the FIFO, or an untracked entry without preview.
    if let Some(f) = st.unstaged.iter().find(|f| f.path() == "pipe") {
        assert!(f.diff.is_none());
    }
}

/// A read-only (0444) tracked file: discarding a line rewrites it (git replaces the file
/// through a new inode, which works in a writable directory), and the mode bits stay.
/// Workflow: DC-09
#[cfg(unix)]
#[gitti_test::test]
fn read_only_file_discard_lines() {
    let fx = fixture(&[("ro.txt", "a\nb\n")]);
    fx.write("ro.txt", "a\nX\nb\nY\n");
    chmod(&fx.path("ro.txt"), 0o444);
    let d = fx.unstaged("ro.txt");
    fx.apply(&d, &lines(&d, &["X"]), PatchMode::Discard);
    assert_eq!(fx.read("ro.txt"), "a\nb\nY\n");
    use std::os::unix::fs::PermissionsExt as _;
    let mode = fs::metadata(fx.path("ro.txt")).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o444, "still read-only (git apply writes the index's mode)");
    let d = fx.unstaged("ro.txt");
    fx.apply(&d, &lines(&d, &["Y"]), PatchMode::Stage);
    assert_eq!(fx.index("ro.txt"), "a\nb\nY\n");
}

/// Files in a read-only directory: staging works (only the index changes), discarding
/// fails cleanly and leaves the file as it was.
/// Workflow: DC-06
#[cfg(unix)]
#[gitti_test::test]
fn read_only_directory() {
    let fx = fixture(&[("ro/f.txt", "a\nb\n")]);
    fx.write("ro/f.txt", "a\nX\nb\n");
    let _restore = Restore(fx.path("ro"), 0o755);
    chmod(&fx.path("ro"), 0o555);
    let d = fx.unstaged("ro/f.txt");
    fx.apply(&d, &lines(&d, &["X"]), PatchMode::Stage);
    assert_eq!(fx.index("ro/f.txt"), "a\nX\nb\n");
    fx.repo.unstage_paths(&["ro/f.txt"]).unwrap();
    let probe = fx.path("ro/probe");
    if fs::write(&probe, "").is_ok() {
        let _ = fs::remove_file(probe);
        return; // root
    }
    let d = fx.unstaged("ro/f.txt");
    let r = fx
        .repo
        .apply_selection(&d, &lines(&d, &["X"]), PatchMode::Discard);
    if r.is_err() {
        assert_eq!(
            fx.read("ro/f.txt"),
            "a\nX\nb\n",
            "a failed discard changes nothing"
        );
    } else {
        assert_eq!(fx.read("ro/f.txt"), "a\nb\n");
    }
}

// ------------------------------------------------------------------------- what the UI does

/// Everything the diff view does with a working state full of odd files — rows with and
/// without wrapping, every gap revealed from the file's text (worktree / index version),
/// patches for every hunk in every mode — must not panic, for both diff modes.
#[cfg(unix)]
#[gitti_test::test]
fn diff_rows_of_odd_files_build() {
    use gitti_core::blobs::Version;
    use gitti_core::context::split_lines;
    use gitti_core::rows::{DiffRow, Expansion, Src, build_worktree_rows};
    use std::sync::Arc;

    let mut files: Vec<(&str, String)> = vec![
        ("mode.sh", numbered(20)),
        ("both.sh", numbered(40)),
        ("type", "file\n".into()),
        ("noeol", "x\na".into()),
        ("bin.dat", "b\n".into()),
        ("deleted", numbered(5)),
        ("empty", String::new()),
        ("secret", "s\n".into()),
        ("long", format!("{}\n", "x".repeat(50_000))),
        ("tab\tname", "1\n2\n".into()),
    ];
    files.extend(UNIX_NAMES.iter().take(4).map(|n| (*n, "n\n".to_string())));
    let refs: Vec<(&str, &str)> = files.iter().map(|(n, c)| (*n, c.as_str())).collect();
    let fx = fixture(&refs);
    symlink("mode.sh", &fx.path("link"));
    fx.commit_all("links");
    chmod(&fx.path("mode.sh"), 0o755);
    chmod(&fx.path("both.sh"), 0o755);
    fx.write(
        "both.sh",
        &numbered(40)
            .replace("line 3\n", "3\n")
            .replace("line 33\n", ""),
    );
    fs::remove_file(fx.path("type")).unwrap();
    symlink("noeol", &fx.path("type"));
    fs::remove_file(fx.path("link")).unwrap();
    symlink("both.sh", &fx.path("link"));
    symlink("gone", &fx.path("dangling"));
    fx.write("noeol", "x\nb");
    fx.write_bytes("bin.dat", b"\0\x01");
    fs::remove_file(fx.path("deleted")).unwrap();
    fx.write("empty", "now\n");
    fx.write("long", &format!("{}y\n", "x".repeat(50_000)));
    fx.write("tab\tname", "1\nzwei\n");
    fx.write("new\tuntracked", "u\n");
    fx.repo.stage_paths(&["empty", "tab\tname"]).unwrap();
    fx.write("empty", "now\nmore\n");
    fx.write("secret", "s\nt\n");
    let _restore = Restore(fx.path("secret"), 0o644);
    chmod(&fx.path("secret"), 0o000);

    for opts in [
        DiffOptions::default(),
        DiffOptions {
            ignore_whitespace: true,
            ..Default::default()
        },
    ] {
        let ws = fx.repo.working_state_with(&opts).unwrap();
        assert!(ws.unstaged.len() >= 12, "{:#?}", ws.status.entries);
        let mut exp = Expansion::default();
        for (src, list, version) in [
            (Src::Unstaged, &ws.unstaged, Version::Worktree),
            (Src::Staged, &ws.staged, Version::Index),
        ] {
            for f in list {
                let Some(d) = &f.diff else { continue };
                let text = fx
                    .repo
                    .read_versions(&[(version.clone(), f.path().to_string())])
                    .unwrap();
                if let Some(t) = &text[0] {
                    let lines = Arc::new(split_lines(t));
                    if exp.set_context_text(src, d, lines) {
                        exp.reveal(src, d, None, gitti_core::context::Reveal::All);
                    }
                }
                let real = f.patch_source().unwrap();
                for mode in [PatchMode::Stage, PatchMode::Unstage, PatchMode::Discard] {
                    for h in 0..real.hunks.len().max(1) {
                        let _ =
                            gitti_core::patch::build_patch(real, &Selection::whole_hunk(h), mode);
                        let _ = gitti_core::patch::build_patch(
                            real,
                            &Selection::lines(h, [0, 1, 2]),
                            mode,
                        );
                    }
                }
            }
        }
        for wrap in [None, Some(10), Some(1)] {
            let rows = build_worktree_rows(&ws, &exp, wrap);
            assert!(!rows.is_empty());
            for r in &rows {
                if let DiffRow::Line {
                    src,
                    file,
                    hunk,
                    line,
                    ..
                } = *r
                {
                    let list = if src == Src::Staged {
                        &ws.staged
                    } else {
                        &ws.unstaged
                    };
                    let d = list[file].diff.as_ref().unwrap();
                    assert!(d.hunks[hunk].lines.get(line).is_some());
                }
            }
        }
    }
}

/// A selection of the change lines (in every hunk) whose text is one of `texts`.
fn lines(f: &FileDiff, texts: &[&str]) -> Selection {
    let mut sel = Selection::default();
    for (h, hunk) in f.hunks.iter().enumerate() {
        let picked: Vec<usize> = hunk
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.is_change() && texts.contains(&l.text.as_str()))
            .map(|(i, _)| i)
            .collect();
        if !picked.is_empty() {
            sel.add_lines(h, picked);
        }
    }
    assert!(!sel.hunks.is_empty(), "none of {texts:?} in {f:#?}");
    sel
}

fn numbered(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

// ------------------------------------------------------------------------- odd file names

/// Names every file system takes.
const PORTABLE_NAMES: &[&str] = &[
    "with space.txt",
    "two  spaces .txt",
    "#hash.txt",
    "-rf",
    "--",
    "-n.txt",
    "ümlaut ä ß.txt",
    "日本語.txt",
    "emoji 🦀.rs",
    "dir with space/-x #y.txt",
    "a+b=c,d;e@f!.txt",
    "[x] y.txt",
    "~tilde",
    "%41percent",
    "'single'",
];

/// The odd names of this platform: the portable ones, on unix also `UNIX_NAMES`.
fn odd_names() -> Vec<&'static str> {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut names = PORTABLE_NAMES.to_vec();
    #[cfg(unix)]
    names.extend(UNIX_NAMES);
    // Decomposed Unicode (macOS would normalise it).
    #[cfg(target_os = "linux")]
    names.push("cafe\u{301}.txt");
    names
}

/// One selection per file, applied as one patch: the patch text `apply_selection` builds
/// for each file (`build_patch`), all files in one `git apply` (`apply_patch`).
fn apply_all(fx: &Fx, files: Vec<(FileDiff, Selection)>, mode: PatchMode) {
    let patch: Vec<u8> = files
        .iter()
        .flat_map(|(d, s)| gitti_core::patch::build_patch(d, s, mode).unwrap_or_else(|| panic!("{:?}", d.path())))
        .collect();
    fx.repo.apply_patch(&patch, mode).unwrap_or_else(|e| panic!("{mode:?}: {e:#}"));
}

/// The index versions of `names` (one batched read; `None` = not in the index).
fn index_all(fx: &Fx, names: &[&str]) -> Vec<Option<String>> {
    let req: Vec<_> = names.iter().map(|n| (gitti_core::blobs::Version::Index, n.to_string())).collect();
    fx.repo.read_versions(&req).unwrap()
}

/// The diff of every name on one side of a working state.
fn diffs(state: &WorkingState, staged: bool, names: &[&str]) -> Vec<FileDiff> {
    let list = if staged { &state.staged } else { &state.unstaged };
    names
        .iter()
        .map(|n| {
            let f = list.iter().find(|f| f.path() == *n);
            let f = f.unwrap_or_else(|| panic!("{n:?} not {}: {:#?}", if staged { "staged" } else { "unstaged" }, state.status.entries));
            f.diff.clone().unwrap_or_else(|| panic!("{n:?}: no diff"))
        })
        .collect()
}

/// The partial re-read after touching `paths` equals a full read; returns the full read.
fn after(fx: &Fx, prev: &WorkingState, paths: &[&str], what: &str) -> WorkingState {
    let opts = DiffOptions::default();
    let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
    let after = fx.repo.working_state_after(&opts, prev, &paths).unwrap();
    let full = fx.repo.working_state_with(&opts).unwrap();
    assert_eq!(format!("{after:?}"), format!("{full:?}"), "{what}");
    full
}

fn all_are(got: Vec<Option<String>>, want: Option<&str>, names: &[&str], what: &str) {
    for (n, g) in names.iter().zip(got) {
        assert_eq!(g.as_deref(), want, "{n:?}: {what}");
    }
}

/// Tracked files with odd names, all in one repository, each changed in two hunks: the
/// fixture, its working state and the content of the unchanged and the changed version.
fn odd_names_changed() -> (Vec<&'static str>, Fx, WorkingState, String) {
    let names = odd_names();
    let base = numbered(30);
    let files: Vec<(&str, &str)> = names.iter().map(|n| (*n, base.as_str())).collect();
    let fx = fixture(&files);
    let changed = base.replace("line 2\n", "line two\nline 2b\n").replace("line 25\n", "line 25!\n");
    for n in &names {
        fx.write(n, &changed);
    }
    let state = fx.state();
    (names, fx, state, base)
}

/// Tracked files with odd names, all in one repository: one status, every diff parsed
/// with its own path, hunk and line stage / unstage (one patch for all files per step),
/// the path-limited re-read.
/// Workflow: NU-05
#[gitti_test::test]
fn odd_names_tracked_hunks_and_lines() {
    let (names, fx, state, base) = odd_names_changed();
    assert_eq!(state.unstaged.len(), names.len());
    for (n, d) in names.iter().zip(diffs(&state, false, &names)) {
        let e = &state.status.entries.iter().find(|e| e.path == *n).unwrap();
        assert_eq!((e.index, e.worktree), (None, Some('M')), "{n:?}");
        assert_eq!(d.path(), *n);
        assert_eq!(d.hunks.len(), 2, "{n:?}: {d:#?}");
    }
    let one_hunk = base.replace("line 25\n", "line 25!\n");

    let sel = diffs(&state, false, &names).into_iter().map(|d| (d, Selection::whole_hunk(1))).collect();
    apply_all(&fx, sel, PatchMode::Stage);
    let state = after(&fx, &state, &names, "stage a hunk");
    all_are(index_all(&fx, &names), Some(&one_hunk), &names, "hunk staged");
    let sel = diffs(&state, false, &names).into_iter().map(|d| {
        let s = lines(&d, &["line two"]);
        (d, s)
    });
    apply_all(&fx, sel.collect(), PatchMode::Stage);
    // `line 2` became `line 2b` (the similar pair), `line two` is new above it: it stays
    // above (`patch::line_order`).
    let staged = one_hunk.replace("line 2\n", "line two\nline 2\n");
    all_are(index_all(&fx, &names), Some(&staged), &names, "line staged");
    let state = fx.state();
    let sel = diffs(&state, true, &names).into_iter().map(|d| {
        let s = lines(&d, &["line two"]);
        (d, s)
    });
    apply_all(&fx, sel.collect(), PatchMode::Unstage);
    let state = after(&fx, &state, &names, "unstage a line");
    all_are(index_all(&fx, &names), Some(&one_hunk), &names, "line unstaged");
    let sel = diffs(&state, true, &names).into_iter().map(|d| (d, Selection::whole_hunk(0)));
    apply_all(&fx, sel.collect(), PatchMode::Unstage);
    all_are(index_all(&fx, &names), Some(&base), &names, "hunk unstaged");
}

/// Tracked files with odd names: line and hunk discard (one patch for all files), then
/// whole-file stage / unstage / discard and the path-limited re-read.
/// Workflow: NU-05
#[gitti_test::test]
fn odd_names_tracked_discard_and_whole_files() {
    let (names, fx, state, base) = odd_names_changed();
    let sel = diffs(&state, false, &names).into_iter().map(|d| {
        let s = lines(&d, &["line 2b"]);
        (d, s)
    });
    apply_all(&fx, sel.collect(), PatchMode::Discard);
    let state = fx.state();
    let sel = diffs(&state, false, &names).into_iter().map(|d| (d, Selection::whole_hunk(1)));
    apply_all(&fx, sel.collect(), PatchMode::Discard);
    let discarded = base.replace("line 2\n", "line two\n");
    for n in &names {
        assert_eq!(fx.read(n), discarded, "{n:?}: line and hunk discarded");
    }

    fx.repo.stage_paths(&names).unwrap();
    all_are(index_all(&fx, &names), Some(&discarded), &names, "file staged");
    fx.repo.unstage_paths(&names).unwrap();
    all_are(index_all(&fx, &names), Some(&base), &names, "file unstaged");
    let state = fx.state();
    let entries: Vec<&StatusEntry> = state.status.entries.iter().collect();
    fx.repo.discard_paths(&entries).unwrap();
    after(&fx, &state, &names, "discard files");
    for n in &names {
        assert_eq!(fx.read(n), base, "{n:?}: file discarded");
    }
    assert!(fx.is_clean());
}

/// Untracked files with odd names: preview, partial stage, partial unstage of the added
/// file, unstage of the rest.
/// Workflow: NU-05
#[gitti_test::test]
fn odd_names_untracked_partial_stage_and_unstage() {
    let names = odd_names();
    let fx = fixture(&[("keep.txt", "k\n")]);
    for n in &names {
        fx.write(n, "one\ntwo\nthree\n");
    }
    let state = fx.state();
    for (n, d) in names.iter().zip(diffs(&state, false, &names)) {
        assert!(state.status.entries.iter().any(|e| e.path == *n && e.untracked), "{n:?}");
        assert_eq!((d.path(), d.change), (*n, FileChange::Added));
    }
    let sel = diffs(&state, false, &names).into_iter().map(|d| {
        let s = lines(&d, &["one", "three"]);
        (d, s)
    });
    apply_all(&fx, sel.collect(), PatchMode::Stage);
    let state = after(&fx, &state, &names, "stage new lines");
    all_are(index_all(&fx, &names), Some("one\nthree\n"), &names, "lines staged");
    let sel = diffs(&state, true, &names).into_iter().map(|d| {
        let s = lines(&d, &["three"]);
        (d, s)
    });
    apply_all(&fx, sel.collect(), PatchMode::Unstage);
    all_are(index_all(&fx, &names), Some("one\n"), &names, "line unstaged");
    let state = fx.state();
    let sel = diffs(&state, true, &names).into_iter().map(|d| (d, Selection::whole_hunk(0)));
    apply_all(&fx, sel.collect(), PatchMode::Unstage);
    all_are(index_all(&fx, &names), None, &names, "unstaged entirely");
}

/// Untracked files with odd names: discard of single lines, deletion.
/// Workflow: NU-05
#[gitti_test::test]
fn odd_names_untracked_discard_lines_and_files() {
    let names = odd_names();
    let fx = fixture(&[("keep.txt", "k\n")]);
    for n in &names {
        fx.write(n, "one\ntwo\nthree\n");
    }
    let state = fx.state();
    let sel = diffs(&state, false, &names).into_iter().map(|d| {
        let s = lines(&d, &["two"]);
        (d, s)
    });
    apply_all(&fx, sel.collect(), PatchMode::Discard);
    for n in &names {
        assert_eq!(fx.read(n), "one\nthree\n", "{n:?}");
    }
    let state = fx.state();
    let entries: Vec<&StatusEntry> = state.status.entries.iter().collect();
    fx.repo.discard_paths(&entries).unwrap();
    after(&fx, &state, &names, "discard untracked");
    for n in &names {
        assert!(fs::symlink_metadata(fx.path(n)).is_err(), "{n:?}");
    }
    assert!(fx.is_clean());
}

/// Deleted files with odd names: partial stage of the deletion (header rewritten to a
/// modification), unstage, the whole deletion, unstaging one line of it, discard.
/// Workflow: NU-05
#[gitti_test::test]
fn odd_names_deleted_partial_stage_and_discard() {
    let names = odd_names();
    let mut files: Vec<(&str, &str)> = names.iter().map(|n| (*n, "one\ntwo\nthree\n")).collect();
    files.push(("keep.txt", "k\n"));
    let fx = fixture(&files);
    for n in &names {
        fs::remove_file(fx.path(n)).unwrap();
    }
    let state = fx.state();
    for (n, d) in names.iter().zip(diffs(&state, false, &names)) {
        assert_eq!(d.change, FileChange::Deleted, "{n:?}");
    }
    let sel = diffs(&state, false, &names).into_iter().map(|d| {
        let s = lines(&d, &["two"]);
        (d, s)
    });
    apply_all(&fx, sel.collect(), PatchMode::Stage);
    all_are(index_all(&fx, &names), Some("one\nthree\n"), &names, "line of a deletion staged");
    let state = fx.state();
    let sel = diffs(&state, true, &names).into_iter().map(|d| {
        let s = lines(&d, &["two"]);
        (d, s)
    });
    apply_all(&fx, sel.collect(), PatchMode::Unstage);
    all_are(index_all(&fx, &names), Some("one\ntwo\nthree\n"), &names, "unstaged");
    let state = fx.state();
    let sel = diffs(&state, false, &names).into_iter().map(|d| (d, Selection::whole_hunk(0)));
    apply_all(&fx, sel.collect(), PatchMode::Stage);
    all_are(index_all(&fx, &names), None, &names, "deletion staged");
    let state = fx.state();
    let staged = diffs(&state, true, &names);
    assert!(staged.iter().all(|s| s.change == FileChange::Deleted));
    let sel = staged.into_iter().map(|d| {
        let s = lines(&d, &["one"]);
        (d, s)
    });
    apply_all(&fx, sel.collect(), PatchMode::Unstage);
    all_are(index_all(&fx, &names), Some("one\n"), &names, "one line of the deletion unstaged");
    fx.repo.unstage_paths(&names).unwrap();
    let state = fx.state();
    let entries: Vec<&StatusEntry> = state.status.entries.iter().collect();
    fx.repo.discard_paths(&entries).unwrap();
    for n in &names {
        assert_eq!(fx.read(n), "one\ntwo\nthree\n", "{n:?}");
    }
    assert!(fx.is_clean());
}

/// Whole-file operations take a path literally: a name with glob characters or pathspec
/// magic must not also hit the files it would match as a pattern (all pairs in one
/// repository: no name may hit any other file).
/// Workflow: ST-07
#[gitti_test::test]
fn whole_file_operations_take_paths_literally() {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut pairs: Vec<(&str, &str)> = vec![("[x] y.txt", "x y.txt"), ("a[bc].txt", "ab.txt")];
    #[cfg(unix)]
    pairs.extend([("*.txt", "other.txt"), ("q?.md", "qq.md"), (":one", "one"), ("\\*", "*z")]);
    let names: Vec<&str> = pairs.iter().map(|p| p.0).collect();
    let victims: Vec<&str> = pairs.iter().map(|p| p.1).collect();
    let files: Vec<(&str, &str)> = names.iter().chain(&victims).map(|n| (*n, "1\n")).collect();
    let fx = fixture(&files);
    for n in names.iter().chain(&victims) {
        fx.write(n, "2\n");
    }
    fx.repo.stage_paths(&names).unwrap();
    all_are(index_all(&fx, &names), Some("2\n"), &names, "staged");
    all_are(index_all(&fx, &victims), Some("1\n"), &victims, "staged by a pattern");
    let mut add = vec!["add", "--"];
    let literal: Vec<String> = victims.iter().map(|v| format!(":(literal){v}")).collect();
    add.extend(literal.iter().map(String::as_str));
    sh(fx.p(), &add);
    fx.repo.unstage_paths(&names).unwrap();
    all_are(index_all(&fx, &names), Some("1\n"), &names, "unstaged");
    all_are(index_all(&fx, &victims), Some("2\n"), &victims, "unstaged by a pattern");
    // Discard the names' changes; untracked "new <name>" files are deleted.
    let new_names: Vec<String> = names.iter().map(|n| format!("new {n}")).collect();
    for n in names.iter().chain(&victims) {
        fx.write(&format!("new {n}"), "n\n");
    }
    let status = fx.repo.status().unwrap();
    let ours: Vec<&StatusEntry> = status
        .entries
        .iter()
        .filter(|e| names.contains(&e.path.as_str()) || new_names.contains(&e.path))
        .collect();
    assert_eq!(ours.len(), 2 * names.len());
    fx.repo.discard_paths(&ours).unwrap();
    for (n, v) in names.iter().zip(&victims) {
        assert_eq!(fx.read(n), "1\n", "{n:?}");
        assert_eq!(fx.read(v), "2\n", "{n:?} discarded {v:?} too");
        assert_eq!(fx.read(&format!("new {v}")), "n\n", "{n:?} deleted new {v:?} too");
    }
    // In a repository without commits (`git rm --cached` unstages there).
    let fx = fixture(&[]);
    fx.write("[x] y.txt", "a\n");
    fx.write("x y.txt", "b\n");
    fx.repo.stage_all().unwrap();
    fx.repo.unstage_paths(&["[x] y.txt"]).unwrap();
    let entries = fx.index_entries();
    assert_eq!(entries, [("100644".to_string(), "x y.txt".to_string())]);
}

/// A tracked directory replaced by a file of the same name: its files are deleted, the
/// file is untracked; stage / unstage / discard bring either state back.
/// Workflow: EX-14
#[gitti_test::test]
fn directory_replaced_by_file() {
    let fx = fixture(&[("d/a.txt", "a\n"), ("d/b.txt", "one\ntwo\n"), ("keep", "k\n")]);
    fs::remove_dir_all(fx.path("d")).unwrap();
    fx.write("d", "now a file\n");
    let state = fx.state();
    let e: Vec<(&str, Option<char>, bool)> =
        state.status.entries.iter().map(|e| (e.path.as_str(), e.worktree, e.untracked)).collect();
    assert_eq!(e, [("d/a.txt", Some('D'), false), ("d/b.txt", Some('D'), false), ("d", None, true)]);
    // A line of a deleted file staged while the directory is gone.
    let d = diffs(&state, false, &["d/b.txt"]).remove(0);
    fx.apply(&d, &lines(&d, &["two"]), PatchMode::Stage);
    assert_eq!(fx.index("d/b.txt"), "one\n");
    fx.repo.unstage_paths(&["d/b.txt"]).unwrap();

    let prev = fx.state();
    let all = ["d/a.txt", "d/b.txt", "d"];
    fx.repo.stage_paths(&all).unwrap();
    after(&fx, &prev, &all, "stage dir → file");
    assert_eq!(index_all(&fx, &all), [None, None, Some("now a file\n".to_string())]);
    fx.repo.unstage_paths(&all).unwrap();
    assert_eq!(index_all(&fx, &all), [Some("a\n".to_string()), Some("one\ntwo\n".to_string()), None]);

    // Discard everything: the file goes, the directory comes back.
    let st = fx.state();
    let entries: Vec<&StatusEntry> = st.unstaged.iter().map(|f| &f.entry).collect();
    fx.repo.discard_paths(&entries).unwrap();
    assert_eq!((fx.read("d/a.txt"), fx.read("d/b.txt")), ("a\n".into(), "one\ntwo\n".into()));
    assert!(fx.is_clean());
}

/// A tracked file replaced by a directory of the same name.
/// Workflow: EX-14
#[gitti_test::test]
fn file_replaced_by_directory() {
    let fx = fixture(&[("f", "one\ntwo\n"), ("keep", "k\n")]);
    fs::remove_file(fx.path("f")).unwrap();
    fx.write("f/inner.txt", "inner\n");
    fx.write("f/deeper/x.txt", "x\n");
    let state = fx.state();
    let e: Vec<(&str, Option<char>, bool)> =
        state.status.entries.iter().map(|e| (e.path.as_str(), e.worktree, e.untracked)).collect();
    assert_eq!(e, [("f", Some('D'), false), ("f/deeper/x.txt", None, true), ("f/inner.txt", None, true)]);
    let [d, inner] = <[FileDiff; 2]>::try_from(diffs(&state, false, &["f", "f/inner.txt"])).unwrap();
    fx.apply(&d, &lines(&d, &["one"]), PatchMode::Stage);
    assert_eq!(fx.index("f"), "two\n");
    // While the file `f` is in the index, `git apply --cached` refuses a file below
    // `f/` ("appears as both a file and as a directory"): an error, nothing changed.
    assert!(fx.repo.apply_selection(&inner, &Selection::whole_hunk(0), PatchMode::Stage).is_err());
    assert_eq!(fx.index("f"), "two\n");
    // With the deletion of `f` staged it works.
    let d = fx.unstaged("f");
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    fx.apply(&inner, &Selection::whole_hunk(0), PatchMode::Stage);
    assert_eq!(fx.index("f/inner.txt"), "inner\n");
    fx.repo.unstage_all().unwrap();
    assert_eq!(fx.index("f"), "one\ntwo\n");
    let all = ["f", "f/inner.txt", "f/deeper/x.txt"];
    fx.repo.stage_paths(&all).unwrap();
    assert_eq!(index_all(&fx, &all), [None, Some("inner\n".to_string()), Some("x\n".to_string())]);
    fx.repo.unstage_all().unwrap();
    let st = fx.state();
    let entries: Vec<&StatusEntry> = st.unstaged.iter().map(|f| &f.entry).collect();
    fx.repo.discard_paths(&entries).unwrap();
    assert_eq!(fx.read("f"), "one\ntwo\n");
    assert!(fx.is_clean());
}

/// Empty files: a new empty file has no hunks and stages as a whole; an empty tracked
/// file gets lines (staged one by one) and loses them again; an emptied file stays in
/// the index (emptied, not deleted); an empty file deleted is a hunkless deletion.
/// Workflow: FS-07
#[gitti_test::test]
fn empty_files() {
    let fx = fixture(&[("empty", ""), ("full", "a\nb\n"), ("gone", "")]);
    fx.write("new-empty", "");
    fx.write("empty", "x\ny\n");
    fx.write("full", "");
    fs::remove_file(fx.path("gone")).unwrap();
    let state = fx.state();
    let [new, empty, full, gone] =
        <[FileDiff; 4]>::try_from(diffs(&state, false, &["new-empty", "empty", "full", "gone"])).unwrap();
    assert!(new.hunks.is_empty() && new.change == FileChange::Added);
    assert!(gone.hunks.is_empty() && gone.change == FileChange::Deleted);
    // One patch: the new empty file, line "y", line "a" of the emptied file, the
    // hunkless deletion.
    let sel = vec![
        (new, Selection::whole_hunk(0)),
        (empty.clone(), lines(&empty, &["y"])),
        (full, Selection::default()),
        (gone, Selection::whole_hunk(0)),
    ];
    let full_sel = lines(&sel[2].0, &["a"]);
    let sel: Vec<_> = sel.into_iter().map(|(d, s)| if s.hunks.is_empty() { (d, full_sel.clone()) } else { (d, s) }).collect();
    apply_all(&fx, sel, PatchMode::Stage);
    let all = ["new-empty", "empty", "full", "gone"];
    let s = |t: &str| Some(t.to_string());
    assert_eq!(index_all(&fx, &all), [s(""), s("y\n"), s("b\n"), None]);
    // Then the other line of the emptied file: emptied, not deleted.
    let state = fx.state();
    let full = diffs(&state, false, &["full"]).remove(0);
    fx.apply(&full, &lines(&full, &["b"]), PatchMode::Stage);
    assert_eq!(fx.index_mode("full").as_deref(), Some("100644"), "emptied, not deleted");
    assert_eq!(fx.index("full"), "");

    // Unstage the hunkless ones and the line, discard a line, delete the new file.
    let state = fx.state();
    let staged = diffs(&state, true, &["new-empty", "empty", "gone"]);
    assert!(staged[0].hunks.is_empty() && staged[2].hunks.is_empty());
    apply_all(&fx, staged.into_iter().map(|d| (d, Selection::whole_hunk(0))).collect(), PatchMode::Unstage);
    assert_eq!(index_all(&fx, &["new-empty", "empty", "gone"]), [None, s(""), s("")]);
    let state = fx.state();
    let [empty, gone] = <[FileDiff; 2]>::try_from(diffs(&state, false, &["empty", "gone"])).unwrap();
    apply_all(&fx, vec![(empty.clone(), lines(&empty, &["x"])), (gone, Selection::whole_hunk(0))], PatchMode::Discard);
    assert_eq!((fx.read("empty"), fx.read("gone")), ("y\n".into(), "".into()));
    fx.repo.discard_paths(&[&fx.entry("new-empty")]).unwrap();
    assert!(!fx.path("new-empty").exists());
}

/// The last line changes and neither version ends with a newline: staging only the
/// added line keeps the old last line (which now needs a newline), staging only the
/// removed line drops it; unstaging one side of the staged change likewise.
/// Workflow: EX-11
#[gitti_test::test]
fn no_eol_both_sides_stage_and_unstage_one_side() {
    let fx = fixture(&[("f", "x\na"), ("g", "x\na")]);
    fx.write("f", "x\nb");
    fx.write("g", "x\nb");
    let state = fx.state();
    let [f, g] = <[FileDiff; 2]>::try_from(diffs(&state, false, &["f", "g"])).unwrap();
    apply_all(&fx, vec![(f.clone(), added_only(&f)), (g.clone(), removed_only(&g))], PatchMode::Stage);
    assert_eq!(fx.index("f"), "x\na\nb");
    assert_eq!(fx.index("g"), "x\n");
    assert_eq!(fx.read("f"), "x\nb", "worktree untouched");

    fx.repo.stage_paths(&["f", "g"]).unwrap();
    let state = fx.state();
    let [f, g] = <[FileDiff; 2]>::try_from(diffs(&state, true, &["f", "g"])).unwrap();
    apply_all(&fx, vec![(f.clone(), removed_only(&f)), (g.clone(), added_only(&g))], PatchMode::Unstage);
    assert_eq!(fx.index("f"), "x\na\nb");
    assert_eq!(fx.index("g"), "x\n");
}

/// A newline added at the end (`a` → `a\n`) or removed, and lines appended after a last
/// line without newline: partial selections.
/// Workflow: EX-11
#[gitti_test::test]
fn no_eol_added_or_removed() {
    let fx = fixture(&[("f", "x\na"), ("g", "x\na\n")]);
    // Gaining the newline plus a line: stage just the new line "b". Losing the newline
    // plus a line removed: stage the removal only.
    fx.write("f", "x\na\nb\n");
    fx.write("g", "a");
    let state = fx.state();
    let [f, g] = <[FileDiff; 2]>::try_from(diffs(&state, false, &["f", "g"])).unwrap();
    apply_all(&fx, vec![(f.clone(), lines(&f, &["b"])), (g.clone(), lines(&g, &["x"]))], PatchMode::Stage);
    let idx = index_all(&fx, &["f", "g"]);
    assert!(matches!(idx[0].as_deref(), Some("x\na\nb\n" | "x\na\nb")), "{idx:?}");
    assert!(matches!(idx[1].as_deref(), Some("a\n" | "a")), "{idx:?}");
    fx.repo.stage_paths(&["f", "g"]).unwrap();
    assert_eq!(index_all(&fx, &["f", "g"]), [Some("x\na\nb\n".to_string()), Some("a".to_string())]);
}

/// A partially staged untracked file without a trailing newline, and its partial
/// unstage.
#[gitti_test::test]
fn no_eol_untracked_partial() {
    let fx = fixture(&[("keep", "k\n")]);
    fx.write("n", "one\ntwo\nthree");
    let d = fx.unstaged("n");
    fx.apply(&d, &lines(&d, &["one", "three"]), PatchMode::Stage);
    assert_eq!(fx.index("n"), "one\nthree");
    let s = fx.staged("n");
    fx.apply(&s, &lines(&s, &["one"]), PatchMode::Unstage);
    assert_eq!(fx.index("n"), "three");
    let state = fx.state();
    let d = diffs(&state, false, &["n"]).remove(0);
    fx.apply(&d, &lines(&d, &["one"]), PatchMode::Stage);
    assert_eq!(fx.index("n"), "one\nthree");
    let s = fx.staged("n");
    fx.apply(&s, &lines(&s, &["three"]), PatchMode::Unstage);
    assert_eq!(fx.index("n"), "one\n");
}

/// Very long lines (200 KB, one with no newline at all) diff and stage byte-exactly.
/// Workflow: BG-02
#[gitti_test::test]
fn very_long_lines() {
    let long_a = "a".repeat(200_000);
    let long_b = format!("{}b", "a".repeat(199_999));
    let fx = fixture(&[("long.txt", &format!("short\n{long_a}\nend\n"))]);
    fx.write("long.txt", &format!("short\n{long_b}\nend\nadded\n"));
    // One single 1 MB line without newline, untracked.
    let huge = "z".repeat(1_000_000);
    fx.write("one-line", &huge);
    let state = fx.state();
    let [d, one] = <[FileDiff; 2]>::try_from(diffs(&state, false, &["long.txt", "one-line"])).unwrap();
    assert!(d.hunks[0].lines.iter().any(|l| l.text.len() == 200_000));
    apply_all(&fx, vec![(d.clone(), lines(&d, &[long_b.as_str()])), (one, Selection::whole_hunk(0))], PatchMode::Stage);
    let idx = index_all(&fx, &["long.txt", "one-line"]);
    assert_eq!(idx[0].as_deref(), Some(format!("short\n{long_a}\n{long_b}\nend\n").as_str()));
    // Read in full (over the size limit of a version shown in the UI).
    assert_eq!(sh(fx.p(), &["cat-file", "-s", ":one-line"]).trim(), "1000000");
    let s = fx.staged("long.txt");
    fx.apply(&s, &Selection::whole_hunk(0), PatchMode::Unstage);
    let d = fx.unstaged("long.txt");
    fx.apply(&d, &lines(&d, &[long_a.as_str()]), PatchMode::Discard);
    assert_eq!(fx.read("long.txt"), format!("short\n{long_a}\n{long_b}\nend\nadded\n"));
}

/// A binary file next to text files: listed with a hunkless binary diff; text lines of
/// the neighbours stage as usual; the binary file stages as a whole; a patch of a binary
/// diff is an error, not a broken index.
/// Workflow: BN-01
#[gitti_test::test]
fn binary_next_to_text() {
    let fx = fixture(&[("a.txt", "a\nb\n"), ("z.txt", "z\n")]);
    fx.write_bytes("bin.dat", &[0, 1, 2, 3, 255]);
    fx.commit_all("bin");
    fx.write_bytes("bin.dat", &[0, 1, 2, 4, 255, 0]);
    fx.write("a.txt", "a\nB\n");
    fx.write("z.txt", "z\nZ\n");
    fx.write_bytes("new.bin", b"\0\x01new");
    let state = fx.state();
    let [bd, nb, a, z] = <[FileDiff; 4]>::try_from(diffs(&state, false, &["bin.dat", "new.bin", "a.txt", "z.txt"])).unwrap();
    assert!(bd.binary && bd.hunks.is_empty(), "{bd:#?}");
    assert!(nb.binary && nb.hunks.is_empty());
    fx.apply(&a, &lines(&a, &["B"]), PatchMode::Stage);
    assert_eq!(fx.index("a.txt"), "a\nb\nB\n");
    // A patch for a binary diff cannot be built from its (missing) hunks.
    assert!(fx.repo.apply_selection(&bd, &Selection::whole_hunk(0), PatchMode::Stage).is_err());
    assert_eq!(git_bytes(fx.p(), &["cat-file", "blob", ":bin.dat"]), [0, 1, 2, 3, 255]);
    // Binary: whole-file stage / unstage / discard.
    fx.repo.stage_paths(&["bin.dat", "new.bin"]).unwrap();
    assert_eq!(git_bytes(fx.p(), &["cat-file", "blob", ":bin.dat"]), [0, 1, 2, 4, 255, 0]);
    let st = fx.state();
    assert!(diffs(&st, true, &["bin.dat"])[0].binary);
    // The text neighbour is unaffected.
    assert_eq!(diffs(&st, false, &["z.txt"])[0].added_removed(), z.added_removed());
    fx.repo.unstage_paths(&["bin.dat", "new.bin"]).unwrap();
    fx.repo.discard_paths(&[&fx.entry("bin.dat")]).unwrap();
    assert_eq!(fs::read(fx.path("bin.dat")).unwrap(), [0, 1, 2, 3, 255]);
}

/// A text file that becomes binary and back.
/// Workflow: EX-10, BN-02
#[gitti_test::test]
fn text_becomes_binary() {
    let fx = fixture(&[("f", "text\n")]);
    fx.write_bytes("f", b"text\n\0bin");
    assert!(fx.unstaged("f").binary);
    fx.repo.stage_paths(&["f"]).unwrap();
    fx.write("f", "text\nagain\n");
    let st = fx.state();
    assert!(diffs(&st, true, &["f"])[0].binary);
    let u = diffs(&st, false, &["f"]).remove(0);
    assert!(u.binary, "index binary → worktree text is a binary diff: {u:#?}");
    fx.repo.stage_paths(&["f"]).unwrap();
    let s = fx.staged("f");
    assert!(!s.binary);
    fx.apply(&s, &lines(&s, &["again"]), PatchMode::Unstage);
    assert_eq!(fx.index("f"), "text\n");
}
