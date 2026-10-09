//! RELEASE TESTS — gitty's old, slow end-to-end tests. Not compiled by default: see
//! `release-tests/README.md`. Only before a release — never in daily work or per merge.
//! Once the `[[test]]` block in `crates/gitty/Cargo.toml` is uncommented:
//! `cargo test --workspace --test release_tui [-- MODULE::]`.

mod common;
mod support;

mod cancel;
mod changes;
mod conflict_flow;
mod file_list;
mod keys;
mod linux;
mod palette;
mod repo_cmds;
mod script;
mod scroll;
mod search;
mod sidebar_menus;
mod speed;
mod submodules;
mod view_anchor;
mod worktrees;
