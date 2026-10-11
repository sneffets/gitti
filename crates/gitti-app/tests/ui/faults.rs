//! Faults injected into the git runner (`gitti_core::fault`) under the real views: a
//! commit whose `git show` failed shows the error — not a commit with no files — and is
//! read again when it is selected next.
//!
//! `cargo test --workspace --test ui -- faults[::FILTER]`.

use crate::common;

use common::*;
use gitti_core::fault::{Fault, inject};

pub fn main() {
    let tmp = common::setup();
    let base = gitti_core::paths::canonical(tmp.path());
    let tests: &[(&str, Scenario)] = &[("failed_commit_load_shows_the_error", failed_commit_load_shows_the_error)];
    common::run(tests, |name| make_repo(&base.join(name)));
}

fn has_file(state: &testing::UiState, path: &str) -> bool {
    state.rows.iter().any(|r| r.split(' ').any(|w| w == path))
}

/// "Initial import" (row 5) shown, then every `git show` killed while HEAD (row 1, not
/// prefetched from there) is selected: the diff says why it isn't shown — not
/// "Loading…", not the previous commit, not a commit without files — and the status bar
/// has the error. Then "Add f2" (row 2) loads, and HEAD selected again shows its files.
fn failed_commit_load_shows_the_error(ui: &mut Ui) {
    let repo = ui.repo.clone();
    ui.step("select:5");
    let s = ui.ui_state();
    if s.detail != "commit" || !has_file(&s, "small.rs") {
        ui.fail(format!("the first commit didn't load: {} {:?}", s.detail, s.rows));
    }
    {
        let _kill = inject(&repo, "show", Fault::Kill, usize::MAX);
        ui.step("select:1");
        let s = ui.ui_state();
        match &s.status {
            Some((text, true)) if text.contains("git show") => {}
            other => ui.fail(format!("no error for the failed load: {other:?}")),
        }
        if s.detail != "failed" || !s.rows.is_empty() {
            ui.fail(format!("the failed load isn't shown as one: {} {:?}", s.detail, s.rows));
        }
        let probes = ui.check("failed load");
        if find(&probes, "detail-failed").is_none() {
            ui.fail("no `detail-failed` element".into());
        }
    }
    ui.step("select:2");
    let s = ui.ui_state();
    if s.detail != "commit" || !has_file(&s, "f2.txt") {
        ui.fail(format!("the next commit didn't load: {} {:?}", s.detail, s.rows));
    }
    ui.step("select:1");
    let s = ui.ui_state();
    if s.detail != "commit" || !["f0.txt", "f1.txt", "f2.txt"].iter().all(|f| has_file(&s, f)) {
        ui.fail(format!("the failed commit didn't load the next time: {} {:?}", s.detail, s.rows));
    }
}
