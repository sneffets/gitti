//! Sidebar entries ("Locations"): branches, worktrees (with linked ones; the main one alone
//! by the setting `show_main_worktree_alone`), remotes with their branches, tags, stashes,
//! submodules and subtrees (only with some).

use gitti_core::RefKind;
use gitti_core::ops::Tracking;

use crate::app::App;

#[derive(Clone, Debug)]
pub enum SideItem {
    Header {
        key: String,
        label: String,
        count: usize,
    },
    Branch {
        name: String,
        current: bool,
        tracking: Option<Tracking>,
    },
    Remote {
        name: String,
    },
    RemoteBranch {
        /// `origin/main`
        full: String,
        /// `main`
        short: String,
    },
    Tag {
        name: String,
    },
    Stash {
        index: usize,
        message: String,
    },
    /// Index into `App::worktrees` and its folder name.
    Worktree {
        index: usize,
        name: String,
    },
    /// Index into `App::submodules` and its path.
    Submodule {
        index: usize,
        path: String,
    },
    /// Index into `App::subtrees` and its folder.
    Subtree {
        index: usize,
        prefix: String,
    },
}

impl SideItem {
    pub fn title(&self) -> String {
        match self {
            SideItem::Header { label, .. } => label.clone(),
            SideItem::Branch { name, .. } => name.clone(),
            SideItem::Remote { name } => name.clone(),
            SideItem::RemoteBranch { full, .. } => full.clone(),
            SideItem::Tag { name } => name.clone(),
            SideItem::Stash { index, .. } => format!("stash@{{{index}}}"),
            SideItem::Submodule { path, .. } => format!("Submodule {path}"),
            SideItem::Subtree { prefix, .. } => format!("Subtree {prefix}/"),
            SideItem::Worktree { name, .. } => format!("Worktree {name}"),
        }
    }

    /// Collapse key of headers and remotes.
    pub fn collapse_key(&self) -> Option<String> {
        match self {
            SideItem::Header { key, .. } => Some(key.clone()),
            SideItem::Remote { name } => Some(format!("remote:{name}")),
            _ => None,
        }
    }

    /// The commit list row this entry points at.
    pub fn target_row(&self, app: &App) -> Option<usize> {
        match self {
            SideItem::Branch { name, .. } => app.row_of_ref(RefKind::LocalBranch, name),
            SideItem::RemoteBranch { full, .. } => app.row_of_ref(RefKind::RemoteBranch, full),
            SideItem::Tag { name } => app.row_of_ref(RefKind::Tag, name),
            SideItem::Stash { index, .. } => app.row_of_stash(*index),
            SideItem::Worktree { index, .. } => {
                let head = app.worktrees.get(*index)?.head?;
                app.history.row_of(&head)
            }
            SideItem::Subtree { index, .. } => app.history.row_of(&app.subtrees.get(*index)?.commit),
            _ => None,
        }
    }
}

pub fn build(app: &App) -> Vec<SideItem> {
    let mut out = Vec::new();
    let collapsed = |k: &str| app.collapsed.contains(k);
    let section = |out: &mut Vec<SideItem>, key: &str, label: &str, count: usize| {
        out.push(SideItem::Header {
            key: key.to_string(),
            label: label.to_string(),
            count,
        });
        !collapsed(key)
    };

    let locals: Vec<_> = app
        .refs
        .iter()
        .filter(|r| r.kind == RefKind::LocalBranch)
        .collect();
    if section(&mut out, "branches", "BRANCHES", locals.len()) {
        for r in locals {
            out.push(SideItem::Branch {
                current: app.current_branch() == Some(r.short_name.as_str()),
                tracking: app.tracking_of(&r.short_name).cloned(),
                name: r.short_name.clone(),
            });
        }
    }

    // With linked worktrees; the main one alone by the setting (as in gitti).
    if gitti_core::worktree::section_shown(&app.worktrees, app.main_worktree_alone)
        && section(&mut out, "worktrees", "WORKTREES", app.worktrees.len())
    {
        for (index, wt) in app.worktrees.iter().enumerate() {
            out.push(SideItem::Worktree {
                index,
                name: wt.name(),
            });
        }
    }

    if section(&mut out, "remotes", "REMOTES", app.remotes.len()) {
        for remote in &app.remotes {
            out.push(SideItem::Remote {
                name: remote.name.clone(),
            });
            if collapsed(&format!("remote:{}", remote.name)) {
                continue;
            }
            let prefix = format!("{}/", remote.name);
            for r in app.refs.iter().filter(|r| {
                r.kind == RefKind::RemoteBranch
                    && r.short_name.starts_with(&prefix)
                    && !r.short_name.ends_with("/HEAD")
            }) {
                out.push(SideItem::RemoteBranch {
                    full: r.short_name.clone(),
                    short: r.short_name[prefix.len()..].to_string(),
                });
            }
        }
    }

    let tags: Vec<_> = app.refs.iter().filter(|r| r.kind == RefKind::Tag).collect();
    if section(&mut out, "tags", "TAGS", tags.len()) {
        // Newest names last in git's order; show them first.
        for r in tags.iter().rev() {
            out.push(SideItem::Tag {
                name: r.short_name.clone(),
            });
        }
    }

    if section(&mut out, "stashes", "STASHES", app.stashes.len()) {
        for s in &app.stashes {
            out.push(SideItem::Stash {
                index: s.index,
                message: s.message.clone(),
            });
        }
    }

    // Last, like gitti; only with submodules.
    if !app.submodules.is_empty()
        && section(&mut out, "submodules", "SUBMODULES", app.submodules.len())
    {
        for (index, s) in app.submodules.iter().enumerate() {
            out.push(SideItem::Submodule {
                index,
                path: s.path.clone(),
            });
        }
    }
    // Only with subtrees (found in the history), like gitti.
    if !app.subtrees.is_empty() && section(&mut out, "subtrees", "SUBTREES", app.subtrees.len()) {
        for (index, s) in app.subtrees.iter().enumerate() {
            out.push(SideItem::Subtree {
                index,
                prefix: s.prefix.clone(),
            });
        }
    }
    out
}
