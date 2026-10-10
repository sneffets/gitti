//! Custom commands in gitty, like gitti's `app/custom_commands.rs`: the palette's Custom
//! Commands… (New…, and per command Run / Edit… / Delete…), Run Custom Command… and one
//! entry per command; entries in the commit / branch / remote / diff menus for the
//! commands that use what that menu is about (greyed with the reason where a value is
//! missing). The form is gitti's (`custom_command::form`) with the live text under it
//! (`custom_command::plan`). A run is an ordinary operation (Esc cancels, in the Git Log,
//! its output in a text window).

use gitti_core::RefKind;
use gitti_core::custom_command::{self as cc, Context as CmdContext, Entry, Menu};
use gitti_core::rows::DiffRow;

use super::{After, App, Detail, work_files};
use crate::modal::{Live, MenuItem, Modal, PickItem};
use crate::sidebar::SideItem;

/// What a context menu is about (fills the placeholders before the selection).
#[derive(Debug, Clone, Default)]
pub(crate) struct Given {
    pub commit: Option<String>,
    pub branch: Option<String>,
    pub remote: Option<String>,
}

impl App {
    /// The commands for this repository: its own, then those for every one.
    pub(crate) fn custom_entries(&self) -> Vec<Entry> {
        cc::list(&cc::repo_key(&self.repo))
    }

    /// The file the diff cursor is in (any view).
    fn cursor_path(&self) -> Option<String> {
        let row: DiffRow = *self.rows.get(self.diff_cursor)?;
        let (src, file) = row.file_ref()?;
        self.file_diff(src, file)
            .map(|f| f.path().to_string())
            .or_else(|| work_files(&self.working, src).get(file).map(|w| w.path().to_string()))
    }

    /// The placeholders' values: `given` (a context menu's), else the selection.
    pub(crate) fn custom_context(&self, given: &Given) -> CmdContext {
        let mut ctx = CmdContext::new(self.repo.workdir());
        let v = &mut ctx.values;
        v.commit = given.commit.clone().or_else(|| {
            if self.history.is_worktree_row(self.selected) {
                return None;
            }
            let c = self.history.commits.get(self.selected)?;
            (c.stash.is_none() && !c.stash_helper).then(|| c.id.to_string())
        });
        v.branch = given.branch.clone().or_else(|| self.head.branch.clone());
        v.remote = given
            .remote
            .clone()
            .or_else(|| cc::default_remote(self.head.branch.as_deref(), &self.tracking, &self.remotes));
        v.file = self.cursor_path();
        v.files = match &self.detail {
            Detail::Worktree => cc::worktree_files(&self.working),
            Detail::Commit(view) if view.compare.is_none() => cc::diff_files(&view.files),
            _ => Vec::new(),
        };
        ctx
    }

    /// The custom commands of `kind`'s menu (after a separator; none without such
    /// commands), greyed with the reason where a value is missing.
    pub(crate) fn custom_menu_items(&self, kind: Menu, given: Given) -> Vec<MenuItem> {
        let entries: Vec<Entry> = self.custom_entries().into_iter().filter(|e| e.offered_in(kind)).collect();
        if entries.is_empty() {
            return Vec::new();
        }
        let ctx = self.custom_context(&given);
        let mut v = vec![MenuItem::separator()];
        for e in entries {
            let why = cc::missing(&e, &ctx);
            let name = e.cmd.name.clone();
            let ctx = ctx.clone();
            v.push(MenuItem::new(name, move |app: &mut App| app.run_custom_command(e, ctx)).unless(why));
        }
        v
    }

    /// The custom commands of a sidebar entry's menu (branches, remote branches, remotes).
    pub(crate) fn custom_side_items(&self, item: &SideItem) -> Vec<MenuItem> {
        let target = |kind: RefKind, name: &str| {
            self.refs
                .iter()
                .find(|r| r.kind == kind && r.short_name == name)
                .map(|r| r.target.to_string())
        };
        match item {
            SideItem::Branch { name, .. } => self.custom_menu_items(
                Menu::Branch,
                Given {
                    commit: target(RefKind::LocalBranch, name),
                    branch: Some(name.clone()),
                    remote: None,
                },
            ),
            SideItem::RemoteBranch { full, .. } => {
                let names: Vec<String> = self.remotes.iter().map(|r| r.name.clone()).collect();
                let (remote, branch) = cc::split_remote_branch(full, &names);
                self.custom_menu_items(
                    Menu::Branch,
                    Given {
                        commit: target(RefKind::RemoteBranch, full),
                        branch: Some(branch),
                        remote,
                    },
                )
            }
            SideItem::Remote { name } => self.custom_menu_items(
                Menu::Remote,
                Given {
                    remote: Some(name.clone()),
                    ..Given::default()
                },
            ),
            _ => Vec::new(),
        }
    }

    /// The list: New…, and per command its menu (Run / Edit… / Delete…).
    pub fn custom_commands_manager(&mut self) {
        let mut items = vec![PickItem::new(
            "New Custom Command…",
            "a git or shell command line with placeholders",
            |app: &mut App| app.custom_command_dialog(None),
        )];
        for e in self.custom_entries() {
            let detail = e.detail();
            items.push(PickItem::new(format!("{}…", e.cmd.name), detail, move |app: &mut App| {
                app.custom_command_menu(e)
            }));
        }
        self.modal = Some(Modal::picker("Custom Commands", items));
    }

    /// One command's menu in the list.
    fn custom_command_menu(&mut self, e: Entry) {
        let ctx = self.custom_context(&Given::default());
        let why = cc::missing(&e, &ctx);
        let title = e.cmd.name.clone();
        let (run, edit, del) = (e.clone(), e.clone(), e);
        let items = vec![
            MenuItem::new("Run", move |app: &mut App| app.run_custom_command(run, ctx))
                .key('r')
                .unless(why),
            MenuItem::new("Edit…", move |app: &mut App| app.custom_command_dialog(Some(edit))).key('e'),
            MenuItem::new("Delete…", move |app: &mut App| app.delete_custom_command(del)).key('d'),
        ];
        self.modal = Some(Modal::menu(title, items));
    }

    fn delete_custom_command(&mut self, e: Entry) {
        let key = cc::repo_key(&self.repo);
        self.modal = Some(Modal::confirm(
            "Delete Custom Command",
            format!(
                "Delete the custom command “{}”? It is kept for {}; nothing in the repository \
                 changes.",
                e.cmd.name,
                e.scope.label()
            ),
            "Delete",
            move |app: &mut App| {
                cc::remove(&key, e.scope, &e.cmd.name);
                app.set_info(format!("Deleted the custom command “{}”", e.cmd.name));
            },
        ));
    }

    /// Which command to run (greyed with the reason where a value is missing); none yet:
    /// the list with New….
    pub fn run_custom_picker(&mut self) {
        let entries = self.custom_entries();
        if entries.is_empty() {
            self.custom_commands_manager();
            self.set_info("No custom commands yet — define one with New Custom Command…");
            return;
        }
        let items = self.custom_run_items(entries);
        self.modal = Some(Modal::picker("Run which custom command?", items));
    }

    fn custom_run_items(&self, entries: Vec<Entry>) -> Vec<PickItem> {
        let ctx = self.custom_context(&Given::default());
        entries
            .into_iter()
            .map(|e| {
                let why = cc::missing(&e, &ctx);
                let (label, detail) = (e.cmd.name.clone(), e.detail());
                let ctx = ctx.clone();
                let it = PickItem::new(label, detail, move |app: &mut App| app.run_custom_command(e, ctx));
                match why {
                    Some(why) => it.unavailable(why),
                    None => it,
                }
            })
            .collect()
    }

    /// Palette: Custom Commands… / Run Custom Command… and every command by its name.
    pub(super) fn custom_palette_items(&self) -> Vec<PickItem> {
        let mut items = vec![
            PickItem::new("Custom Commands…", "define, edit or delete your commands", |app: &mut App| {
                app.custom_commands_manager()
            }),
            PickItem::new("Run Custom Command…", "pick one of your commands", |app: &mut App| {
                app.run_custom_picker()
            }),
        ];
        items.extend(self.custom_run_items(self.custom_entries()));
        items
    }

    /// Runs `e` with `ctx`: why it can't, the command line to confirm, or right away.
    pub(crate) fn run_custom_command(&mut self, e: Entry, ctx: CmdContext) {
        let p = match cc::prepare(&e, &ctx) {
            Ok(p) => p,
            Err(why) => {
                self.set_info(gitti_core::availability::refusal(&e.cmd.name, &why));
                return;
            }
        };
        if p.confirm {
            let title = format!("Run {}?", p.name);
            let body = cc::confirm_text(&p);
            self.modal = Some(Modal::confirm(title, body, "Run", move |app: &mut App| app.run_prepared_custom(p)));
        } else {
            self.run_prepared_custom(p);
        }
    }

    fn run_prepared_custom(&mut self, p: cc::Prepared) {
        let title = format!("Output of {}", p.name);
        self.run_op(&cc::busy_label(&p), true, After::ShowOutput(title), move |r| r.run_custom(&p));
    }

    /// The define / edit form (`old`: the command edited) with the live text of what it
    /// will run, where, and what follows.
    pub fn custom_command_dialog(&mut self, old: Option<Entry>) {
        let key = cc::repo_key(&self.repo);
        let existing = cc::list(&key);
        let ctx = self.custom_context(&Given::default());
        let start = old.clone().unwrap_or(Entry {
            scope: cc::Scope::Repo,
            cmd: cc::CustomCommand::default(),
        });
        let form = cc::form(&start);
        let fields: Vec<(&str, &str)> = form.fields.iter().map(|(l, v)| (*l, v.as_str())).collect();
        let title = match &old {
            Some(o) => format!("Edit Custom Command “{}”", o.cmd.name),
            None => "New Custom Command".to_string(),
        };
        let old_id = old.map(|o| (o.scope, o.cmd.name));
        let live_old = old_id.clone();
        let modal = Modal::input_checks(title, fields, form.checks.clone(), move |app, values, checks| {
            let e = cc::from_form(&values, &checks);
            let (name, scope) = (e.cmd.name.clone(), e.scope);
            cc::save(&key, old_id.as_ref().map(|(s, n)| (*s, n.as_str())), e);
            app.set_info(format!(
                "Saved “{name}” for {} — run it from the palette (Run Custom Command…)",
                scope.label()
            ));
        })
        .with_live(move |values, checks| {
            let e = cc::from_form(values, checks);
            let plan = cc::plan(&e, live_old.as_ref().map(|(s, n)| (*s, n.as_str())), &existing, &ctx);
            Live {
                text: plan.text,
                ok_label: plan.ok_label,
                blocked: plan.blocked,
                matches: Vec::new(),
            }
        });
        self.modal = Some(modal);
    }
}
