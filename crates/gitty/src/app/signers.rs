//! An allowed signers file for a repository without one, in gitty (as gitti's
//! `app/signers.rs`; logic in `gitti_core::allowed_signers`). When the shown SSH signature
//! can't be checked because `gpg.ssh.allowedSignersFile` isn't set, the menu (`m`) on the
//! header's Signature line offers the file the user already has — set globally (or in a
//! file the global config includes), in a repository open in gitti, at a usual place —
//! "for This Repository" or "for All Repositories"; the dialog says what is written where.
//! Without any known file it explains how to set one up. The files are looked for in the
//! verification's job (config files read in-process, no git); after the write the
//! signature is checked again.

use std::path::PathBuf;

use gitti_core::allowed_signers::{self as al, Candidate, Scope};
use gitti_core::signature::SignatureInfo;
use gitti_core::signing_keys::UserDirs;
use gitti_core::{ObjectId, Repo};

use super::{App, Detail, MenuItem, Modal, Msg, spawn};
use crate::modal::{Live, PickItem};

/// The shown commit's SSH signature can't be checked (no allowed signers file set); the
/// files that could be used, best first (none: explain how to set one up).
pub struct SignersOffer {
    id: ObjectId,
    candidates: Vec<Candidate>,
    dirs: UserDirs,
}

/// The palette entry.
pub const USE_LABEL: &str = "Use an Allowed Signers File…";

/// Verifies commit `id`'s signature and, when no allowed signers file is set, looks for
/// the user's (other repositories: the tabs open in gitti). Run on a job.
pub fn verify(repo: &Repo, id: ObjectId) -> (Option<SignatureInfo>, Option<SignersOffer>) {
    let info = repo.verify_signature(id).ok().flatten();
    let offer = info
        .as_ref()
        .filter(|i| al::missing_setting(i))
        .and_then(|_| UserDirs::from_env())
        .map(|dirs| {
            let (others, _) = gitti_core::store::load_tabs();
            let candidates = al::candidates(&dirs, repo.workdir(), &others);
            SignersOffer { id, candidates, dirs }
        });
    (info, offer)
}

impl App {
    /// Checks commit `id`'s signature on a job (`Msg::Signature`).
    pub(super) fn spawn_verify(&mut self, id: ObjectId) {
        let repo = self.repo.clone();
        spawn(&self.tx, move || verify(&repo, id), move |r| Msg::Signature(id, Box::new(r)));
    }

    /// The offer, when it is about the shown commit.
    fn shown_signers_offer(&self) -> Option<&SignersOffer> {
        let Detail::Commit(cv) = &self.detail else {
            return None;
        };
        if cv.compare.is_some() {
            return None;
        }
        self.signers_offer.as_ref().filter(|o| o.id == cv.id)
    }

    /// The shown signature needs an allowed signers file: how many the user has (`None`:
    /// it doesn't need one).
    pub fn signers_files_known(&self) -> Option<usize> {
        self.shown_signers_offer().map(|o| o.candidates.len())
    }

    /// The menu entries for commit `sha` (the commit menu) or the shown one (`None`: the
    /// Signature line's menu): both scopes for the best file, or how to set one up.
    pub(super) fn signers_items(&self, sha: Option<&str>) -> Vec<MenuItem> {
        let Some(o) = self.shown_signers_offer() else {
            return Vec::new();
        };
        if sha.is_some_and(|s| ObjectId::from_hex(s.as_bytes()).ok() != Some(o.id)) {
            return Vec::new();
        }
        match o.candidates.first() {
            Some(c) => [Scope::Repository, Scope::Global]
                .into_iter()
                .map(|s| MenuItem::new(s.menu_label(c, &o.dirs), move |app: &mut App| app.allowed_signers_dialog(s)))
                .collect(),
            None => vec![MenuItem::new(al::HELP_LABEL, |app: &mut App| app.allowed_signers_help())],
        }
    }

    /// "Use an Allowed Signers File…" (palette): the dialog with the best file, or how to
    /// set one up.
    pub fn use_allowed_signers_file(&mut self) {
        match self.signers_files_known() {
            None => self.set_info(
                "Only for an SSH signature that can't be checked because no allowed signers file \
                 is set: show such a commit first",
            ),
            Some(0) => self.allowed_signers_help(),
            Some(_) => self.allowed_signers_dialog(Scope::Repository),
        }
    }

    /// The dialog: the file (the best one filled in; the others listed), a tick for all
    /// repositories, and live what Enter writes where (`allowed_signers::plan`).
    fn allowed_signers_dialog(&mut self, scope: Scope) {
        let Some(o) = self.shown_signers_offer() else {
            return;
        };
        let (dirs, candidates) = (o.dirs.clone(), o.candidates.clone());
        let first = candidates.first().map(|c| dirs.show(&c.path)).unwrap_or_default();
        // Listed under the text ("Existing: …"): the other files the user has.
        let others: Vec<String> = candidates.iter().skip(1).map(|c| dirs.show(&c.path)).collect();
        let live_dirs = dirs.clone();
        let modal = Modal::input_checks(
            al::TITLE,
            vec![("Allowed signers file", first.as_str())],
            vec![(Scope::LABELS[1], scope == Scope::Global)],
            move |app, values, checks| {
                let path = al::resolve(values.first().map(String::as_str).unwrap_or(""), &dirs);
                app.set_allowed_signers(path, scope_of(&checks), dirs);
            },
        )
        .with_live(move |values, checks| {
            let plan = al::plan(values.first().map(String::as_str).unwrap_or(""), scope_of(checks), &candidates, &live_dirs);
            Live {
                text: plan.text,
                ok_label: plan.ok_label,
                blocked: plan.blocked,
                matches: others.clone(),
            }
        });
        self.modal = Some(modal);
    }

    /// Writes the setting (`git config`) on a job, then checks the signature again.
    fn set_allowed_signers(&mut self, path: PathBuf, scope: Scope, dirs: UserDirs) {
        self.keys_job = true;
        let repo = self.repo.clone();
        spawn(
            &self.tx,
            move || repo.use_allowed_signers(&path, scope, &dirs).map_err(|e| format!("{e:#}")),
            Msg::SignersSet,
        );
    }

    pub(super) fn signers_set(&mut self, result: Result<String, String>) {
        self.keys_job = false;
        match result {
            Ok(summary) => {
                self.set_info(format!("{summary}; checking the signature again"));
                if let Detail::Commit(cv) = &self.detail {
                    let id = cv.id;
                    self.reverify_signature(id);
                }
            }
            Err(e) => self.set_error(e),
        }
    }

    /// No allowed signers file known: what to do, in plain words.
    fn allowed_signers_help(&mut self) {
        let Some(dirs) = self.shown_signers_offer().map(|o| o.dirs.clone()) else {
            return;
        };
        let github = gitti_core::signing_keys::github_repo(&self.remotes).is_some();
        self.modal = Some(Modal::text("Checking SSH Signatures", al::setup_help(&dirs, github)));
    }

    /// Palette: "Use an Allowed Signers File…".
    pub(super) fn signers_palette_item(&self) -> PickItem {
        PickItem::new(USE_LABEL, "to check SSH signatures (gpg.ssh.allowedSignersFile)", |app: &mut App| {
            app.use_allowed_signers_file()
        })
    }
}

fn scope_of(checks: &[bool]) -> Scope {
    if checks.first().copied().unwrap_or(false) { Scope::Global } else { Scope::Repository }
}
