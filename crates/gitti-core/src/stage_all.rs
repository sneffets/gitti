//! Stage All (the button, menus, key and palette of both apps) stages what the file list
//! shows — and asks first when that would mark conflicted files resolved that still
//! contain conflict markers.
//!
//! - **Hidden untracked files**: with `status.showUntrackedFiles=no` the list shows no
//!   untracked files, but `git add -A` would add them all the same. Stage All then stages
//!   the listed changes only (`git add -u`) and says that the hidden files stayed out.
//!   ("Stage All and Commit" of the nothing-staged question says it takes them, and
//!   does.)
//! - **Conflict markers**: `git add` of a conflicted file marks it resolved, markers and
//!   all; single-file Mark Resolved asks first, so does Stage All
//!   ([`Repo::with_conflict_markers`], [`markers_prompt`]).

use anyhow::Result;

use crate::Repo;
use crate::status::UntrackedFiles;

/// The status line after Stage All with untracked files hidden by the git config.
pub const HIDDEN_UNTRACKED_NOTE: &str = "Staged the listed changes. Untracked files were \
    left out: your git config hides them (status.showUntrackedFiles=no), so they are not \
    shown here.";

/// The question Stage All asks when conflicted files still contain conflict markers:
/// (title, detail, button).
pub fn markers_prompt(paths: &[String]) -> (String, String, &'static str) {
    let title = match paths {
        [one] => format!("{one} still contains conflict markers"),
        many => format!("{} conflicted files still contain conflict markers", many.len()),
    };
    let mut list: String = paths.iter().take(8).map(|p| format!("    {p}\n")).collect();
    if paths.len() > 8 {
        list.push_str(&format!("    … and {} more\n", paths.len() - 8));
    }
    let (it, them) = if paths.len() == 1 { ("it", "it") } else { ("them", "they") };
    let detail = format!(
        "{list}\nStaging marks {it} as resolved with the <<<<<<< / >>>>>>> lines in {it}: \
         committing then would put the markers into the commit. Resolve {it} first (merge \
         editor, take ours / theirs), or stage all anyway if {them} really should contain \
         these lines."
    );
    (title, detail, "Stage All Anyway")
}

impl Repo {
    /// Those of `paths` (conflicted files) whose working-tree file still has conflict
    /// markers — of their `conflict-marker-size` (one `git check-attr` for all).
    pub fn with_conflict_markers(&self, paths: &[&str]) -> Vec<String> {
        if paths.is_empty() {
            return Vec::new();
        }
        let mut input = Vec::new();
        for p in paths {
            input.extend_from_slice(p.as_bytes());
            input.push(0);
        }
        let sizes = self
            .git()
            .run_stdin(&["check-attr", "-z", "--stdin", "conflict-marker-size"], Some(&input))
            .map(|o| marker_sizes(&o.stdout))
            .unwrap_or_default();
        paths
            .iter()
            .filter(|p| {
                let Ok(bytes) = std::fs::read(crate::paths::join_repo_path(self.workdir(), p)) else {
                    return false;
                };
                let size = sizes
                    .iter()
                    .find(|(q, _)| q == *p)
                    .map_or(crate::merge::DEFAULT_MARKER_SIZE, |(_, s)| *s);
                crate::merge::has_conflict_markers_sized(&String::from_utf8_lossy(&bytes), size)
            })
            .map(|p| p.to_string())
            .collect()
    }

    /// Stage All of what the list shows: `git add -A`, or `git add -u` (the listed, tracked
    /// changes) while `untracked` hides untracked files — then with
    /// [`HIDDEN_UNTRACKED_NOTE`] for the status line.
    pub fn stage_all_shown(&self, untracked: UntrackedFiles) -> Result<Option<String>> {
        if untracked == UntrackedFiles::No {
            self.git().run(&["add", "-u", "--", ":/"])?;
            return Ok(Some(HIDDEN_UNTRACKED_NOTE.to_string()));
        }
        self.stage_all()?;
        Ok(None)
    }
}

/// `git check-attr -z` output for several paths: (path, marker size).
fn marker_sizes(out: &[u8]) -> Vec<(String, usize)> {
    let fields: Vec<&[u8]> = out.split(|b| *b == 0).collect();
    fields
        .chunks(3)
        .filter(|c| c.len() == 3)
        .map(|c| {
            let size = std::str::from_utf8(c[2])
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|n| *n > 0)
                .unwrap_or(crate::merge::DEFAULT_MARKER_SIZE);
            (String::from_utf8_lossy(c[0]).into_owned(), size)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn sizes_of_several_paths() {
        let out = b"a.txt\0conflict-marker-size\0unspecified\0b.txt\0conflict-marker-size\0\
                    12\0";
        assert_eq!(marker_sizes(out), [("a.txt".to_string(), 7), ("b.txt".to_string(), 12)]);
    }

    #[gitti_test::test]
    fn the_question_names_the_files() {
        let (title, detail, ok) = markers_prompt(&["a.txt".into()]);
        assert_eq!(title, "a.txt still contains conflict markers");
        assert!(detail.contains("Staging marks it as resolved"), "{detail}");
        assert_eq!(ok, "Stage All Anyway");
        let (title, ..) = markers_prompt(&["a".into(), "b".into()]);
        assert_eq!(title, "2 conflicted files still contain conflict markers");
    }
}
