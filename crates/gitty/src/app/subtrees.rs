//! `git subtree` in gitty, like gitti's `app/subtrees.rs`: the SUBTREES sidebar entries
//! (found from the history, `gitti_core::subtree`, read in the background when HEAD moves)
//! and their menu (Pull…, Merge…, Push…, Split…, Show History, Edit Source…), the palette
//! commands (Add / Pull / Merge into / Push / Split Subtree…), "Add as Subtree…" /
//! "Merge into Subtree…" on commits and branches, and the dialogs — gitti's fields
//! (`subtree::form`) with the live plan under them (`subtree::plan`). The runs are
//! ordinary operations (cancellable with Esc, the split's progress after the busy label).

use gitti_core::ObjectId;
use gitti_core::subtree::{self, Action, Request, Subtree};

use super::{After, App, FollowUp, Msg};
use crate::modal::{Live, MenuItem, Modal, PickItem};

impl App {
    /// Reads the subtrees again in the background when HEAD moved (or `force`).
    pub(super) fn refresh_subtrees(&mut self, force: bool) {
        let head = self.head.id;
        if !force && self.subtrees_head == Some(head) {
            return;
        }
        if self.subtrees_loading {
            self.subtrees_queued = true;
            return;
        }
        self.subtrees_loading = true;
        self.loads_in_flight += 1;
        let repo = self.repo.clone();
        super::spawn(&self.tx, move || repo.subtrees(), move |r| Msg::Subtrees(head, r));
    }

    /// The list read by [`App::refresh_subtrees`] (for HEAD `head`).
    pub(super) fn on_subtrees(&mut self, head: Option<ObjectId>, result: anyhow::Result<Vec<Subtree>>) {
        self.loads_in_flight -= 1;
        self.subtrees_loading = false;
        self.subtrees_head = Some(head);
        // An unreadable history: no list (the refresh reports what is wrong).
        self.subtrees = result.unwrap_or_default();
        self.rebuild_sidebar();
        if std::mem::take(&mut self.subtrees_queued) {
            self.refresh_subtrees(true);
        }
    }

    /// The menu of the subtree at `index`.
    pub(crate) fn subtree_menu_items(&self, index: usize) -> Vec<MenuItem> {
        let Some(s) = self.subtrees.get(index) else {
            return Vec::new();
        };
        let gone = (!s.present).then_some("folder removed");
        let mut v = Vec::new();
        for (label, a, key) in [
            ("Pull…", Action::Pull, 'p'),
            ("Merge…", Action::Merge, 'm'),
            ("Push…", Action::Push, 'P'),
            ("Split…", Action::Split, 's'),
        ] {
            let r = Request::for_subtree(a, s);
            let item = MenuItem::new(label, move |app: &mut App| app.subtree_dialog(r)).key(key);
            v.push(if a == Action::Split { item } else { item.unless(gone) });
        }
        if !s.present {
            let r = Request::for_subtree(Action::Add, s);
            v.push(MenuItem::new("Add Again…", move |app: &mut App| app.subtree_dialog(r)).key('a'));
        }
        v.push(MenuItem::separator());
        let p = s.prefix.clone();
        v.push(
            MenuItem::new(format!("Show History of {}/", s.prefix), move |app: &mut App| {
                app.show_subtree_history(&p)
            })
            .key('h'),
        );
        let p = s.prefix.clone();
        v.push(MenuItem::new("Edit Source…", move |app: &mut App| app.edit_subtree_source(&p)).key('e'));
        v.push(MenuItem::separator());
        let p = s.prefix.clone();
        v.push(MenuItem::new("Copy Path", move |app: &mut App| app.copy(p)).key('y'));
        v
    }

    /// The SUBTREES header's menu: the palette's subtree commands.
    pub(super) fn subtree_section_items(&self) -> Vec<MenuItem> {
        Action::ALL
            .into_iter()
            .map(|a| MenuItem::new(a.menu_label(), move |app: &mut App| app.subtree_command(a)))
            .collect()
    }

    /// "Add as Subtree…" and "Merge into Subtree…" for a commit / branch / tag `rev` (the
    /// commit `target`); `remote`: a remote branch's remote and branch (its Add fetches
    /// from there, so Pull knows where it comes from later). Merge is greyed with the
    /// reason for a commit already in HEAD's history (git would only say "already up to
    /// date") — from the loaded history, no git (the key bar builds this every frame).
    pub(crate) fn subtree_ref_items(
        &self,
        rev: &str,
        target: Option<ObjectId>,
        remote: Option<(String, String)>,
    ) -> Vec<MenuItem> {
        let mut add = Request::new(Action::Add, "");
        match remote {
            Some((r, b)) => {
                add.repository = r;
                add.reference = b;
            }
            None => add.reference = rev.to_string(),
        }
        add.prefix = subtree::default_prefix("", &add.reference);
        if add.prefix.len() >= 40 && add.prefix.bytes().all(|b| b.is_ascii_hexdigit()) {
            add.prefix.clear();
        }
        let mut v = vec![MenuItem::new("Add as Subtree…", move |app: &mut App| app.subtree_dialog(add))];
        let rev = rev.to_string();
        let merged = target.and_then(|t| subtree::merge_unavailable(&self.history, &t, self.head.branch.as_deref()));
        match self.subtrees.as_slice() {
            [] => {}
            [one] => {
                let mut r = Request::for_subtree(Action::Merge, one);
                r.reference = rev;
                v.push(
                    MenuItem::new(format!("Merge into Subtree {}/…", one.prefix), move |app: &mut App| {
                        app.subtree_merge_dialog(r, target)
                    })
                    .unless(merged.as_deref()),
                );
            }
            _ => v.push(
                MenuItem::new("Merge into Subtree…", move |app: &mut App| {
                    app.merge_into_subtree_picker(rev, target)
                })
                .unless(merged.as_deref()),
            ),
        }
        v
    }

    /// Which subtree to merge `rev` (commit `target`) into (several of them).
    fn merge_into_subtree_picker(&mut self, rev: String, target: Option<ObjectId>) {
        let items = self
            .subtrees
            .iter()
            .filter(|s| s.present)
            .map(|s| {
                let mut r = Request::for_subtree(Action::Merge, s);
                r.reference = rev.clone();
                PickItem::new(format!("{}/", s.prefix), s.detail(), move |app: &mut App| {
                    app.subtree_merge_dialog(r, target)
                })
            })
            .collect();
        self.modal = Some(Modal::picker(format!("Merge {rev} into which subtree?"), items));
    }

    /// A palette / section command: Add opens its dialog; the others act on the one
    /// subtree there is, ask which with several, and open with an empty folder without
    /// any (a subtree without trailers, or — Split — any folder).
    pub(super) fn subtree_command(&mut self, a: Action) {
        let candidates: Vec<Subtree> = match a {
            Action::Add => Vec::new(),
            Action::Split => self.subtrees.clone(),
            _ => self.subtrees.iter().filter(|s| s.present).cloned().collect(),
        };
        match candidates.as_slice() {
            [] => self.subtree_dialog(Request::new(a, "")),
            [one] => self.subtree_dialog(Request::for_subtree(a, one)),
            many => {
                let items = many
                    .iter()
                    .map(|s| {
                        let r = Request::for_subtree(a, s);
                        PickItem::new(format!("{}/", s.prefix), s.detail(), move |app: &mut App| {
                            app.subtree_dialog(r)
                        })
                    })
                    .collect();
                let title = match a {
                    Action::Pull => "Pull which subtree?",
                    Action::Merge => "Merge into which subtree?",
                    Action::Push => "Push which subtree?",
                    _ => "Split which subtree? (any other folder: type it in the dialog)",
                };
                self.modal = Some(Modal::picker(title, items));
            }
        }
    }

    /// The facts the dialogs' live text needs, as shown now; the loaded history lets the
    /// Merge dialog warn about a commit typed into it (`subtree::resolve_commit`).
    fn subtree_context(&self) -> subtree::Context {
        let mut ctx = subtree::Context::of(
            self.repo.workdir(),
            &self.head,
            self.repo_state,
            &self.working,
            &self.subtrees,
            &self.remotes,
            &self.refs,
        );
        ctx.history = Some(self.history.clone());
        ctx
    }

    /// The dialog of `req.action`, filled from `req`; under its fields what Enter does
    /// and why it can't run now, the remotes listed for the repository field.
    pub(crate) fn subtree_dialog(&mut self, req: Request) {
        self.subtree_dialog_with(req, None)
    }

    /// The Merge dialog for commit `target` (a commit / branch / tag menu's): it warns when
    /// the commit doesn't continue the subtree's history (`subtree::merge_warning`).
    fn subtree_merge_dialog(&mut self, req: Request, target: Option<ObjectId>) {
        let warning = target.and_then(|t| {
            let s = self.subtrees.iter().find(|s| s.prefix == subtree::normalize_prefix(&req.prefix))?;
            subtree::merge_warning(&self.history, s, &t, &req.reference)
        });
        self.subtree_dialog_with(req, warning)
    }

    fn subtree_dialog_with(&mut self, req: Request, warning: Option<subtree::MergeWarning>) {
        let a = req.action;
        let mut ctx = self.subtree_context();
        ctx.merge_warning = warning;
        let form = subtree::form(&req);
        let fields: Vec<(&str, &str)> = form.fields.iter().map(|(l, v)| (*l, v.as_str())).collect();
        let checks: Vec<(&str, bool)> = form.checks.clone();
        let remotes: Vec<String> = match a {
            Action::Add | Action::Pull | Action::Push => self.remotes.iter().map(|r| r.name.clone()).collect(),
            _ => Vec::new(),
        };
        let modal = Modal::input_checks(a.title(), fields, checks, move |app, values, checks| {
            app.run_subtree(subtree::from_form(a, &values, &checks))
        })
        .with_live(move |values, checks| {
            let r = subtree::from_form(a, values, checks);
            let plan = subtree::plan(&r, &ctx);
            Live {
                text: plan.text,
                ok_label: plan.ok_label,
                blocked: plan.blocked,
                matches: remotes.iter().map(|r| format!("remote {r}")).collect(),
            }
        });
        self.modal = Some(modal);
    }

    /// Runs `req` as an operation; a split without a branch then offers to create one.
    pub(crate) fn run_subtree(&mut self, req: Request) {
        let offer_branch = req.action == Action::Split && req.branch.trim().is_empty();
        let prefix = subtree::normalize_prefix(&req.prefix);
        // The source may be remembered: read the list again afterwards.
        self.subtrees_dirty = true;
        self.run_op(req.action.busy_label(), true, After::Summary, move |r| {
            let out = r.run_subtree(&req)?;
            match out.split {
                Some(id) if offer_branch => Err(FollowUp::SplitDone {
                    prefix,
                    id,
                    message: out.message,
                }
                .into()),
                _ => Ok(out.message),
            }
        });
    }

    /// After a split without a branch: its commit, and a branch to keep it.
    pub(super) fn split_done(&mut self, prefix: &str, id: ObjectId, message: &str) {
        self.set_info(message.to_string());
        let name = prefix.rsplit('/').next().unwrap_or(prefix);
        let default = format!("{name}-split");
        let modal = Modal::input(
            format!("Split done: {} — create a branch on it?", subtree::short(id)),
            vec![("New branch at the split (Esc: none)", default.as_str())],
            move |app, v| {
                let b = v[0].trim().to_string();
                if let Some(p) = gitti_core::worktree::branch_name_problem(&b) {
                    app.set_error(format!("“{b}” can't be a branch: {p}"));
                    return;
                }
                app.run_op("Create branch", true, After::Nothing, move |r| r.create_branch_at(&b, id));
            },
        )
        .with_note(format!(
            "The history of {prefix}/ is now commit {id}, with the folder's files at its top. \
             No branch points at it yet: create one to push it somewhere or to keep it (git \
             may clean up commits no branch keeps)."
        ));
        self.modal = Some(modal);
    }

    /// "Edit Source…": the repository / branches remembered for the subtree at `prefix`.
    pub(crate) fn edit_subtree_source(&mut self, prefix: &str) {
        let src = self
            .subtrees
            .iter()
            .find(|s| s.prefix == prefix)
            .and_then(|s| s.source.clone())
            .unwrap_or_default();
        let p = prefix.to_string();
        let modal = Modal::input(
            format!("Source of {prefix}/"),
            vec![
                ("Repository: URL, path or remote", src.repository.as_str()),
                ("Branch to pull", src.reference.as_str()),
                ("Branch to push to (empty: the same)", src.push_ref.as_str()),
            ],
            move |app, v| {
                let src = subtree::Source {
                    repository: v[0].trim().to_string(),
                    reference: v[1].trim().to_string(),
                    push_ref: v[2].trim().to_string(),
                };
                app.subtrees_dirty = true;
                app.run_op("Save the subtree source", false, After::Nothing, move |r| {
                    r.set_subtree_source(&p, &src).map(|_| String::new())
                });
            },
        )
        .with_note(format!(
            "Where {prefix}/ comes from — git doesn't record it. Pull and Push start with \
             these. Kept in this repository's .git/config, section [{} \"{prefix}\"]; all \
             fields empty forgets it.",
            subtree::source::SECTION
        ));
        self.modal = Some(modal);
    }

    /// The commits that changed the folder: Go to Commit… searching for it.
    pub(crate) fn show_subtree_history(&mut self, prefix: &str) {
        self.goto_commit_picker();
        if let Some(Modal::Picker { query, .. }) = &mut self.modal {
            *query = crate::edit::TextEdit::single(&format!("{prefix}/"));
        }
        self.start_commit_search();
    }

    /// Palette: the subtree commands, plus Pull / Push of each subtree.
    pub(super) fn subtree_palette_items(&self) -> Vec<PickItem> {
        let mut items: Vec<PickItem> = Action::ALL
            .into_iter()
            .map(|a| {
                PickItem::new(a.menu_label(), format!("git subtree {}", a.word()), move |app: &mut App| {
                    app.subtree_command(a)
                })
            })
            .collect();
        for s in self.subtrees.iter().filter(|s| s.present) {
            for a in [Action::Pull, Action::Push] {
                let r = Request::for_subtree(a, s);
                let label = match a {
                    Action::Pull => format!("Pull Subtree {}/…", s.prefix),
                    _ => format!("Push Subtree {}/…", s.prefix),
                };
                items.push(PickItem::new(label, s.detail(), move |app: &mut App| app.subtree_dialog(r)));
            }
        }
        items
    }
}
