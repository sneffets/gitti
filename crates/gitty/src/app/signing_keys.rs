//! "Get Signing Keys from GitHub…" in gitty (as gitti's `app/signing_keys.rs`;
//! logic in `gitti_core::signing_keys`). Offered when the shown commit's signature can't be
//! verified because its key is unknown here and a remote is on GitHub: in the menu (`m`) on
//! the header's Signature line — always there, greyed with the reason when it doesn't apply
//! (no GitHub remote, the key is known) —, in the commit's menu and as a key (`K`) of the
//! signature's details window; the Signature line says so. GitHub is asked on a job (`Msg::SigningKeys`); the dialog lists the keys
//! found (the one that made the signature ticked) and says what importing writes where;
//! after the import the signature is verified again.

use gitti_core::ObjectId;
use gitti_core::signature::{SigKind, SignatureInfo};
use gitti_core::signing_keys::{
    self as sk, FetchError, GitHubKey, GitHubRepo, ImportTarget, KeySet, Lookup,
};

use gitti_core::availability::NO_KEY_MISSING;

use super::{App, Detail, MenuItem, Modal, Msg, spawn};

/// A finished job of the flow.
pub enum KeysMsg {
    Found(ObjectId, Box<(Result<Lookup, FetchError>, Option<ImportTarget>)>),
    Imported(ObjectId, Result<String, String>),
}

/// The shown commit's signature, when keys from GitHub can help verify it.
struct KeyOffer {
    id: ObjectId,
    info: SignatureInfo,
    gh: GitHubRepo,
    email: String,
}

impl App {
    fn signing_key_offer(&self) -> Option<KeyOffer> {
        let Detail::Commit(cv) = &self.detail else {
            return None;
        };
        if cv.compare.is_some() {
            return None;
        }
        let (id, info) = self.signature.as_ref()?;
        let info = info.as_ref().filter(|_| *id == cv.id)?;
        let gh = sk::offer(info, &self.remotes)?;
        Some(KeyOffer {
            id: *id,
            info: info.clone(),
            gh,
            email: cv.details.recorded_committer_email().to_string(),
        })
    }

    /// The action applies to the shown commit (header hint, details window).
    pub fn signing_keys_offered(&self) -> bool {
        self.signing_key_offer().is_some()
    }

    /// Why "Get Signing Keys from GitHub…" can't act on the shown signature (`None`: it
    /// can) — the menu bar's rule (`availability`), so all places give the same reason.
    pub(super) fn signing_keys_why_not(&self) -> Option<&'static str> {
        if self.signing_key_offer().is_some() {
            return None;
        }
        Some(gitti_core::availability::why_not("GetSigningKeys", &self.repo_facts()).unwrap_or(NO_KEY_MISSING))
    }

    /// Menu entries for commit `sha` (the commit menu: only when it applies) or the shown
    /// one (`None`: the Signature line's menu, which also has the details and always the
    /// action, greyed with the reason when it doesn't apply).
    pub(super) fn signing_key_items(&self, sha: Option<&str>) -> Vec<MenuItem> {
        let get = || MenuItem::new(sk::ACTION_LABEL, |app: &mut App| app.get_signing_keys(None)).key('K');
        let Some(sha) = sha else {
            if !matches!(&self.signature, Some((_, Some(_)))) {
                return Vec::new();
            }
            return vec![
                MenuItem::new("Signature Details", |app: &mut App| app.show_signature()),
                get().unless(self.signing_keys_why_not()),
            ];
        };
        let offered = self
            .signing_key_offer()
            .is_some_and(|o| ObjectId::from_hex(sha.as_bytes()).is_ok_and(|id| id == o.id));
        if offered { vec![get()] } else { Vec::new() }
    }

    /// Asks GitHub for the signer's keys (of `login`, else of the commit's accounts).
    pub fn get_signing_keys(&mut self, login: Option<String>) {
        let Some(KeyOffer { id, info, gh, email }) = self.signing_key_offer() else {
            self.set_info("The shown signature needs no keys from GitHub");
            return;
        };
        self.set_info(match &login {
            Some(l) => format!("Asking GitHub for the signing keys of {l}…"),
            None => format!("Asking GitHub ({}) for the signing keys…", gh.slug()),
        });
        self.keys_job = true;
        let repo = self.repo.clone();
        spawn(
            &self.tx,
            move || {
                let fetch = sk::default_fetcher();
                let found = repo.lookup_signing_keys(&*fetch, &gh, id, &info, login.as_deref());
                let target = sk::UserDirs::from_env().map(|d| repo.import_target(&email, &d));
                (found, target)
            },
            move |found| Msg::SigningKeys(Box::new(KeysMsg::Found(id, Box::new(found)))),
        );
    }

    pub(super) fn signing_keys_msg(&mut self, msg: KeysMsg) {
        self.keys_job = false;
        match msg {
            KeysMsg::Found(id, found) => match *found {
                (Err(e), _) => self.set_error(e.to_string()),
                (Ok(Lookup::AskLogin { why }), _) => {
                    self.status = None;
                    self.ask_github_login(why);
                }
                (Ok(Lookup::Found(_)), None) => {
                    self.set_error("No home directory: nowhere to put the keys".into())
                }
                (Ok(Lookup::Found(set)), Some(target)) => {
                    self.status = None;
                    self.show_found_keys(id, set, target);
                }
            },
            KeysMsg::Imported(_, Err(e)) => self.set_error(e),
            KeysMsg::Imported(id, Ok(summary)) => {
                self.set_info(format!("{summary}; checking the signature again"));
                self.reverify_signature(id);
            }
        }
    }

    fn ask_github_login(&mut self, why: String) {
        self.modal = Some(
            Modal::input("GitHub User", vec![("GitHub user name", "")], |app, values| {
                let login = values.first().map(|v| v.trim().to_string()).unwrap_or_default();
                if login.is_empty() {
                    app.set_info("No user name given: nothing fetched");
                } else {
                    app.get_signing_keys(Some(login));
                }
            })
            .with_note(why),
        );
    }

    fn show_found_keys(&mut self, id: ObjectId, set: KeySet, target: ImportTarget) {
        let mut note = sk::plan_text(&set, &target);
        if set.keys.is_empty() {
            self.modal = Some(Modal::text("Signing Keys from GitHub", note));
            return;
        }
        note.push_str("\n\nspace ticks a key, ⏎ imports the ticked ones, esc cancels.");
        let labels: Vec<String> = set.keys.iter().map(GitHubKey::short_label).collect();
        let checks: Vec<(&str, bool)> = set
            .keys
            .iter()
            .zip(&labels)
            .map(|(k, l)| {
                let usable = k.kind != SigKind::Gpg || target.gpg_found;
                (l.as_str(), k.signed_this && k.importable() && usable)
            })
            .collect();
        let keys = set.keys.clone();
        let modal = Modal::input_checks(
            "Signing Keys from GitHub",
            Vec::new(),
            checks,
            move |app, _, on| {
                let chosen: Vec<GitHubKey> = keys
                    .into_iter()
                    .zip(on)
                    .filter(|(k, on)| *on && k.importable())
                    .map(|(k, _)| k)
                    .collect();
                if chosen.is_empty() {
                    app.set_info("No key ticked: nothing imported");
                    return;
                }
                app.import_signing_keys(id, chosen, target);
            },
        )
        .with_note(note);
        self.modal = Some(modal);
    }

    fn import_signing_keys(&mut self, id: ObjectId, keys: Vec<GitHubKey>, target: ImportTarget) {
        self.set_info("Importing the signing keys…");
        self.keys_job = true;
        let repo = self.repo.clone();
        spawn(
            &self.tx,
            move || repo.import_signing_keys(&keys, &target).map_err(|e| format!("{e:#}")),
            move |r| Msg::SigningKeys(Box::new(KeysMsg::Imported(id, r))),
        );
    }

    /// Verifies the shown commit's signature again (keys changed).
    pub(super) fn reverify_signature(&mut self, id: ObjectId) {
        let shown = matches!(&self.detail, Detail::Commit(cv) if cv.id == id);
        if !shown {
            return;
        }
        self.signature = None;
        self.signers_offer = None;
        if let Detail::Commit(cv) = &self.detail {
            let cv = cv.clone();
            self.header = crate::ui::commit_header(self, &cv);
        }
        self.spawn_verify(id);
    }
}
