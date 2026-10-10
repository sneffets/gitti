//! Push with options and the palette's second steps, as in gitti: "Push…"
//! → a remote / all remotes / Options… (the dialog), "Pull from…" and "Fetch from…" → a
//! remote. Plus the palette entries of gitti's menu bar that gitty has too
//! (`gitti_core::palette`), with the Branch menu's pickers (merge into / rebase
//! the current branch onto a branch or tag). The logic is `gitti_core::push`.

use gitti_core::RefKind;
use gitti_core::push::{
    PUSH_CHOICES_KEY, PushChoices, PushOptions, PushTags, Upstream, quick_pushes,
};
use gitti_core::rows::DiffRow;

use super::{After, App, Focus};
use crate::modal::{Modal, PickItem};

impl App {
    fn remote_names(&self) -> Vec<String> {
        self.remotes.iter().map(|r| r.name.clone()).collect()
    }

    pub(super) fn upstream_split(&self, branch: &str) -> Option<Upstream> {
        let short = self.tracking_of(branch)?.upstream.clone()?;
        Upstream::parse(&short, &self.remote_names())
    }

    fn run_push(&mut self, opts: PushOptions) {
        let no_verify = opts.no_verify;
        self.run_push_op("Push", After::Summary, no_verify, move |r| r.push_with(&opts));
    }

    /// "Push…" (and the main push without an upstream): the remotes to push the current
    /// branch to, Set Upstream… when it has none, then Options….
    pub fn push_step(&mut self) {
        let mut items: Vec<PickItem> = match self.current_branch().map(str::to_string) {
            Some(branch) => {
                quick_pushes(&branch, self.upstream_split(&branch).as_ref(), &self.remote_names())
                    .into_iter()
                    .map(|q| {
                        let opts = q.opts;
                        PickItem::new(q.label, q.detail, move |app: &mut App| app.run_push(opts))
                    })
                    .collect()
            }
            None => Vec::new(),
        };
        // Without an upstream: a remote above (it becomes the upstream), or an
        // existing remote branch as the upstream.
        let no_upstream = self
            .current_branch()
            .filter(|b| self.tracking_of(b).is_none_or(|t| t.upstream.is_none()))
            .map(str::to_string);
        if let Some(b) = &no_upstream {
            let b2 = b.clone();
            items.push(PickItem::new(
                "Set Upstream…",
                format!("a remote branch for {b}"),
                move |app: &mut App| app.upstream_picker(b2.clone()),
            ));
        }
        items.push(PickItem::new(
            "Options…",
            "remotes, branch name, upstream, force, tags, hooks",
            |app: &mut App| app.push_dialog(None),
        ));
        let title = match (self.current_branch(), &no_upstream) {
            (Some(b), Some(_)) => format!("Push {b} to… (no upstream yet)"),
            (Some(b), None) => format!("Push {b} to…"),
            (None, _) => "Push (HEAD is detached)".to_string(),
        };
        self.modal = Some(Modal::picker(title, items));
    }

    /// The Push… dialog for `branch` (default: the current one); the choices are
    /// remembered per repository (not force), shared with gitti.
    pub fn push_dialog(&mut self, branch: Option<String>) {
        let Some(branch) = branch.or_else(|| self.current_branch().map(str::to_string)) else {
            self.set_error("HEAD is detached — no branch to push (switch to a branch first)".into());
            return;
        };
        let remotes = self.remote_names();
        if remotes.is_empty() {
            self.set_error("No remote to push to — add one first".into());
            return;
        }
        let remembered: Option<PushChoices> =
            gitti_core::store::load_repo_pref(self.repo.workdir(), PUSH_CHOICES_KEY);
        let upstream = self.upstream_split(&branch);
        let d = PushOptions::defaults(&branch, upstream.as_ref(), &remotes, None, remembered.as_ref());
        // What the user sees now: a force push pins its lease to it.
        let seen: Vec<_> = self
            .refs
            .iter()
            .filter(|r| r.kind == RefKind::RemoteBranch)
            .cloned()
            .collect();
        let labels: Vec<String> = remotes.iter().map(|r| format!("Push to {r}")).collect();
        let mut checks: Vec<(&str, bool)> = labels
            .iter()
            .zip(&remotes)
            .map(|(l, r)| (l.as_str(), d.remotes.contains(r)))
            .collect();
        checks.extend([
            ("Set upstream (-u, first remote)", d.set_upstream),
            ("Force with lease", false),
            ("Push tags of these commits (--follow-tags)", d.tags == PushTags::Follow),
            ("Push all tags (--tags)", d.tags == PushTags::All),
            (self.no_verify_label(), d.no_verify),
        ]);
        let title = match &upstream {
            Some(u) => format!("Push {branch} (upstream {})", u.short()),
            None => format!("Push {branch} (no upstream yet)"),
        };
        let n = remotes.len();
        self.modal = Some(Modal::input_checks(
            title,
            vec![
                ("Local branch", branch.as_str()),
                ("Remote branch (empty = same name)", d.remote_branch.as_str()),
            ],
            checks,
            move |app, values, flags| {
                let chosen: Vec<String> = remotes
                    .iter()
                    .zip(&flags)
                    .filter(|(_, on)| **on)
                    .map(|(r, _)| r.clone())
                    .collect();
                let c = &flags[n..];
                let mut opts = PushOptions {
                    remotes: chosen,
                    branch: values[0].trim().to_string(),
                    remote_branch: values[1].trim().to_string(),
                    set_upstream: c[0],
                    force_with_lease: c[1],
                    tags: match (c[2], c[3]) {
                        (_, true) => PushTags::All,
                        (true, false) => PushTags::Follow,
                        _ => PushTags::None,
                    },
                    no_verify: c[4],
                    leases: Default::default(),
                };
                opts.pin_leases(&seen);
                if opts.remotes.is_empty() {
                    app.set_error("Push: no remote chosen".into());
                    return;
                }
                gitti_core::store::save_repo_pref(
                    app.repo.workdir(),
                    PUSH_CHOICES_KEY,
                    &opts.choices(),
                );
                app.run_push(opts);
            },
        ));
    }

    /// "Pull from…": the upstream, or one remote. Without an upstream: the
    /// remotes, then Set Upstream… instead of the plain Pull.
    pub fn pull_step(&mut self) {
        let branch = self.current_branch().unwrap_or_default().to_string();
        let upstream = self.upstream_split(&branch);
        let mut items = Vec::new();
        if let Some(u) = &upstream {
            items.push(PickItem::new(
                "Pull",
                format!("from the upstream {}", u.short()),
                |app: &mut App| app.pull(),
            ));
        }
        let mut remotes = self.remote_names();
        if let Some(u) = &upstream {
            remotes.sort_by_key(|r| *r != u.remote);
        }
        for remote in remotes {
            let rbranch = match &upstream {
                Some(u) if u.remote == remote => u.branch.clone(),
                _ => branch.clone(),
            };
            let rm = remote.clone();
            items.push(PickItem::new(
                format!("Pull from {remote}"),
                format!("{remote}/{rbranch}"),
                move |app: &mut App| {
                    app.run_op("Pull", true, After::Nothing, move |r| r.pull_from_saved(&rm))
                },
            ));
        }
        let title = if upstream.is_none() && !branch.is_empty() {
            let b = branch.clone();
            items.push(PickItem::new(
                "Set Upstream…",
                format!("a remote branch for {branch}"),
                move |app: &mut App| app.upstream_picker(b.clone()),
            ));
            format!("Pull {branch} from… (no upstream yet)")
        } else {
            "Pull from…".to_string()
        };
        self.modal = Some(Modal::picker(title, items));
    }

    /// "Fetch from…": all remotes, or one.
    pub fn fetch_step(&mut self) {
        let mut items = vec![PickItem::new(
            "Fetch All Remotes",
            "git fetch --all --prune",
            |app: &mut App| app.fetch(),
        )];
        for remote in self.remote_names() {
            let rm = remote.clone();
            items.push(PickItem::new(
                format!("Fetch {remote}"),
                "git fetch --prune",
                move |app: &mut App| {
                    app.run_op("Fetch", true, After::Nothing, move |r| r.fetch_remote(&rm))
                },
            ));
        }
        self.modal = Some(Modal::picker("Fetch from…", items));
    }

    /// "Delete Branch…": a local branch other than the current one (merged only; git's
    /// refusal offers a force delete, as the branch menu does).
    fn delete_branch_picker(&mut self) {
        let current = self.current_branch().map(str::to_string);
        let items: Vec<PickItem> = self
            .refs
            .iter()
            .filter(|r| r.kind == RefKind::LocalBranch && Some(&r.short_name) != current.as_ref())
            .map(|r| {
                let name = r.short_name.clone();
                PickItem::new(name.clone(), "local branch", move |app: &mut App| {
                    app.delete_branch(name)
                })
            })
            .collect();
        self.modal = Some(Modal::picker("Delete branch…", items));
    }

    /// The current branch, or says that HEAD is detached (the palette's "… Current
    /// Branch…" commands).
    fn current_branch_or_info(&mut self) -> Option<String> {
        let b = self.current_branch().map(str::to_string);
        if b.is_none() {
            self.set_info("HEAD is detached — no current branch");
        }
        b
    }

    /// What HEAD can merge or rebase onto, as in gitti's pickers: the local branches but
    /// the current one, the remote branches (not `origin/HEAD`), the tags; picking one
    /// runs `run` with its name.
    fn branch_and_tag_items(&self, run: fn(&mut App, String)) -> Vec<PickItem> {
        let current = self.current_branch();
        self.refs
            .iter()
            .filter_map(|r| {
                let kind = match r.kind {
                    RefKind::LocalBranch if current != Some(r.short_name.as_str()) => {
                        "local branch"
                    }
                    RefKind::RemoteBranch if !r.short_name.ends_with("/HEAD") => "remote branch",
                    RefKind::Tag => "tag",
                    _ => return None,
                };
                let name = r.short_name.clone();
                Some(PickItem::new(name.clone(), kind, move |app: &mut App| run(app, name)))
            })
            .collect()
    }

    /// "Merge Branch into Current…": a branch or tag → the merge of its branch
    /// menu. A detached HEAD merges too (git does), the title says so.
    fn merge_picker(&mut self) {
        let items = self.branch_and_tag_items(App::merge_into_head);
        if items.is_empty() {
            self.set_info("Nothing to merge: no other branch or tag");
            return;
        }
        let title = match self.current_branch() {
            Some(b) => format!("Merge into {b}…"),
            None => "Merge into HEAD (detached)…".to_string(),
        };
        self.modal = Some(Modal::picker(title, items));
    }

    /// "Rebase Current Branch onto…": a branch or tag → the rebase of its branch
    /// menu.
    fn rebase_picker(&mut self) {
        let Some(current) = self.current_branch_or_info() else {
            return;
        };
        let items = self.branch_and_tag_items(App::rebase_head_onto);
        if items.is_empty() {
            self.set_info("Nothing to rebase onto: no other branch or tag");
            return;
        }
        self.modal = Some(Modal::picker(format!("Rebase {current} onto…"), items));
    }

    pub(super) fn latest_stash(&mut self, pop: bool) {
        if self.stashes.is_empty() {
            self.set_info("No stashes");
            return;
        }
        let label = if pop { "Pop stash" } else { "Apply stash" };
        self.run_op(label, true, After::Nothing, move |r| r.stash_apply(0, pop));
    }

    /// Palette entries for gitti's menu-bar commands gitty has as well.
    pub fn menu_palette_items(&self) -> Vec<PickItem> {
        use gitti_core::keys::Command as KC;
        vec![
            PickItem::new("Push…", "a remote, all remotes, options", |app: &mut App| {
                app.push_step()
            }),
            PickItem::new("Pull from…", "a remote", |app: &mut App| app.pull_step()),
            PickItem::new("Pull with…", "merge, rebase or fast-forward only", |app: &mut App| {
                app.pull_with_menu()
            }),
            PickItem::new("Fetch from…", "a remote", |app: &mut App| app.fetch_step()),
            PickItem::new(
                "Reset Remote Branch to Selected Commit…",
                "a push with lease; the local branch stays",
                |app: &mut App| app.reset_remote_selected(),
            ),
            PickItem::new("Add Remote…", "name and URL", |app: &mut App| app.add_remote_dialog()),
            PickItem::cmd("Show Error Details", KC::ErrorDetails, |app: &mut App| {
                if !app.show_error_details() {
                    app.set_info("No error to show");
                }
            }),
            PickItem::new("Create Tag at HEAD…", "", |app: &mut App| {
                app.create_tag_dialog("HEAD".into())
            }),
            PickItem::new("Delete Branch…", "", |app: &mut App| app.delete_branch_picker()),
            PickItem::new("Merge Branch into Current…", "a branch or tag", |app: &mut App| {
                app.merge_picker()
            }),
            PickItem::new("Rebase Current Branch onto…", "a branch or tag", |app: &mut App| {
                app.rebase_picker()
            }),
            PickItem::new("Rename Current Branch…", "", |app: &mut App| {
                if let Some(b) = app.current_branch_or_info() {
                    app.rename_branch_dialog(b);
                }
            }),
            PickItem::new("Set Upstream of Current Branch…", "a remote branch", |app: &mut App| {
                if let Some(b) = app.current_branch_or_info() {
                    app.upstream_picker(b);
                }
            }),
            PickItem::cmd("Go to Working Directory", KC::FocusMessage, |app: &mut App| {
                app.select_row(0);
                app.focus = Focus::Detail;
            }),
            PickItem::new("Go to HEAD", "", |app: &mut App| {
                let row = app
                    .head
                    .id
                    .and_then(|id| app.history.commits.iter().position(|c| c.id == id));
                match row {
                    Some(row) => {
                        app.select_row(row);
                        app.focus = Focus::Commits;
                    }
                    None => app.set_info("HEAD is not in the loaded history"),
                }
            }),
            PickItem::new("Next Commit", "", |app: &mut App| {
                app.select_row(app.selected + 1)
            })
            .keys("j / ↓"),
            PickItem::new("Previous Commit", "", |app: &mut App| {
                app.select_row(app.selected.saturating_sub(1))
            })
            .keys("k / ↑"),
            PickItem::cmd("Next File", KC::NextFile, |app: &mut App| {
                app.jump_diff(true, |r| matches!(r, DiffRow::File { .. }))
            }),
            PickItem::cmd("Previous File", KC::PrevFile, |app: &mut App| {
                app.jump_diff(false, |r| matches!(r, DiffRow::File { .. }))
            }),
            PickItem::cmd("Next Hunk", KC::NextHunk, |app: &mut App| {
                app.jump_diff(true, |r| matches!(r, DiffRow::Hunk { .. }))
            }),
            PickItem::cmd("Previous Hunk", KC::PrevHunk, |app: &mut App| {
                app.jump_diff(false, |r| matches!(r, DiffRow::Hunk { .. }))
            }),
            PickItem::new("Stash Including Untracked", "git stash -u", |app: &mut App| {
                app.run_op("Stash", true, After::Nothing, |r| r.stash_save("", true))
            }),
            PickItem::new("Apply Latest Stash", "stash@{0}", |app: &mut App| {
                app.latest_stash(false)
            }),
            PickItem::new("Pop Latest Stash", "stash@{0}", |app: &mut App| {
                app.latest_stash(true)
            }),
        ]
    }
}
