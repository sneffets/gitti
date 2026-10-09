//! What "Add Worktree" will do for the dialog's current input: one branch field
//! (a new branch, an existing local one, or a remote branch like `origin/x`) plus the
//! choice "Detached HEAD (no branch)". [`plan_add`] turns the input into the git call
//! ([`AddMode`]), a sentence for the dialog, the OK button's label and — when it can't
//! work — the reason. Both frontends show it live while the user types, so whether the
//! branch exists is never a separate choice that can contradict the name.

use std::path::Path;

use super::{AddMode, Worktree};
use crate::{RefInfo, RefKind};

/// The Add Worktree dialog's input.
#[derive(Debug, Clone, Copy)]
pub struct AddInput<'a> {
    /// The branch field as typed: a new branch, an existing local branch, or a remote
    /// branch (`origin/x`).
    pub branch: &'a str,
    /// "Detached HEAD (no branch)": `branch` is not used.
    pub detached: bool,
    /// Where a new branch / the detached HEAD starts (a revision for git).
    pub start: &'a str,
    /// The start as the dialog names it (`1a2b3c4 “subject”`).
    pub start_label: &'a str,
    /// The worktree's folder.
    pub path: &'a Path,
    /// The remotes' names: splits `origin/x` into remote and branch even when a remote's
    /// name has a `/` itself (empty: split at the first `/`).
    pub remotes: &'a [String],
}

/// [`plan_add`]'s answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddPlan {
    /// The git call (`None`: nothing sensible to run, e.g. an empty or invalid name).
    pub action: Option<AddMode>,
    /// What will happen, in plain words (or, with nothing to run, what to enter).
    pub explanation: String,
    /// The OK button: "Create" (a new branch) or "Check Out".
    pub ok_label: &'static str,
    /// Why it can't work as entered (the OK button is disabled and says so).
    pub blocked: Option<String>,
    /// Existing branches matching the typed name (completion), best first.
    pub completions: Vec<String>,
}

impl AddPlan {
    /// The git call, when nothing blocks it.
    pub fn runnable(&self) -> Option<&AddMode> {
        self.action.as_ref().filter(|_| self.blocked.is_none())
    }
}

/// OK label for a worktree on a new branch.
pub const OK_CREATE: &str = "Create";
/// OK label for checking out something that exists (a branch, or a commit detached).
pub const OK_CHECK_OUT: &str = "Check Out";

/// How many completions a plan offers at most.
const MAX_COMPLETIONS: usize = 6;

/// What adding a worktree with `input` does, given the repository's `refs` and
/// `worktrees` (a refresh old; git has the last word). Pure apart from looking at the
/// folder (`input.path`: must be new or empty).
pub fn plan_add(refs: &[RefInfo], worktrees: &[Worktree], input: &AddInput) -> AddPlan {
    let at = input.path.display();
    let path_problem = path_problem(input.path);
    if input.detached {
        return AddPlan {
            action: Some(AddMode::Detached(input.start.to_string())),
            explanation: format!(
                "Checks out {} in {at} with a detached HEAD — no branch: commits made there \
                 belong to no branch until you create one.",
                input.start_label
            ),
            ok_label: OK_CHECK_OUT,
            blocked: path_problem,
            completions: Vec::new(),
        };
    }
    let typed = input.branch.trim();
    let completions = completions(refs, typed);
    let local = |name: &str| {
        refs.iter()
            .any(|r| r.kind == RefKind::LocalBranch && r.short_name == name)
    };
    let remote = |name: &str| {
        refs.iter()
            .find(|r| r.kind == RefKind::RemoteBranch && r.short_name == name && !is_remote_head(r))
    };
    let blocked = |explanation: String, why: String, ok_label| AddPlan {
        action: None,
        explanation,
        ok_label,
        blocked: Some(why),
        completions: completions.clone(),
    };
    if typed.is_empty() {
        return blocked(
            String::new(),
            "Enter a branch name — a new one, an existing local branch, or a remote branch \
             like origin/main — or choose Detached HEAD."
                .into(),
            OK_CREATE,
        );
    }
    // An existing local branch: checked out as it is.
    if local(typed) {
        let explanation = format!("Checks out the existing branch {typed} in {at}.");
        let why = worktrees
            .iter()
            .find(|w| w.branch.as_deref() == Some(typed))
            .map(|w| checked_out_problem(typed, w))
            .or(path_problem);
        return AddPlan {
            action: Some(AddMode::Existing(typed.to_string())),
            explanation,
            ok_label: OK_CHECK_OUT,
            blocked: why,
            completions,
        };
    }
    // A remote branch: a new local branch tracking it.
    if let Some(r) = remote(typed) {
        let name = local_part(typed, input.remotes);
        if local(name) {
            return blocked(
                format!("{typed} is a remote branch."),
                format!(
                    "The local branch {name} exists already — type {name} to check it out, \
                     or another name for a new branch."
                ),
                OK_CREATE,
            );
        }
        if let Some(why) = new_branch_problem(refs, name) {
            return blocked(
                format!("{typed} is a remote branch."),
                format!("Its local branch can't be called “{name}”: {why}."),
                OK_CREATE,
            );
        }
        return AddPlan {
            action: Some(AddMode::Track {
                name: name.to_string(),
                upstream: r.short_name.clone(),
            }),
            explanation: format!(
                "{typed} is a remote branch: creates the local branch {name} from it, tracking \
                 {typed} (pull and push there go to it), and checks it out in {at}."
            ),
            ok_label: OK_CREATE,
            blocked: path_problem,
            completions,
        };
    }
    if let Some(why) = new_branch_problem(refs, typed) {
        return blocked(
            String::new(),
            format!("“{typed}” can't be a branch name: {why}."),
            OK_CREATE,
        );
    }
    // A name only remotes have: git's own guess (`worktree add <path> <branch>`) — a local
    // branch tracking the one remote that has it. With several, a new branch (git would
    // need `checkout.defaultRemote` to choose; typing `origin/x` chooses explicitly).
    let on_remotes: Vec<&RefInfo> = refs
        .iter()
        .filter(|r| {
            r.kind == RefKind::RemoteBranch
                && !is_remote_head(r)
                && local_part(&r.short_name, input.remotes) == typed
        })
        .collect();
    if let [r] = on_remotes.as_slice() {
        let up = &r.short_name;
        return AddPlan {
            action: Some(AddMode::Track {
                name: typed.to_string(),
                upstream: up.clone(),
            }),
            explanation: format!(
                "{typed} exists only as the remote branch {up}: creates the local branch \
                 {typed} from it, tracking {up}, and checks it out in {at}."
            ),
            ok_label: OK_CREATE,
            blocked: path_problem,
            completions,
        };
    }
    let mut explanation = format!(
        "Creates the new branch {typed} from {} and checks it out in {at}.",
        input.start_label
    );
    if on_remotes.len() > 1 {
        let names: Vec<&str> = on_remotes.iter().map(|r| r.short_name.as_str()).collect();
        explanation.push_str(&format!(
            " ({} exist — type one of them to track it instead.)",
            names.join(", ")
        ));
    }
    if refs
        .iter()
        .any(|r| r.kind == RefKind::Tag && r.short_name == typed)
    {
        explanation.push_str(&format!(
            " A tag {typed} exists too: where git reads “{typed}” as a revision, it means \
             the tag."
        ));
    }
    AddPlan {
        action: Some(AddMode::NewBranch {
            name: typed.to_string(),
            start: input.start.to_string(),
        }),
        explanation,
        ok_label: OK_CREATE,
        blocked: path_problem,
        completions,
    }
}

/// The local branch a worktree for the typed `branch` gets: `x` for the remote branch
/// `origin/x` (when `refs` has it), else the name as typed — for the folder name.
pub fn local_branch_name<'a>(refs: &[RefInfo], remotes: &[String], branch: &'a str) -> &'a str {
    let typed = branch.trim();
    let is_remote = refs.iter().any(|r| {
        r.kind == RefKind::RemoteBranch && r.short_name == typed && !is_remote_head(r)
    });
    let is_local = refs
        .iter()
        .any(|r| r.kind == RefKind::LocalBranch && r.short_name == typed);
    if is_remote && !is_local {
        local_part(typed, remotes)
    } else {
        typed
    }
}

/// Why `name` can't be a branch name (git's `check-ref-format --branch` rules, in plain
/// words), `None` when it can.
pub fn branch_name_problem(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("it is empty".into());
    }
    if name == "@" {
        return Some("“@” alone means HEAD".into());
    }
    if name == "HEAD" {
        return Some("HEAD is git's name for the current commit".into());
    }
    if name.starts_with('-') {
        return Some("it starts with “-” (git would read it as an option)".into());
    }
    if let Some(c) = name.chars().find(|c| c.is_whitespace() || c.is_control()) {
        return Some(if c == ' ' {
            "it contains a space".into()
        } else {
            "it contains a blank or control character".into()
        });
    }
    if let Some(c) = name.chars().find(|c| matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\')) {
        return Some(format!("it contains “{c}”"));
    }
    if name.contains("..") {
        return Some("it contains “..”".into());
    }
    if name.contains("@{") {
        return Some("it contains “@{”".into());
    }
    if name.starts_with('/') || name.ends_with('/') || name.contains("//") {
        return Some("it starts or ends with “/”, or has “//”".into());
    }
    if name.ends_with('.') {
        return Some("it ends with “.”".into());
    }
    for part in name.split('/') {
        if part.starts_with('.') {
            return Some(format!("“{part}” starts with “.”"));
        }
        if part.ends_with(".lock") {
            return Some(format!("“{part}” ends with “.lock”"));
        }
    }
    None
}

/// Why a *new* local branch can't be called `name`: its syntax, or a clash with an
/// existing branch as a folder (git stores `a/b` under `a`).
fn new_branch_problem(refs: &[RefInfo], name: &str) -> Option<String> {
    if let Some(why) = branch_name_problem(name) {
        return Some(why);
    }
    refs.iter()
        .filter(|r| r.kind == RefKind::LocalBranch)
        .find_map(|r| {
            let other = r.short_name.as_str();
            if name.starts_with(&format!("{other}/")) {
                Some(format!(
                    "the branch {other} exists, so no branch can be named {other}/… (git \
                     stores branches like files and folders)"
                ))
            } else if other.starts_with(&format!("{name}/")) {
                Some(format!(
                    "the branch {other} exists, so {name} can't be a branch too (git stores \
                     branches like files and folders)"
                ))
            } else {
                None
            }
        })
}

/// Why the folder can't take a new worktree (git needs a new or empty one).
pub fn path_problem(path: &Path) -> Option<String> {
    if path.as_os_str().is_empty() {
        return Some("Enter a folder for the worktree.".into());
    }
    if path.is_file() {
        return Some(format!("{} is a file — git needs a new or empty folder.", path.display()));
    }
    if path.exists() && std::fs::read_dir(path).is_ok_and(|mut d| d.next().is_some()) {
        return Some(format!(
            "{} exists and is not empty — git needs a new or empty folder.",
            path.display()
        ));
    }
    None
}

/// `branch` is checked out in `wt`: git refuses it a second time.
fn checked_out_problem(branch: &str, wt: &Worktree) -> String {
    let at = wt.path.display();
    if wt.prunable.is_some() {
        format!(
            "{branch} is still recorded as checked out in the worktree at {at}, whose folder \
             is gone — prune worktrees first."
        )
    } else if wt.is_current {
        format!(
            "{branch} is checked out here already — git keeps a branch in one worktree only. \
             Type a new name to branch off from it."
        )
    } else {
        format!(
            "{branch} is already checked out in the worktree at {at} — git refuses a branch \
             in two worktrees (only --force overrides that, and then both move the same \
             branch). Open that worktree, or type a new name."
        )
    }
}

/// `refs/remotes/<remote>/HEAD` (the remote's default branch pointer, not a branch).
fn is_remote_head(r: &RefInfo) -> bool {
    r.full_name.ends_with("/HEAD")
}

/// `x` of the remote branch `origin/x`: after the longest remote name that prefixes it,
/// else after the first `/`.
fn local_part<'a>(remote_branch: &'a str, remotes: &[String]) -> &'a str {
    remotes
        .iter()
        .filter_map(|r| remote_branch.strip_prefix(r.as_str())?.strip_prefix('/'))
        .min_by_key(|rest| rest.len())
        .or_else(|| remote_branch.split_once('/').map(|(_, b)| b))
        .unwrap_or(remote_branch)
}

/// Existing branches (local first, then remote) whose name contains `typed` (any case),
/// prefix matches first; never `typed` itself; nothing for an empty field.
fn completions(refs: &[RefInfo], typed: &str) -> Vec<String> {
    if typed.is_empty() {
        return Vec::new();
    }
    let needle = typed.to_lowercase();
    let mut hits: Vec<(bool, bool, &str)> = refs
        .iter()
        .filter(|r| {
            matches!(r.kind, RefKind::LocalBranch | RefKind::RemoteBranch)
                && !is_remote_head(r)
                && r.short_name != typed
        })
        .filter_map(|r| {
            let name = r.short_name.to_lowercase();
            let at = name.find(&needle)?;
            // Prefix of the name, or of its part after the remote.
            let prefix = at == 0 || (r.kind == RefKind::RemoteBranch && name[..at].ends_with('/'));
            Some((!prefix, r.kind != RefKind::LocalBranch, r.short_name.as_str()))
        })
        .collect();
    hits.sort();
    hits.dedup_by(|a, b| a.2 == b.2);
    hits.into_iter()
        .take(MAX_COMPLETIONS)
        .map(|(_, _, n)| n.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use gix::ObjectId;

    use super::*;

    fn r(kind: RefKind, short: &str) -> RefInfo {
        let full = match kind {
            RefKind::LocalBranch => format!("refs/heads/{short}"),
            RefKind::RemoteBranch => format!("refs/remotes/{short}"),
            RefKind::Tag => format!("refs/tags/{short}"),
            _ => short.to_string(),
        };
        RefInfo {
            full_name: full,
            short_name: short.into(),
            kind,
            target: ObjectId::null(gix::hash::Kind::Sha1),
        }
    }

    fn refs() -> Vec<RefInfo> {
        vec![
            r(RefKind::LocalBranch, "main"),
            r(RefKind::LocalBranch, "feature"),
            r(RefKind::LocalBranch, "free"),
            r(RefKind::RemoteBranch, "origin/HEAD"),
            r(RefKind::RemoteBranch, "origin/main"),
            r(RefKind::RemoteBranch, "origin/topic"),
            r(RefKind::RemoteBranch, "origin/shared"),
            r(RefKind::RemoteBranch, "up/stream/shared"),
            r(RefKind::RemoteBranch, "up/stream/fix/x"),
            r(RefKind::Tag, "v1"),
        ]
    }

    fn wt(path: &str, branch: Option<&str>, current: bool, gone: bool) -> Worktree {
        Worktree {
            path: PathBuf::from(path),
            head: None,
            branch: branch.map(String::from),
            detached: branch.is_none(),
            bare: false,
            locked: None,
            prunable: gone.then(|| "gone".to_string()),
            is_main: current,
            is_current: current,
            sparse: None,
        }
    }

    fn worktrees() -> Vec<Worktree> {
        vec![
            wt("/src/repo", Some("main"), true, false),
            wt("/src/repo-feature", Some("feature"), false, false),
        ]
    }

    const REMOTES: &[&str] = &["origin", "up/stream"];

    fn plan(branch: &str, detached: bool) -> AddPlan {
        let remotes: Vec<String> = REMOTES.iter().map(|s| s.to_string()).collect();
        plan_add(
            &refs(),
            &worktrees(),
            &AddInput {
                branch,
                detached,
                start: "abc1234",
                start_label: "abc1234 “Fix”",
                path: Path::new("/nonexistent-gitti/repo-x"),
                remotes: &remotes,
            },
        )
    }

    #[gitti_test::test]
    fn new_existing_detached() {
        let p = plan("topic2", false);
        assert_eq!(
            p.action,
            Some(AddMode::NewBranch { name: "topic2".into(), start: "abc1234".into() })
        );
        assert_eq!(
            p.explanation,
            "Creates the new branch topic2 from abc1234 “Fix” and checks it out in \
             /nonexistent-gitti/repo-x."
        );
        assert_eq!((p.ok_label, p.blocked.as_deref()), (OK_CREATE, None));
        assert!(p.runnable().is_some());

        let p = plan("  free ", false);
        assert_eq!(p.action, Some(AddMode::Existing("free".into())));
        assert_eq!(
            p.explanation,
            "Checks out the existing branch free in /nonexistent-gitti/repo-x."
        );
        assert_eq!(p.ok_label, OK_CHECK_OUT);
        assert!(p.blocked.is_none());

        // Detached ignores the name.
        let p = plan("whatever", true);
        assert_eq!(p.action, Some(AddMode::Detached("abc1234".into())));
        assert!(p.explanation.starts_with("Checks out abc1234 “Fix” in /nonexistent-gitti/repo-x with a detached HEAD"));
        assert_eq!(p.ok_label, OK_CHECK_OUT);
        assert!(p.completions.is_empty() && p.blocked.is_none());
    }

    #[gitti_test::test]
    fn checked_out_elsewhere_is_blocked() {
        let p = plan("feature", false);
        assert_eq!(p.action, Some(AddMode::Existing("feature".into())));
        assert!(p.runnable().is_none());
        let why = p.blocked.unwrap();
        assert!(
            why.starts_with("feature is already checked out in the worktree at /src/repo-feature"),
            "{why}"
        );
        assert!(why.contains("--force"), "{why}");
        let why = plan("main", false).blocked.unwrap();
        assert!(why.starts_with("main is checked out here already"), "{why}");
        // Its folder is gone: prune first.
        let gone = vec![wt("/src/repo-f", Some("free"), false, true)];
        let p = plan_add(
            &refs(),
            &gone,
            &AddInput {
                branch: "free",
                detached: false,
                start: "HEAD",
                start_label: "HEAD",
                path: Path::new("/nonexistent-gitti/x"),
                remotes: &[],
            },
        );
        assert!(p.blocked.unwrap().contains("prune worktrees first"));
    }

    #[gitti_test::test]
    fn remote_branches_track() {
        // Typed as a remote branch.
        let p = plan("origin/topic", false);
        assert_eq!(
            p.action,
            Some(AddMode::Track { name: "topic".into(), upstream: "origin/topic".into() })
        );
        assert!(p.explanation.starts_with("origin/topic is a remote branch: creates the local branch topic"));
        assert_eq!((p.ok_label, p.blocked), (OK_CREATE, None));
        // A remote whose name has a slash.
        let p = plan("up/stream/fix/x", false);
        assert_eq!(
            p.action,
            Some(AddMode::Track { name: "fix/x".into(), upstream: "up/stream/fix/x".into() })
        );
        // The local branch exists already: say so.
        let p = plan("origin/main", false);
        assert_eq!(p.action, None);
        assert!(p.blocked.unwrap().starts_with("The local branch main exists already"));
        // Only on one remote: git's guess, tracking it.
        let p = plan("topic", false);
        assert_eq!(
            p.action,
            Some(AddMode::Track { name: "topic".into(), upstream: "origin/topic".into() })
        );
        assert!(p.explanation.starts_with("topic exists only as the remote branch origin/topic"));
        // On two remotes: a new branch, naming both.
        let p = plan("shared", false);
        assert!(matches!(p.action, Some(AddMode::NewBranch { .. })));
        assert!(p.explanation.contains("(origin/shared, up/stream/shared exist"), "{}", p.explanation);
        // origin/HEAD is no branch to track.
        assert!(!matches!(plan("origin/HEAD", false).action, Some(AddMode::Track { .. })));
        assert_eq!(local_branch_name(&refs(), &[], "origin/topic"), "topic");
        assert_eq!(local_branch_name(&refs(), &[], "new"), "new");
    }

    #[gitti_test::test]
    fn invalid_and_empty_names() {
        let p = plan("", false);
        assert_eq!(p.action, None);
        assert!(p.blocked.unwrap().starts_with("Enter a branch name"));
        for (name, why) in [
            ("a b", "it contains a space"),
            ("x..y", "it contains “..”"),
            ("-x", "it starts with “-”"),
            ("a/.b", "“.b” starts with “.”"),
            ("a.lock", "“a.lock” ends with “.lock”"),
            ("x~1", "it contains “~”"),
            ("a/", "it starts or ends with “/”"),
            ("HEAD", "HEAD is git's name"),
            ("feature/sub", "the branch feature exists"),
        ] {
            let p = plan(name, false);
            let got = p.blocked.unwrap_or_default();
            assert!(
                got.starts_with(&format!("“{name}” can't be a branch name: {why}")),
                "{name}: {got}"
            );
            assert_eq!(p.action, None);
        }
        // A tag of the same name: allowed, with a warning.
        let p = plan("v1", false);
        assert!(p.blocked.is_none() && p.explanation.contains("A tag v1 exists too"));
        assert_eq!(branch_name_problem("feature/ok-1.2"), None);
    }

    #[gitti_test::test]
    fn folder_must_be_new_or_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f"), "x").unwrap();
        let input = |path| AddInput {
            branch: "new",
            detached: false,
            start: "HEAD",
            start_label: "HEAD",
            path,
            remotes: &[],
        };
        let p = plan_add(&refs(), &[], &input(dir.path()));
        assert!(p.blocked.unwrap().contains("exists and is not empty"));
        let p = plan_add(&refs(), &[], &input(Path::new("")));
        assert_eq!(p.blocked.as_deref(), Some("Enter a folder for the worktree."));
        let (file, new) = (dir.path().join("f"), dir.path().join("new"));
        let p = plan_add(&refs(), &[], &input(&file));
        assert!(p.blocked.unwrap().contains("is a file"));
        assert!(plan_add(&refs(), &[], &input(&new)).blocked.is_none());
    }

    #[gitti_test::test]
    fn completion_order() {
        assert_eq!(plan("f", false).completions, ["feature", "free", "up/stream/fix/x"]);
        // Remote part prefixes count; local before remote; never the typed name or origin/HEAD.
        assert_eq!(plan("ma", false).completions, ["main", "origin/main"]);
        assert_eq!(plan("main", false).completions, ["origin/main"]);
        assert_eq!(plan("hea", false).completions, Vec::<String>::new());
        assert!(plan("", false).completions.is_empty());
    }
}
