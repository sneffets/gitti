//! The shared part of gitti's **workflow tests**: realistic sequences of user actions
//! (stage some hunks and commit, cherry-pick and later merge, an editor changing line
//! endings behind the app's back, …) checked by the repository they leave. Each workflow
//! runs on three layers — `gitti-core` (`tests/core/wf_*.rs`, the `Repo` API), gitti
//! (`tests/ui/wf_*.rs`, script steps in the real views) and gitty (`tests/tui/wf_*.rs`,
//! one `GITTY_SCRIPT` run) — and this crate holds what they share:
//!
//! - [`fixture`]: [`Fixture`], a starting state in three git processes (one
//!   `git fast-import` for any history; index, working tree, config, hooks without
//!   further git);
//! - [`ext`]: "another program" changing files behind the app's back (line endings, byte
//!   order mark, code page, exec bit, file ↔ symlink / folder, old modification times, a
//!   terminal `git`);
//! - [`expect`]: [`Expect`], the repository afterwards read with few git processes, and
//!   [`Checks`] collecting the mismatches;
//! - the workflows, one module each in their area's module ([`st`], [`cp`], …): a
//!   `fixture(dir)`, the step constants the drivers need, and `expect…(dir)` functions
//!   returning `Err(text)` with every mismatch.
//!
//! The catalog of all workflows — IDs, steps, expected results and which test covers each
//! on which layer — is `WORKFLOWS.md` next to this crate's `Cargo.toml`; keep it in sync.
//!
//! std only (no gitti-core, no gix): a dev-dependency that adds no dependency features to
//! the workspace build. Every git process here runs without the user's global config.

pub mod cp;
pub mod expect;
pub mod ext;
pub mod fixture;
pub mod git;
pub mod rm;
pub mod st;
pub mod wt;

pub use expect::{Checks, Expect, OpState};
pub use fixture::{Fixture, Ids, Mark, del, exec, link, put, put_bytes};
