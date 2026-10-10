//! "Edit Commit Message": HEAD is amended with `--only` (staged
//! changes stay staged); an older commit of the current branch is reworded by a
//! non-interactive `rebase -i` — the sequence editor turns its `pick` into `reword`, the
//! editor copies the new message in. So hooks, signing and the user's git config apply as
//! with git itself. Only for commits that are not pushed yet and have no merge commits
//! after them (a rebase would flatten those).

use anyhow::{Context as _, Result, bail};
use gix::ObjectId;

use crate::Repo;

impl Repo {
    /// Why `id`'s message can't be edited here, or `None` if it can.
    pub fn reword_blocker(&self, id: ObjectId) -> Result<Option<String>> {
        let hex = id.to_string();
        let git = self.git();
        let head = git.run(&["rev-parse", "HEAD"])?.stdout_str().trim().to_string();
        if git
            .run(&["merge-base", "--is-ancestor", &hex, "HEAD"])
            .is_err()
        {
            return Ok(Some("the commit is not on the current branch".into()));
        }
        let remotes = git
            .run(&["branch", "-r", "--contains", &hex, "--format=%(refname:short)"])?
            .stdout_str();
        if let Some(remote) = remotes.lines().map(str::trim).find(|l| !l.is_empty()) {
            return Ok(Some(format!(
                "the commit is already pushed ({remote}) — rewriting it would diverge"
            )));
        }
        if hex != head {
            let after = git
                .run(&["rev-list", "--merges", &format!("{hex}..HEAD")])?
                .stdout_str();
            let parents = git.run(&["rev-list", "--parents", "-n", "1", &hex])?.stdout_str();
            let is_merge = parents.split_whitespace().count() > 2;
            if is_merge || !after.trim().is_empty() {
                return Ok(Some(
                    "there are merge commits from it up to HEAD (a rebase would flatten them)"
                        .into(),
                ));
            }
        }
        Ok(None)
    }

    /// Replaces the message of `id` (see the module docs).
    pub fn reword(&self, id: ObjectId, message: &str) -> Result<String> {
        if let Some(why) = self.reword_blocker(id)? {
            bail!("cannot edit the message: {why}");
        }
        let hex = id.to_string();
        let git = self.git();
        let head = git.run(&["rev-parse", "HEAD"])?.stdout_str().trim().to_string();
        if hex == head {
            let out = git.run_stdin(
                &["commit", "--amend", "--only", "--allow-empty", "--cleanup=whitespace", "-F", "-"],
                Some(message.as_bytes()),
            )?;
            return Ok(format!("{}{}", out.stdout_str(), out.stderr).trim().to_string());
        }
        // The message goes through a file in the git dir; git calls `$GIT_EDITOR <file>` via
        // its shell (also on Windows: Git for Windows' sh). Both editors are shell
        // functions of builtins only: no sed / cp, which a minimal Git for Windows
        // doesn't ship.
        let git_dir = git
            .run(&["rev-parse", "--absolute-git-dir"])?
            .stdout_str()
            .trim()
            .to_string();
        let msg_file = std::path::Path::new(&git_dir).join("GITTI_REWORD_MSG");
        std::fs::write(&msg_file, message).context("writing the new message")?;
        let msg_path = msg_file.to_string_lossy().replace('\\', "/");
        let editor = copy_editor(&msg_path);
        let sequence_editor = REWORD_FIRST_PICK;
        let parent = git.run(&["rev-parse", "--verify", "-q", &format!("{hex}^")]);
        let base_owned;
        // The editor's message would be cleaned with `strip` (`#123 fix` lines dropped).
        let mut args = vec!["-c", "commit.cleanup=whitespace", "rebase", "-i", "--autostash"];
        match parent {
            Ok(p) => {
                base_owned = p.stdout_str().trim().to_string();
                args.push(&base_owned);
            }
            Err(_) => args.push("--root"),
        }
        let result = git
            .with_env(&[
                ("GIT_SEQUENCE_EDITOR", sequence_editor),
                ("GIT_EDITOR", &editor),
            ])
            .run(&args);
        let _ = std::fs::remove_file(&msg_file);
        let out = match result {
            Ok(out) => out,
            Err(e) => {
                // Never leave the user in a half-done rebase.
                let _ = git.run(&["rebase", "--abort"]);
                return Err(e.into());
            }
        };
        Ok(format!("{}{}", out.stdout_str(), out.stderr).trim().to_string())
    }
}

/// The sequence editor: the todo's first `pick` (or `p`) — the commit itself — becomes
/// `reword`. A POSIX sh function of builtins only (`read`, `printf`, `case`), so it runs
/// wherever git's shell does, without sed.
const REWORD_FIRST_PICK: &str = "gitti_todo() { t=; d=; \
     while IFS= read -r l || [ -n \"$l\" ]; do \
     case \"$d$l\" in 'pick '*|'p '*) l=\"reword ${l#* }\"; d=1;; esac; \
     t=\"$t$l\n\"; done < \"$1\"; printf '%s' \"$t\" > \"$1\"; }; gitti_todo";

/// The editor: copies the file at `path` over the message git hands it (builtins only, no
/// cp; a missing last newline is added, which `--cleanup=whitespace` drops again).
fn copy_editor(path: &str) -> String {
    let quoted = format!("'{}'", path.replace('\'', r"'\''"));
    format!(
        "gitti_msg() {{ while IFS= read -r l || [ -n \"$l\" ]; do printf '%s\\n' \"$l\"; \
         done < {quoted} > \"$1\"; }}; gitti_msg"
    )
}

/// [`copy_editor`] that copies only once: it empties the file at `path` afterwards and
/// leaves the messages of later calls alone (a `reword` further down a rebase's todo
/// keeps its own). Builtins only (`: >` empties).
pub(crate) fn copy_editor_once(path: &str) -> String {
    let quoted = format!("'{}'", path.replace('\'', r"'\''"));
    format!(
        "gitti_msg() {{ if [ -s {quoted} ]; then while IFS= read -r l || [ -n \"$l\" ]; do \
         printf '%s\\n' \"$l\"; done < {quoted} > \"$1\"; : > {quoted}; fi; }}; gitti_msg"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn editors_use_no_external_tools() {
        for script in [
            REWORD_FIRST_PICK.to_string(),
            copy_editor("/r/.git/GITTI_REWORD_MSG"),
            copy_editor_once("/r/.git/GITTI_CONTINUE_MSG"),
        ] {
            for tool in ["sed", "cp ", "cat ", "mv ", "awk", "perl"] {
                assert!(!script.contains(tool), "{tool} in {script}");
            }
        }
        assert!(copy_editor("/a b/it's").contains(r"'/a b/it'\''s'"));
    }
}
