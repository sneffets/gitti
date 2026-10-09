//! RELEASE TESTS — gitti-core's old, slow integration tests plus long randomized
//! ones (`staging_fuzz`). Not compiled by default: see `release-tests/README.md`. Only
//! before a release or overnight — never in daily work or per merge.
//! Once the `[[test]]` block in `crates/gitti-core/Cargo.toml` is uncommented:
//! `cargo test --workspace --test release_core [-- MODULE::]`.

mod conflicts;
mod hostile_config;
mod linux_env;
mod navigate;
mod push;
mod repo_setup;
mod repo_shapes;
mod runner;
mod search;
mod sidebar_ops;
mod staging_fuzz;
mod staging;
mod submodules;
mod unix_fs;
mod worktrees;
