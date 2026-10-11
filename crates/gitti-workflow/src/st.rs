//! ST — staging hunks, lines and files.

/// **ST-13 — split a change into two commits.** Two files are changed: `a.txt` in two
/// places (two hunks), `b.txt` by two added lines (one hunk). The user stages the first
/// hunk of `a.txt` and only the line [`PICKED_LINE`] of `b.txt`, commits [`FIRST`], then
/// stages the rest and commits [`SECOND`].
///
/// Expected: after the first commit ([`expect_first`]) HEAD holds exactly the staged part,
/// the rest is still unstaged in the working tree; at the end ([`expect`]) two commits on
/// top of `init`, each with exactly its part, and a clean tree.
pub mod split_into_two_commits {
    use std::path::Path;

    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    /// The first commit's message.
    pub const FIRST: &str = "First part";
    /// The second commit's message.
    pub const SECOND: &str = "Second part";
    /// The one line of `b.txt` that goes into the first commit (an added line; the other
    /// added line, [`OTHER_LINE`], waits for the second).
    pub const PICKED_LINE: &str = "b new 1";
    pub const OTHER_LINE: &str = "b new 2";

    /// `a.txt`: 30 lines `a N`; `hunk1` changes line 3, `hunk2` line 25 — far enough apart
    /// for two hunks.
    pub fn a(hunk1: bool, hunk2: bool) -> String {
        (1..=30)
            .map(|i| match i {
                3 if hunk1 => "a 3 changed\n".to_string(),
                25 if hunk2 => "a 25 changed\n".to_string(),
                _ => format!("a {i}\n"),
            })
            .collect()
    }

    /// `b.txt`: 8 lines `b N`; [`PICKED_LINE`] / [`OTHER_LINE`] added after line 4.
    pub fn b(picked: bool, other: bool) -> String {
        let mut s = String::new();
        for i in 1..=8 {
            s.push_str(&format!("b {i}\n"));
            if i == 4 {
                if picked {
                    s.push_str(&format!("{PICKED_LINE}\n"));
                }
                if other {
                    s.push_str(&format!("{OTHER_LINE}\n"));
                }
            }
        }
        s
    }

    /// `main` with `init` (`a.txt`, `b.txt`); both files changed in the working tree,
    /// nothing staged. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", &a(false, false)), put("b.txt", &b(false, false))]);
        fx.write("a.txt", &a(true, true));
        fx.write("b.txt", &b(true, true));
        fx.build(dir);
    }

    /// The state after the first commit as a starting point (for a driver that splits
    /// the workflow to stay within the time limit): [`FIRST`] committed with its part,
    /// the rest unstaged in the working tree — what [`expect_first`] checks. Three git
    /// processes.
    pub fn fixture_first_committed(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", &a(false, false)), put("b.txt", &b(false, false))]);
        fx.commit("main", FIRST, &[put("a.txt", &a(true, false)), put("b.txt", &b(true, false))]);
        fx.write("a.txt", &a(true, true));
        fx.write("b.txt", &b(true, true));
        fx.build(dir);
    }

    /// After the first commit: HEAD is [`FIRST`] on `init` with the first hunk of `a.txt`
    /// and [`PICKED_LINE`]; the index equals HEAD; the working tree still has everything,
    /// so the rest shows as unstaged. Three git processes.
    pub fn expect_first(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("ST-13 after the first commit");
        c.eq("state", e.state(), OpState::Clean);
        let log = e.log("HEAD", 3);
        c.eq("subjects", log.iter().map(|l| l.subject()).collect(), vec![FIRST, "init"]);
        let blobs = e.blobs(&["HEAD:a.txt", "HEAD:b.txt", ":a.txt", ":b.txt"]);
        c.text("HEAD:a.txt", blobs[0].as_deref(), Some(&a(true, false)));
        c.text("HEAD:b.txt", blobs[1].as_deref(), Some(&b(true, false)));
        c.text("index a.txt", blobs[2].as_deref(), Some(&a(true, false)));
        c.text("index b.txt", blobs[3].as_deref(), Some(&b(true, false)));
        c.text("a.txt on disk", e.file("a.txt").as_deref(), Some(&a(true, true)));
        c.text("b.txt on disk", e.file("b.txt").as_deref(), Some(&b(true, true)));
        c.eq("status", e.status_lines(), vec![" M a.txt".to_string(), " M b.txt".to_string()]);
        c.done()
    }

    /// At the end: [`SECOND`] on [`FIRST`] on `init`, a straight line; the first commit
    /// unchanged, the second with the rest; nothing left to commit. Three git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("ST-13 at the end");
        c.eq("state", e.state(), OpState::Clean);
        c.eq("branch", e.branch().as_deref(), Some("main"));
        let log = e.log("HEAD", 4);
        c.eq("subjects", log.iter().map(|l| l.subject()).collect(), vec![SECOND, FIRST, "init"]);
        c.ok("one parent each", log.iter().all(|l| l.parents.len() <= 1), &log);
        let blobs = e.blobs(&["HEAD~1:a.txt", "HEAD~1:b.txt", "HEAD:a.txt", "HEAD:b.txt"]);
        c.text("first a.txt", blobs[0].as_deref(), Some(&a(true, false)));
        c.text("first b.txt", blobs[1].as_deref(), Some(&b(true, false)));
        c.text("second a.txt", blobs[2].as_deref(), Some(&a(true, true)));
        c.text("second b.txt", blobs[3].as_deref(), Some(&b(true, true)));
        c.eq("status", e.status_lines(), Vec::<String>::new());
        c.done()
    }
}

// ------------------------------------------------------------------ round trips (P2)

use crate::expect::{Checks, Expect};

/// `n` lines `{prefix} {i}` (1-based); `edit(i)` replaces line `i` with its lines (none =
/// the line deleted, several = lines inserted after it).
pub(crate) fn numbered(prefix: &str, n: usize, edit: impl Fn(usize) -> Option<Vec<String>>) -> String {
    let mut s = String::new();
    for i in 1..=n {
        match edit(i) {
            Some(lines) => {
                for l in lines {
                    s.push_str(&l);
                    s.push('\n');
                }
            }
            None => s.push_str(&format!("{prefix} {i}\n")),
        }
    }
    s
}

/// HEAD's, the index's (stage 0) and the working tree's version of `path` against the
/// expected ones (`None` = not there); one git process.
pub(crate) fn versions(
    c: &mut Checks,
    e: &Expect,
    path: &str,
    head: Option<&str>,
    index: Option<&str>,
    disk: Option<&str>,
) {
    let blobs = e.blobs(&[&format!("HEAD:{path}"), &format!(":{path}")]);
    c.text(&format!("HEAD:{path}"), blobs[0].as_deref(), head);
    c.text(&format!("index {path}"), blobs[1].as_deref(), index);
    c.text(&format!("{path} on disk"), e.file(path).as_deref(), disk);
}

/// The status as sorted `XY path` lines.
pub(crate) fn status(lines: &[&str]) -> Vec<String> {
    let mut l: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    l.sort();
    l
}

/// **ST-03 — unstage lines while another hunk stays staged.** `a.txt` is staged in two
/// hunks: line 3 changed, and [`NEW1`] / [`NEW2`] added after line 25; the working tree
/// equals the index (nothing unstaged). The user selects [`NEW2`] in the staged diff's
/// second hunk and unstages it.
///
/// Expected ([`expect`]): the first hunk and [`NEW1`] stay in the index, only [`NEW2`]
/// left it; the working tree is untouched, so [`NEW2`] shows as unstaged.
pub mod unstage_lines_beside_a_staged_hunk {
    use std::path::Path;

    use super::{numbered, status, versions};
    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    pub const NEW1: &str = "new 1";
    pub const NEW2: &str = "new 2";

    /// `a.txt`: 30 lines `a N`; `hunk1` changes line 3, `new1` / `new2` add [`NEW1`] /
    /// [`NEW2`] after line 25.
    pub fn a(hunk1: bool, new1: bool, new2: bool) -> String {
        numbered("a", 30, |i| match i {
            3 if hunk1 => Some(vec!["a 3 changed".into()]),
            25 => {
                let mut l = vec!["a 25".to_string()];
                l.extend(new1.then(|| NEW1.to_string()));
                l.extend(new2.then(|| NEW2.to_string()));
                Some(l)
            }
            _ => None,
        })
    }

    /// `init` (`a.txt`); both hunks staged, the working tree equal to the index. Three
    /// git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", &a(false, false, false))]);
        fx.stage(&[put("a.txt", &a(true, true, true))]);
        fx.build(dir);
    }

    /// After unstaging [`NEW2`]; two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("ST-03 after unstaging a line");
        c.eq("state", e.state(), OpState::Clean);
        let (head, index, disk) = (a(false, false, false), a(true, true, false), a(true, true, true));
        versions(&mut c, &e, "a.txt", Some(&head), Some(&index), Some(&disk));
        c.eq("status", e.status_lines(), status(&["MM a.txt"]));
        c.done()
    }
}

/// **ST-04 — partial stage of a new file.** `new.txt` (six lines `n N`) is untracked. The
/// user selects its lines [`PICKED`] and stages them.
///
/// Expected ([`expect`]): the index has `new.txt` with only those two lines; the file on
/// disk is unchanged, the rest of it shows as unstaged.
pub mod partial_stage_of_a_new_file {
    use std::path::Path;

    use super::{numbered, status, versions};
    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    /// The lines staged: the second and third.
    pub const PICKED: [&str; 2] = ["n 2", "n 3"];

    pub fn new_txt() -> String {
        numbered("n", 6, |_| None)
    }

    /// `init` (`keep.txt`); `new.txt` untracked. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("keep.txt", "keep\n")]);
        fx.write("new.txt", &new_txt());
        fx.build(dir);
    }

    /// After staging [`PICKED`]; two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("ST-04 after staging two lines of a new file");
        c.eq("state", e.state(), OpState::Clean);
        let index = format!("{}\n{}\n", PICKED[0], PICKED[1]);
        versions(&mut c, &e, "new.txt", None, Some(&index), Some(&new_txt()));
        c.eq("status", e.status_lines(), status(&["AM new.txt"]));
        c.done()
    }
}

/// **ST-05 — partial unstage of an added file.** `new.txt` (six lines `n N`) is staged as
/// a new file, the working tree equal to the index. The user selects its lines
/// [`DROPPED`] in the staged diff and unstages them.
///
/// Expected ([`expect`]): `new.txt` stays added in the index, without those two lines; the
/// file on disk is unchanged.
pub mod partial_unstage_of_an_added_file {
    use std::path::Path;

    use super::{numbered, status, versions};
    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    /// The lines unstaged: the second and third.
    pub const DROPPED: [&str; 2] = ["n 2", "n 3"];

    pub fn new_txt() -> String {
        numbered("n", 6, |_| None)
    }

    /// `init` (`keep.txt`); `new.txt` staged (index = working tree). Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("keep.txt", "keep\n")]);
        fx.stage(&[put("new.txt", &new_txt())]);
        fx.build(dir);
    }

    /// After unstaging [`DROPPED`]; two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("ST-05 after unstaging two lines of an added file");
        c.eq("state", e.state(), OpState::Clean);
        let index = numbered("n", 6, |i| (i == 2 || i == 3).then(Vec::new));
        versions(&mut c, &e, "new.txt", None, Some(&index), Some(&new_txt()));
        c.eq("status", e.status_lines(), status(&["AM new.txt"]));
        c.done()
    }
}

/// **ST-06 — partial stage of a deletion.** `gone.txt` (six lines `g N`) is deleted in
/// the working tree. The user selects its removed lines [`PICKED`] and stages them.
///
/// Expected ([`expect`]): the index keeps `gone.txt` without those two lines — a
/// modification, not a deletion; the file stays deleted on disk.
pub mod partial_stage_of_a_deletion {
    use std::path::Path;

    use super::{numbered, status, versions};
    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    /// The removed lines staged: the second and third.
    pub const PICKED: [&str; 2] = ["g 2", "g 3"];

    pub fn gone_txt() -> String {
        numbered("g", 6, |_| None)
    }

    /// `init` (`gone.txt`, `keep.txt`); `gone.txt` deleted. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("gone.txt", &gone_txt()), put("keep.txt", "keep\n")]);
        fx.remove("gone.txt");
        fx.build(dir);
    }

    /// After staging [`PICKED`]; two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("ST-06 after staging two removed lines");
        c.eq("state", e.state(), OpState::Clean);
        let index = numbered("g", 6, |i| (i == 2 || i == 3).then(Vec::new));
        versions(&mut c, &e, "gone.txt", Some(&gone_txt()), Some(&index), None);
        c.eq("status", e.status_lines(), status(&["MD gone.txt"]));
        c.done()
    }
}

/// **ST-08 — Stage All, then Unstage All.** HEAD has `a.txt`, `b.txt`, `d.txt`; `a.txt`
/// has a staged change and another one on top in the working tree, `b.txt` is modified,
/// `d.txt` deleted, `new.txt` untracked. The user stages all, then unstages all.
///
/// Expected: after Stage All ([`expect_staged`]) the index equals the working tree —
/// every change staged, the deletion and the new file too; after Unstage All
/// ([`expect_unstaged`]) the index equals HEAD again and the working tree is still the
/// same — nothing lost, everything unstaged.
pub mod stage_all_then_unstage_all {
    use std::path::Path;

    use super::status;
    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    pub const A_STAGED: &str = "a staged\n";
    pub const A_WORK: &str = "a staged\nand more\n";
    pub const B_WORK: &str = "b changed\n";
    pub const NEW: &str = "new\n";

    /// `init` (`a.txt`, `b.txt`, `d.txt`); the changes above. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", "a\n"), put("b.txt", "b\n"), put("d.txt", "d\n")]);
        fx.stage(&[put("a.txt", A_STAGED)]);
        fx.write("a.txt", A_WORK);
        fx.write("b.txt", B_WORK);
        fx.remove("d.txt");
        fx.write("new.txt", NEW);
        fx.build(dir);
    }

    fn files(c: &mut Checks, e: &Expect) {
        c.text("a.txt on disk", e.file("a.txt").as_deref(), Some(A_WORK));
        c.text("b.txt on disk", e.file("b.txt").as_deref(), Some(B_WORK));
        c.text("d.txt on disk", e.file("d.txt").as_deref(), None);
        c.text("new.txt on disk", e.file("new.txt").as_deref(), Some(NEW));
    }

    /// After Stage All; two git processes.
    pub fn expect_staged(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("ST-08 after Stage All");
        c.eq("state", e.state(), OpState::Clean);
        let blobs = e.blobs(&[":a.txt", ":b.txt", ":d.txt", ":new.txt"]);
        c.text("index a.txt", blobs[0].as_deref(), Some(A_WORK));
        c.text("index b.txt", blobs[1].as_deref(), Some(B_WORK));
        c.text("index d.txt", blobs[2].as_deref(), None);
        c.text("index new.txt", blobs[3].as_deref(), Some(NEW));
        files(&mut c, &e);
        c.eq("status", e.status_lines(), status(&["M  a.txt", "M  b.txt", "D  d.txt", "A  new.txt"]));
        c.done()
    }

    /// After Unstage All; two git processes.
    pub fn expect_unstaged(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("ST-08 after Unstage All");
        c.eq("state", e.state(), OpState::Clean);
        let blobs = e.blobs(&[":a.txt", ":b.txt", ":d.txt", ":new.txt"]);
        c.text("index a.txt", blobs[0].as_deref(), Some("a\n"));
        c.text("index b.txt", blobs[1].as_deref(), Some("b\n"));
        c.text("index d.txt", blobs[2].as_deref(), Some("d\n"));
        c.text("index new.txt", blobs[3].as_deref(), None);
        files(&mut c, &e);
        c.eq("status", e.status_lines(), status(&[" M a.txt", " M b.txt", " D d.txt", "?? new.txt"]));
        c.done()
    }
}

/// **ST-09 — stage a group.** The unstaged section has both groups: tracked changes
/// (`a.txt` modified, `d.txt` deleted) and untracked files ([`UNTRACKED`]). The user
/// stages the Untracked Files group.
///
/// Expected ([`expect`]): exactly the untracked files are staged as new files; the
/// tracked group's changes stay unstaged. (DC-07 starts from the same fixture.)
pub mod stage_a_group {
    use std::path::Path;

    use super::status;
    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    /// The untracked group's files (one in a folder: listed by file, not as the folder).
    pub const UNTRACKED: [&str; 2] = ["new1.txt", "sub/new2.txt"];
    pub const A_WORK: &str = "a changed\n";

    /// `init` (`a.txt`, `d.txt`); `a.txt` modified, `d.txt` deleted, [`UNTRACKED`]
    /// written. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", "a\n"), put("d.txt", "d\n")]);
        fx.write("a.txt", A_WORK);
        fx.remove("d.txt");
        for p in UNTRACKED {
            fx.write(p, &format!("{p}\n"));
        }
        fx.build(dir);
    }

    /// After staging the Untracked Files group; two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("ST-09 after staging the untracked group");
        c.eq("state", e.state(), OpState::Clean);
        let blobs = e.blobs(&[":new1.txt", ":sub/new2.txt", ":a.txt", ":d.txt"]);
        c.text("index new1.txt", blobs[0].as_deref(), Some("new1.txt\n"));
        c.text("index sub/new2.txt", blobs[1].as_deref(), Some("sub/new2.txt\n"));
        c.text("index a.txt", blobs[2].as_deref(), Some("a\n"));
        c.text("index d.txt", blobs[3].as_deref(), Some("d\n"));
        c.eq(
            "status",
            e.status_lines(),
            status(&[" M a.txt", " D d.txt", "A  new1.txt", "A  sub/new2.txt"]),
        );
        c.done()
    }
}

/// **ST-10 — stage with ignore-whitespace.** In `a.txt` (20 lines `line N`) line 3 is
/// changed and line 5 got trailing blanks — one hunk. With whitespace ignored the view
/// shows only the change of line 3; the user stages that hunk.
///
/// Expected ([`expect`]): only the visible change is staged; the whitespace change stays
/// unstaged in the working tree (the selection is mapped onto the exact diff).
pub mod stage_ignoring_whitespace {
    use std::path::Path;

    use super::{numbered, status, versions};
    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    pub fn a(three: bool, blanks: bool) -> String {
        numbered("line", 20, |i| match i {
            3 if three => Some(vec!["line THREE".into()]),
            5 if blanks => Some(vec!["line 5  ".into()]),
            _ => None,
        })
    }

    /// `init` (`a.txt`); both changes in the working tree. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", &a(false, false))]);
        fx.write("a.txt", &a(true, true));
        fx.build(dir);
    }

    /// After staging the shown hunk; two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("ST-10 after staging with whitespace ignored");
        c.eq("state", e.state(), OpState::Clean);
        versions(&mut c, &e, "a.txt", Some(&a(false, false)), Some(&a(true, false)), Some(&a(true, true)));
        c.eq("status", e.status_lines(), status(&["MM a.txt"]));
        c.done()
    }
}

/// **ST-11 — stage with expanded context.** In `a.txt` (60 lines `line N`) line 30 is
/// changed to [`THIRTY`] and [`ADDED`] is added after line 31 — one hunk in the middle.
/// The user shows the whole file, selects from a revealed unchanged line above the hunk
/// down to [`THIRTY`] and stages the selection.
///
/// Expected ([`expect`]): the patch is still exact — only the change of line 30 is in the
/// index, [`ADDED`] stays unstaged.
pub mod stage_with_expanded_context {
    use std::path::Path;

    use super::{numbered, status, versions};
    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    pub const THIRTY: &str = "line thirty";
    pub const ADDED: &str = "line 31b";

    pub fn a(thirty: bool, added: bool) -> String {
        numbered("line", 60, |i| match i {
            30 if thirty => Some(vec![THIRTY.into()]),
            31 if added => Some(vec!["line 31".into(), ADDED.into()]),
            _ => None,
        })
    }

    /// `init` (`a.txt`); both changes in the working tree. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", &a(false, false))]);
        fx.write("a.txt", &a(true, true));
        fx.build(dir);
    }

    /// After staging the selection; two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("ST-11 after staging with the whole file shown");
        c.eq("state", e.state(), OpState::Clean);
        versions(&mut c, &e, "a.txt", Some(&a(false, false)), Some(&a(true, false)), Some(&a(true, true)));
        c.eq("status", e.status_lines(), status(&["MM a.txt"]));
        c.done()
    }
}

/// **ST-12 — stage with minimal context.** In `a.txt` (20 lines `line N`) lines 5 and 9
/// are changed: one hunk with git's default three context lines, two with the fewest (the
/// setting 0; the working directory's diff keeps one context line, which a partial patch
/// needs to apply). The user sets the context lines to 0 and stages the first hunk.
///
/// Expected ([`expect`]): the small hunk applies — only the change of line 5 is in the
/// index.
pub mod stage_with_minimal_context {
    use std::path::Path;

    use super::{numbered, status, versions};
    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    pub fn a(five: bool, nine: bool) -> String {
        numbered("line", 20, |i| match i {
            5 if five => Some(vec!["line FIVE".into()]),
            9 if nine => Some(vec!["line NINE".into()]),
            _ => None,
        })
    }

    /// `init` (`a.txt`); both changes in the working tree. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", &a(false, false))]);
        fx.write("a.txt", &a(true, true));
        fx.build(dir);
    }

    /// After staging the first zero-context hunk; two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("ST-12 after staging with no context lines");
        c.eq("state", e.state(), OpState::Clean);
        versions(&mut c, &e, "a.txt", Some(&a(false, false)), Some(&a(true, false)), Some(&a(true, true)));
        c.eq("status", e.status_lines(), status(&["MM a.txt"]));
        c.done()
    }
}

/// **ST-19 — staging under a hostile global config.** The user's global git config sets
/// everything that changes the diff output git prints ([`hostile_config`]: no prefixes,
/// mnemonic prefixes, forced colour, a textconv driver for every file, an external diff,
/// relative paths, other context, `apply.whitespace=fix`, …). `a.txt` (30 lines `a N`) is
/// changed in two places (lines 3 and 25), `b.txt` (8 lines `b N`) got [`B_PICKED`] and
/// [`B_OTHER`] after line 4. The user stages the first hunk of `a.txt` and the line
/// [`B_PICKED`], then discards the second hunk of `a.txt`.
///
/// Expected ([`expect`]): exactly as without that config — the index has the first hunk of
/// `a.txt` and [`B_PICKED`]; the second hunk of `a.txt` is gone from the working tree,
/// `b.txt` on disk unchanged.
pub mod hostile_config {
    use std::path::Path;

    use super::{numbered, status, versions};
    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    pub const B_PICKED: &str = "b new 1";
    pub const B_OTHER: &str = "b new 2";

    pub fn a(hunk1: bool, hunk2: bool) -> String {
        numbered("a", 30, |i| match i {
            3 if hunk1 => Some(vec!["a 3 changed".into()]),
            25 if hunk2 => Some(vec!["a 25 changed".into()]),
            _ => None,
        })
    }

    pub fn b(picked: bool, other: bool) -> String {
        numbered("b", 8, |i| {
            (i == 4).then(|| {
                let mut l = vec!["b 4".to_string()];
                l.extend(picked.then(|| B_PICKED.to_string()));
                l.extend(other.then(|| B_OTHER.to_string()));
                l
            })
        })
    }

    /// A global git config that changes every part of git's diff output a parser or a
    /// patch builder could trip over, with `attributes` (an attributes file it names,
    /// written next to it by [`write_hostile_config`]) giving every file a textconv
    /// driver.
    pub fn hostile_config(attributes: &Path) -> String {
        let path = attributes.display().to_string().replace('\\', "/");
        format!(
            "[core]\n\tattributesFile = \"{path}\"\n\tquotepath = true\n\
             [diff]\n\tnoprefix = true\n\tmnemonicPrefix = true\n\trelative = true\n\
             \tsuppressBlankEmpty = true\n\tsrcPrefix = SRC/\n\tdstPrefix = DST/\n\
             \texternal = echo EXTERNAL DIFF\n\trenames = copies\n\tcontext = 9\n\
             \tinterHunkContext = 0\n\
             [diff \"hostile\"]\n\ttextconv = sed -e 's/^/TEXTCONV /'\n\
             [color]\n\tui = always\n\tdiff = always\n\tstatus = always\n\
             [log]\n\tshowSignature = true\n\tdecorate = full\n\
             [format]\n\tpretty = oneline\n\
             [commit]\n\tcleanup = strip\n\tverbose = true\n\
             [apply]\n\twhitespace = fix\n\tignoreWhitespace = change\n\
             [status]\n\tshort = true\n\tbranch = true\n"
        )
    }

    /// Writes [`hostile_config`] and its attributes file into `dir` (created); the
    /// config's path, for `GIT_CONFIG_GLOBAL`.
    pub fn write_hostile_config(dir: &Path) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let attributes = dir.join("hostile-attributes");
        std::fs::write(&attributes, "* diff=hostile\n").unwrap();
        let config = dir.join("hostile.gitconfig");
        std::fs::write(&config, hostile_config(&attributes)).unwrap();
        config
    }

    /// `init` (`a.txt`, `b.txt`); the changes above in the working tree. Three git
    /// processes (the fixture's git never reads the global config).
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", &a(false, false)), put("b.txt", &b(false, false))]);
        fx.write("a.txt", &a(true, true));
        fx.write("b.txt", &b(true, true));
        fx.build(dir);
    }

    /// After staging and discarding; three git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("ST-19 under a hostile global config");
        c.eq("state", e.state(), OpState::Clean);
        versions(&mut c, &e, "a.txt", Some(&a(false, false)), Some(&a(true, false)), Some(&a(true, false)));
        versions(&mut c, &e, "b.txt", Some(&b(false, false)), Some(&b(true, false)), Some(&b(true, true)));
        c.eq("status", e.status_lines(), status(&["M  a.txt", "MM b.txt"]));
        c.done()
    }
}
