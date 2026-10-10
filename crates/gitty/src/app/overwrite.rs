//! A refused push and overwriting a remote branch, as in gitti (`gitti_core::overwrite`):
//! the ways out of a refused push (Pull, Push with Lease…, Fetch, Then Decide…), the
//! confirmation that lists what the remote loses, and "Reset <remote>/<branch> to
//! Here…" (commit menu, a remote branch's menu, the palette).

use gitti_core::RefKind;
use gitti_core::overwrite::{Overwrite, PushRejected, Source, WayOut};
use gitti_core::pull::PullStrategy;
use gitti_core::push::{Lease, Upstream};

use super::{After, App, Msg};
use crate::modal::{MenuItem, Modal, PickItem};

impl App {
    /// git refused a push (or the main push knew it would): why, in plain words, and the
    /// ways out; the server's own refusal is an error naming it.
    pub(crate) fn push_rejected(&mut self, rej: PushRejected) {
        self.dismiss_warnings();
        let ways = rej.ways_out();
        if ways.is_empty() {
            let details = if rej.output.trim().is_empty() {
                rej.explain()
            } else {
                format!("{rej}\n\n{}", rej.output.trim_end())
            };
            self.errors.failed("Push", details);
            self.set_error(rej.to_string());
            return;
        }
        self.set_info(rej.to_string());
        let mut items = MenuItem::notes(&rej.explain(), 70);
        items.push(MenuItem::separator());
        for w in ways {
            let key = match w {
                WayOut::Pull(PullStrategy::Rebase) => 'r',
                WayOut::Pull(_) => 'm',
                WayOut::PushWithLease => 'f',
                WayOut::FetchThenDecide => 'F',
            };
            let label = format!("{} — {}", rej.way_label(w), rej.way_detail(w));
            let r = rej.clone();
            items.push(MenuItem::new(label, move |app: &mut App| app.push_way_out(r, w)).key(key));
        }
        self.modal = Some(Modal::menu(rej.title(), items));
    }

    fn push_way_out(&mut self, rej: PushRejected, way: WayOut) {
        match way {
            WayOut::Pull(strategy) => {
                self.run_op("Pull", true, After::Nothing, move |r| rej.pull(r, strategy))
            }
            WayOut::PushWithLease => match rej.plan.clone() {
                Some(plan) => self.overwrite_confirm(plan),
                None => self.push_way_out(rej, WayOut::FetchThenDecide),
            },
            WayOut::FetchThenDecide => self.run_op("Fetch", true, After::Nothing, move |r| {
                let plan = rej.fetch_then_plan(r)?;
                Err(super::FollowUp::Overwrite(Box::new(plan)).into())
            }),
        }
    }

    /// The confirmation of an overwrite: what moves, what the remote loses (with
    /// authors), the lease pinned to what is listed; then the push.
    pub(crate) fn overwrite_confirm(&mut self, ov: Overwrite) {
        let (title, body, ok) = (ov.title(), ov.describe(), ov.ok_label());
        self.modal = Some(Modal::confirm(title, body, ok, move |app: &mut App| {
            let no_verify = ov.no_verify;
            app.run_push_op("Push with lease", After::Summary, no_verify, move |r| {
                r.overwrite(&ov)
            })
        }));
    }

    /// Reads (in the background, local git only) what overwriting `<remote>/<branch>`
    /// with `source` does — the lease pinned to the remote branch as shown now — then
    /// the confirmation.
    pub(crate) fn plan_overwrite(&mut self, remote: String, branch: String, source: Source) {
        let lease = Lease::seen(&self.refs, &remote, &branch);
        let short = format!("{remote}/{branch}");
        let local = self
            .tracking
            .iter()
            .find(|t| t.upstream.as_deref() == Some(short.as_str()))
            .map(|t| t.branch.clone());
        // A force push of a branch without upstream sets it (as Push does).
        let set_upstream = matches!(&source, Source::Branch(b)
            if self.tracking_of(b).is_none_or(|t| t.upstream.is_none()));
        let repo = self.repo.clone();
        self.start_load(
            move || {
                let mut plan =
                    repo.overwrite_plan(&remote, &branch, source, lease, local.as_deref())?;
                plan.set_upstream = set_upstream;
                anyhow::Ok(plan)
            },
            |plan| Msg::OverwritePlan(Box::new(plan)),
        );
    }

    pub(crate) fn overwrite_planned(&mut self, plan: anyhow::Result<Overwrite>) {
        match plan {
            Ok(plan) => self.overwrite_confirm(plan),
            Err(e) => self.set_error(format!("{e:#}")),
        }
    }

    /// "Reset <remote>/<branch> to Here…": `remote_branch` (`origin/main`) onto commit
    /// `rev` (hex); the local branch stays as it is.
    pub(crate) fn reset_remote_branch(&mut self, remote_branch: String, rev: String) {
        let remotes: Vec<String> = self.remotes.iter().map(|r| r.name.clone()).collect();
        match Upstream::parse(&remote_branch, &remotes) {
            Some(u) => self.plan_overwrite(u.remote, u.branch, Source::Commit(rev)),
            None => self.set_error(format!("{remote_branch}: no such remote")),
        }
    }

    /// The current branch's upstream (`origin/main`), from the last refresh.
    pub(crate) fn head_upstream(&self) -> Option<String> {
        self.tracking_of(self.current_branch()?)?.upstream.clone()
    }

    /// The remote branches to reset to commit `rev`: the current branch's upstream first.
    pub(crate) fn reset_remote_picker(&mut self, rev: String) {
        let upstream = self.head_upstream();
        let mut names: Vec<String> = self
            .refs
            .iter()
            .filter(|r| r.kind == RefKind::RemoteBranch)
            .map(|r| r.short_name.clone())
            .collect();
        if names.is_empty() {
            self.set_info("No remote branches — fetch or push one first");
            return;
        }
        names.sort_by_key(|n| Some(n) != upstream.as_ref());
        let short = rev[..rev.len().min(7)].to_string();
        let items = names
            .into_iter()
            .map(|n| {
                let detail = if Some(&n) == upstream.as_ref() {
                    "upstream of the current branch"
                } else {
                    ""
                };
                let (n2, rev) = (n.clone(), rev.clone());
                PickItem::new(format!("Reset {n} to {short}…"), detail, move |app: &mut App| {
                    app.reset_remote_branch(n2, rev)
                })
            })
            .collect();
        self.modal = Some(Modal::picker(format!("Reset which remote branch to {short}?"), items));
    }

    /// The selected commit (hex), or why there is none (status line).
    fn selected_commit_or_info(&mut self) -> Option<String> {
        if self.history.is_worktree_row(self.selected) {
            self.set_info("Select a commit first — the working directory is selected");
            return None;
        }
        match self.history.commits.get(self.selected) {
            Some(c) if c.stash.is_some() || c.stash_helper => {
                self.set_info("A stash is selected — select a commit");
                None
            }
            Some(c) => Some(c.id.to_string()),
            None => {
                self.set_info("No commit selected");
                None
            }
        }
    }

    /// Palette: "Reset Remote Branch to Selected Commit…".
    pub(crate) fn reset_remote_selected(&mut self) {
        if let Some(rev) = self.selected_commit_or_info() {
            self.reset_remote_picker(rev);
        }
    }

    /// A remote branch's "Reset origin/x to Selected Commit…".
    pub(crate) fn reset_remote_to_selected(&mut self, remote_branch: String) {
        if let Some(rev) = self.selected_commit_or_info() {
            self.reset_remote_branch(remote_branch, rev);
        }
    }

    /// The commit menu's "Reset <upstream> to Here…" and "Reset Another / a Remote Branch
    /// to Here…".
    pub(crate) fn remote_reset_items(&self, sha: &str) -> Vec<MenuItem> {
        let mut v = Vec::new();
        let upstream = self.head_upstream();
        if let Some(u) = upstream.clone() {
            let s = sha.to_string();
            v.push(MenuItem::new(format!("Reset {u} to Here…"), move |app: &mut App| {
                app.reset_remote_branch(u, s)
            }));
        }
        if self.refs.iter().any(|r| r.kind == RefKind::RemoteBranch) {
            let s = sha.to_string();
            let label = if upstream.is_some() {
                "Reset Another Remote Branch to Here…"
            } else {
                "Reset a Remote Branch to Here…"
            };
            v.push(MenuItem::new(label, move |app: &mut App| app.reset_remote_picker(s)));
        }
        v
    }
}
