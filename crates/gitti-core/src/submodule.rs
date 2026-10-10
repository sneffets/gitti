//! Submodules: the list with each one's state, the pointer changes of diffs as
//! commit summaries, and the commands (add, update — also to the remote branch —, sync,
//! remove with the local work it would lose).
//!
//! The list carries what `git submodule status --recursive` prints — path, recorded and
//! checked-out commit, uninitialised / out of date / conflicting — but is read in-process
//! (`.gitmodules` via gix-config, the recorded commit from the index, the checked-out one
//! from the submodule's HEAD, ahead / behind by a rev walk in the submodule): `git
//! submodule status` spawns a `git describe` per submodule, and a refresh must stay cheap.
//! Repositories without a `.gitmodules` file pay one `stat`. What changed *inside* a
//! top-level submodule (new commits, modified / untracked files) comes from the `git
//! status` the refresh runs anyway ([`crate::status::SubmoduleFlags`]); a nested one's
//! from one more `git status` in its parent, only when that parent is dirty.

use std::path::{Path, PathBuf};

use anyhow::Result;
use gix::ObjectId;
use gix::bstr::ByteSlice as _;

use crate::Repo;
use crate::status::{Status, SubmoduleFlags};

pub mod add;

/// Nested submodules deeper than this are not listed (cycles through odd setups).
const MAX_DEPTH: usize = 8;
/// Commits counted per side of a pointer change; more show as "10000+".
const COUNT_LIMIT: usize = 10_000;
/// Commit subjects listed per side of a pointer change in a diff.
pub const LOG_LIMIT: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubmoduleState {
    /// Not checked out (`git submodule update --init` not run yet).
    Uninitialized,
    /// Checked out at the recorded commit, nothing changed inside.
    UpToDate,
    /// Checked out at an older commit than the superproject records (or the recorded one
    /// isn't fetched yet): `git submodule update` brings it there.
    OutOfDate,
    /// New commits (not recorded yet) or uncommitted changes inside.
    Modified,
    /// The submodule's pointer is unmerged in the superproject.
    Conflicted,
}

impl SubmoduleState {
    /// One-character mark for lists (same in gitti and gitty).
    pub fn mark(self) -> char {
        match self {
            SubmoduleState::Uninitialized => '○',
            SubmoduleState::UpToDate => '✓',
            SubmoduleState::OutOfDate => '↓',
            SubmoduleState::Modified => '●',
            SubmoduleState::Conflicted => '!',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SubmoduleState::Uninitialized => "not initialised",
            SubmoduleState::UpToDate => "up to date",
            SubmoduleState::OutOfDate => "out of date",
            SubmoduleState::Modified => "modified",
            SubmoduleState::Conflicted => "conflicted",
        }
    }
}

/// One submodule (nested ones follow their parent, `depth` > 0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submodule {
    /// Name in `.gitmodules` (`[submodule "name"]`).
    pub name: String,
    /// Path relative to the top-level work dir, `/` separated (nested: `lib/a/vendor/b`).
    pub path: String,
    pub url: Option<String>,
    pub branch: Option<String>,
    /// 0 = a submodule of this repository, 1 = of a submodule, …
    pub depth: usize,
    /// The commit the parent's index records.
    pub recorded: Option<ObjectId>,
    /// The commit the parent's HEAD records (differs from `recorded` while a new pointer
    /// is staged; `None`: not committed yet, or no HEAD).
    pub committed: Option<ObjectId>,
    /// The submodule's HEAD (`None`: not checked out).
    pub checked_out: Option<ObjectId>,
    /// Commits of `checked_out` that `recorded` lacks (new commits) …
    pub ahead: usize,
    /// … and commits of `recorded` that `checked_out` lacks (out of date).
    pub behind: usize,
    /// The recorded commit isn't in the submodule's object database (not fetched).
    pub missing: bool,
    /// What `git status` of the parent reports about it (top level only).
    pub flags: SubmoduleFlags,
    pub conflicted: bool,
    pub state: SubmoduleState,
}

impl Submodule {
    /// The last path component (list label).
    pub fn short_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    /// The submodule's work dir under `top` (the top-level work dir).
    pub fn dir(&self, top: &Path) -> PathBuf {
        crate::paths::join_repo_path(top, &self.path)
    }

    /// A short state for lists: "3 behind", "2 new commits", "modified", ….
    pub fn detail(&self) -> String {
        let mut parts = Vec::new();
        match self.state {
            SubmoduleState::Uninitialized | SubmoduleState::Conflicted => {
                return self.state.label().to_string();
            }
            _ => {}
        }
        if self.missing {
            parts.push("not fetched".to_string());
        } else {
            if self.behind > 0 {
                parts.push(format!("{} behind", count_text(self.behind)));
            }
            if self.ahead > 0 {
                parts.push(format!(
                    "{} new commit{}",
                    count_text(self.ahead),
                    plural(self.ahead)
                ));
            }
        }
        if self.flags.modified {
            parts.push("modified".into());
        } else if self.flags.untracked {
            parts.push("untracked files".into());
        }
        parts.join(", ")
    }

    /// The menu's "Copy SHA" entries: `(label, full id)`, `Err(why)` = greyed out. The
    /// checked-out commit; not checked out (uninitialised): the commit the parent
    /// records, labelled so. When the checkout differs from the recorded commit, "Copy
    /// Recorded SHA" copies the latter.
    pub fn copy_sha_entries(&self) -> Vec<(String, std::result::Result<String, &'static str>)> {
        let hex = |id: ObjectId| Ok(id.to_hex().to_string());
        match (self.checked_out, self.recorded) {
            (Some(c), Some(r)) if r != c => vec![
                ("Copy SHA".into(), hex(c)),
                ("Copy Recorded SHA".into(), hex(r)),
            ],
            (Some(c), _) => vec![("Copy SHA".into(), hex(c))],
            (None, Some(r)) => vec![("Copy SHA (recorded commit, not checked out)".into(), hex(r))],
            (None, None) => vec![("Copy SHA".into(), Err("no commit recorded"))],
        }
    }

    /// Tooltip / details text.
    pub fn describe(&self) -> String {
        let mut s = format!("Submodule {} — {}", self.path, self.state.label());
        let detail = self.detail();
        if !detail.is_empty() && detail != self.state.label() {
            s.push_str(&format!(" ({detail})"));
        }
        if self.name != self.path {
            s.push_str(&format!("\nname: {}", self.name));
        }
        if let Some(url) = &self.url {
            s.push_str(&format!("\nurl: {url}"));
        }
        if let Some(b) = &self.branch {
            s.push_str(&format!("\nbranch: {b}"));
        }
        // The pointer: what the superproject records (committed in HEAD, staged in the
        // index) and what is checked out — a commit of the superproject records the
        // staged one.
        let staged = self.recorded.filter(|r| self.committed != Some(*r));
        if let Some(c) = self.committed {
            s.push_str(&format!("\nrecorded in HEAD: {}", short_id(c)));
        }
        if let Some(r) = staged {
            s.push_str(&format!("\nrecorded in the index (staged): {}", short_id(r)));
        }
        if let Some(c) = self.checked_out {
            let differs = self.recorded.is_some_and(|r| r != c);
            s.push_str(&format!(
                "\nchecked out: {}{}",
                short_id(c),
                if differs { " (not recorded — stage it to record it)" } else { "" }
            ));
        }
        s
    }

    /// The pointer as it moves from HEAD over the index to the checked-out commit, equal
    /// neighbours merged: `None` while all three agree, else "a1b2c3d → d4e5f6a" (see
    /// [`Self::describe`] for which is which).
    pub fn pointer_chain(&self) -> Option<String> {
        let mut ids: Vec<ObjectId> = Vec::new();
        for id in [self.committed, self.recorded, self.checked_out].into_iter().flatten() {
            if ids.last() != Some(&id) {
                ids.push(id);
            }
        }
        (ids.len() > 1).then(|| ids.iter().map(|&i| short_id(i)).collect::<Vec<_>>().join(" → "))
    }

    /// The sidebar row's detail: the [`Self::pointer_chain`] when the recorded and
    /// checked-out commits differ (the counts are in the tooltip, the state in the mark),
    /// else [`Self::detail`] (nothing while up to date).
    pub fn row_detail(&self) -> String {
        if let Some(chain) = self.pointer_chain() {
            return chain;
        }
        match self.state {
            SubmoduleState::UpToDate => String::new(),
            _ => self.detail(),
        }
    }

    /// The state from the fields (after they changed).
    fn derive_state(&mut self) {
        self.state = if self.conflicted {
            SubmoduleState::Conflicted
        } else if self.checked_out.is_none() {
            SubmoduleState::Uninitialized
        } else if self.recorded != self.checked_out {
            if self.missing || (self.ahead == 0 && self.behind > 0) {
                SubmoduleState::OutOfDate
            } else {
                SubmoduleState::Modified
            }
        } else if self.flags.dirty() {
            SubmoduleState::Modified
        } else {
            SubmoduleState::UpToDate
        };
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn count_text(n: usize) -> String {
    if n >= COUNT_LIMIT {
        format!("{COUNT_LIMIT}+")
    } else {
        n.to_string()
    }
}

/// The 7-character abbreviation shown everywhere.
pub fn short_id(id: ObjectId) -> String {
    id.to_hex_with_len(7).to_string()
}

/// Submodules that `git submodule update` would move (the banner after checkout / pull /
/// merge).
pub fn out_of_date(list: &[Submodule]) -> Vec<&Submodule> {
    list.iter()
        .filter(|s| s.state == SubmoduleState::OutOfDate)
        .collect()
}

/// One `[submodule "name"]` section of `.gitmodules`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ModuleEntry {
    name: String,
    path: String,
    url: Option<String>,
    branch: Option<String>,
}

fn parse_gitmodules(text: &str) -> Vec<ModuleEntry> {
    let Ok(file) = gix::config::File::try_from(text) else {
        return Vec::new();
    };
    let Some(sections) = file.sections_by_name("submodule") else {
        return Vec::new();
    };
    let value = |s: &gix::config::file::SectionRef<'_>, key: &str| {
        s.value(key)
            .map(|v| v.to_str_lossy().into_owned())
            .filter(|v| !v.is_empty())
    };
    let mut out: Vec<ModuleEntry> = Vec::new();
    for s in sections {
        let Some(name) = s.header().subsection_name().map(|n| n.to_str_lossy().into_owned())
        else {
            continue;
        };
        let Some(path) = value(&s, "path") else {
            continue;
        };
        let path = path.trim_end_matches('/').to_string();
        let entry = ModuleEntry {
            name,
            path,
            url: value(&s, "url"),
            branch: value(&s, "branch"),
        };
        // A name given twice: git merges the sections, the later values win.
        match out.iter_mut().find(|e| e.name == entry.name) {
            Some(e) => *e = entry,
            None => out.push(entry),
        }
    }
    out
}

/// Every submodule under the work dir `top` (nested ones after their parent), without
/// the `git status` part; see [`apply_status`]. Empty — one failed `stat` — without a
/// `.gitmodules` file.
pub fn list(top: &Path) -> Vec<Submodule> {
    let mut out = Vec::new();
    collect(top, "", 0, &mut out);
    out
}

fn collect(dir: &Path, prefix: &str, depth: usize, out: &mut Vec<Submodule>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(text) = std::fs::read_to_string(dir.join(".gitmodules")) else {
        return;
    };
    let modules = parse_gitmodules(&text);
    if modules.is_empty() {
        return;
    }
    let Ok(repo) = gix::open(dir) else {
        return;
    };
    let Ok(index) = repo.index_or_empty() else {
        return;
    };
    let head_tree = repo.head_tree().ok();
    for m in modules {
        // Only paths the index has as a gitlink are submodules (like `git submodule`).
        let Some(entry) = index.entry_by_path(m.path.as_bytes().as_bstr()) else {
            continue;
        };
        if entry.mode != gix::index::entry::Mode::COMMIT {
            continue;
        }
        let conflicted = entry.stage() != gix::index::entry::Stage::Unconflicted;
        let recorded = Some(entry.id);
        let committed = head_tree
            .as_ref()
            .and_then(|t| t.lookup_entry_by_path(&m.path).ok().flatten())
            .filter(|e| e.mode().is_commit())
            .map(|e| e.object_id());
        let sub_dir = crate::paths::join_repo_path(dir, &m.path);
        let sub_repo = sub_dir
            .join(".git")
            .exists()
            .then(|| gix::open(&sub_dir).ok())
            .flatten();
        let checked_out = sub_repo
            .as_ref()
            .and_then(|r| r.head_id().ok())
            .map(|id| id.detach());
        let mut sm = Submodule {
            name: m.name,
            path: format!("{prefix}{}", m.path),
            url: m.url,
            branch: m.branch,
            depth,
            recorded,
            committed,
            checked_out,
            ahead: 0,
            behind: 0,
            missing: false,
            flags: SubmoduleFlags::default(),
            conflicted,
            state: SubmoduleState::UpToDate,
        };
        if let (Some(r), Some(c), Some(repo)) = (recorded, checked_out, sub_repo.as_ref())
            && r != c
        {
            if repo.has_object(r) {
                sm.ahead = count_between(repo, c, r);
                sm.behind = count_between(repo, r, c);
            } else {
                sm.missing = true;
            }
        }
        sm.derive_state();
        let nested = format!("{}/", sm.path);
        let populated = sub_repo.is_some();
        out.push(sm);
        if populated {
            collect(&sub_dir, &nested, depth + 1, out);
        }
    }
}

/// Commits reachable from `from` but not from `hide` (at most [`COUNT_LIMIT`]).
fn count_between(repo: &gix::Repository, from: ObjectId, hide: ObjectId) -> usize {
    match repo.rev_walk([from]).with_hidden([hide]).all() {
        Ok(walk) => walk.take(COUNT_LIMIT).filter(Result::is_ok).count(),
        Err(_) => 0,
    }
}

/// `(id, subject)` of the commits reachable from `from` but not from `hide`, newest
/// first, at most [`LOG_LIMIT`]; plus how many there are (up to [`COUNT_LIMIT`]).
fn log_between(
    repo: &gix::Repository,
    from: ObjectId,
    hide: ObjectId,
) -> (Vec<(ObjectId, String)>, usize) {
    let Ok(walk) = repo.rev_walk([from]).with_hidden([hide]).all() else {
        return (Vec::new(), 0);
    };
    let mut list = Vec::new();
    let mut total = 0;
    for info in walk.take(COUNT_LIMIT).filter_map(Result::ok) {
        total += 1;
        if list.len() < LOG_LIMIT {
            let subject = repo
                .find_commit(info.id)
                .ok()
                .and_then(|c| c.message().ok().map(|m| m.summary().to_string()))
                .unwrap_or_default();
            list.push((info.id, subject));
        }
    }
    (list, total)
}

/// Takes in what `git status` says about the top-level submodules (dirty content,
/// conflicts) and derives each state again.
pub fn apply_status(list: &mut [Submodule], status: &Status) {
    for sm in list.iter_mut().filter(|s| s.depth == 0) {
        let entry = status.entries.iter().find(|e| e.path == sm.path);
        sm.flags = entry.and_then(|e| e.submodule).unwrap_or_default();
        if entry.is_some_and(|e| e.conflicted) {
            sm.conflicted = true;
        }
        sm.derive_state();
    }
}

/// The indices of the direct children of `list[i]` (nested submodules one level down).
fn children(list: &[Submodule], i: usize) -> Vec<usize> {
    let prefix = format!("{}/", list[i].path);
    let depth = list[i].depth + 1;
    (i + 1..list.len())
        .take_while(|&j| list[j].depth >= depth)
        .filter(|&j| list[j].depth == depth && list[j].path.starts_with(&prefix))
        .collect()
}

/// Takes the dirty flags of the nested submodules over from `prev` (an earlier list of
/// the same repository): a stage / unstage / discard in the top level doesn't change what
/// is inside them, and re-reading would cost a `git status` per dirty parent.
pub fn keep_nested_flags(list: &mut [Submodule], prev: &[Submodule]) {
    for sm in list.iter_mut().filter(|s| s.depth > 0) {
        if let Some(old) = prev.iter().find(|o| o.path == sm.path) {
            sm.flags = old.flags;
            sm.derive_state();
        }
    }
}

/// What removing a submodule would throw away: uncommitted changes, commits no remote
/// has, stashes (in its repository, which the removal deletes).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalWork {
    pub path: String,
    pub modified: bool,
    pub untracked: bool,
    /// Commits of HEAD / the local branches that no remote-tracking branch has (up to
    /// [`COUNT_LIMIT`]).
    pub unpushed: usize,
    pub stash: bool,
}

impl LocalWork {
    pub fn any(&self) -> bool {
        self.modified || self.untracked || self.unpushed > 0 || self.stash
    }

    /// "libs/lib: uncommitted changes, 2 commits on no remote, stashed changes".
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if self.modified {
            parts.push("uncommitted changes".to_string());
        }
        if self.untracked {
            parts.push("untracked files".to_string());
        }
        if self.unpushed > 0 {
            parts.push(format!(
                "{} commit{} on no remote",
                count_text(self.unpushed),
                plural(self.unpushed)
            ));
        }
        if self.stash {
            parts.push("stashed changes".to_string());
        }
        format!("{}: {}", self.path, parts.join(", "))
    }
}

/// The local work in the submodule at `path` and in the ones nested in it (in-process: the
/// dirty flags of `list`, the refs of each checked-out submodule). Empty: nothing would be
/// lost.
pub fn local_work(top: &Path, list: &[Submodule], path: &str) -> Vec<LocalWork> {
    let prefix = format!("{path}/");
    list.iter()
        .filter(|s| s.path == path || s.path.starts_with(&prefix))
        .filter(|s| s.checked_out.is_some())
        .filter_map(|s| {
            let repo = gix::open(s.dir(top)).ok();
            let (unpushed, stash) = repo.as_ref().map(unpushed_and_stash).unwrap_or_default();
            let w = LocalWork {
                path: s.path.clone(),
                modified: s.flags.modified,
                untracked: s.flags.untracked,
                unpushed,
                stash,
            };
            w.any().then_some(w)
        })
        .collect()
}

/// Commits of HEAD and the local branches that no remote-tracking branch has; whether
/// there is a stash.
fn unpushed_and_stash(repo: &gix::Repository) -> (usize, bool) {
    let mut tips: Vec<ObjectId> = repo.head_id().ok().map(|id| id.detach()).into_iter().collect();
    let mut hidden = Vec::new();
    if let Ok(refs) = repo.references() {
        if let Ok(local) = refs.local_branches() {
            tips.extend(
                local
                    .filter_map(Result::ok)
                    .filter_map(|mut r| r.peel_to_id().ok().map(|id| id.detach())),
            );
        }
        if let Ok(remote) = refs.remote_branches() {
            hidden.extend(
                remote
                    .filter_map(Result::ok)
                    .filter_map(|mut r| r.peel_to_id().ok().map(|id| id.detach())),
            );
        }
    }
    tips.sort();
    tips.dedup();
    let unpushed = if tips.is_empty() {
        0
    } else {
        match repo.rev_walk(tips).with_hidden(hidden).all() {
            Ok(walk) => walk.take(COUNT_LIMIT).filter(Result::is_ok).count(),
            Err(_) => 0,
        }
    };
    let stash = repo.try_find_reference("refs/stash").ok().flatten().is_some();
    (unpushed, stash)
}

// ---------------------------------------------------------------- checking out a commit

/// Commits the "Check Out Commit in Submodule…" picker lists at most.
pub const HISTORY_LIMIT: usize = 5_000;

/// One commit of a submodule's history (the "Check Out Commit in Submodule…" picker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmoduleCommit {
    pub id: ObjectId,
    pub subject: String,
    /// Committer time, seconds since the epoch.
    pub time: i64,
    /// Branches / tags at this commit (`main`, `origin/main`, `v1.0`).
    pub refs: Vec<String>,
    /// The superproject's HEAD records it …
    pub committed: bool,
    /// … its index (a staged pointer) …
    pub recorded: bool,
    /// … it is checked out in the submodule.
    pub checked_out: bool,
}

impl SubmoduleCommit {
    /// "checked out · recorded · main, origin/main" — what the picker shows after the id
    /// and date (empty for a plain commit).
    pub fn marks(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.checked_out {
            parts.push("checked out".into());
        }
        match (self.committed, self.recorded) {
            (true, true) => parts.push("recorded".into()),
            (true, false) => parts.push("recorded in HEAD".into()),
            (false, true) => parts.push("staged".into()),
            (false, false) => {}
        }
        if !self.refs.is_empty() {
            parts.push(self.refs.join(", "));
        }
        parts.join(" · ")
    }
}

/// The history of the submodule `sm` (checked out under the work dir `top`), newest
/// first, at most `limit` commits: everything reachable from its HEAD, its branches,
/// remote-tracking branches and tags, and the commits the superproject records
/// (in-process, gix). Nested submodules read their own repository like any other.
pub fn history(top: &Path, sm: &Submodule, limit: usize) -> Result<Vec<SubmoduleCommit>> {
    let dir = sm.dir(top);
    if sm.checked_out.is_none() || !dir.join(".git").exists() {
        anyhow::bail!("{} is not initialised — Update checks it out", sm.path);
    }
    let repo = gix::open(&dir)?;
    let mut names: std::collections::HashMap<ObjectId, Vec<String>> = Default::default();
    let mut tips: Vec<ObjectId> = Vec::new();
    if let Ok(refs) = repo.references() {
        // Local branches first, then remote-tracking ones, then tags.
        let groups = [refs.local_branches(), refs.remote_branches(), refs.tags()];
        for group in groups.into_iter().flatten() {
            for mut r in group.filter_map(Result::ok) {
                let name = r.name().shorten().to_string();
                if name.ends_with("/HEAD") {
                    continue;
                }
                let Ok(commit) = r.peel_to_commit() else {
                    continue;
                };
                tips.push(commit.id);
                names.entry(commit.id).or_default().push(name);
            }
        }
    }
    tips.extend(
        [sm.checked_out, sm.recorded, sm.committed]
            .into_iter()
            .flatten()
            .filter(|&id| repo.has_object(id)),
    );
    tips.sort();
    tips.dedup();
    if tips.is_empty() {
        return Ok(Vec::new());
    }
    // `git rev-list --date-order`: newest first, but never a parent before its children
    // (commits of the same second keep their order).
    let walk = gix::traverse::commit::topo::Builder::from_iters(&repo.objects, tips, None::<Vec<ObjectId>>)
        .sorting(gix::traverse::commit::topo::Sorting::DateOrder)
        .build()
        .map_err(|e| anyhow::anyhow!("reading the history of {}: {e:?}", sm.path))?;
    let mut out = Vec::new();
    for info in walk.filter_map(Result::ok).take(limit) {
        let Ok(commit) = repo.find_commit(info.id) else {
            continue;
        };
        let subject = commit
            .message()
            .map(|m| m.summary().to_string())
            .unwrap_or_default();
        let time = info
            .commit_time
            .or_else(|| commit.time().ok().map(|t| t.seconds))
            .unwrap_or(0);
        out.push(SubmoduleCommit {
            id: info.id,
            subject,
            time,
            refs: names.remove(&info.id).unwrap_or_default(),
            committed: sm.committed == Some(info.id),
            recorded: sm.recorded == Some(info.id),
            checked_out: sm.checked_out == Some(info.id),
        });
    }
    Ok(out)
}

/// What checking out `target` in the submodule at `path` would put at risk, as lines for
/// a confirmation (empty: nothing — check it out directly): uncommitted changes (git
/// carries them over, or refuses when the commit changes the same files), and commits
/// of a detached HEAD that no branch, remote-tracking branch or tag has (afterwards
/// only the reflog knows them). In-process.
pub fn checkout_risks(top: &Path, list: &[Submodule], path: &str, target: ObjectId) -> Vec<String> {
    let Some(sm) = list.iter().find(|s| s.path == path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if sm.flags.modified {
        out.push(
            "uncommitted changes — git keeps them, or refuses if the commit changes the same files"
                .to_string(),
        );
    }
    let Ok(repo) = gix::open(sm.dir(top)) else {
        return out;
    };
    let detached = repo.head().is_ok_and(|h| h.is_detached());
    let Some(head) = repo.head_id().ok().map(|id| id.detach()) else {
        return out;
    };
    if detached && head != target {
        let mut hidden = vec![target];
        if let Ok(refs) = repo.references()
            && let Ok(all) = refs.all()
        {
            hidden.extend(
                all.filter_map(Result::ok)
                    .filter_map(|mut r| r.peel_to_id().ok().map(|id| id.detach())),
            );
        }
        let n = match repo.rev_walk([head]).with_hidden(hidden).all() {
            Ok(walk) => walk.take(COUNT_LIMIT).filter(Result::is_ok).count(),
            Err(_) => 0,
        };
        if n > 0 {
            out.push(format!(
                "{} commit{} on no branch (detached HEAD {}) — afterwards only the reflog has {}",
                count_text(n),
                plural(n),
                short_id(head),
                if n == 1 { "it" } else { "them" }
            ));
        }
    }
    out
}

// ---------------------------------------------------------------- pointer diffs

/// A submodule pointer change in a diff (`Subproject commit` lines of
/// `--submodule=short`), shown as a commit summary instead of the raw lines and staged
/// as one unit (the path).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubmoduleDiff {
    /// `None` when the submodule was added.
    pub old: Option<ObjectId>,
    /// `None` when it was removed.
    pub new: Option<ObjectId>,
    /// Uncommitted changes inside it (`-dirty`, working-tree diffs only).
    pub dirty: bool,
    /// The commits between `old` and `new` (filled by [`fill_logs`]; `None` when the
    /// submodule isn't checked out here or nothing moved).
    pub log: Option<SubmoduleLog>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubmoduleLog {
    /// Commits in `new` but not in `old` (newest first, at most [`LOG_LIMIT`]) …
    pub added: Vec<(ObjectId, String)>,
    pub added_total: usize,
    /// … and in `old` but not in `new` (the pointer went back / sideways).
    pub removed: Vec<(ObjectId, String)>,
    pub removed_total: usize,
    /// One of the two commits isn't in the submodule's repository (not fetched).
    pub missing: bool,
}

/// One side of a pointer change: listed commits, how many more, added (vs. removed).
type Side<'a> = (&'a [(ObjectId, String)], usize, bool);

/// One display row of a [`SubmoduleDiff`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubRow<'a> {
    /// "Submodule x: a1b2c3d → d4e5f6a (+3 / −1 commits)".
    Summary(String),
    /// A commit that came in (`added`) or went away.
    Commit {
        added: bool,
        id: ObjectId,
        subject: &'a str,
    },
    /// "… N more".
    More { added: bool, n: usize },
    /// A remark (dirty content, not checked out, not fetched).
    Note(&'static str),
}

impl SubRow<'_> {
    /// The row as plain text (gitty, copying).
    pub fn text(&self) -> String {
        match self {
            SubRow::Summary(s) => s.clone(),
            SubRow::Commit { added, id, subject } => {
                format!("{} {} {subject}", sign(*added), short_id(*id))
            }
            SubRow::More { added, n } => format!("{} … {} more", sign(*added), count_text(*n)),
            SubRow::Note(s) => (*s).to_string(),
        }
    }
}

/// `+` for commits that came in, `−` for ones that went away.
pub fn sign(added: bool) -> char {
    if added { '+' } else { '−' }
}

const NOTE_DIRTY: &str = "Uncommitted changes inside the submodule — open it to commit or discard them";
const NOTE_UNKNOWN: &str = "The submodule isn't checked out here — its commits can't be listed";
const NOTE_MISSING: &str = "The commits aren't in the submodule yet — Update fetches them";

impl SubmoduleDiff {
    /// Recognises a gitlink file diff: every line is `Subproject commit <id>[-dirty]` and
    /// the header names no file mode but 160000 (a dirty-only change has no `index` line).
    pub(crate) fn detect(header: &[String], hunks: &[crate::diff::Hunk]) -> Option<Self> {
        let other_mode = header.iter().any(|h| {
            let mode = if let Some(rest) = h.strip_prefix("index ") {
                rest.split(' ').nth(1)
            } else {
                ["new file mode ", "deleted file mode ", "old mode ", "new mode "]
                    .iter()
                    .find_map(|p| h.strip_prefix(p))
            };
            mode.is_some_and(|m| m.trim() != "160000")
        });
        if other_mode || hunks.is_empty() {
            return None;
        }
        let mut d = SubmoduleDiff::default();
        for line in hunks.iter().flat_map(|h| &h.lines) {
            use crate::diff::LineKind;
            let rest = line.text.strip_prefix("Subproject commit ")?;
            let (hex, dirty) = match rest.strip_suffix("-dirty") {
                Some(h) => (h, true),
                None => (rest, false),
            };
            let id = ObjectId::from_hex(hex.trim().as_bytes()).ok()?;
            match line.kind {
                LineKind::Removed => d.old = Some(id),
                LineKind::Added => {
                    d.new = Some(id);
                    d.dirty |= dirty;
                }
                LineKind::Context => {
                    d.old = Some(id);
                    d.new = Some(id);
                    d.dirty |= dirty;
                }
                LineKind::NoNewline => {}
            }
        }
        (d.old.is_some() || d.new.is_some()).then_some(d)
    }

    fn moved(&self) -> bool {
        matches!((self.old, self.new), (Some(a), Some(b)) if a != b)
    }

    /// The first row: "Submodule x: a1b2c3d → d4e5f6a (+3 / −1 commits)".
    pub fn summary(&self, path: &str) -> String {
        match (self.old, self.new) {
            (Some(a), Some(b)) if a != b => {
                let counts = match &self.log {
                    Some(l) if !l.missing => {
                        let (a, r) = (l.added_total, l.removed_total);
                        let n = match (a, r) {
                            (_, 0) => format!("+{}", count_text(a)),
                            (0, _) => format!("−{}", count_text(r)),
                            _ => format!("+{} / −{}", count_text(a), count_text(r)),
                        };
                        format!(" ({n} commit{})", plural(a.max(r)))
                    }
                    _ => String::new(),
                };
                format!("Submodule {path}: {} → {}{counts}", short_id(a), short_id(b))
            }
            (Some(a), Some(_)) => format!("Submodule {path} at {}", short_id(a)),
            (None, Some(b)) => format!("Submodule {path} added at {}", short_id(b)),
            (Some(a), None) => format!("Submodule {path} removed (was {})", short_id(a)),
            (None, None) => format!("Submodule {path}"),
        }
    }

    fn notes(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.moved() {
            match &self.log {
                None => v.push(NOTE_UNKNOWN),
                Some(l) if l.missing => v.push(NOTE_MISSING),
                _ => {}
            }
        }
        if self.dirty {
            v.push(NOTE_DIRTY);
        }
        v
    }

    /// `(listed commits, how many more, added?)` per side.
    fn sides(&self) -> [Side<'_>; 2] {
        match &self.log {
            Some(l) if !l.missing => [
                (&l.added, l.added_total.saturating_sub(l.added.len()), true),
                (&l.removed, l.removed_total.saturating_sub(l.removed.len()), false),
            ],
            _ => [(&[], 0, true), (&[], 0, false)],
        }
    }

    /// Number of display rows ([`Self::row`]).
    pub fn row_count(&self) -> usize {
        let side_rows: usize = self
            .sides()
            .iter()
            .map(|(list, more, _)| list.len() + usize::from(*more > 0))
            .sum();
        1 + side_rows + self.notes().len()
    }

    /// Display row `i` of the change of submodule `path`.
    pub fn row(&self, path: &str, i: usize) -> Option<SubRow<'_>> {
        if i == 0 {
            return Some(SubRow::Summary(self.summary(path)));
        }
        let mut i = i - 1;
        for (list, more, added) in self.sides() {
            if i < list.len() {
                let (id, subject) = &list[i];
                return Some(SubRow::Commit {
                    added,
                    id: *id,
                    subject: subject.as_str(),
                });
            }
            i -= list.len();
            if more > 0 {
                if i == 0 {
                    return Some(SubRow::More { added, n: more });
                }
                i -= 1;
            }
        }
        self.notes().get(i).map(|n| SubRow::Note(n))
    }
}

/// Fills [`SubmoduleDiff::log`] of the submodule pointer changes in `files` from the
/// submodules checked out under `top` (in-process; nothing to do without any).
pub fn fill_logs(top: &Path, files: &mut [crate::diff::FileDiff]) {
    for f in files.iter_mut() {
        let path = f.path().to_string();
        let Some(sd) = f.submodule.as_mut() else {
            continue;
        };
        if sd.log.is_some() {
            continue;
        }
        let (Some(old), Some(new)) = (sd.old, sd.new) else {
            continue;
        };
        if old == new {
            continue;
        }
        let dir = crate::paths::join_repo_path(top, &path);
        if !dir.join(".git").exists() {
            continue;
        }
        let Ok(repo) = gix::open(&dir) else {
            continue;
        };
        if !repo.has_object(old) || !repo.has_object(new) {
            sd.log = Some(SubmoduleLog {
                missing: true,
                ..Default::default()
            });
            continue;
        }
        let (added, added_total) = log_between(&repo, new, old);
        let (removed, removed_total) = log_between(&repo, old, new);
        sd.log = Some(SubmoduleLog {
            added,
            added_total,
            removed,
            removed_total,
            missing: false,
        });
    }
}

// ---------------------------------------------------------------- commands

impl Repo {
    /// Whether the work dir has a `.gitmodules` file (one `stat`).
    pub fn has_gitmodules(&self) -> bool {
        self.workdir().join(".gitmodules").is_file()
    }

    /// Every submodule with its state, `status` = the working state's (dirty content,
    /// conflicts). In-process — but for nested submodules: those whose parent submodule
    /// is dirty get their own state from one path-limited `git status` in that parent
    /// ([`Repo::nested_submodule_status`]). Empty without `.gitmodules`.
    pub fn submodules(&self, status: &Status) -> Vec<Submodule> {
        if !self.has_gitmodules() {
            return Vec::new();
        }
        let mut list = list(self.workdir());
        apply_status(&mut list, status);
        self.nested_submodule_status(&mut list);
        list
    }

    /// [`Repo::submodules`] after a stage / unstage / discard in the top level: the nested
    /// submodules keep their dirty flags from `prev` (no git call at all).
    pub fn submodules_after(&self, status: &Status, prev: &[Submodule]) -> Vec<Submodule> {
        if !self.has_gitmodules() {
            return Vec::new();
        }
        let mut list = list(self.workdir());
        apply_status(&mut list, status);
        keep_nested_flags(&mut list, prev);
        list
    }

    /// The dirty flags of nested submodules (`git status` of the top level only reports
    /// its own): a nested submodule's changes make its parent dirty, so only a dirty
    /// parent with submodules of its own is asked — one `git status` there, limited to
    /// those children. Parents come first in `list`, so this goes down level by level.
    /// No git call while nothing nested can be dirty.
    pub fn nested_submodule_status(&self, list: &mut [Submodule]) {
        for i in 0..list.len() {
            if !list[i].flags.dirty() || list[i].checked_out.is_none() {
                continue;
            }
            let kids = children(list, i);
            if kids.is_empty() {
                continue;
            }
            let prefix_len = list[i].path.len() + 1;
            let rel: Vec<String> = kids.iter().map(|&j| list[j].path[prefix_len..].to_string()).collect();
            let mut args = vec!["status", "--porcelain=v2", "-z", "--untracked-files=normal", "--"];
            args.extend(rel.iter().map(String::as_str));
            let git = self
                .git()
                .with_workdir(list[i].dir(self.workdir()))
                .with_env(&[("GIT_LITERAL_PATHSPECS", "1")]);
            let Ok(out) = git.run(&args) else {
                continue;
            };
            let status = crate::status::parse_porcelain_v2(&out.stdout);
            for (&j, rel) in kids.iter().zip(&rel) {
                let entry = status.entries.iter().find(|e| &e.path == rel);
                list[j].flags = entry.and_then(|e| e.submodule).unwrap_or_default();
                if entry.is_some_and(|e| e.conflicted) {
                    list[j].conflicted = true;
                }
                list[j].derive_state();
            }
        }
    }

    /// `(repository dir, path relative to it)` of each of `paths`, grouped: a nested
    /// submodule (`libs/lib/vendor/inner`) belongs to its parent submodule's repository —
    /// `git submodule` of the top level doesn't know it.
    fn by_owner(&self, paths: &[&str]) -> Vec<(PathBuf, Vec<String>)> {
        let top = self.workdir();
        let all = if paths.iter().any(|p| p.contains('/')) { list(top) } else { Vec::new() };
        let mut out: Vec<(PathBuf, Vec<String>)> = Vec::new();
        for p in paths {
            let owner = all
                .iter()
                .filter(|s| p.starts_with(&format!("{}/", s.path)))
                .max_by_key(|s| s.path.len());
            let (dir, rel) = match owner {
                Some(s) => (s.dir(top), p[s.path.len() + 1..].to_string()),
                None => (top.to_path_buf(), p.to_string()),
            };
            match out.iter_mut().find(|(d, _)| *d == dir) {
                Some((_, v)) => v.push(rel),
                None => out.push((dir, vec![rel])),
            }
        }
        out
    }

    /// `git <args> [-- paths]`, run in the repository owning each path (see
    /// [`Self::by_owner`]); empty `paths` = once here, for all. The outputs joined.
    fn submodule_command(&self, args: &[&str], paths: &[&str]) -> Result<String> {
        if paths.is_empty() {
            let out = self.git().run(args)?;
            return Ok(format!("{}{}", out.stdout_str(), out.stderr));
        }
        let mut text = String::new();
        for (dir, rel) in self.by_owner(paths) {
            let mut a = args.to_vec();
            a.push("--");
            a.extend(rel.iter().map(String::as_str));
            let out = self.git().with_workdir(dir).run(&a)?;
            text.push_str(&out.stdout_str());
            text.push_str(&out.stderr);
        }
        Ok(text)
    }

    /// `git submodule update --init --recursive [-- paths]`: checks out the recorded
    /// commits (cloning / fetching what is missing). Empty `paths` = all.
    pub fn update_submodules(&self, paths: &[&str]) -> Result<String> {
        self.submodule_command(&["submodule", "update", "--init", "--recursive"], paths)
    }

    /// `git submodule update --init --recursive --remote [-- paths]`: checks out the tip
    /// of the remote branch each submodule follows (`branch` in `.gitmodules`, else the
    /// remote's HEAD) instead of the recorded commit — the new pointers show as changes,
    /// a commit records them. Empty `paths` = all.
    pub fn update_submodules_remote(&self, paths: &[&str]) -> Result<String> {
        self.submodule_command(
            &["submodule", "update", "--init", "--recursive", "--remote"],
            paths,
        )
    }

    /// `git submodule sync --recursive [-- paths]`: copies the URLs of `.gitmodules` into
    /// the configuration (after a URL change upstream).
    pub fn sync_submodules(&self, paths: &[&str]) -> Result<String> {
        self.submodule_command(&["submodule", "sync", "--recursive"], paths)
    }

    /// Removes the submodule at `path` the way git documents it: `git submodule deinit -f`
    /// (its files and its configuration), `git rm -f` (the pointer and its `.gitmodules`
    /// section, staged — a commit records the removal; an emptied `.gitmodules` goes too),
    /// then its repository under `.git/modules/<name>` (nested ones included). Local work
    /// in it is lost — callers ask first ([`local_work`]). A nested submodule is removed
    /// from its parent submodule (which then has uncommitted changes).
    pub fn remove_submodule(&self, path: &str) -> Result<String> {
        let (dir, rel) = self
            .by_owner(&[path])
            .pop()
            .map(|(d, mut r)| (d, r.pop().unwrap_or_default()))
            .unwrap_or_else(|| (self.workdir().to_path_buf(), path.to_string()));
        let name = std::fs::read_to_string(dir.join(".gitmodules"))
            .ok()
            .and_then(|t| parse_gitmodules(&t).into_iter().find(|m| m.path == rel))
            .map(|m| m.name)
            .unwrap_or_else(|| rel.clone());
        let owner = gix::open(&dir)?;
        let sub_dir = crate::paths::join_repo_path(&dir, &rel);
        // Where its repository lives: the `.git` file's `gitdir:`, else git's default.
        let gitfile_dir = std::fs::read_to_string(sub_dir.join(".git"))
            .ok()
            .and_then(|t| t.strip_prefix("gitdir:").map(|g| sub_dir.join(g.trim())));
        let git = self
            .git()
            .with_workdir(&dir)
            .with_env(&[("GIT_LITERAL_PATHSPECS", "1")]);
        let mut text = String::new();
        let out = git.run(&["submodule", "deinit", "-f", "--", &rel])?;
        text.push_str(&out.stdout_str());
        text.push_str(&out.stderr);
        let out = git.run(&["rm", "-f", "-q", "--", &rel])?;
        text.push_str(&out.stderr);
        // `git rm` leaves an empty `.gitmodules` behind after the last one.
        let gitmodules = dir.join(".gitmodules");
        if std::fs::read_to_string(&gitmodules).is_ok_and(|t| t.trim().is_empty()) {
            git.run(&["rm", "-f", "-q", "--", ".gitmodules"])?;
        }
        // Its repository: only ever a folder inside a `modules` folder of the git dir.
        let mut roots: Vec<PathBuf> = [owner.git_dir(), owner.common_dir()]
            .iter()
            .map(|g| crate::paths::canonical(&g.join("modules")))
            .collect();
        roots.dedup();
        let candidates = gitfile_dir
            .into_iter()
            .chain(roots.iter().map(|r| r.join(&name)));
        for c in candidates {
            if !c.is_dir() {
                continue;
            }
            let c = crate::paths::canonical(&c);
            if !roots.iter().any(|r| c.starts_with(r) && c != *r) {
                continue;
            }
            std::fs::remove_dir_all(&c).map_err(|e| {
                anyhow::anyhow!(
                    "removed submodule {path}, but could not delete its repository {}: {e}",
                    c.display()
                )
            })?;
            text.push_str(&format!("deleted {}\n", c.display()));
            break;
        }
        Ok(text)
    }

    /// Checks out commit `id` in the submodule at `path` (relative to the work dir; nested
    /// ones too) with a detached HEAD — `git checkout --detach` run in the submodule. Its
    /// parent (the top level, or the parent submodule) then shows the pointer change;
    /// staging and committing it records that commit. git refuses when uncommitted
    /// changes there would be overwritten (callers warn first, [`checkout_risks`]).
    pub fn checkout_submodule_commit(&self, path: &str, id: ObjectId) -> Result<String> {
        let dir = crate::paths::join_repo_path(self.workdir(), path);
        if !dir.join(".git").exists() {
            anyhow::bail!("{path} is not initialised — Update checks it out");
        }
        let hex = id.to_string();
        let out = self.git().with_workdir(dir).run(&[
            "-c",
            "advice.detachedHead=false",
            "checkout",
            "--detach",
            &hex,
            "--",
        ])?;
        Ok(format!("{}{}", out.stdout_str(), out.stderr))
    }
}

/// The folder `git submodule add` picks for `url` without a path: its last component
/// without `.git` (`https://host/a/lib.git` → `lib`, `../lib/` → `lib`).
pub fn default_path(url: &str) -> String {
    let url = url.trim().trim_end_matches(['/', '\\']);
    let last = url.rsplit(['/', '\\', ':']).next().unwrap_or(url);
    last.strip_suffix(".git").unwrap_or(last).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn default_path_is_the_last_component() {
        assert_eq!(default_path("https://host/a/lib.git"), "lib");
        assert_eq!(default_path("git@host:team/tool.git"), "tool");
        assert_eq!(default_path("git@host:tool"), "tool");
        assert_eq!(default_path("../lib/"), "lib");
        assert_eq!(default_path(r"C:\repos\x"), "x");
    }

    #[gitti_test::test]
    fn gitmodules_sections() {
        let text = "[submodule \"lib\"]\n\tpath = libs/lib/\n\turl = ../lib.git\n\
                    [submodule \"x y\"]\n\tpath = \"x y\"\n\turl = git@host:x.git\n\tbranch = main\n\
                    [submodule \"nopath\"]\n\turl = a\n";
        let m = parse_gitmodules(text);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].name, "lib");
        assert_eq!(m[0].path, "libs/lib");
        assert_eq!(m[0].url.as_deref(), Some("../lib.git"));
        assert_eq!(m[1].path, "x y");
        assert_eq!(m[1].branch.as_deref(), Some("main"));
    }

    fn id(n: u8) -> ObjectId {
        ObjectId::from_bytes_or_panic(&[n; 20])
    }

    #[gitti_test::test]
    fn copy_sha_entries_say_which_commit() {
        let sm = |checked_out: Option<ObjectId>, recorded: Option<ObjectId>| Submodule {
            name: "lib".into(),
            path: "lib".into(),
            url: None,
            branch: None,
            depth: 0,
            recorded,
            committed: recorded,
            checked_out,
            ahead: 0,
            behind: 0,
            missing: false,
            flags: SubmoduleFlags::default(),
            conflicted: false,
            state: if checked_out.is_some() {
                SubmoduleState::UpToDate
            } else {
                SubmoduleState::Uninitialized
            },
        };
        let hex = |n: u8| Ok(id(n).to_hex().to_string());
        assert_eq!(sm(Some(id(1)), Some(id(1))).copy_sha_entries(), [("Copy SHA".into(), hex(1))]);
        assert_eq!(
            sm(Some(id(1)), Some(id(2))).copy_sha_entries(),
            [("Copy SHA".into(), hex(1)), ("Copy Recorded SHA".into(), hex(2))]
        );
        assert_eq!(
            sm(None, Some(id(2))).copy_sha_entries(),
            [("Copy SHA (recorded commit, not checked out)".into(), hex(2))]
        );
        assert_eq!(sm(None, None).copy_sha_entries(), [("Copy SHA".into(), Err("no commit recorded"))]);
    }

    #[gitti_test::test]
    fn diff_rows() {
        let commits = |n: usize, base: u8| -> Vec<(ObjectId, String)> {
            (0..n).map(|i| (id(base + i as u8), format!("c{i}"))).collect()
        };
        let d = SubmoduleDiff {
            old: Some(id(1)),
            new: Some(id(2)),
            dirty: true,
            log: Some(SubmoduleLog {
                added: commits(3, 10),
                added_total: 3,
                removed: commits(1, 20),
                removed_total: 61,
                missing: false,
            }),
        };
        assert_eq!(
            d.summary("lib"),
            "Submodule lib: 0101010 → 0202020 (+3 / −61 commits)"
        );
        // summary, 3 added, 1 removed + "more", dirty note
        assert_eq!(d.row_count(), 7);
        assert!(matches!(d.row("lib", 1), Some(SubRow::Commit { added: true, .. })));
        assert_eq!(d.row("lib", 4).unwrap().text(), "− 1414141 c0");
        assert_eq!(d.row("lib", 5).unwrap().text(), "− … 60 more");
        assert_eq!(d.row("lib", 6), Some(SubRow::Note(NOTE_DIRTY)));
        assert_eq!(d.row("lib", 7), None);

        let unknown = SubmoduleDiff {
            old: Some(id(1)),
            new: Some(id(2)),
            ..Default::default()
        };
        assert_eq!(unknown.row_count(), 2);
        assert_eq!(unknown.row("x", 1), Some(SubRow::Note(NOTE_UNKNOWN)));
        let added = SubmoduleDiff {
            new: Some(id(2)),
            ..Default::default()
        };
        assert_eq!(added.summary("x"), "Submodule x added at 0202020");
        assert_eq!(added.row_count(), 1);
    }

    #[gitti_test::test]
    fn detects_gitlink_diffs() {
        let text = "diff --git a/lib b/lib\nindex 1111111..2222222 160000\n--- a/lib\n+++ b/lib\n\
                    @@ -1 +1 @@\n-Subproject commit 1111111111111111111111111111111111111111\n\
                    +Subproject commit 2222222222222222222222222222222222222222-dirty\n\
                    diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-Subproject commit x\n+y\n\
                    diff --git a/g b/g\nindex 1111111..2222222 100644\n--- a/g\n+++ b/g\n@@ -1 +1 @@\n\
                    -Subproject commit 1111111111111111111111111111111111111111\n\
                    +Subproject commit 2222222222222222222222222222222222222222\n\
                    diff --git a/d b/d\n--- a/d\n+++ b/d\n@@ -1 +1 @@\n\
                    -Subproject commit 1111111111111111111111111111111111111111\n\
                    +Subproject commit 1111111111111111111111111111111111111111-dirty\n";
        let files = crate::diff::parse_unified(text);
        let d = files[0].submodule.as_ref().unwrap();
        assert_eq!(d.old, Some(ObjectId::from_hex(&[b'1'; 40]).unwrap()));
        assert_eq!(d.new, Some(ObjectId::from_hex(&[b'2'; 40]).unwrap()));
        assert!(d.dirty);
        assert!(files[1].submodule.is_none());
        assert!(files[2].submodule.is_none(), "a text file with such content");
        let dirty = files[3].submodule.as_ref().expect("dirty only: no index line");
        assert!(dirty.dirty && dirty.old == dirty.new);
    }
}
