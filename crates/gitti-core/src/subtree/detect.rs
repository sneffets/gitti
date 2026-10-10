//! Finding the subtrees of a history: git keeps no list of them, only the trailers
//! `git subtree` writes into its commit messages —
//!
//! - `git subtree add` (and `split --rejoin`): a merge commit "Add 'dir/' from commit '…'"
//!   (rejoin: "Split 'dir/' into commit '…'") with `git-subtree-dir`, `git-subtree-mainline`
//!   and `git-subtree-split`;
//! - `--squash`: a squash commit "Squashed 'dir/' changes from a..b" / "content from commit
//!   b" (`git-subtree-dir`, `git-subtree-split`, no mainline) that a merge commit "Merge
//!   commit '<squash>' as 'dir'" (no trailers) brings in;
//! - `git subtree merge` / `pull` without `--squash`: a plain `git merge -Xsubtree` — no
//!   trailers. Such a merge is recognised by its second parent: a commit of the subtree's
//!   own history whose tree is the folder's tree in the merge.
//!
//! [`scan`] walks the history in-process (gix, no git process), newest first, reading every
//! commit message once; [`Scan::update`] folds only the commits a new HEAD added on top of
//! the scanned one, so the scan runs in full once per history, not per commit.

use std::collections::{BTreeMap, HashMap};

use gix::ObjectId;
use gix::bstr::ByteSlice as _;

use super::{Subtree, Update};

/// The trailers of one commit message (the last value of each wins, like `git subtree`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trailers {
    /// `git-subtree-dir`, normalised ([`normalize_dir`]).
    pub dir: String,
    /// `git-subtree-mainline`: set on add / rejoin commits, not on squash commits.
    pub mainline: Option<String>,
    /// `git-subtree-split`: the subtree commit that was added / merged / split.
    pub split: Option<String>,
}

/// The `git-subtree-*` trailers of `message` (`None`: no `git-subtree-dir` line).
pub fn parse_trailers(message: &str) -> Option<Trailers> {
    let mut t = Trailers::default();
    let mut has_dir = false;
    for line in message.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(v) = line.strip_prefix("git-subtree-dir:") {
            t.dir = normalize_dir(v.trim());
            has_dir = !t.dir.is_empty();
        } else if let Some(v) = line.strip_prefix("git-subtree-mainline:") {
            t.mainline = Some(v.trim().to_string()).filter(|v| !v.is_empty());
        } else if let Some(v) = line.strip_prefix("git-subtree-split:") {
            t.split = Some(v.trim().to_string()).filter(|v| !v.is_empty());
        }
    }
    has_dir.then_some(t)
}

/// A folder as `git subtree` names it: no leading `./`, no trailing or doubled `/`.
pub fn normalize_dir(dir: &str) -> String {
    let parts: Vec<&str> = dir
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    parts.join("/")
}

/// What the scan keeps of a subtree (the rest of [`Subtree`] is filled in per call).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Found {
    split: Option<ObjectId>,
    squash: bool,
    commit: ObjectId,
    time: i64,
    update: Update,
    added: ObjectId,
    /// Every commit that brought it in, oldest first.
    updates: Vec<(ObjectId, Update)>,
}

/// The subtrees found in the history of one HEAD, kept to fold newer commits into.
#[derive(Debug, Clone, Default)]
pub struct Scan {
    pub head: Option<ObjectId>,
    found: BTreeMap<String, Found>,
    /// Squash commits → their folder (the merge that brings one in names no folder).
    squashes: HashMap<ObjectId, String>,
}

/// A commit that matters to the scan: one with trailers, or a merge.
struct Event {
    id: ObjectId,
    parents: Vec<ObjectId>,
    time: i64,
    subject: String,
    trailers: Option<Trailers>,
}

impl Scan {
    /// The subtrees, by folder (without `present` / `source`, see [`super::Repo::subtrees`]).
    pub fn subtrees(&self) -> Vec<Subtree> {
        self.found
            .iter()
            .map(|(prefix, f)| Subtree {
                prefix: prefix.clone(),
                split: f.split,
                squash: f.squash,
                commit: f.commit,
                time: f.time,
                update: f.update,
                added: f.added,
                updates: f.updates.clone(),
                present: true,
                source: None,
            })
            .collect()
    }

    /// The scan of `head`: the commits `head` has on top of this scan's HEAD folded in when
    /// that is an ancestor, else a full walk.
    pub fn update(&self, repo: &gix::Repository, head: Option<ObjectId>) -> Scan {
        let Some(head) = head else {
            return Scan::default();
        };
        if self.head == Some(head) {
            return self.clone();
        }
        if let Some(old) = self.head
            && let Some(events) = walk(repo, head, Some(old))
        {
            let mut next = self.clone();
            next.head = Some(head);
            next.fold(repo, events);
            return next;
        }
        let mut next = Scan {
            head: Some(head),
            ..Default::default()
        };
        if let Some(events) = walk(repo, head, None) {
            next.fold(repo, events);
        }
        next
    }

    /// Applies `events` (newest first, as walked) oldest first: the newest word on a
    /// folder wins.
    fn fold(&mut self, repo: &gix::Repository, events: Vec<Event>) {
        for e in events.into_iter().rev() {
            match &e.trailers {
                Some(t) => self.trailer_commit(&e, t),
                None => self.merge_commit(repo, &e),
            }
        }
    }

    fn trailer_commit(&mut self, e: &Event, t: &Trailers) {
        let split = t.split.as_deref().and_then(|s| ObjectId::from_hex(s.as_bytes()).ok());
        let known = self.found.get(&t.dir).cloned();
        if t.mainline.is_none() {
            // A squash commit; the merge bringing it in comes next.
            self.squashes.insert(e.id, t.dir.clone());
            let (update, added) = match &known {
                Some(k) => (Update::Merged, k.added),
                None => (Update::Added, e.id),
            };
            let mut updates = known.map(|k| k.updates).unwrap_or_default();
            updates.push((e.id, update));
            self.found.insert(
                t.dir.clone(),
                Found {
                    split,
                    squash: true,
                    commit: e.id,
                    time: e.time,
                    update,
                    added,
                    updates,
                },
            );
            return;
        }
        // An add or a rejoin merge; with `--squash` its second parent is a squash commit.
        let squash = e
            .parents
            .get(1)
            .is_some_and(|p| self.squashes.get(p) == Some(&t.dir));
        let update = if e.subject.starts_with("Add '") {
            Update::Added
        } else if e.subject.starts_with("Split '") || known.is_some() {
            Update::Rejoined
        } else {
            Update::Added
        };
        let added = match (&known, update) {
            (Some(k), Update::Rejoined) => k.added,
            _ => e.id,
        };
        let mut updates = known.map(|k| k.updates).unwrap_or_default();
        updates.push((e.id, update));
        self.found.insert(
            t.dir.clone(),
            Found {
                split,
                squash,
                commit: e.id,
                time: e.time,
                update,
                added,
                updates,
            },
        );
    }

    fn merge_commit(&mut self, repo: &gix::Repository, e: &Event) {
        let Some(&second) = e.parents.get(1) else {
            return;
        };
        // "Merge commit '<squash>' as 'dir'": brings a squash commit in.
        if let Some(dir) = self.squashes.get(&second).cloned() {
            if let Some(f) = self.found.get_mut(&dir) {
                f.commit = e.id;
                f.time = e.time;
                f.updates.push((e.id, f.update));
            }
            return;
        }
        // A `git subtree merge` / `pull` without squash: the second parent's tree is the
        // folder's tree in the merge.
        let plain: Vec<String> = self
            .found
            .iter()
            .filter(|(_, f)| !f.squash)
            .map(|(d, _)| d.clone())
            .collect();
        if plain.is_empty() {
            return;
        }
        let Some(theirs) = tree_of(repo, second) else {
            return;
        };
        let Some(ours) = repo.find_commit(e.id).ok().and_then(|c| c.tree().ok()) else {
            return;
        };
        for dir in plain {
            let at = ours
                .lookup_entry_by_path(&dir)
                .ok()
                .flatten()
                .map(|entry| entry.object_id());
            if at == Some(theirs)
                && let Some(f) = self.found.get_mut(&dir)
            {
                f.split = Some(second);
                f.commit = e.id;
                f.time = e.time;
                f.update = Update::Merged;
                f.updates.push((e.id, Update::Merged));
            }
        }
    }
}

fn tree_of(repo: &gix::Repository, id: ObjectId) -> Option<ObjectId> {
    repo.find_commit(id).ok()?.tree_id().ok().map(|t| t.detach())
}

/// The commits of `head` (not of `hidden`) that matter to a scan, newest first; `None`
/// when `hidden` is given but is no ancestor of `head` (the caller walks in full then).
fn walk(repo: &gix::Repository, head: ObjectId, hidden: Option<ObjectId>) -> Option<Vec<Event>> {
    use gix::revision::walk::Sorting;
    let mut platform = repo
        .rev_walk([head])
        .sorting(Sorting::ByCommitTime(Default::default()));
    if let Some(h) = hidden {
        platform = platform.with_hidden([h]);
    }
    let walk = platform.all().ok()?;
    let mut events = Vec::new();
    let mut reaches_hidden = hidden.is_none();
    for info in walk {
        let Ok(info) = info else {
            // A missing object (shallow / partial clone): what was read so far counts.
            break;
        };
        let parents: Vec<ObjectId> = info.parent_ids.iter().copied().collect();
        if let Some(h) = hidden
            && parents.contains(&h)
        {
            reaches_hidden = true;
        }
        let Ok(commit) = repo.find_commit(info.id) else {
            continue;
        };
        let has_trailer = buf_message(&commit.data)
            .find(b"\ngit-subtree-dir:")
            .is_some();
        if !has_trailer && parents.len() < 2 {
            continue;
        }
        let message = commit.message_raw_sloppy().to_str_lossy().into_owned();
        let trailers = if has_trailer { parse_trailers(&message) } else { None };
        if trailers.is_none() && parents.len() < 2 {
            continue;
        }
        let time = info
            .commit_time
            .or_else(|| commit.time().ok().map(|t| t.seconds))
            .unwrap_or(0);
        events.push(Event {
            id: info.id,
            subject: message.lines().next().unwrap_or("").to_string(),
            parents,
            time,
            trailers,
        });
    }
    // `head` already contains `hidden`… or not at all: then fold nothing onto it.
    reaches_hidden.then_some(events)
}

/// The bytes after a commit's header (its message), searched before decoding.
fn buf_message(buf: &[u8]) -> &[u8] {
    match buf.find(b"\n\n") {
        Some(i) => &buf[i + 1..],
        None => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn trailers_of_add_squash_and_rejoin_messages() {
        let add = "Add 'lib/' from commit 'abc'\n\ngit-subtree-dir: lib\n\
                   git-subtree-mainline: 111\ngit-subtree-split: 222\n";
        assert_eq!(
            parse_trailers(add),
            Some(Trailers {
                dir: "lib".into(),
                mainline: Some("111".into()),
                split: Some("222".into()),
            })
        );
        let squash = "Squashed 'vendor/x/' content from commit 2b3c\n\n\
                      git-subtree-dir: vendor/x/\ngit-subtree-split: 2b3c4d\n";
        let t = parse_trailers(squash).unwrap();
        assert_eq!(t.dir, "vendor/x");
        assert_eq!(t.mainline, None);
        assert_eq!(t.split.as_deref(), Some("2b3c4d"));
        // Only at the start of a line.
        assert_eq!(parse_trailers("see git-subtree-dir: x"), None);
        assert_eq!(parse_trailers("plain message\n\nbody\n"), None);
        // An empty folder is no subtree.
        assert_eq!(parse_trailers("x\n\ngit-subtree-dir: /\n"), None);
        // CRLF messages.
        let crlf = "m\r\n\r\ngit-subtree-dir: a/b\r\ngit-subtree-split: 33\r\n";
        assert_eq!(parse_trailers(crlf).unwrap().dir, "a/b");
    }

    #[gitti_test::test]
    fn folders_are_normalised_like_git_subtree() {
        assert_eq!(normalize_dir("lib/"), "lib");
        assert_eq!(normalize_dir("./lib//x/"), "lib/x");
        assert_eq!(normalize_dir("a/./b"), "a/b");
        assert_eq!(normalize_dir(""), "");
    }

    #[gitti_test::test]
    fn the_message_starts_after_the_header() {
        let raw = b"tree 1\nparent 2\nauthor a\n\nsubject\n\ngit-subtree-dir: x\n";
        assert!(buf_message(raw).find(b"\ngit-subtree-dir:").is_some());
        assert!(buf_message(b"tree 1\nauthor a\n").is_empty());
    }
}
