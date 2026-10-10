//! An allowed signers file for a repository without one, in gitty (as gitti's
//! `app/signers.rs`; logic in `gitti_core::allowed_signers`). When the shown SSH signature
//! can't be checked because `gpg.ssh.allowedSignersFile` isn't set, the menu (`m`) on the
//! header's Signature line offers the file the user already has — set globally (or in a
//! file the global config includes), in a repository open in gitti, at a usual place —
//! "for This Repository" or "for All Repositories"; the dialog says what is written where.
//! With the user's own SSH signing key it also offers to create one that lists it.
//! Without any known file it explains how to set one up. The files are looked for in the
//! verification's job (config files read in-process, no git); after the write the
//! signature is checked again.

use gitti_core::allowed_signers::{self as al, Candidate, OwnKey, Scope, Situation};
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
    /// How the setting stands (unset, empty, a missing file; where).
    situation: Situation,
    /// The user's own SSH signing key, when no offered file lists it yet; whether it made
    /// the signature.
    own: Option<(OwnKey, bool)>,
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
            let situation = Situation::of(repo.workdir());
            let own = al::own_key(repo.workdir(), &dirs)
                .ok()
                .filter(|k| !al::lists_key(&candidates, k))
                .map(|k| {
                    let signed = k.signed(repo, id);
                    (k, signed)
                });
            SignersOffer { id, candidates, dirs, situation, own }
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

    /// The shown signature needs an allowed signers file and the user's own key could
    /// make one: `Some(true)` when that key made the signature.
    pub fn own_signers_offered(&self) -> Option<bool> {
        self.shown_signers_offer()?.own.as_ref().map(|(_, signed)| *signed)
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
        let mut items: Vec<MenuItem> = match o.candidates.first() {
            Some(c) => [Scope::Repository, Scope::Global]
                .into_iter()
                .map(|s| MenuItem::new(s.menu_label(c, &o.dirs), move |app: &mut App| app.allowed_signers_dialog(s)))
                .collect(),
            None => Vec::new(),
        };
        if o.own.is_some() {
            items.push(MenuItem::new(al::OWN_LABEL, |app: &mut App| app.own_signers_dialog()));
        }
        if o.candidates.is_empty() {
            items.push(MenuItem::new(al::HELP_LABEL, |app: &mut App| app.allowed_signers_help()));
        }
        items
    }

    /// "Use an Allowed Signers File…" (palette): the dialog with the best file, else the
    /// one creating a file with the user's own key, else how to set one up.
    pub fn use_allowed_signers_file(&mut self) {
        match (self.signers_files_known(), self.own_signers_offered()) {
            (None, _) => self.set_info(
                "Only for an SSH signature that can't be checked because no allowed signers file \
                 is set: show such a commit first",
            ),
            (Some(0), Some(_)) => self.own_signers_dialog(),
            (Some(0), None) => self.allowed_signers_help(),
            (Some(_), _) => self.allowed_signers_dialog(Scope::Repository),
        }
    }

    /// The dialog: the file (the best one filled in; the others listed), a tick for all
    /// repositories, and live what Enter writes where (`allowed_signers::plan`).
    fn allowed_signers_dialog(&mut self, scope: Scope) {
        let Some(o) = self.shown_signers_offer() else {
            return;
        };
        let (dirs, candidates, situation) = (o.dirs.clone(), o.candidates.clone(), o.situation.clone());
        let first = candidates.first().map(|c| c.entry(&dirs)).unwrap_or_default();
        // Listed under the text ("Existing: …"): the other files the user has.
        let others: Vec<String> = candidates.iter().skip(1).map(|c| c.entry(&dirs)).collect();
        let (live_dirs, live_situation) = (dirs.clone(), situation.clone());
        let modal = Modal::input_checks(
            al::TITLE,
            vec![("Allowed signers file", first.as_str())],
            vec![(Scope::LABELS[1], scope == Scope::Global)],
            move |app, values, checks| {
                let entered = values.first().cloned().unwrap_or_default();
                let scope = scope_of(&checks);
                app.set_allowed_signers(move |repo| repo.use_allowed_signers(&entered, scope, &dirs, &situation));
            },
        )
        .with_live(move |values, checks| {
            let plan = al::plan(
                values.first().map(String::as_str).unwrap_or(""),
                scope_of(checks),
                &candidates,
                &live_dirs,
                &live_situation,
            );
            Live {
                text: plan.text,
                ok_label: plan.ok_label,
                blocked: plan.blocked,
                matches: others.clone(),
            }
        });
        self.modal = Some(modal);
    }

    /// "Create Allowed Signers File with Your Key…": the file (the default one filled
    /// in), a tick for all repositories (on), and live what Enter writes where
    /// (`allowed_signers::plan_own`).
    fn own_signers_dialog(&mut self) {
        let Some(o) = self.shown_signers_offer() else {
            return;
        };
        let Some((own, _)) = o.own.clone() else {
            return;
        };
        let (dirs, situation) = (o.dirs.clone(), o.situation.clone());
        let file = dirs.show(&dirs.default_signers_file());
        let (live_dirs, live_situation, live_own) = (dirs.clone(), situation.clone(), own.clone());
        let modal = Modal::input_checks(
            al::OWN_TITLE,
            vec![("Allowed signers file", file.as_str())],
            vec![(Scope::LABELS[1], true)],
            move |app, values, checks| {
                let entered = values.first().cloned().unwrap_or_default();
                let scope = scope_of(&checks);
                app.set_allowed_signers(move |repo| repo.create_own_signers(&entered, scope, &own, &dirs, &situation));
            },
        )
        .with_live(move |values, checks| {
            let plan = al::plan_own(
                values.first().map(String::as_str).unwrap_or(""),
                scope_of(checks),
                &live_own,
                &live_dirs,
                &live_situation,
            );
            Live {
                text: plan.text,
                ok_label: plan.ok_label,
                blocked: plan.blocked,
                matches: Vec::new(),
            }
        });
        self.modal = Some(modal);
    }

    /// Writes the setting (`write`: `git config`, maybe the file first) on a job, then
    /// checks the signature again.
    fn set_allowed_signers(&mut self, write: impl FnOnce(&Repo) -> anyhow::Result<String> + Send + 'static) {
        self.keys_job = true;
        let repo = self.repo.clone();
        spawn(&self.tx, move || write(&repo).map_err(|e| format!("{e:#}")), Msg::SignersSet);
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
        let Some((dirs, own)) = self.shown_signers_offer().map(|o| (o.dirs.clone(), o.own.clone())) else {
            return;
        };
        let github = gitti_core::signing_keys::github_repo(&self.remotes).is_some();
        let help = al::setup_help(&dirs, github, own.as_ref().map(|(k, _)| k));
        self.modal = Some(Modal::text("Checking SSH Signatures", help));
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
