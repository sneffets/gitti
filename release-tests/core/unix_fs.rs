//! gitti-core on unix file-system semantics: exec bit, symlinks, type changes,
//! case-sensitive and odd file names, unreadable files and directories, directory ↔ file
//! swaps, empty files, missing trailing newlines, very long lines, binary files. Status,
//! diff and hunk + line stage / unstage / discard against temporary repositories.
//!
//! Cases that need unix semantics are behind `cfg(unix)` (or `target_os = "linux"` where
//! macOS differs: case-insensitive APFS, Unicode normalisation, UTF-8-only names); the
//! rest runs on every platform.

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
    sh(p, &["config", "user.name", "T"]);
    sh(p, &["config", "user.email", "t@example.com"]);
    sh(p, &["config", "commit.gpgsign", "false"]);
    sh(p, &["config", "core.autocrlf", "false"]);
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

fn numbered(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
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
#[cfg(unix)]
#[test]
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
#[cfg(unix)]
#[test]
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
#[test]
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
#[test]
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
#[test]
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
#[cfg(unix)]
#[test]
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

/// A new symlink is an untracked entry (no preview: it is not a regular file); staging,
/// unstaging and discarding it act on the link, never on its target.
#[cfg(unix)]
#[test]
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
#[test]
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
#[cfg(unix)]
#[test]
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
#[test]
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
#[test]
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
#[cfg(unix)]
#[test]
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
#[cfg(unix)]
#[test]
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
#[cfg(unix)]
#[test]
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
#[cfg(unix)]
#[test]
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
#[cfg(unix)]
#[test]
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
#[test]
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

/// Status, hunk + line stage / unstage / discard and the whole-file operations of a
/// tracked file called `name`, plus a partial re-read check.
fn exercise_tracked(name: &str) {
    let base = numbered(30);
    let fx = fixture(&[(name, &base)]);
    let changed = base
        .replace("line 2\n", "line two\nline 2b\n")
        .replace("line 25\n", "line 25!\n");
    fx.write(name, &changed);
    let e = fx.entry(name);
    assert_eq!((e.index, e.worktree), (None, Some('M')), "{name:?}");
    let d = fx.unstaged(name);
    assert_eq!(d.path(), name);
    assert_eq!(d.hunks.len(), 2, "{name:?}: {d:#?}");

    let prev = fx.state();
    fx.apply(&d, &Selection::whole_hunk(1), PatchMode::Stage);
    fx.check_after(&prev, &[name], &format!("{name:?}: stage hunk"));
    assert_eq!(
        fx.index(name),
        base.replace("line 25\n", "line 25!\n"),
        "{name:?}"
    );
    let d = fx.unstaged(name);
    fx.apply(&d, &lines(&d, &["line two"]), PatchMode::Stage);
    let staged = base
        .replace("line 25\n", "line 25!\n")
        .replace("line 2\n", "line 2\nline two\n");
    assert_eq!(fx.index(name), staged, "{name:?}");
    assert_eq!(fx.read(name), changed, "{name:?}: worktree untouched");

    let s = fx.staged(name);
    assert_eq!(s.path(), name);
    let prev = fx.state();
    fx.apply(&s, &lines(&s, &["line two"]), PatchMode::Unstage);
    fx.check_after(&prev, &[name], &format!("{name:?}: unstage line"));
    assert_eq!(
        fx.index(name),
        base.replace("line 25\n", "line 25!\n"),
        "{name:?}"
    );
    let s = fx.staged(name);
    fx.apply(&s, &Selection::whole_hunk(0), PatchMode::Unstage);
    assert_eq!(fx.index(name), base, "{name:?}");

    let d = fx.unstaged(name);
    fx.apply(&d, &lines(&d, &["line 2b"]), PatchMode::Discard);
    assert_eq!(fx.read(name), changed.replace("line 2b\n", ""), "{name:?}");
    let d = fx.unstaged(name);
    fx.apply(&d, &Selection::whole_hunk(1), PatchMode::Discard);
    assert_eq!(
        fx.read(name),
        base.replace("line 2\n", "line two\n"),
        "{name:?}"
    );

    fx.repo.stage_paths(&[name]).unwrap();
    assert_eq!(
        fx.index(name),
        base.replace("line 2\n", "line two\n"),
        "{name:?}"
    );
    fx.repo.unstage_paths(&[name]).unwrap();
    assert_eq!(fx.index(name), base, "{name:?}");
    let prev = fx.state();
    fx.repo.discard_paths(&[&fx.entry(name)]).unwrap();
    fx.check_after(&prev, &[name], &format!("{name:?}: discard file"));
    assert_eq!(fx.read(name), base, "{name:?}");
    assert!(fx.is_clean(), "{name:?}");
}

/// An untracked file called `name`: preview, partial stage, partial unstage of the added
/// file (its header is rewritten to a modification), discard of the untracked file.
fn exercise_untracked(name: &str) {
    let fx = fixture(&[("keep.txt", "k\n")]);
    fx.write(name, "one\ntwo\nthree\n");
    let e = fx.entry(name);
    assert!(e.untracked, "{name:?}");
    let d = fx.unstaged(name);
    assert_eq!(d.path(), name);
    assert_eq!(d.change, FileChange::Added);
    let prev = fx.state();
    fx.apply(&d, &lines(&d, &["one", "three"]), PatchMode::Stage);
    fx.check_after(&prev, &[name], &format!("{name:?}: stage new lines"));
    assert_eq!(fx.index(name), "one\nthree\n", "{name:?}");
    let s = fx.staged(name);
    fx.apply(&s, &lines(&s, &["three"]), PatchMode::Unstage);
    assert_eq!(fx.index(name), "one\n", "{name:?}");
    let s = fx.staged(name);
    fx.apply(&s, &Selection::whole_hunk(0), PatchMode::Unstage);
    assert_eq!(fx.index_mode(name), None, "{name:?}");
    // Discarding single lines of the untracked file.
    let d = fx.unstaged(name);
    fx.apply(&d, &lines(&d, &["two"]), PatchMode::Discard);
    assert_eq!(fx.read(name), "one\nthree\n", "{name:?}");
    let prev = fx.state();
    fx.repo.discard_paths(&[&fx.entry(name)]).unwrap();
    fx.check_after(&prev, &[name], &format!("{name:?}: discard untracked"));
    assert!(fs::symlink_metadata(fx.path(name)).is_err(), "{name:?}");
    assert!(fx.is_clean(), "{name:?}");
}

/// A deleted file called `name`: partial stage of the deletion (header rewritten to a
/// modification), unstage, discard of single removed lines.
fn exercise_deleted(name: &str) {
    let fx = fixture(&[(name, "one\ntwo\nthree\n"), ("keep.txt", "k\n")]);
    fs::remove_file(fx.path(name)).unwrap();
    assert_eq!(fx.entry(name).worktree, Some('D'), "{name:?}");
    let d = fx.unstaged(name);
    assert_eq!(d.change, FileChange::Deleted);
    fx.apply(&d, &lines(&d, &["two"]), PatchMode::Stage);
    assert_eq!(fx.index(name), "one\nthree\n", "{name:?}");
    let s = fx.staged(name);
    fx.apply(&s, &lines(&s, &["two"]), PatchMode::Unstage);
    assert_eq!(fx.index(name), "one\ntwo\nthree\n", "{name:?}");
    let d = fx.unstaged(name);
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    assert_eq!(fx.index_mode(name), None, "{name:?}");
    let s = fx.staged(name);
    assert_eq!(s.change, FileChange::Deleted);
    fx.apply(&s, &lines(&s, &["one"]), PatchMode::Unstage);
    assert_eq!(fx.index(name), "one\n", "{name:?}");
    fx.repo.unstage_paths(&[name]).unwrap();
    fx.repo.discard_paths(&[&fx.entry(name)]).unwrap();
    assert_eq!(fx.read(name), "one\ntwo\nthree\n", "{name:?}");
    assert!(fx.is_clean(), "{name:?}");
}

fn exercise(name: &str) {
    exercise_tracked(name);
    exercise_untracked(name);
    exercise_deleted(name);
}

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

#[test]
fn portable_odd_names() {
    for name in PORTABLE_NAMES {
        exercise(name);
    }
}

/// Names only unix allows: tab, newline, double quote, backslash, glob characters,
/// leading / trailing spaces, control characters.
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

/// Whole-file operations take a path literally: a name with glob characters or pathspec
/// magic must not also hit the files it would match as a pattern.
#[test]
fn whole_file_operations_take_paths_literally() {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut pairs: Vec<(&str, &str)> = vec![("[x] y.txt", "x y.txt"), ("a[bc].txt", "ab.txt")];
    #[cfg(unix)]
    pairs.extend([
        ("*.txt", "other.txt"),
        ("q?.md", "qq.md"),
        (":one", "one"),
        ("\\*", "*z"),
    ]);
    for (name, victim) in pairs {
        let fx = fixture(&[(name, "1\n"), (victim, "1\n")]);
        fx.write(name, "2\n");
        fx.write(victim, "2\n");
        fx.repo.stage_paths(&[name]).unwrap();
        assert_eq!(fx.index(name), "2\n", "{name:?}");
        assert_eq!(fx.index(victim), "1\n", "{name:?} staged {victim:?} too");
        sh(fx.p(), &["add", "--", &format!(":(literal){victim}")]);
        fx.repo.unstage_paths(&[name]).unwrap();
        assert_eq!(fx.index(name), "1\n", "{name:?}");
        assert_eq!(fx.index(victim), "2\n", "{name:?} unstaged {victim:?} too");
        fx.repo.discard_paths(&[&fx.entry(name)]).unwrap();
        assert_eq!(fx.read(name), "1\n", "{name:?}");
        assert_eq!(fx.read(victim), "2\n", "{name:?} discarded {victim:?} too");
        // Untracked, both: deleting one keeps the other.
        fx.write(&format!("new {name}"), "n\n");
        fx.write(&format!("new {victim}"), "n\n");
        fx.repo
            .discard_paths(&[&fx.entry(&format!("new {name}"))])
            .unwrap();
        assert_eq!(fx.read(&format!("new {victim}")), "n\n", "{name:?}");
    }
    // In a repository without commits (`git rm --cached` unstages there).
    let fx = fixture(&[]);
    fx.write("[x] y.txt", "a\n");
    fx.write("x y.txt", "b\n");
    fx.repo.stage_all().unwrap();
    fx.repo.unstage_paths(&["[x] y.txt"]).unwrap();
    assert_eq!(fx.index_mode("x y.txt").as_deref(), Some("100644"));
    assert_eq!(fx.index_mode("[x] y.txt"), None);
}

#[cfg(unix)]
#[test]
fn unix_odd_names() {
    for name in UNIX_NAMES {
        exercise(name);
    }
}

/// All odd names in one repository at once: one status, one diff, every file found.
#[test]
fn many_odd_names_in_one_status() {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut names: Vec<&str> = PORTABLE_NAMES.to_vec();
    #[cfg(unix)]
    names.extend(UNIX_NAMES);
    let files: Vec<(&str, String)> = names.iter().map(|n| (*n, format!("{n}\n"))).collect();
    let refs: Vec<(&str, &str)> = files.iter().map(|(n, c)| (*n, c.as_str())).collect();
    let fx = fixture(&refs);
    for n in &names {
        fx.write(n, &format!("{n}\nchanged\n"));
    }
    let st = fx.state();
    assert_eq!(st.unstaged.len(), names.len());
    for n in &names {
        let f = st
            .unstaged
            .iter()
            .find(|f| f.path() == *n)
            .unwrap_or_else(|| panic!("{n:?}"));
        let d = f.diff.as_ref().unwrap_or_else(|| panic!("{n:?}: no diff"));
        assert_eq!(d.path(), *n);
        assert_eq!(d.added_removed(), (1, 0), "{n:?}");
    }
    let paths: Vec<&str> = names.clone();
    fx.repo.stage_paths(&paths).unwrap();
    assert_eq!(fx.state().staged.len(), names.len());
    fx.repo.unstage_all().unwrap();
    let entries: Vec<StatusEntry> = fx
        .state()
        .unstaged
        .iter()
        .map(|f| f.entry.clone())
        .collect();
    fx.repo
        .discard_paths(&entries.iter().collect::<Vec<_>>())
        .unwrap();
    assert!(fx.is_clean());
}

/// `Foo` and `foo` are two files on a case-sensitive file system: each is staged on its
/// own, line by line.
#[cfg(target_os = "linux")]
#[test]
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
#[cfg(target_os = "linux")]
#[test]
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
#[cfg(target_os = "linux")]
#[test]
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
    exercise(nfd);
}

/// A file name that is not UTF-8 (Latin-1 "é"): listed, and refresh does not fail.
#[cfg(target_os = "linux")]
#[test]
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
#[test]
#[ignore = "paths are Strings (lossy UTF-8): a non-UTF-8 name becomes U+FFFD and \
            git does not find it"]
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

/// File content that is not UTF-8 (Latin-1 text): hunk and line staging must move the
/// exact bytes.
#[test]
#[ignore = "diff text is decoded lossily (String), so a patch of a Latin-1 file \
            carries U+FFFD and does not apply"]
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
#[cfg(unix)]
#[test]
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
#[test]
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
#[test]
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
#[cfg(unix)]
#[test]
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

/// A tracked directory replaced by a file of the same name: its files are deleted, the
/// file is untracked; stage / unstage / discard bring either state back.
#[test]
fn directory_replaced_by_file() {
    let fx = fixture(&[
        ("d/a.txt", "a\n"),
        ("d/b.txt", "one\ntwo\n"),
        ("keep", "k\n"),
    ]);
    fs::remove_dir_all(fx.path("d")).unwrap();
    fx.write("d", "now a file\n");
    let st = fx.repo.status().unwrap();
    let e: Vec<(&str, Option<char>, bool)> = st
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e.worktree, e.untracked))
        .collect();
    assert_eq!(
        e,
        [
            ("d/a.txt", Some('D'), false),
            ("d/b.txt", Some('D'), false),
            ("d", None, true)
        ]
    );
    // A line of a deleted file staged while the directory is gone.
    let d = fx.unstaged("d/b.txt");
    fx.apply(&d, &lines(&d, &["two"]), PatchMode::Stage);
    assert_eq!(fx.index("d/b.txt"), "one\n");
    fx.repo.unstage_paths(&["d/b.txt"]).unwrap();

    let prev = fx.state();
    fx.repo.stage_paths(&["d/a.txt", "d/b.txt", "d"]).unwrap();
    fx.check_after(&prev, &["d/a.txt", "d/b.txt", "d"], "stage dir → file");
    assert_eq!(fx.index("d"), "now a file\n");
    assert_eq!(fx.index_mode("d/a.txt"), None);
    fx.repo.unstage_paths(&["d/a.txt", "d/b.txt", "d"]).unwrap();
    assert_eq!(fx.index("d/a.txt"), "a\n");
    assert_eq!(fx.index_mode("d"), None);

    // Discard everything: the file goes, the directory comes back.
    let st = fx.state();
    let entries: Vec<&StatusEntry> = st.unstaged.iter().map(|f| &f.entry).collect();
    fx.repo.discard_paths(&entries).unwrap();
    assert_eq!(fx.read("d/a.txt"), "a\n");
    assert_eq!(fx.read("d/b.txt"), "one\ntwo\n");
    assert!(fx.is_clean());
}

/// Discarding the deleted files of a directory replaced by a file, in the order the UI
/// lists them (tracked first) — and the untracked file alone.
#[test]
fn directory_replaced_by_file_discard_in_steps() {
    let fx = fixture(&[("d/a.txt", "a\n"), ("keep", "k\n")]);
    fs::remove_dir_all(fx.path("d")).unwrap();
    fx.write("d", "file\n");
    fx.repo.discard_paths(&[&fx.entry("d")]).unwrap();
    fx.repo.discard_paths(&[&fx.entry("d/a.txt")]).unwrap();
    assert_eq!(fx.read("d/a.txt"), "a\n");
    assert!(fx.is_clean());
}

/// A tracked file replaced by a directory of the same name.
#[test]
fn file_replaced_by_directory() {
    let fx = fixture(&[("f", "one\ntwo\n"), ("keep", "k\n")]);
    fs::remove_file(fx.path("f")).unwrap();
    fx.write("f/inner.txt", "inner\n");
    fx.write("f/deeper/x.txt", "x\n");
    let st = fx.repo.status().unwrap();
    let e: Vec<(&str, Option<char>, bool)> = st
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e.worktree, e.untracked))
        .collect();
    assert_eq!(
        e,
        [
            ("f", Some('D'), false),
            ("f/deeper/x.txt", None, true),
            ("f/inner.txt", None, true)
        ]
    );
    let d = fx.unstaged("f");
    fx.apply(&d, &lines(&d, &["one"]), PatchMode::Stage);
    assert_eq!(fx.index("f"), "two\n");
    // While the file `f` is in the index, `git apply --cached` refuses a file below
    // `f/` ("appears as both a file and as a directory"): an error, nothing changed.
    let d = fx.unstaged("f/inner.txt");
    let err = fx
        .repo
        .apply_selection(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    assert!(err.is_err());
    assert_eq!(fx.index("f"), "two\n");
    // With the deletion of `f` staged it works.
    let d = fx.unstaged("f");
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    let d = fx.unstaged("f/inner.txt");
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    assert_eq!(fx.index("f/inner.txt"), "inner\n");
    fx.repo.unstage_all().unwrap();
    assert_eq!(fx.index("f"), "one\ntwo\n");
    fx.repo
        .stage_paths(&["f", "f/inner.txt", "f/deeper/x.txt"])
        .unwrap();
    assert_eq!(fx.index_mode("f"), None);
    assert_eq!(fx.index("f/inner.txt"), "inner\n");
    fx.repo.unstage_all().unwrap();
    let st = fx.state();
    let entries: Vec<&StatusEntry> = st.unstaged.iter().map(|f| &f.entry).collect();
    fx.repo.discard_paths(&entries).unwrap();
    assert_eq!(fx.read("f"), "one\ntwo\n");
    assert!(fx.is_clean());
}

// ------------------------------------------------------------------------- empty files

/// Empty files: a new empty file has no hunks and stages as a whole; an empty tracked
/// file gets lines (staged one by one) and loses them again.
#[test]
fn empty_files() {
    let fx = fixture(&[("empty", ""), ("full", "a\nb\n")]);
    fx.write("new-empty", "");
    let d = fx.unstaged("new-empty");
    assert!(d.hunks.is_empty());
    assert_eq!(d.change, FileChange::Added);
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    assert_eq!(fx.index_mode("new-empty").as_deref(), Some("100644"));
    assert_eq!(fx.index("new-empty"), "");
    let s = fx.staged("new-empty");
    assert!(s.hunks.is_empty());
    fx.apply(&s, &Selection::whole_hunk(0), PatchMode::Unstage);
    assert_eq!(fx.index_mode("new-empty"), None);
    fx.repo.discard_paths(&[&fx.entry("new-empty")]).unwrap();
    assert!(!fx.path("new-empty").exists());

    fx.write("empty", "x\ny\n");
    let d = fx.unstaged("empty");
    fx.apply(&d, &lines(&d, &["y"]), PatchMode::Stage);
    assert_eq!(fx.index("empty"), "y\n");
    let s = fx.staged("empty");
    fx.apply(&s, &Selection::whole_hunk(0), PatchMode::Unstage);
    assert_eq!(fx.index("empty"), "");
    let d = fx.unstaged("empty");
    fx.apply(&d, &lines(&d, &["x"]), PatchMode::Discard);
    assert_eq!(fx.read("empty"), "y\n");

    // Emptied: stage one removed line, then the other.
    fx.write("full", "");
    let d = fx.unstaged("full");
    fx.apply(&d, &lines(&d, &["a"]), PatchMode::Stage);
    assert_eq!(fx.index("full"), "b\n");
    let d = fx.unstaged("full");
    fx.apply(&d, &lines(&d, &["b"]), PatchMode::Stage);
    assert_eq!(fx.index("full"), "");
    assert_eq!(
        fx.index_mode("full").as_deref(),
        Some("100644"),
        "emptied, not deleted"
    );

    // An empty file deleted: a hunkless deletion.
    fx.repo.unstage_all().unwrap();
    sh(fx.p(), &["checkout", "--", "empty", "full"]);
    fs::remove_file(fx.path("empty")).unwrap();
    let d = fx.unstaged("empty");
    assert_eq!(d.change, FileChange::Deleted);
    assert!(d.hunks.is_empty());
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    assert_eq!(fx.index_mode("empty"), None);
    let s = fx.staged("empty");
    fx.apply(&s, &Selection::whole_hunk(0), PatchMode::Unstage);
    let d = fx.unstaged("empty");
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Discard);
    assert_eq!(fx.read("empty"), "");
    assert!(fx.is_clean());
}

// ------------------------------------------------------------------------- no newline at EOF

/// The last line changes and neither version ends with a newline: staging only the
/// added line keeps the old last line (which now needs a newline), staging only the
/// removed line drops it.
#[test]
fn no_eol_both_sides_stage_one_side() {
    let fx = fixture(&[("f", "x\na")]);
    fx.write("f", "x\nb");
    let d = fx.unstaged("f");
    fx.apply(&d, &added_only(&d), PatchMode::Stage);
    assert_eq!(fx.index("f"), "x\na\nb");
    sh(fx.p(), &["reset", "-q"]);
    let d = fx.unstaged("f");
    fx.apply(&d, &removed_only(&d), PatchMode::Stage);
    assert_eq!(fx.index("f"), "x\n");
    assert_eq!(fx.read("f"), "x\nb", "worktree untouched");
}

/// Unstaging one side of a staged no-EOL change.
#[test]
fn no_eol_both_sides_unstage_one_side() {
    let fx = fixture(&[("f", "x\na")]);
    fx.write("f", "x\nb");
    fx.repo.stage_paths(&["f"]).unwrap();
    let s = fx.staged("f");
    fx.apply(&s, &removed_only(&s), PatchMode::Unstage);
    assert_eq!(fx.index("f"), "x\na\nb");
    fx.repo.stage_paths(&["f"]).unwrap();
    let s = fx.staged("f");
    fx.apply(&s, &added_only(&s), PatchMode::Unstage);
    assert_eq!(fx.index("f"), "x\n");
}

/// Discarding one side of an unstaged no-EOL change.
#[test]
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

/// A newline added at the end (`a` → `a\n`) or removed, and lines appended after a last
/// line without newline: partial selections.
#[test]
fn no_eol_added_or_removed() {
    let fx = fixture(&[("f", "x\na"), ("g", "x\na\n")]);
    // Gaining the newline plus a line: stage just the new line "b".
    fx.write("f", "x\na\nb\n");
    let d = fx.unstaged("f");
    fx.apply(&d, &lines(&d, &["b"]), PatchMode::Stage);
    let idx = fx.index("f");
    assert!(idx == "x\na\nb\n" || idx == "x\na\nb", "{idx:?}");
    fx.repo.stage_paths(&["f"]).unwrap();
    assert_eq!(fx.index("f"), "x\na\nb\n");
    // Losing the newline plus a line removed: stage the removal only.
    fx.write("g", "a");
    let d = fx.unstaged("g");
    fx.apply(&d, &lines(&d, &["x"]), PatchMode::Stage);
    let idx = fx.index("g");
    assert!(idx == "a\n" || idx == "a", "{idx:?}");
    fx.repo.stage_paths(&["g"]).unwrap();
    assert_eq!(fx.index("g"), "a");
}

/// A partially staged untracked file without a trailing newline, and its partial
/// unstage.
#[test]
fn no_eol_untracked_partial() {
    let fx = fixture(&[("keep", "k\n")]);
    fx.write("n", "one\ntwo\nthree");
    let d = fx.unstaged("n");
    fx.apply(&d, &lines(&d, &["one", "three"]), PatchMode::Stage);
    assert_eq!(fx.index("n"), "one\nthree");
    let s = fx.staged("n");
    fx.apply(&s, &lines(&s, &["one"]), PatchMode::Unstage);
    assert_eq!(fx.index("n"), "three");
    let d = fx.unstaged("n");
    fx.apply(&d, &lines(&d, &["one"]), PatchMode::Stage);
    assert_eq!(fx.index("n"), "one\nthree");
    let s = fx.staged("n");
    fx.apply(&s, &lines(&s, &["three"]), PatchMode::Unstage);
    assert_eq!(fx.index("n"), "one\n");
}

// ------------------------------------------------------------------------- long lines

/// Very long lines (200 KB, one with no newline at all) diff and stage byte-exactly.
#[test]
fn very_long_lines() {
    let long_a = "a".repeat(200_000);
    let long_b = format!("{}b", "a".repeat(199_999));
    let fx = fixture(&[("long.txt", &format!("short\n{long_a}\nend\n"))]);
    fx.write("long.txt", &format!("short\n{long_b}\nend\nadded\n"));
    let d = fx.unstaged("long.txt");
    assert!(d.hunks[0].lines.iter().any(|l| l.text.len() == 200_000));
    fx.apply(&d, &lines(&d, &[long_b.as_str()]), PatchMode::Stage);
    assert_eq!(
        fx.index("long.txt"),
        format!("short\n{long_a}\n{long_b}\nend\n")
    );
    let s = fx.staged("long.txt");
    fx.apply(&s, &Selection::whole_hunk(0), PatchMode::Unstage);
    let d = fx.unstaged("long.txt");
    fx.apply(&d, &lines(&d, &[long_a.as_str()]), PatchMode::Discard);
    assert_eq!(
        fx.read("long.txt"),
        format!("short\n{long_a}\n{long_b}\nend\nadded\n")
    );
    // One single 1 MB line without newline, untracked.
    let huge = "z".repeat(1_000_000);
    fx.write("one-line", &huge);
    let d = fx.unstaged("one-line");
    fx.apply(&d, &Selection::whole_hunk(0), PatchMode::Stage);
    assert_eq!(fx.index("one-line"), huge);
}

// ------------------------------------------------------------------------- binary

/// A binary file next to text files: listed with a hunkless binary diff; text hunks and
/// lines of the neighbours stage as usual; the binary file stages as a whole.
#[test]
fn binary_next_to_text() {
    let fx = fixture(&[("a.txt", "a\nb\n"), ("z.txt", "z\n")]);
    fx.write_bytes("bin.dat", &[0, 1, 2, 3, 255]);
    fx.commit_all("bin");
    fx.write_bytes("bin.dat", &[0, 1, 2, 4, 255, 0]);
    fx.write("a.txt", "a\nB\n");
    fx.write("z.txt", "z\nZ\n");
    fx.write_bytes("new.bin", b"\0\x01new");
    let st = fx.state();
    let bin = st.unstaged.iter().find(|f| f.path() == "bin.dat").unwrap();
    let bd = bin.diff.as_ref().unwrap();
    assert!(bd.binary && bd.hunks.is_empty(), "{bd:#?}");
    let nb = fx.unstaged("new.bin");
    assert!(nb.binary && nb.hunks.is_empty());
    let d = fx.unstaged("a.txt");
    fx.apply(&d, &lines(&d, &["B"]), PatchMode::Stage);
    assert_eq!(fx.index("a.txt"), "a\nb\nB\n");
    // Binary: whole-file stage / unstage / discard.
    fx.repo.stage_paths(&["bin.dat", "new.bin"]).unwrap();
    let raw = git_bytes(fx.p(), &["cat-file", "blob", ":bin.dat"]);
    assert_eq!(raw, [0, 1, 2, 4, 255, 0]);
    let s = fx.staged("bin.dat");
    assert!(s.binary);
    fx.repo.unstage_paths(&["bin.dat", "new.bin"]).unwrap();
    fx.repo.discard_paths(&[&fx.entry("bin.dat")]).unwrap();
    assert_eq!(fs::read(fx.path("bin.dat")).unwrap(), [0, 1, 2, 3, 255]);
    // A patch for a binary diff cannot be built from its (missing) hunks: an error, not
    // a broken index.
    fx.write_bytes("bin.dat", &[9, 0, 9]);
    let d = fx.unstaged("bin.dat");
    assert!(
        fx.repo
            .apply_selection(&d, &Selection::whole_hunk(0), PatchMode::Stage)
            .is_err()
    );
    assert_eq!(fx.index_mode("bin.dat").as_deref(), Some("100644"));
    assert_eq!(
        git_bytes(fx.p(), &["cat-file", "blob", ":bin.dat"]),
        [0, 1, 2, 3, 255]
    );
    // The text neighbour is unaffected.
    assert_eq!(fx.unstaged("z.txt").added_removed(), (1, 0));
}

/// A text file that becomes binary and back.
#[test]
fn text_becomes_binary() {
    let fx = fixture(&[("f", "text\n")]);
    fx.write_bytes("f", b"text\n\0bin");
    let d = fx.unstaged("f");
    assert!(d.binary);
    fx.repo.stage_paths(&["f"]).unwrap();
    fx.write("f", "text\nagain\n");
    let st = fx.state();
    assert!(st.staged[0].diff.as_ref().unwrap().binary);
    let u = fx.unstaged("f");
    assert!(
        u.binary,
        "index binary → worktree text is a binary diff: {u:#?}"
    );
    fx.repo.stage_paths(&["f"]).unwrap();
    let s = fx.staged("f");
    assert!(!s.binary);
    fx.apply(&s, &lines(&s, &["again"]), PatchMode::Unstage);
    assert_eq!(fx.index("f"), "text\n");
}

// ------------------------------------------------------------------------- misc unix

/// A file in a directory reached through a tracked symlink is outside the repository
/// (git refuses `beyond a symbolic link`): staging fails cleanly.
#[cfg(unix)]
#[test]
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
#[cfg(unix)]
#[test]
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
#[cfg(unix)]
#[test]
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
#[cfg(unix)]
#[test]
fn read_only_file_discard_lines() {
    let fx = fixture(&[("ro.txt", "a\nb\n")]);
    fx.write("ro.txt", "a\nX\nb\nY\n");
    chmod(&fx.path("ro.txt"), 0o444);
    let d = fx.unstaged("ro.txt");
    fx.apply(&d, &lines(&d, &["X"]), PatchMode::Discard);
    assert_eq!(fx.read("ro.txt"), "a\nb\nY\n");
    let d = fx.unstaged("ro.txt");
    fx.apply(&d, &lines(&d, &["Y"]), PatchMode::Stage);
    assert_eq!(fx.index("ro.txt"), "a\nb\nY\n");
}

/// Files in a read-only directory: staging works (only the index changes), discarding
/// fails cleanly and leaves the file as it was.
#[cfg(unix)]
#[test]
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
#[test]
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
