//! BG — big repositories and many paths: what still works in one go.

/// **BG-04 — stage a group of more than 200 paths.** [`COUNT`] untracked files
/// `fNNN.txt` (more than one `git add` takes: the apps chunk by 200) beside a modified
/// tracked file `a.txt`. The user stages the Untracked Files group.
///
/// Expected ([`expect`]): every one of the files is staged as a new file; `a.txt` stays
/// unstaged.
pub mod stage_a_big_group {
    use std::path::Path;

    use crate::expect::{Checks, Expect, OpState};
    use crate::fixture::{Fixture, put};

    pub const COUNT: usize = 250;

    /// The untracked file `i` (1-based).
    pub fn name(i: usize) -> String {
        format!("f{i:03}.txt")
    }

    /// `init` (`a.txt`); `a.txt` modified, [`COUNT`] untracked files. Three git
    /// processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put("a.txt", "a\n")]);
        fx.write("a.txt", "a changed\n");
        for i in 1..=COUNT {
            fx.write(&name(i), &format!("{i}\n"));
        }
        fx.build(dir);
    }

    /// After staging the untracked group; one git process.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("BG-04 after staging the big group");
        c.eq("state", e.state(), OpState::Clean);
        let mut want: Vec<String> = (1..=COUNT).map(|i| format!("A  {}", name(i))).collect();
        want.push(" M a.txt".into());
        want.sort();
        let got = e.status_lines();
        if got != want {
            let missing: Vec<&String> = want.iter().filter(|w| !got.contains(w)).take(5).collect();
            let extra: Vec<&String> = got.iter().filter(|g| !want.contains(g)).take(5).collect();
            c.fail(format!("status: {} lines, want {}; missing {missing:?}, unexpected {extra:?}", got.len(), want.len()));
        }
        c.done()
    }
}
