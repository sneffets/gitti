//! gitty's integration tests: one test binary (one link) instead of one per file.
//! `cargo test --workspace --test tui -- staging::` runs one module's tests (`--workspace`,
//! not `-p`: cargo would resolve dependency features differently and rebuild them).

mod common;
mod support;

mod branches;
mod cancel;
mod change_search;
mod commit_box;
mod config_safety;
mod custom_commands;
mod ext_tools;
mod file_tree;
mod fs_paths;
mod keybar;
mod lfs;
mod linux;
mod menus;
mod merge_editor;
mod navigation;
mod op_safety;
mod palette;
mod progress;
mod reflog;
mod remote;
mod repo_cmds;
mod scroll;
mod sha256;
mod signing_keys;
mod speed;
mod staging;
mod stash;
mod submodules;
mod subtrees;
mod unicode;
mod views;
mod wf_branches;
mod wf_commit;
mod wf_eol;
mod wf_modules;
mod wf_pilot;
mod wf_remotes;
mod wf_reset;
mod wf_tags;
