//! Git LFS without git-lfs: pointer files committed via fast-import, objects
//! written into `.git/lfs/objects` by the test, the LFS filter switched off in the
//! repository's config (whatever the user's global config installs), or replaced by a
//! fake clean filter. Commit diffs show LFS rows (sizes, ids) and the content's diff when
//! the objects are local text; hunk / line staging of LFS files is refused before any
//! git runs, the whole file stages through the filter.

use std::fs;
use std::path::Path;

use gitti_core::Repo;
use gitti_core::diff::{FileDiff, LineKind};
use gitti_core::lfs::{LfsContent, Pointer, WholeFileOnly};
use gitti_core::patch::{PatchMode, Selection};

use crate::fixture_kit::{E, History, append_config, f, repo_with, sh, slash};

const ATTRS: &str = "*.dat filter=lfs diff=lfs merge=lfs -text\n";
/// The LFS filter off for this repository (empty commands = no filter, not required):
/// pointers stay pointers, as without git-lfs.
const NO_FILTER: &str =
    "[filter \"lfs\"]\n\tclean =\n\tsmudge =\n\tprocess =\n\trequired = false\n";

const DOC1: &[u8] = b"alpha\nbeta\n";
const DOC2: &[u8] = b"alpha\nBETA\ngamma\n";
const IMG1: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR one";
const IMG2: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR two";

fn pointer(content: &[u8]) -> Vec<u8> {
    Pointer::of(content).text().into_bytes()
}

/// Puts `content` into the repository's local LFS store.
fn store(repo: &Path, content: &[u8]) {
    let p = Pointer::of(content);
    let dir = repo
        .join(".git/lfs/objects")
        .join(&p.oid[..2])
        .join(&p.oid[2..4]);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(&p.oid), content).unwrap();
}

/// Two commits changing three LFS files and a plain one; `doc.dat` and `img.dat` have
/// their objects in the store, `far.dat` has none (not fetched). `config` is appended to
/// the repository's config ([`NO_FILTER`] or a fake filter: an empty `process` would hide
/// a later `clean`).
fn lfs_repo(
    config: &str,
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    [gitti_core::ObjectId; 2],
) {
    let (p_doc1, p_doc2) = (pointer(DOC1), pointer(DOC2));
    let (p_img1, p_img2) = (pointer(IMG1), pointer(IMG2));
    let (p_far1, p_far2) = (pointer(b"far one\n"), pointer(b"far two\n"));
    let mut h = History::new();
    let c1 = h.commit(
        "main",
        &[],
        "init",
        &[
            (".gitattributes", f(ATTRS)),
            ("doc.dat", E::File(&p_doc1)),
            ("img.dat", E::File(&p_img1)),
            ("far.dat", E::File(&p_far1)),
            ("plain.txt", f("a\n")),
        ],
    );
    let c2 = h.commit(
        "main",
        &[c1],
        "update",
        &[
            ("doc.dat", E::File(&p_doc2)),
            ("img.dat", E::File(&p_img2)),
            ("far.dat", E::File(&p_far2)),
            ("plain.txt", f("b\n")),
        ],
    );
    let (tmp, p, ids) = repo_with(&h);
    append_config(&p, config);
    for c in [DOC1, DOC2, IMG1, IMG2] {
        store(&p, c);
    }
    (tmp, p, [ids[c1], ids[c2]])
}

fn file<'a>(files: &'a [FileDiff], name: &str) -> &'a FileDiff {
    files
        .iter()
        .find(|d| d.path() == name)
        .unwrap_or_else(|| panic!("no {name}"))
}

fn work<'a>(files: &'a [gitti_core::WorkFile], name: &str) -> &'a FileDiff {
    files
        .iter()
        .find(|w| w.path() == name)
        .and_then(|w| w.diff.as_ref())
        .unwrap_or_else(|| panic!("no diff of {name}"))
}

/// The changed lines of a diff as `+text` / `-text`.
fn changes(d: &FileDiff) -> Vec<String> {
    d.hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter_map(|l| match l.kind {
            LineKind::Added => Some(format!("+{}", l.text)),
            LineKind::Removed => Some(format!("-{}", l.text)),
            _ => None,
        })
        .collect()
}

#[gitti_test::test]
fn commit_diffs_show_lfs_rows_and_local_text_content() {
    let (_tmp, p, [c1, c2]) = lfs_repo(NO_FILTER);
    let r = Repo::open(&p).unwrap();
    let files = r.commit_diff(c2).unwrap();

    // Both objects local, text: the content's diff instead of the pointer lines.
    let doc = file(&files, "doc.dat");
    let lfs = doc.lfs.as_deref().expect("doc.dat is an LFS change");
    assert_eq!(lfs.content, LfsContent::Diffed);
    assert_eq!(
        lfs.old.as_ref().unwrap().oid.as_deref(),
        Some(Pointer::of(DOC1).oid.as_str())
    );
    assert_eq!(lfs.new.as_ref().unwrap().size, DOC2.len() as u64);
    assert_eq!(changes(doc), ["-beta", "+BETA", "+gamma"]);
    let summary = lfs.summary(doc.hunks.len());
    assert_eq!(
        summary,
        format!(
            "LFS object, 11 B → 17 B (sha256 {}…)",
            &Pointer::of(DOC2).oid[..12]
        )
    );

    // Binary content: the row says so, no lines.
    let img = file(&files, "img.dat");
    assert_eq!(img.lfs.as_ref().unwrap().content, LfsContent::Binary);
    assert!(img.hunks.is_empty());
    // Not in the store.
    let far = file(&files, "far.dat");
    let far_lfs = far.lfs.as_deref().unwrap();
    assert_eq!(far_lfs.content, LfsContent::NotFetched);
    assert!(
        far_lfs
            .summary(0)
            .ends_with("— content not in the local LFS store"),
        "{}",
        far_lfs.summary(0)
    );
    assert!(far.hunks.is_empty());
    // Everything else as before.
    assert!(file(&files, "plain.txt").lfs.is_none());
    assert_eq!(changes(file(&files, "plain.txt")), ["-a", "+b"]);

    // A comparison of the two commits shows the same.
    use gitti_core::compare::CompareSide::Commit;
    let cmp = r
        .compare_diff(Commit(c1), Commit(c2), &Default::default())
        .unwrap();
    assert_eq!(
        changes(file(cmp.files(), "doc.dat")),
        ["-beta", "+BETA", "+gamma"]
    );
    assert!(file(cmp.files(), "far.dat").lfs.is_some());

    // The root commit: added LFS files, the added content as lines.
    let root = r.commit_diff(c1).unwrap();
    let doc = file(&root, "doc.dat");
    let lfs = doc.lfs.as_deref().unwrap();
    assert!(lfs.old.is_none() && lfs.content == LfsContent::Diffed);
    assert_eq!(changes(doc), ["+alpha", "+beta"]);
    assert!(lfs.summary(1).starts_with("LFS object added, 11 B"));
    assert!(file(&root, ".gitattributes").lfs.is_none());

    // The rows: the LFS row first, then the content's hunk.
    let exp = gitti_core::rows::Expansion::default();
    let rows = gitti_core::rows::build_commit_rows(&root, None, &exp, None);
    let doc_ix = root.iter().position(|d| d.path() == "doc.dat").unwrap();
    let kinds: Vec<&str> = rows
        .iter()
        .filter(|r| r.file_ref().is_some_and(|(_, f)| f == doc_ix))
        .map(|r| r.kind_name())
        .collect();
    assert_eq!(kinds, ["file", "note", "hunk", "line", "line"]);
}

/// Workflow: LF-01
#[gitti_test::test]
fn working_lfs_files_stage_only_whole() {
    let (_tmp, p, _) = lfs_repo(NO_FILTER);
    let r = Repo::open(&p).unwrap();
    // A new version as git-lfs would leave it without a filter: a pointer in the working
    // tree, its object in the store. And an untracked file under an LFS path.
    let doc3 = b"alpha\nBETA\ngamma\ndelta\n";
    store(&p, doc3);
    fs::write(p.join("doc.dat"), pointer(doc3)).unwrap();
    fs::write(p.join("new.dat"), "raw\ncontent\n").unwrap();
    fs::write(p.join("plain.txt"), "c\n").unwrap();

    let ws = r.working_state().unwrap();
    let doc = work(&ws.unstaged, "doc.dat");
    assert_eq!(
        doc.lfs.as_ref().map(|l| l.content),
        Some(LfsContent::Diffed)
    );
    assert_eq!(changes(doc), ["+delta"]);
    let new = work(&ws.unstaged, "new.dat");
    assert_eq!(new.lfs.as_ref().map(|l| l.content), Some(LfsContent::File));
    assert_eq!(
        changes(new),
        ["+raw", "+content"],
        "an untracked file keeps its lines"
    );
    assert!(work(&ws.unstaged, "plain.txt").lfs.is_none());

    // Refused before any git runs.
    let before = r.command_log().snapshot().len();
    for (d, sel, mode) in [
        (doc, Selection::lines(0, [1]), PatchMode::Stage),
        (doc, Selection::whole_hunk(0), PatchMode::Discard),
        (new, Selection::whole_hunk(0), PatchMode::Stage),
    ] {
        let err = r.apply_selection(d, &sel, mode).unwrap_err();
        let typed = err.downcast_ref::<WholeFileOnly>().expect("WholeFileOnly");
        assert_eq!((typed.path.as_str(), typed.mode), (d.path(), mode));
        assert!(err.to_string().contains("is a Git LFS file — "), "{err}");
    }
    assert_eq!(r.command_log().snapshot().len(), before);
    assert_eq!(
        gitti_core::lfs::WHOLE_FILE_NOTE,
        "LFS file: whole file only"
    );

    // The whole file by path: the pointer goes into the index, the staged diff is the
    // LFS row with the content, and unstaging it is whole-file only too.
    r.stage_paths(&["doc.dat"]).unwrap();
    assert_eq!(sh(&p, &["show", ":doc.dat"]).as_bytes(), pointer(doc3));
    let ws = r.working_state().unwrap();
    let staged = work(&ws.staged, "doc.dat");
    assert_eq!(changes(staged), ["+delta"]);
    let err = r
        .apply_selection(staged, &Selection::whole_hunk(0), PatchMode::Unstage)
        .unwrap_err();
    assert!(err.downcast_ref::<WholeFileOnly>().is_some(), "{err}");
    assert!(r.lfs_state().uses);
}

/// With a clean filter (a fake git-lfs: doc.dat's content becomes its pointer, other
/// files pass) the working tree holds the real content; git's diff shows the cleaned
/// pointer, gitti the content against the store's old object — the new one isn't in the
/// store, the file is. Staging the whole file runs the filter: the pointer lands in the
/// index. (Where git-lfs is installed its `filter.lfs.process` wins over the fake and
/// writes the same pointer.)
/// Workflow: LF-01
#[gitti_test::test]
fn whole_file_stage_runs_the_clean_filter() {
    let doc3 = b"alpha\nBETA\ngamma\nfrom the filter\n";
    let fake_dir = tempfile::tempdir().unwrap();
    let ptr_file = fake_dir.path().join("doc3.pointer");
    fs::write(&ptr_file, pointer(doc3)).unwrap();
    let script = fake_dir.path().join("clean.sh");
    fs::write(
        &script,
        format!(
            "if [ \"$1\" = doc.dat ]; then cat >/dev/null; cat '{}'; else cat; fi\n",
            slash(&ptr_file)
        ),
    )
    .unwrap();
    let fake = format!(
        "[filter \"lfs\"]\n\tclean = sh '{}' %f\n\tsmudge = cat\n\trequired = true\n",
        slash(&script)
    );
    let (_tmp, p, _) = lfs_repo(&fake);
    fs::write(p.join("doc.dat"), doc3).unwrap();
    let r = Repo::open(&p).unwrap();

    let ws = r.working_state().unwrap();
    let doc = work(&ws.unstaged, "doc.dat");
    let lfs = doc.lfs.as_deref().expect("an LFS change");
    assert_eq!(
        lfs.new.as_ref().unwrap().oid.as_deref(),
        Some(Pointer::of(doc3).oid.as_str())
    );
    assert_eq!(lfs.content, LfsContent::Diffed);
    assert_eq!(changes(doc), ["+from the filter"]);

    r.stage_paths(&["doc.dat"]).unwrap();
    assert_eq!(sh(&p, &["show", ":doc.dat"]).as_bytes(), pointer(doc3));
}

/// Locking needs git-lfs: without it the command is refused before git runs; with it,
/// these repositories have no LFS server (nothing checked here — a manual check, not
/// automated).
#[gitti_test::test]
fn locks_need_git_lfs() {
    let (_tmp, p, _) = lfs_repo(NO_FILTER);
    let r = Repo::open(&p).unwrap();
    let state = r.lfs_state();
    assert!(state.uses);
    if state.installed {
        println!("note: git-lfs is installed — the missing-git-lfs paths are not checked");
        return;
    }
    assert_eq!(state.notice(), Some(gitti_core::lfs::MISSING_NOTICE));
    let before = r.command_log().snapshot().len();
    let err = r.lfs_lock("doc.dat").unwrap_err();
    assert!(
        err.to_string().contains("git-lfs is not installed"),
        "{err}"
    );
    assert!(r.lfs_locks().is_err() && r.lfs_unlock("doc.dat", false).is_err());
    assert_eq!(r.command_log().snapshot().len(), before);
}

/// A nested `.gitattributes` that isn't committed yet (just written by "Track with Git
/// LFS…" in a folder) makes the repository an LFS one and marks the files under it.
#[gitti_test::test]
fn nested_untracked_attributes_count() {
    let mut h = History::new();
    h.commit("main", &[], "init", &[("plain.txt", f("a\n"))]);
    let (_tmp, p, _) = repo_with(&h);
    append_config(&p, NO_FILTER);
    let r = Repo::open(&p).unwrap();
    assert!(!r.lfs_state().uses);
    fs::create_dir_all(p.join("art")).unwrap();
    fs::write(p.join("art/.gitattributes"), "*.psd filter=lfs\n").unwrap();
    fs::write(p.join("art/x.psd"), "PSD").unwrap();
    fs::write(p.join("y.psd"), "PSD").unwrap();
    let ws = r.working_state().unwrap();
    assert!(work(&ws.unstaged, "art/x.psd").lfs.is_some(), "under the nested file");
    assert!(work(&ws.unstaged, "y.psd").lfs.is_none(), "outside its folder");
    sh(&p, &["add", "art/.gitattributes"]);
    sh(&p, &["commit", "-qm", "attrs"]);
    assert!(r.lfs_state().uses, "committed nested file");
}

// ------------------------------------------------------- downloading objects

/// The commit and the comparison caches: a diff showing `far.dat`'s objects as missing is
/// offered for download; once the store has them (however they came) the cached entries
/// are dropped and the next load shows the content. No git-lfs needed.
/// Workflow: LF-04
#[gitti_test::test]
fn cached_diffs_reannotate_when_objects_arrive() {
    use gitti_core::compare::CompareSide::Commit;
    use gitti_core::lfs_fetch::{LfsDownload, downloadable};
    let (_tmp, p, [c1, c2]) = lfs_repo(NO_FILTER);
    let r = Repo::open(&p).unwrap();
    let opts = Default::default();
    let data = r.load_commit(c2, None, &opts).unwrap();
    let far = file(&data.files, "far.dat");
    assert!(downloadable(far));
    // Binary and diffed LFS changes are no downloads; nor is the plain file.
    for name in ["img.dat", "doc.dat", "plain.txt"] {
        assert!(!downloadable(file(&data.files, name)), "{name}");
    }
    let dl = LfsDownload::of_view(&data, None).unwrap();
    assert_eq!(dl.revs, [c2, c1], "the commit and its parent");
    assert_eq!(dl.include, ["far.dat"]);
    assert_eq!(dl.files.len(), 1);
    assert_eq!(dl.files[0].1.len(), 2, "both sides' objects");
    let cmp = r.compare_diff(Commit(c1), Commit(c2), &opts).unwrap();
    let cdl = LfsDownload::of_view(&cmp.data, Some(&cmp)).unwrap();
    assert_eq!(cdl.revs, [c1, c2]);
    // Unchanged store: the cache answers (no git).
    let n = r.command_log().snapshot().len();
    assert!(r.cached_commit(c2, &opts).is_some_and(|d| std::sync::Arc::ptr_eq(&d, &data)));
    assert!(r.cached_compare(Commit(c1), Commit(c2), &opts).is_some());
    assert_eq!(r.command_log().snapshot().len(), n);

    // One side arriving changes nothing yet; both do.
    store(&p, b"far one\n");
    assert!(r.cached_commit(c2, &opts).is_some(), "the new side is still missing");
    store(&p, b"far two\n");
    assert!(r.cached_commit(c2, &opts).is_none(), "dropped");
    assert!(r.cached_compare(Commit(c1), Commit(c2), &opts).is_none(), "dropped");
    let again = r.load_commit(c2, None, &opts).unwrap();
    let far = file(&again.files, "far.dat");
    assert_eq!(far.lfs.as_ref().unwrap().content, LfsContent::Diffed);
    assert_eq!(changes(far), ["-far one", "+far two"]);
    assert!(LfsDownload::of_view(&again, None).is_none(), "nothing left to download");
    assert!(r.cached_commit(c2, &opts).is_some_and(|d| std::sync::Arc::ptr_eq(&d, &again)));
}

/// Real git-lfs (where installed): "Download LFS objects" fetches a commit's missing
/// objects from a local bare remote (`origin.git/lfs/objects`), also in a folder; an
/// object the server lacks is named; afterwards the commit shows the content.
/// Workflow: LF-04
#[gitti_test::test]
fn download_fetches_missing_objects_from_the_remote() {
    use gitti_core::lfs_fetch::LfsDownload;
    let tmp = tempfile::tempdir().unwrap();
    let base = gitti_core::paths::canonical(tmp.path());
    let Some((env, bare)) = local_remote(&base) else {
        return;
    };
    let (far1, far2, gone): (&[u8], &[u8], &[u8]) = (b"far one\n", b"far two\n", b"gone\n");
    let (p1, p2, pg) = (pointer(far1), pointer(far2), pointer(gone));
    let mut h = History::new();
    let c1 = h.commit(
        "main",
        &[],
        "init",
        &[(".gitattributes", f(ATTRS)), ("art/far.dat", E::File(&p1))],
    );
    let c2 = h.commit(
        "main",
        &[c1],
        "update",
        &[("art/far.dat", E::File(&p2)), ("gone.dat", E::File(&pg))],
    );
    let (_t, p, ids) = repo_with(&h);
    append_config(
        &p,
        &format!("{NO_FILTER}[remote \"origin\"]\n\turl = {}\n", slash(&bare)),
    );
    for c in [far1, far2] {
        let o = Pointer::of(c).oid;
        let dir = bare.join("lfs/objects").join(&o[..2]).join(&o[2..4]);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(&o), c).unwrap();
    }
    let mut r = Repo::open(&p).unwrap();
    r.set_env(env);
    assert_eq!(r.lfs_remote().unwrap(), "origin");
    let opts = Default::default();
    let data = r.load_commit(ids[c2], None, &opts).unwrap();
    let dl = LfsDownload::of_view(&data, None).unwrap();
    assert_eq!(dl.include, ["art/far.dat", "gone.dat"]);
    let err = r.lfs_fetch(&dl).unwrap_err().to_string();
    let first = err.lines().next().unwrap();
    assert_eq!(first, "Downloading LFS objects from origin failed — still missing: gone.dat");
    assert!(err.contains("remote missing object"), "git-lfs's reason follows: {err}");
    let local = p.join(".git/lfs/objects");
    for c in [far1, far2] {
        let o = Pointer::of(c).oid;
        assert!(local.join(&o[..2]).join(&o[2..4]).join(&o).is_file(), "downloaded");
    }
    let shown = r.load_commit(ids[c2], None, &opts).unwrap();
    assert_eq!(changes(file(&shown.files, "art/far.dat")), ["-far one", "+far two"]);
    let rest = LfsDownload::of_view(&shown, None).unwrap();
    assert_eq!(rest.include, ["gone.dat"]);
    assert!(sh(&p, &["status", "--porcelain"]).is_empty(), "working tree untouched");
}

/// A global config for `Repo::set_env` (`filters`: git-lfs's filter section).
fn global_config(dir: &Path, filters: bool) -> Vec<(String, String)> {
    let path = dir.join("global.gitconfig");
    let text = if filters {
        "[filter \"lfs\"]\n\tclean = git-lfs clean -- %f\n\tsmudge = git-lfs smudge -- %f\n\
         \tprocess = git-lfs filter-process\n\trequired = true\n"
    } else {
        ""
    };
    fs::write(&path, text).unwrap();
    vec![
        ("GIT_CONFIG_GLOBAL".into(), path.display().to_string()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ]
}

/// The repository for "Track with Git LFS…": committed `.psd` files inside and outside
/// `art/`, plain git (no LFS yet).
fn track_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let mut h = History::new();
    h.commit(
        "main",
        &[],
        "init",
        &[
            ("art/x.psd", f("x\n")),
            ("art/sub/y.psd", f("y\n")),
            ("art/n.txt", f("n\n")),
            ("top.psd", f("t\n")),
        ],
    );
    let (tmp, p, _) = repo_with(&h);
    (tmp, p)
}

/// "Track with Git LFS…" in a folder: the line goes into the folder's `.gitattributes`
/// (staged on request), the committed files it newly matches show as changed, staging one
/// stores the pointer, the hooks are installed, a second time changes nothing.
/// Workflow: LF-02
#[gitti_test::test]
fn track_in_a_folder() {
    use gitti_core::lfs_track::{LfsTrackOptions, NOT_INSTALLED};
    let (tmp, p) = track_repo();
    let mut r = Repo::open(&p).unwrap();
    r.set_env(global_config(tmp.path(), true));
    let opts = LfsTrackOptions {
        stage: true,
        ..Default::default()
    };
    if r.lfs_setup().version.is_none() {
        let err = r.lfs_track("art", "*.psd", opts).unwrap_err();
        assert_eq!(err.to_string(), NOT_INSTALLED);
        println!("note: git-lfs isn't installed — tracking itself is not checked");
        return;
    }
    assert!(r.lfs_setup().filters);
    let out = r.lfs_track("art", "*.psd", opts).unwrap();
    assert_eq!(
        fs::read_to_string(p.join("art/.gitattributes")).unwrap(),
        "*.psd filter=lfs diff=lfs merge=lfs -text\n"
    );
    let mut matching = out.matching.clone();
    matching.sort();
    assert_eq!(matching, ["art/sub/y.psd", "art/x.psd"]);
    assert!(out.added && out.staged && !out.installed, "{out:?}");
    assert_eq!(out.hook_warning, None);
    assert!(p.join(".git/hooks/pre-push").is_file(), "LFS hooks installed");
    assert!(out.message().contains("2 committed file(s) now match"), "{}", out.message());
    let ws = r.working_state().unwrap();
    let unstaged: Vec<&str> = ws.unstaged.iter().map(|w| w.path()).collect();
    assert!(unstaged.contains(&"art/x.psd"), "touched: {unstaged:?}");
    assert!(!unstaged.contains(&"top.psd"), "outside the folder: {unstaged:?}");
    assert!(ws.staged.iter().any(|w| w.path() == "art/.gitattributes"));
    assert!(r.lfs_state().uses);
    r.stage_paths(&["art/x.psd"]).unwrap();
    assert!(sh(&p, &["show", ":art/x.psd"]).starts_with("version https://git-lfs"));
    let again = r.lfs_track("art", "*.psd", opts).unwrap();
    assert!(!again.added && again.matching.is_empty());
    assert!(again.message().contains("already tracks"), "{}", again.message());

    // "Folder art/" also matches the files git reads itself: they stay plain.
    let out = r.lfs_track("", "/art/**", opts).unwrap();
    assert_eq!(out.kept_plain, ["art/.gitattributes"]);
    assert_eq!(
        fs::read_to_string(p.join(".gitattributes")).unwrap(),
        "/art/** filter=lfs diff=lfs merge=lfs -text\n\
         /art/.gitattributes !filter !diff !merge !text\n"
    );
    assert!(out.matching.contains(&"art/n.txt".to_string()), "{out:?}");
    assert!(out.message().contains("kept art/.gitattributes out of LFS"), "{}", out.message());
    let attr = sh(&p, &["check-attr", "filter", "--", "art/.gitattributes", "art/n.txt"]);
    assert_eq!(attr, "art/.gitattributes: filter: unspecified\nart/n.txt: filter: lfs\n");
}

/// Without `git lfs install` (no filters in the user's config) tracking is refused unless
/// the dialog's "set up" is on; then `git lfs install` writes the (test's) global config.
/// Workflow: LF-02
#[gitti_test::test]
fn track_sets_up_git_lfs_only_when_asked() {
    use gitti_core::lfs_track::LfsTrackOptions;
    let (tmp, p) = track_repo();
    let mut r = Repo::open(&p).unwrap();
    r.set_env(global_config(tmp.path(), false));
    let setup = r.lfs_setup();
    if setup.version.is_none() {
        println!("note: git-lfs isn't installed — set-up is not checked");
        return;
    }
    assert!(!setup.filters);
    let err = r
        .lfs_track("", "/top.psd", LfsTrackOptions::default())
        .unwrap_err();
    assert!(err.to_string().contains("not set up"), "{err}");
    assert!(!p.join(".gitattributes").exists(), "nothing written");
    let opts = LfsTrackOptions {
        install: true,
        lockable: true,
        ..Default::default()
    };
    let out = r.lfs_track("", "/top.psd", opts).unwrap();
    assert!(out.installed && !out.staged);
    assert_eq!(out.matching, ["top.psd"]);
    assert!(r.lfs_setup().filters);
    assert_eq!(
        fs::read_to_string(p.join(".gitattributes")).unwrap(),
        "/top.psd filter=lfs diff=lfs merge=lfs -text lockable\n"
    );
}

/// A local bare repository as the LFS remote — git-lfs serves path remotes itself, no
/// server — and the global config with git-lfs's filters (`None`: git-lfs not installed).
/// Locks need a server (a manual check against a Gitea server, not automated).
fn local_remote(base: &Path) -> Option<(Vec<(String, String)>, std::path::PathBuf)> {
    if !gitti_core::lfs::LfsCaps::get().installed() {
        println!("note: git-lfs isn't installed — the LFS remote is not checked");
        return None;
    }
    let bare = base.join("origin.git");
    crate::fx::bare(&bare);
    Some((global_config(base, true), bare))
}

/// Binary content of an LFS file.
fn lfs_content() -> Vec<u8> {
    (0..20_000u32).map(|i| (i * 7 % 251) as u8).collect()
}

/// Real git-lfs (where installed): stage stores the pointer, commit and push — the
/// `pre-push` hook (installed by tracking) uploads into `origin.git/lfs/objects`.
/// Workflow: LF-03
#[gitti_test::test]
fn push_uploads_to_a_local_remote() {
    let tmp = tempfile::tempdir().unwrap();
    let base = gitti_core::paths::canonical(tmp.path());
    let Some((env, bare)) = local_remote(&base) else {
        return;
    };
    let work = base.join("work");
    crate::fixture_kit::init(&work);
    append_config(
        &work,
        &format!(
            "[remote \"origin\"]\n\turl = {}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n\
             [branch \"main\"]\n\tremote = origin\n\tmerge = refs/heads/main\n",
            slash(&bare)
        ),
    );
    let content = lfs_content();
    fs::write(work.join(".gitattributes"), "*.bin filter=lfs diff=lfs merge=lfs -text\n").unwrap();
    fs::write(work.join("x.bin"), &content).unwrap();
    let mut r = Repo::open(&work).unwrap();
    r.set_env(env);
    sh(&work, &["lfs", "update"]); // the hooks, as "Track with Git LFS…" installs them
    r.stage_paths(&[".gitattributes", "x.bin"]).unwrap();
    assert!(sh(&work, &["show", ":x.bin"]).starts_with("version https://git-lfs"));
    r.commit("Add x.bin via LFS", false).unwrap();
    r.push().unwrap();
    let oid = Pointer::of(&content).oid;
    let stored = bare.join("lfs/objects").join(&oid[..2]).join(&oid[2..4]).join(&oid);
    assert_eq!(fs::read(&stored).unwrap(), content, "uploaded on push");
}

/// Real git-lfs (where installed): a clone of a remote holding an LFS file (pointer
/// committed, object in `origin.git/lfs/objects`) checks out the content, not the pointer,
/// and has nothing to commit.
/// Workflow: LF-06
#[gitti_test::test]
fn clone_checks_out_the_content() {
    let tmp = tempfile::tempdir().unwrap();
    let base = gitti_core::paths::canonical(tmp.path());
    let Some((env, bare)) = local_remote(&base) else {
        return;
    };
    let content = lfs_content();
    let pointer = Pointer::of(&content);
    let dir = bare.join("lfs/objects").join(&pointer.oid[..2]).join(&pointer.oid[2..4]);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(&pointer.oid), &content).unwrap();
    let mut i = crate::fx::Import::new();
    let text = pointer.text();
    i.commit(
        "main",
        None,
        &[],
        "lfs",
        &[
            crate::fx::put(".gitattributes", "*.bin filter=lfs diff=lfs merge=lfs -text\n"),
            crate::fx::put("x.bin", &text),
        ],
    );
    i.run(&bare);
    let mut git = gitti_core::git_cli::Git::new(&base);
    git.set_env(env.clone());
    let clone = base.join("clone");
    gitti_core::create::clone_repo(&git, &slash(&bare), &clone, &mut |_| {}).unwrap();
    assert_eq!(fs::read(clone.join("x.bin")).unwrap(), content, "smudged on checkout");
    let mut c = Repo::open(&clone).unwrap();
    c.set_env(env);
    let ws = c.working_state().unwrap();
    assert!(ws.unstaged.is_empty() && ws.staged.is_empty(), "clean clone");
    assert!(c.lfs_state().uses);
}
