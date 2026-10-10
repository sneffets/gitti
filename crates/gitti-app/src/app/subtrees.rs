//! `git subtree` in gitti: the SUBTREES sidebar rows (found from the history,
//! `gitti_core::subtree`) and their menu (Pull…, Merge…, Push…, Split…, Show History,
//! Edit Source…), the Repository menu / palette commands (Add / Pull / Merge into / Push /
//! Split Subtree…), "Add as Subtree…" / "Merge into Subtree…" on commits and branches, and
//! the dialogs — the same fields as gitty's (`subtree::form`), saying live what OK does
//! and why it can't run (`subtree::plan`). The runs are ordinary operations: cancellable,
//! with the split's progress in the status bar.
//!
//! The list is read in the background when HEAD moves (in-process, cached per HEAD in
//! the core): navigation never reads it.

use std::rc::Rc;

use gitti_core::ObjectId;
use gitti_core::subtree::{self, Action, Request, Subtree};
use gpui_kit::component::Icon;
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::GittiApp;
use super::modal::{ModalLive, ModalSpec};
use super::picker::PickerItem;
use crate::probe::ProbeExt as _;
use crate::theme::Palette;
use crate::{AddSubtree, MergeIntoSubtree, PullSubtree, PushSubtree, SplitSubtree};

type AppHandle = WeakEntity<GittiApp>;

fn item(
    label: impl Into<SharedString>,
    app: &AppHandle,
    f: impl Fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>) + 'static,
) -> PopupMenuItem {
    let app = app.clone();
    super::menus::menu_entry(label).on_click(move |_, window, cx| {
        app.update(cx, |this, cx| f(this, window, cx)).ok();
    })
}

/// The actions on one subtree (its sidebar entry).
fn subtree_menu(menu: PopupMenu, app: &AppHandle, s: &Subtree) -> PopupMenu {
    let gone = (!s.present).then_some("folder removed");
    let with = |label: &str, action: Action| {
        let s = s.clone();
        let label = match gone.filter(|_| action != Action::Split) {
            Some(why) => gitti_core::availability::with_reason(label, why),
            None => label.to_string(),
        };
        item(label, app, move |this, w, cx| {
            this.subtree_dialog(Request::for_subtree(action, &s), w, cx)
        })
        .disabled(gone.is_some() && action != Action::Split)
    };
    let mut menu = menu
        .item(with("Pull…", Action::Pull))
        .item(with("Merge…", Action::Merge))
        .item(with("Push…", Action::Push))
        .item(with("Split…", Action::Split));
    if !s.present {
        let s = s.clone();
        menu = menu.item(item("Add Again…", app, move |this, w, cx| {
            this.subtree_dialog(Request::for_subtree(Action::Add, &s), w, cx)
        }));
    }
    let prefix = s.prefix.clone();
    menu.separator()
        .item({
            let p = prefix.clone();
            item(format!("Show History of {prefix}/"), app, move |this, w, cx| {
                this.show_subtree_history(&p, w, cx)
            })
        })
        .item({
            let p = prefix.clone();
            item("Edit Source…", app, move |this, w, cx| this.edit_subtree_source(&p, w, cx))
        })
        .separator()
        .item(super::menus::copy_item("Copy Path", prefix))
}

/// The SUBTREES header's menu: the Repository menu's subtree commands.
pub(super) fn section_menu(menu: PopupMenu, app: &AppHandle) -> PopupMenu {
    Action::ALL.into_iter().fold(menu, |menu, a| {
        menu.item(item(a.menu_label(), app, move |this, w, cx| {
            this.subtree_command(a, w, cx)
        }))
    })
}

/// "Add as Subtree…" and "Merge into Subtree…" for a commit / branch / tag (`rev`, the
/// commit `target`); `remote`: a remote branch's remote and branch name (its Add fetches
/// from there, so Pull knows where it comes from later). Merge is greyed with the reason
/// for a commit already in HEAD's history (git would only say "already up to date") —
/// decided from the loaded history, no git.
pub(super) fn ref_items(
    menu: PopupMenu,
    app: &AppHandle,
    rev: &str,
    target: Option<ObjectId>,
    remote: Option<(String, String)>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(this) = app.upgrade() else {
        return menu;
    };
    let (subtrees, merged) = {
        let this = this.read(cx);
        let merged = target.and_then(|t| {
            subtree::merge_unavailable(&this.history, &t, this.head.branch.as_deref())
        });
        (this.subtrees.clone(), merged)
    };
    let mut add = Request::new(Action::Add, "");
    match &remote {
        Some((r, b)) => {
            add.repository = r.clone();
            add.reference = b.clone();
        }
        None => add.reference = rev.to_string(),
    }
    add.prefix = subtree::default_prefix("", &add.reference);
    if add.prefix.len() >= 40 && add.prefix.bytes().all(|b| b.is_ascii_hexdigit()) {
        add.prefix.clear();
    }
    let menu = menu.separator().item(item("Add as Subtree…", app, move |this, w, cx| {
        this.subtree_dialog(add.clone(), w, cx)
    }));
    let merge = |s: &Subtree| {
        let mut r = Request::for_subtree(Action::Merge, s);
        r.reference = rev.to_string();
        r
    };
    let open = move |r: Request| {
        move |this: &mut GittiApp, w: &mut Window, cx: &mut Context<GittiApp>| {
            this.subtree_merge_dialog(r.clone(), target, w, cx)
        }
    };
    match (subtrees.as_slice(), merged) {
        ([], _) => menu,
        ([one], why) => {
            let label = format!("Merge into Subtree {}/…", one.prefix);
            let label = match &why {
                Some(why) => gitti_core::availability::with_reason(&label, why),
                None => label,
            };
            menu.item(item(label, app, open(merge(one))).disabled(why.is_some()))
        }
        (_, Some(why)) => menu.item(
            item(gitti_core::availability::with_reason("Merge into Subtree…", &why), app, |_, _, _| {})
                .disabled(true),
        ),
        (many, None) => {
            let app = app.clone();
            let reqs: Vec<(String, Request)> =
                many.iter().map(|s| (format!("{}/", s.prefix), merge(s))).collect();
            menu.submenu("Merge into Subtree", window, cx, move |mut m, _, _| {
                for (label, r) in &reqs {
                    m = m.item(item(format!("{label}…"), &app, open(r.clone())));
                }
                m
            })
        }
    }
}

impl GittiApp {
    /// Reads the subtrees again in the background when HEAD moved (or `force`: after a
    /// subtree operation, whose source may have been remembered). In-process.
    pub(super) fn refresh_subtrees(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let head = self.head.id;
        if !force && self.subtrees_head == Some(head) {
            return;
        }
        if self.subtrees_loading {
            self.subtrees_queued = true;
            return;
        }
        self.subtrees_loading = true;
        let task = cx.background_executor().spawn(async move { repo.subtrees() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.subtrees_loading = false;
                this.subtrees_head = Some(head);
                // An unreadable history: no list (the refresh reports what is wrong).
                this.subtrees = result.unwrap_or_default();
                if std::mem::take(&mut this.subtrees_queued) {
                    this.refresh_subtrees(true, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A subtree's sidebar row: icon, folder, a faint detail (squash, branch, removed);
    /// everything in the tooltip. Click selects the commit that last brought it in.
    pub(super) fn subtree_row(&self, si: usize, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.subtrees[si].clone();
        let app = cx.entity().downgrade();
        let tip = s.describe(&self.remotes);
        let detail = s.detail();
        let commit = s.commit;
        let small = self.settings.ui_small();
        self.sidebar_row(format!("subtree-{}", s.prefix), 1, p)
            .cursor_pointer()
            .when(!s.present, |d| d.text_color(p.text_faint))
            .child(
                div()
                    .w(px(self.settings.ui_font_size + 4.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .child(Icon::new(crate::assets::SUBTREE_ICON).xsmall().text_color(p.text_muted)),
            )
            .child(div().flex_shrink(1.).min_w_0().truncate().child(s.prefix.clone()))
            .child(div().flex_1())
            .when(!detail.is_empty(), |d| {
                d.child(
                    div()
                        .probe(format!("subtree-detail:{}", s.prefix))
                        .min_w_0()
                        .flex_shrink(1.)
                        .max_w(relative(0.6))
                        .truncate()
                        .text_size(small)
                        .text_color(p.text_muted)
                        .child(detail),
                )
            })
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                if let Some(row) = this.history.row_of(&commit) {
                    this.select_row(row, window, cx);
                }
            }))
            .context_menu(move |menu, _, _| subtree_menu(menu, &app, &s))
    }

    /// Menu-bar actions (Repository ▸ Add / Pull / Merge into / Push / Split Subtree…).
    pub(super) fn subtree_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &AddSubtree, w, cx| {
            this.subtree_command(Action::Add, w, cx)
        }))
        .on_action(cx.listener(|this, _: &PullSubtree, w, cx| {
            this.subtree_command(Action::Pull, w, cx)
        }))
        .on_action(cx.listener(|this, _: &MergeIntoSubtree, w, cx| {
            this.subtree_command(Action::Merge, w, cx)
        }))
        .on_action(cx.listener(|this, _: &PushSubtree, w, cx| {
            this.subtree_command(Action::Push, w, cx)
        }))
        .on_action(cx.listener(|this, _: &SplitSubtree, w, cx| {
            this.subtree_command(Action::Split, w, cx)
        }))
    }

    /// A menu-bar / palette command: Add opens its dialog; the others act on the one
    /// subtree there is, ask which with several, and open with an empty folder without
    /// any (a subtree git found no trailers for, or — Split — any folder).
    pub(super) fn subtree_command(&mut self, a: Action, window: &mut Window, cx: &mut Context<Self>) {
        if self.repo.is_none() {
            return;
        }
        let candidates: Vec<Subtree> = match a {
            Action::Add => Vec::new(),
            Action::Split => self.subtrees.clone(),
            _ => self.subtrees.iter().filter(|s| s.present).cloned().collect(),
        };
        match candidates.as_slice() {
            [] => self.subtree_dialog(Request::new(a, ""), window, cx),
            [one] => self.subtree_dialog(Request::for_subtree(a, one), window, cx),
            many => {
                let items = many
                    .iter()
                    .map(|s| {
                        let r = Request::for_subtree(a, s);
                        super::picker::item(format!("{}/", s.prefix), s.detail(), move |t, w, cx| {
                            t.subtree_dialog(r.clone(), w, cx)
                        })
                    })
                    .collect();
                let title = match a {
                    Action::Pull => "Pull which subtree?",
                    Action::Merge => "Merge into which subtree?",
                    Action::Push => "Push which subtree?",
                    _ => "Split which subtree? (any other folder: type it in the dialog)",
                };
                self.open_picker(title, items, window, cx);
            }
        }
    }

    /// The facts the dialogs' live text needs, as shown now; the loaded history lets the
    /// Merge dialog warn about a commit typed into it (`subtree::resolve_commit`).
    fn subtree_context(&self) -> subtree::Context {
        let workdir = self.repo.as_ref().map(|r| r.workdir().to_path_buf()).unwrap_or_default();
        let mut ctx = subtree::Context::of(
            &workdir,
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

    /// The dialog of `req.action`, filled from `req`: its fields (`subtree::form`), the
    /// remotes as fill buttons for the repository, and live under them what OK does and
    /// why it can't run (`subtree::plan`).
    pub(super) fn subtree_dialog(&mut self, req: Request, window: &mut Window, cx: &mut Context<Self>) {
        self.subtree_dialog_with(req, None, window, cx)
    }

    /// The Merge dialog for commit `target` (a commit / branch / tag menu's): it warns when
    /// the commit doesn't continue the subtree's history (`subtree::merge_warning`).
    fn subtree_merge_dialog(
        &mut self,
        req: Request,
        target: Option<ObjectId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let warning = target.and_then(|t| {
            let s = self.subtrees.iter().find(|s| s.prefix == subtree::normalize_prefix(&req.prefix))?;
            subtree::merge_warning(&self.history, s, &t, &req.reference)
        });
        self.subtree_dialog_with(req, warning, window, cx)
    }

    fn subtree_dialog_with(
        &mut self,
        req: Request,
        warning: Option<subtree::MergeWarning>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.repo.is_none() {
            return;
        }
        let a = req.action;
        let mut ctx = self.subtree_context();
        ctx.merge_warning = warning;
        let form = subtree::form(&req);
        let mut spec = ModalSpec::new(a.title());
        for (label, value) in &form.fields {
            spec = spec.field(*label, value.clone());
        }
        for (label, on) in &form.checks {
            spec = spec.check(*label, *on);
        }
        // The remotes as fill buttons under the repository field.
        let fills: Vec<(usize, String)> = match a {
            Action::Add | Action::Pull | Action::Push => {
                self.remotes.iter().take(6).map(|r| (1, r.name.clone())).collect()
            }
            _ => Vec::new(),
        };
        if a == Action::Add {
            spec = spec.derive(1, 0, |repo| subtree::default_prefix(repo, ""));
        }
        if matches!(a, Action::Add | Action::Pull | Action::Merge) {
            spec = spec.spell(form.fields.len() - 1);
        }
        let spec = spec
            .live(move |res| {
                let r = subtree::from_form(a, &res.values, &res.checks);
                let plan = subtree::plan(&r, &ctx);
                ModalLive {
                    text: plan.text,
                    ok_label: Some(plan.ok_label),
                    blocked: plan.blocked,
                    fills: fills.clone(),
                    fills_label: if fills.is_empty() { String::new() } else { "Remotes:".into() },
                }
            })
            .ok(a.title());
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let r = subtree::from_form(a, &res.values, &res.checks);
            this.run_subtree(r, w, cx)
        });
    }

    /// Runs `req` as an operation (cancellable, progress in the status bar), then says
    /// what happened; a split without a branch offers to create one.
    pub(super) fn run_subtree(&mut self, req: Request, window: &mut Window, cx: &mut Context<Self>) {
        let offer_branch = req.action == Action::Split && req.branch.trim().is_empty();
        let prefix = subtree::normalize_prefix(&req.prefix);
        self.run_op(
            req.action.busy_label(),
            true,
            window,
            cx,
            move |r| r.run_subtree(&req),
            move |this, out, w, cx| {
                this.set_info(out.message.clone());
                // The source may have been remembered.
                this.refresh_subtrees(true, cx);
                if offer_branch && let Some(id) = out.split {
                    this.split_result_dialog(&prefix, id, w, cx);
                }
            },
        );
    }

    /// After a split without a branch: its commit, and a branch to keep it.
    fn split_result_dialog(&mut self, prefix: &str, id: ObjectId, window: &mut Window, cx: &mut Context<Self>) {
        let name = prefix.rsplit('/').next().unwrap_or(prefix);
        let hex = id.to_string();
        let msg = format!(
            "The history of {prefix}/ is now commit {} ({hex}), with the folder's files at \
             its top. No branch points at it yet: create one to push it somewhere or to keep \
             it (git may clean up commits no branch keeps).",
            subtree::short(id)
        );
        let copy = hex.clone();
        let spec = ModalSpec::new("Split Done")
            .message(msg)
            .field("New branch at the split", format!("{name}-split"))
            .extra("Copy Commit ID", move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
            })
            .cancel(Some("Close"))
            .ok("Create Branch");
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let b = res.values[0].trim().to_string();
            if let Some(p) = gitti_core::worktree::branch_name_problem(&b) {
                this.set_error(format!("“{b}” can't be a branch: {p}"));
                cx.notify();
                return;
            }
            this.git_op("Create branch", w, cx, move |r| r.create_branch_at(&b, id));
        });
    }

    /// "Edit Source…": the repository / branches remembered for the subtree at `prefix`
    /// (git doesn't record them; gitti keeps them in .git/config).
    pub(super) fn edit_subtree_source(&mut self, prefix: &str, window: &mut Window, cx: &mut Context<Self>) {
        let src = self
            .subtrees
            .iter()
            .find(|s| s.prefix == prefix)
            .and_then(|s| s.source.clone())
            .unwrap_or_default();
        let msg = format!(
            "Where {prefix}/ comes from — git doesn't record it. Pull and Push start with \
             these. Kept in this repository's .git/config, section [{} \"{prefix}\"]; empty \
             fields are removed.",
            subtree::source::SECTION
        );
        let mut spec = ModalSpec::new(format!("Source of {prefix}/"))
            .message(msg)
            .field("Repository: URL, path or remote", src.repository)
            .field("Branch to pull", src.reference)
            .field("Branch to push to (empty: the same)", src.push_ref);
        for r in self.remotes.iter().take(4) {
            spec = spec.choice(r.name.clone(), 0, r.name.clone());
        }
        let p = prefix.to_string();
        let forget = p.clone();
        let spec = spec
            .extra("Forget", move |this, w, cx| {
                let f = forget.clone();
                this.run_op("Forgetting the subtree source", false, w, cx, move |r| {
                    r.forget_subtree_source(&f)
                }, |this, _, _, cx| this.refresh_subtrees(true, cx));
            })
            .ok("Save");
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let src = subtree::Source {
                repository: res.values[0].trim().to_string(),
                reference: res.values[1].trim().to_string(),
                push_ref: res.values[2].trim().to_string(),
            };
            let p = p.clone();
            this.run_op("Saving the subtree source", false, w, cx, move |r| {
                r.set_subtree_source(&p, &src)
            }, |this, _, _, cx| this.refresh_subtrees(true, cx));
        });
    }

    /// The commits that changed the folder: Go to Commit… with it as the search.
    pub(super) fn show_subtree_history(&mut self, prefix: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_goto_commit(window, cx);
        self.script_picker_query(&format!("{prefix}/"), window, cx);
    }

    /// Palette: the Repository menu's commands, plus Pull / Push of each subtree.
    pub(super) fn subtree_palette_items(&self) -> Vec<PickerItem> {
        let mut items: Vec<PickerItem> = Action::ALL
            .into_iter()
            .map(|a| {
                let detail = format!("git subtree {}", a.word());
                super::picker::item(a.menu_label(), detail, move |t, w, cx| t.subtree_command(a, w, cx))
            })
            .collect();
        for s in self.subtrees.iter().filter(|s| s.present) {
            for a in [Action::Pull, Action::Push] {
                let r = Request::for_subtree(a, s);
                let label = match a {
                    Action::Pull => format!("Pull Subtree {}/…", s.prefix),
                    _ => format!("Push Subtree {}/…", s.prefix),
                };
                items.push(PickerItem {
                    run: Rc::new(move |t, w, cx| t.subtree_dialog(r.clone(), w, cx)),
                    ..super::picker::item(label, s.detail(), |_, _, _| {})
                });
            }
        }
        items
    }

    /// Script: `subtree:ACTION,PREFIX` opens that dialog (for the subtree at PREFIX, or
    /// with PREFIX as its folder).
    pub fn script_subtree(&mut self, action: &str, prefix: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = Action::ALL.into_iter().find(|a| a.word() == action) else {
            eprintln!("[script] subtree: unknown action {action}");
            return;
        };
        let req = match self.subtrees.iter().find(|s| s.prefix == prefix) {
            Some(s) => Request::for_subtree(a, s),
            None => Request::new(a, prefix),
        };
        self.subtree_dialog(req, window, cx);
    }

    /// Script / tests: the subtrees as `prefix [squash] [removed] update source`.
    pub fn script_subtrees(&self) -> Vec<String> {
        self.subtrees
            .iter()
            .map(|s| {
                let mut line = s.prefix.clone();
                if s.squash {
                    line.push_str(" squash");
                }
                if !s.present {
                    line.push_str(" removed");
                }
                line.push_str(&format!(" {}", s.update.label()));
                if let Some(src) = &s.source {
                    line.push_str(&format!(" {} {}", src.repository, src.reference));
                }
                line
            })
            .collect()
    }
}
