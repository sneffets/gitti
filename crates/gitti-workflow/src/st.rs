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
