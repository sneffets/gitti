//! Custom commands in gitti (`gitti_core::custom_command`): Repository ▸ Custom
//! Commands… (the list: New…, Edit…), Repository ▸ Run Custom Command… (a picker), one
//! palette entry per command, and entries in the commit / branch / tag / remote / diff context
//! menus for the commands that use what that menu is about — greyed with the reason when
//! a value they need is missing. The define / edit dialog shares its fields and live text
//! with gitty (`custom_command::form` / `plan`). A run is an ordinary operation
//! (cancellable, in the Git Log, a refresh afterwards); its output opens in a window.

use std::path::PathBuf;
use std::rc::Rc;

use gitti_core::custom_command::{self as cc, Context as CmdContext, Entry, Menu};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::*;

use super::modal::{ModalLive, ModalSpec};
use super::picker::PickerItem;
use super::{Detail, GittiApp};
use crate::{CustomCommands, RunCustomCommand};

type AppHandle = WeakEntity<GittiApp>;

/// What a context menu is about (fills the placeholders before the app's selection).
#[derive(Debug, Clone, Default)]
pub(super) struct Given {
    pub commit: Option<String>,
    pub branch: Option<String>,
    pub remote: Option<String>,
    pub file: Option<String>,
    pub tag: Option<String>,
}

/// The entries of `menu`'s custom commands (after a separator; none without such
/// commands), greyed with the reason where a value is missing.
pub(super) fn menu_items(menu: PopupMenu, app: &AppHandle, kind: Menu, given: Given, cx: &App) -> PopupMenu {
    let Some(this) = app.upgrade() else {
        return menu;
    };
    let (entries, ctx) = {
        let this = this.read(cx);
        let entries: Vec<Entry> = this.custom_entries().into_iter().filter(|e| e.offered_in(kind)).collect();
        (entries, this.custom_context(&given))
    };
    if entries.is_empty() {
        return menu;
    }
    let mut menu = menu.separator();
    for e in entries {
        let why = cc::missing(&e, &ctx);
        let label = match why {
            Some(why) => gitti_core::availability::with_reason(&e.cmd.name, why),
            None => e.cmd.name.clone(),
        };
        let app = app.clone();
        let ctx = ctx.clone();
        menu = menu.item(
            super::menus::menu_entry(label)
                .on_click(move |_, window, cx| {
                    let (e, ctx) = (e.clone(), ctx.clone());
                    app.update(cx, |this, cx| this.run_custom_command(e, ctx, window, cx)).ok();
                })
                .disabled(why.is_some()),
        );
    }
    menu
}

impl GittiApp {
    /// Where this repository's commands are kept (its common git dir).
    fn custom_key(&self) -> Option<PathBuf> {
        self.repo.as_ref().map(|r| cc::repo_key(r))
    }

    /// The commands for this repository: its own, then those for every one.
    pub(super) fn custom_entries(&self) -> Vec<Entry> {
        self.custom_key().map(|k| cc::list(&k)).unwrap_or_default()
    }

    /// The placeholders' values: `given` (a context menu's), else the selection.
    pub(super) fn custom_context(&self, given: &Given) -> CmdContext {
        let Some(repo) = &self.repo else {
            return CmdContext::default();
        };
        let mut ctx = CmdContext::new(repo.workdir());
        let v = &mut ctx.values;
        v.commit = given.commit.clone().or_else(|| {
            let row = self.selected;
            if self.history.is_worktree_row(row) {
                return None;
            }
            let c = self.history.commits.get(row)?;
            (c.stash.is_none() && !c.stash_helper).then(|| c.id.to_string())
        });
        v.branch = given.branch.clone().or_else(|| self.head.branch.clone());
        // `{tag}`: the menu's tag, else the first tag (by name) on the selected commit.
        v.tag = given.tag.clone().or_else(|| {
            let id = gitti_core::ObjectId::from_hex(v.commit.as_deref()?.as_bytes()).ok()?;
            self.history
                .refs_by_commit
                .get(&id)?
                .iter()
                .filter(|r| r.kind == gitti_core::RefKind::Tag)
                .map(|r| r.short_name.clone())
                .min()
        });
        v.remote = given.remote.clone().or_else(|| {
            cc::default_remote(self.head.branch.as_deref(), &self.tracking, &self.remotes)
        });
        v.file = given.file.clone().or_else(|| {
            let row = self.menu_row.unwrap_or_else(|| self.diff_cursor_row());
            self.row_location(row).map(|(p, _)| p)
        });
        v.files = match &self.detail {
            Detail::Worktree => cc::worktree_files(&self.working),
            Detail::Commit(view) if view.compare.is_none() => cc::diff_files(&view.files),
            _ => Vec::new(),
        };
        ctx
    }

    /// Menu-bar actions (Repository ▸ Custom Commands… / Run Custom Command…).
    pub(super) fn custom_command_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &CustomCommands, w, cx| this.custom_commands_manager(w, cx)))
            .on_action(cx.listener(|this, _: &RunCustomCommand, w, cx| this.run_custom_picker(w, cx)))
    }

    /// The list of commands to edit, and New….
    pub(super) fn custom_commands_manager(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.repo.is_none() {
            return;
        }
        let mut items = vec![super::picker::item(
            "New Custom Command…",
            "a git or shell command line with placeholders",
            |t, w, cx| t.custom_command_dialog(None, w, cx),
        )];
        for e in self.custom_entries() {
            let detail = e.detail();
            let label = format!("Edit “{}”…", e.cmd.name);
            items.push(super::picker::item(label, detail, move |t, w, cx| {
                t.custom_command_dialog(Some(e.clone()), w, cx)
            }));
        }
        self.open_picker("Custom Commands", items, window, cx);
    }

    /// Which command to run (greyed with the reason where a value is missing); none yet:
    /// the list with New….
    pub(super) fn run_custom_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.custom_entries();
        if entries.is_empty() {
            self.set_info("No custom commands yet — define one with New Custom Command…");
            self.custom_commands_manager(window, cx);
            return;
        }
        let (items, off) = self.custom_run_items(entries);
        self.open_picker("Run which custom command?", items, window, cx);
        if let Some(p) = self.picker.as_mut() {
            p.set_disabled(off);
        }
    }

    /// One entry per command (label = its name) and why each can't run now.
    fn custom_run_items(&self, entries: Vec<Entry>) -> (Vec<PickerItem>, Vec<Option<&'static str>>) {
        let ctx = self.custom_context(&Given::default());
        entries
            .into_iter()
            .map(|e| {
                let why = cc::missing(&e, &ctx);
                let detail = match why {
                    Some(why) => why.to_string(),
                    None => e.detail(),
                };
                let label = e.cmd.name.clone();
                let ctx = ctx.clone();
                let it = PickerItem {
                    run: Rc::new(move |t: &mut GittiApp, w: &mut Window, cx: &mut Context<GittiApp>| {
                        t.run_custom_command(e.clone(), ctx.clone(), w, cx)
                    }),
                    ..super::picker::item(label, detail, |_, _, _| {})
                };
                (it, why)
            })
            .unzip()
    }

    /// Palette: Custom Commands… / Run Custom Command… and every command by its name;
    /// appended to `items`, returns why each command can't run now (parallel to the
    /// commands, which come last).
    pub(super) fn custom_palette_items(&self, items: &mut Vec<PickerItem>) -> Vec<Option<&'static str>> {
        items.push(
            super::picker::item("Custom Commands…", "define, edit or delete your commands", |t, w, cx| {
                t.custom_commands_manager(w, cx)
            })
            .act(&CustomCommands),
        );
        items.push(
            super::picker::item("Run Custom Command…", "pick one of your commands", |t, w, cx| {
                t.run_custom_picker(w, cx)
            })
            .act(&RunCustomCommand),
        );
        let (cmds, off) = self.custom_run_items(self.custom_entries());
        items.extend(cmds);
        off
    }

    /// Runs `e` with `ctx`: why it can't, the command line to confirm, or right away.
    pub(super) fn run_custom_command(&mut self, e: Entry, ctx: CmdContext, window: &mut Window, cx: &mut Context<Self>) {
        let p = match cc::prepare(&e, &ctx) {
            Ok(p) => p,
            Err(why) => {
                self.set_info(gitti_core::availability::refusal(&e.cmd.name, &why));
                cx.notify();
                return;
            }
        };
        if !p.confirm {
            self.run_prepared_custom(p, window, cx);
            return;
        }
        let spec = ModalSpec::new(format!("Run {}?", p.name)).message(cc::confirm_text(&p)).ok("Run");
        self.open_modal(spec, window, cx, move |this, _, w, cx| this.run_prepared_custom(p, w, cx));
    }

    fn run_prepared_custom(&mut self, p: cc::Prepared, window: &mut Window, cx: &mut Context<Self>) {
        let name = p.name.clone();
        self.run_op(
            &cc::busy_label(&p),
            true,
            window,
            cx,
            move |r| r.run_custom(&p),
            move |this, out, w, cx| {
                if out.is_empty() {
                    this.set_info(format!("{name}: done, no output"));
                } else {
                    this.set_info(format!("{name}: done"));
                    this.show_text_modal(&format!("Output of {name}"), out, w, cx);
                }
            },
        );
    }

    /// The define / edit dialog (`old`: the command edited), saying live what it will
    /// run, where, and what follows; Delete… for an existing one.
    pub(super) fn custom_command_dialog(&mut self, old: Option<Entry>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.custom_key() else {
            return;
        };
        let existing = cc::list(&key);
        let ctx = self.custom_context(&Given::default());
        let start = old.clone().unwrap_or(Entry {
            scope: cc::Scope::Repo,
            cmd: cc::CustomCommand::default(),
        });
        let form = cc::form(&start);
        let title = match &old {
            Some(o) => format!("Edit Custom Command “{}”", o.cmd.name),
            None => "New Custom Command".to_string(),
        };
        let mut spec = ModalSpec::new(title);
        for (label, value) in &form.fields {
            spec = spec.field(*label, value.clone());
        }
        for (label, on) in &form.checks {
            spec = spec.check(*label, *on);
        }
        let old_id = old.as_ref().map(|o| (o.scope, o.cmd.name.clone()));
        let live_old = old_id.clone();
        let spec = spec.live(move |res| {
            let e = cc::from_form(&res.values, &res.checks);
            let plan = cc::plan(&e, live_old.as_ref().map(|(s, n)| (*s, n.as_str())), &existing, &ctx);
            ModalLive {
                text: plan.text,
                ok_label: Some(plan.ok_label),
                blocked: plan.blocked,
                fills: Vec::new(),
                fills_label: String::new(),
            }
        });
        let spec = match &old {
            Some(o) => {
                let o = o.clone();
                let key = key.clone();
                spec.extra("Delete…", move |this, w, cx| {
                    let (o, key) = (o.clone(), key.clone());
                    this.confirm(
                        format!("Delete the custom command “{}”?", o.cmd.name),
                        Some(format!(
                            "It is kept for {}; nothing in the repository changes.",
                            o.scope.label()
                        )),
                        "Delete",
                        w,
                        cx,
                        move |this, _, cx| {
                            cc::remove(&key, o.scope, &o.cmd.name);
                            this.set_info(format!("Deleted the custom command “{}”", o.cmd.name));
                            cx.notify();
                        },
                    )
                })
            }
            None => spec,
        }
        .ok(if old.is_some() { "Save" } else { "Add" });
        self.open_modal(spec, window, cx, move |this, res, _, cx| {
            let e = cc::from_form(&res.values, &res.checks);
            let name = e.cmd.name.clone();
            let scope = e.scope;
            cc::save(&key, old_id.as_ref().map(|(s, n)| (*s, n.as_str())), e);
            this.set_info(format!(
                "Saved “{name}” for {} — run it from the palette or Repository ▸ Run Custom Command…",
                scope.label()
            ));
            cx.notify();
        });
    }

    /// Script: `custom:new`, `custom:manage`, `custom:edit,NAME`, `custom:run,NAME`.
    pub fn script_custom(&mut self, action: &str, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let find = || self.custom_entries().into_iter().find(|e| e.cmd.name == name);
        match action {
            "new" => self.custom_command_dialog(None, window, cx),
            "manage" => self.custom_commands_manager(window, cx),
            "edit" => match find() {
                Some(e) => self.custom_command_dialog(Some(e), window, cx),
                None => eprintln!("[script] custom: no command {name}"),
            },
            "run" => match find() {
                Some(e) => {
                    let ctx = self.custom_context(&Given::default());
                    self.run_custom_command(e, ctx, window, cx)
                }
                None => eprintln!("[script] custom: no command {name}"),
            },
            _ => eprintln!("[script] custom: unknown action {action}"),
        }
    }

    /// Script / tests: the commands as `name [repo] git|shell command`.
    pub fn script_custom_commands(&self) -> Vec<String> {
        self.custom_entries()
            .into_iter()
            .map(|e| {
                let scope = if e.scope == cc::Scope::Repo { " [repo]" } else { "" };
                format!("{}{scope} {}", e.cmd.name, e.command_text())
            })
            .collect()
    }
}
