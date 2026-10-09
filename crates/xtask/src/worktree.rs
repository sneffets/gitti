//! `cargo xtask worktree`: a pool of long-lived worktrees ("slots") for parallel agents.
//!
//! A fresh worktree costs a full cold build (~7 min); a slot keeps its `target/`, so a new
//! package in it only compiles what changed since its last use. Slots are named
//! `gitti-agent-N` and live in the machine's worktree root (see `config.rs`; default
//! `<main repo>/.claude/worktrees`). A slot is busy while a branch is checked out in it and
//! free when parked (detached) on main. When the workspace is a subfolder of a bigger
//! repository, a new slot is sparse: only gitti's folder is checked out.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{self, DEFAULT_SLOTS};
use crate::{Failure, Result};

const BASE: &str = "main";
const SLOT_PREFIX: &str = "gitti-agent-";
const BRANCH_PREFIX: &str = "gitti-";

pub fn run(args: &[String]) -> Result {
    let repo = Repo::locate()?;
    match (args.first().map(String::as_str), args.get(1), args.len()) {
        (Some("add"), Some(name), 2) => add(&repo, name),
        (Some("done"), Some(name), 2) => done(&repo, name),
        (Some("drop"), Some(which), 2) => drop_slots(&repo, which),
        (Some("list"), None, 1) => list(&repo),
        (Some("prepare"), None, 1) => prepare(&repo, true),
        (Some("prepare"), Some(flag), 2) if flag == "--no-build" => prepare(&repo, false),
        (Some("root"), None, 1) => {
            println!("{}", repo.root()?.display());
            Ok(())
        }
        _ => Err(Failure::usage(
            "cargo xtask worktree add <name> | done <name> | list | prepare [--no-build] | drop <N|all> | root",
        )),
    }
}

struct Repo {
    /// The main worktree (where the common git dir lives).
    main: PathBuf,
    /// gitti's folder relative to the repository root ("" when gitti is the repository).
    rel: String,
    config: config::Config,
}

#[derive(Debug, PartialEq)]
enum Slot {
    Missing,
    Free,
    Busy(String),
}

struct Worktree {
    path: PathBuf,
    branch: Option<String>,
}

impl Repo {
    fn locate() -> Result<Repo> {
        let ws = crate::workspace_root();
        let common = git(
            &ws,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        let main = Path::new(&common)
            .parent()
            .ok_or_else(|| Failure::new(format!("odd git dir {common}")))?
            .to_path_buf();
        let rel = git(&ws, &["rev-parse", "--show-prefix"])?
            .trim_end_matches('/')
            .to_string();
        Ok(Repo {
            main,
            rel,
            config: config::load()?,
        })
    }

    /// The configured root must exist (an unplugged drive → exit 3: ask the user); the
    /// default one is created.
    fn root(&self) -> Result<PathBuf> {
        match &self.config.worktree_root {
            Some(root) if root.is_dir() => Ok(root.clone()),
            Some(root) => Err(Failure {
                code: 3,
                message: format!(
                    "the worktree root {} is not available (drive not mounted?) — ask the user: \
                     plug it in, or use another place for this call with GITTI_WORKTREE_ROOT=…\n\
                     (set by {})",
                    root.display(),
                    config::root_source()
                ),
            }),
            None => {
                let root = self.main.join(".claude/worktrees");
                std::fs::create_dir_all(&root)?;
                // Inside the repository: keep the slots out of its status, whatever the
                // repository's own .gitignore says.
                let ignore = root.join(".gitignore");
                if !ignore.exists() {
                    std::fs::write(ignore, "*\n")?;
                }
                Ok(root)
            }
        }
    }

    fn slots(&self) -> usize {
        self.config.slots.unwrap_or(DEFAULT_SLOTS)
    }

    fn worktrees(&self) -> Result<Vec<Worktree>> {
        Ok(parse_worktrees(&git(
            &self.main,
            &["worktree", "list", "--porcelain"],
        )?))
    }

    fn slot_state(&self, path: &Path, worktrees: &[Worktree]) -> Slot {
        match worktrees.iter().find(|w| same_path(&w.path, path)) {
            None => Slot::Missing,
            Some(w) => w.branch.clone().map_or(Slot::Free, Slot::Busy),
        }
    }

    /// The path an agent works in: gitti's folder inside the slot.
    fn work_dir(&self, slot: &Path) -> PathBuf {
        if self.rel.is_empty() {
            slot.to_path_buf()
        } else {
            slot.join(&self.rel)
        }
    }

    fn branch_exists(&self, branch: &str) -> bool {
        git(
            &self.main,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .is_ok()
    }
}

fn add(repo: &Repo, name: &str) -> Result {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    {
        return Err(Failure::usage(format!(
            "bad name `{name}` (letters, digits, - _ .)"
        )));
    }
    let branch = format!("{BRANCH_PREFIX}{name}");
    if repo.branch_exists(&branch) {
        return Err(Failure::new(format!("branch {branch} exists already")));
    }
    let root = repo.root()?;
    let worktrees = repo.worktrees()?;
    let slots: Vec<(PathBuf, Slot)> = (1..=repo.slots())
        .map(|k| {
            let path = root.join(format!("{SLOT_PREFIX}{k}"));
            let state = repo.slot_state(&path, &worktrees);
            (path, state)
        })
        .collect();

    // A free slot with a build first; one with local changes is left alone.
    for (path, _) in slots.iter().filter(|(_, s)| *s == Slot::Free) {
        let dirty = git(path, &["status", "--porcelain"])?;
        if !dirty.is_empty() {
            eprintln!("skipping {} (local changes):\n{dirty}", path.display());
            continue;
        }
        git(path, &["switch", "--quiet", "--create", &branch, BASE])?;
        eprintln!(
            "reused {} (its target/ is warm: only the diff compiles)",
            path.display()
        );
        println!("{}", repo.work_dir(path).display());
        return Ok(());
    }

    let Some((path, _)) = slots
        .iter()
        .find(|(p, s)| *s == Slot::Missing && !p.exists())
    else {
        let busy: Vec<String> = slots
            .iter()
            .map(|(p, s)| format!("  {} — {s:?}", p.display()))
            .collect();
        return Err(Failure::new(format!(
            "no free slot ({} configured):\n{}",
            repo.slots(),
            busy.join("\n")
        )));
    };
    create_slot(repo, path, Some(&branch))?;
    let p = path.to_string_lossy();
    eprintln!(
        "created {} (first build is cold, ~7 min; the slot stays for later packages)",
        p
    );
    println!("{}", repo.work_dir(path).display());
    Ok(())
}

/// A new slot from main: on `branch`, or parked (detached) when `None`. Sparse before
/// the checkout when gitti is a folder of a bigger repository, so only it is ever written.
fn create_slot(repo: &Repo, path: &Path, branch: Option<&str>) -> Result {
    let p = path.to_string_lossy();
    let mut add = vec!["worktree", "add", "--quiet"];
    if !repo.rel.is_empty() {
        add.push("--no-checkout");
    }
    match branch {
        Some(b) => add.extend(["-b", b]),
        None => add.push("--detach"),
    }
    add.extend([&*p, BASE]);
    git(&repo.main, &add)?;
    if !repo.rel.is_empty() {
        git(path, &["sparse-checkout", "set", "--cone", &repo.rel])?;
        match branch {
            Some(b) => git(path, &["checkout", "--quiet", b])?,
            None => git(path, &["checkout", "--quiet", "--detach", BASE])?,
        };
    }
    Ok(())
}

/// Creates the missing slots (parked on main) and builds every free one once (the
/// workspace test build, clippy, the screenshot and gitty binaries), so the first package in
/// a slot only compiles its diff.
fn prepare(repo: &Repo, build: bool) -> Result {
    let root = repo.root()?;
    let worktrees = repo.worktrees()?;
    for k in 1..=repo.slots() {
        let path = root.join(format!("{SLOT_PREFIX}{k}"));
        match repo.slot_state(&path, &worktrees) {
            Slot::Busy(b) => {
                eprintln!("{}: busy ({b}), not touched", path.display());
                continue;
            }
            Slot::Missing if path.exists() => {
                eprintln!("{}: exists but is no worktree, skipped", path.display());
                continue;
            }
            Slot::Missing => {
                create_slot(repo, &path, None)?;
                eprintln!("{}: created", path.display());
            }
            Slot::Free => {
                if git(&path, &["status", "--porcelain"])?.is_empty() {
                    git(&path, &["switch", "--quiet", "--detach", BASE])?;
                }
            }
        }
        if build {
            let dir = repo.work_dir(&path);
            let start = std::time::Instant::now();
            // Exactly the builds an agent runs: each package selection / feature set is a
            // separate set of artifacts (cargo unifies features over the selected packages
            // only), so anything else would rebuild the dependencies in the slot.
            let cargo = || Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
            let mut builds = Vec::new();
            for args in [
                &["test", "--workspace", "--no-run"][..],
                &["clippy", "--workspace", "--all-targets"][..],
                // `--features screenshot` runs (`shot:` steps) and `GITTY_SCRIPT` runs.
                &["build", "-p", "gitti-app", "--features", "screenshot"][..],
                &["build", "-p", "gitty"][..],
            ] {
                let mut cmd = cargo();
                cmd.args(args).current_dir(&dir);
                builds.push((format!("cargo {}", args.join(" ")), cmd));
            }
            for (label, mut cmd) in builds {
                let status = cmd
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()?;
                if !status.success() {
                    return Err(Failure::new(format!("{label} failed in {}", dir.display())));
                }
            }
            eprintln!(
                "{}: built in {:.0}s",
                path.display(),
                start.elapsed().as_secs_f64()
            );
        }
    }
    Ok(())
}

fn done(repo: &Repo, name: &str) -> Result {
    let branch = format!("{BRANCH_PREFIX}{name}");
    if !repo.branch_exists(&branch) {
        return Err(Failure::new(format!("no branch {branch}")));
    }
    if git(&repo.main, &["merge-base", "--is-ancestor", &branch, BASE]).is_err() {
        return Err(Failure::new(format!(
            "{branch} is not merged into {BASE} — nothing done"
        )));
    }
    let worktrees = repo.worktrees()?;
    if let Some(w) = worktrees
        .iter()
        .find(|w| w.branch.as_deref() == Some(branch.as_str()))
    {
        let dirty = git(&w.path, &["status", "--porcelain"])?;
        if !dirty.is_empty() {
            return Err(Failure::new(format!(
                "{} has local changes — commit or drop them first:\n{dirty}",
                w.path.display()
            )));
        }
        let is_slot = w
            .path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with(SLOT_PREFIX));
        if is_slot {
            git(&w.path, &["switch", "--quiet", "--detach", BASE])?;
            eprintln!("{} is free again (parked on {BASE})", w.path.display());
        } else {
            git(
                &repo.main,
                &["worktree", "remove", "--force", &w.path.to_string_lossy()],
            )?;
            eprintln!("removed {} (not a slot)", w.path.display());
        }
    }
    git(&repo.main, &["branch", "--quiet", "-d", &branch])?;
    eprintln!("deleted branch {branch}");
    Ok(())
}

fn list(repo: &Repo) -> Result {
    let root = repo.root()?;
    let worktrees = repo.worktrees()?;
    println!("{} (up to {} slots)", root.display(), repo.slots());
    let mut paths: Vec<PathBuf> = (1..=repo.slots())
        .map(|k| root.join(format!("{SLOT_PREFIX}{k}")))
        .collect();
    // Slots beyond the configured count (it was lowered) still show up.
    for w in &worktrees {
        let in_root = w.path.parent().is_some_and(|p| same_path(p, &root));
        let named = w
            .path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with(SLOT_PREFIX));
        if in_root && named && !paths.iter().any(|p| same_path(p, &w.path)) {
            paths.push(w.path.clone());
        }
    }
    for path in paths {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let state = match repo.slot_state(&path, &worktrees) {
            Slot::Missing => "—".to_string(),
            Slot::Free => "free".to_string(),
            Slot::Busy(b) => {
                let merged = git(&repo.main, &["merge-base", "--is-ancestor", &b, BASE]).is_ok();
                format!(
                    "{b}{}",
                    if merged {
                        " (merged — `done` frees it)"
                    } else {
                        ""
                    }
                )
            }
        };
        println!("  {name:<16} {state}");
    }
    Ok(())
}

fn drop_slots(repo: &Repo, which: &str) -> Result {
    let root = repo.root()?;
    let worktrees = repo.worktrees()?;
    let ks: Vec<usize> = if which == "all" {
        let max = worktrees
            .iter()
            .filter_map(|w| {
                w.path
                    .file_name()?
                    .to_string_lossy()
                    .strip_prefix(SLOT_PREFIX)?
                    .parse()
                    .ok()
            })
            .max()
            .unwrap_or(0);
        (1..=max.max(repo.slots())).collect()
    } else {
        vec![
            which
                .parse()
                .map_err(|_| Failure::usage("drop takes a slot number or `all`"))?,
        ]
    };
    for k in ks {
        let path = root.join(format!("{SLOT_PREFIX}{k}"));
        match repo.slot_state(&path, &worktrees) {
            Slot::Missing => {}
            Slot::Busy(b) => eprintln!("kept {} — {b} is checked out there", path.display()),
            Slot::Free => {
                git(
                    &repo.main,
                    &["worktree", "remove", "--force", &path.to_string_lossy()],
                )?;
                eprintln!("removed {}", path.display());
            }
        }
    }
    Ok(())
}

fn parse_worktrees(porcelain: &str) -> Vec<Worktree> {
    let mut out = Vec::new();
    for block in porcelain.split("\n\n") {
        let mut path = None;
        let mut branch = None;
        for line in block.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(p));
            } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
                branch = Some(b.to_string());
            }
        }
        if let Some(path) = path {
            out.push(Worktree { path, branch });
        }
    }
    out
}

fn same_path(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

/// Runs git in `dir`; stdout trimmed, or git's stderr as the error.
fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output()?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(Failure::new(format!(
            "git {} failed in {}:\n{}",
            args.join(" "),
            dir.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn worktree_list_porcelain() {
        let text = "worktree /r\nHEAD 1\nbranch refs/heads/main\n\n\
                    worktree /t/gitti-agent-1\nHEAD 2\ndetached\n\n\
                    worktree /t/gitti-agent-2\nHEAD 3\nbranch refs/heads/gitti-x\n";
        let w = parse_worktrees(text);
        assert_eq!(w.len(), 3);
        assert_eq!(
            (w[1].path.as_path(), w[1].branch.as_deref()),
            (Path::new("/t/gitti-agent-1"), None)
        );
        assert_eq!(w[2].branch.as_deref(), Some("gitti-x"));
    }
}
