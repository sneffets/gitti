//! "Get Signing Keys from GitHub…" (GitLab, Gitea, Forgejo, a self-hosted server: the label
//! names it) in gitty (as gitti's `app/signing_keys.rs`; logic in
//! `gitti_core::signing_keys`). Offered when the shown commit's signature can't be verified
//! because its key is unknown here and a remote is on a forge: in the menu (`m`) on the
//! header's Signature line — always there, greyed with the reason when it doesn't apply
//! (no forge remote, the key is known) —, in the commit's menu and as a key (`K`) of the
//! signature's details window; the Signature line says so. The server is asked on a job
//! (`Msg::SigningKeys`) — a self-hosted one whose kind isn't known first what it is, or the
//! user says (`ask_server`; "Server Kind of …" in the Signature line's menu changes it
//! later); the dialog lists the keys found (the one that made the signature ticked) and
//! says what importing writes where; after the import the signature is verified again.

use gitti_core::ObjectId;
use gitti_core::signature::{SigKind, SignatureInfo};
use gitti_core::signing_keys::{
    self as sk, FetchError, ForgeKey, ForgeRepo, ImportTarget, KeySet, Lookup, ServerQuestion,
};

use gitti_core::availability::NO_KEY_MISSING;

use super::{App, Detail, MenuItem, Modal, Msg, spawn};

/// A finished job of the flow.
pub enum KeysMsg {
    Found(ObjectId, Box<(Result<Lookup, FetchError>, Option<ImportTarget>)>),
    Imported(ObjectId, Result<String, String>),
}

/// The shown commit's signature, when keys from its forge can help verify it.
struct KeyOffer {
    id: ObjectId,
    info: SignatureInfo,
    gh: ForgeRepo,
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

    /// The action's label: naming the shown signature's forge, else the first forge
    /// remote's, else the general one.
    pub fn signing_keys_label(&self) -> String {
        match self.signing_key_offer() {
            Some(o) => sk::action_label(&o.gh),
            None => sk::action_label_for(&self.remotes),
        }
    }

    /// Why "Get Signing Keys from …" can't act on the shown signature (`None`: it
    /// can) — the menu bar's rule (`availability`), so all places give the same reason.
    pub(super) fn signing_keys_why_not(&self) -> Option<&'static str> {
        if self.signing_key_offer().is_some() {
            return None;
        }
        Some(gitti_core::availability::why_not("GetSigningKeys", &self.repo_facts()).unwrap_or(NO_KEY_MISSING))
    }

    /// Menu entries for commit `sha` (the commit menu: only when it applies) or the shown
    /// one (`None`: the Signature line's menu, which also has the details, always the
    /// action — greyed with the reason when it doesn't apply — and what a self-hosted
    /// server is).
    pub(super) fn signing_key_items(&self, sha: Option<&str>) -> Vec<MenuItem> {
        let label = self.signing_keys_label();
        let get = || MenuItem::new(label.clone(), |app: &mut App| app.get_signing_keys(None)).key('K');
        let Some(sha) = sha else {
            if !matches!(&self.signature, Some((_, Some(_)))) {
                return Vec::new();
            }
            let mut items = vec![
                MenuItem::new("Signature Details", |app: &mut App| app.show_signature()),
                get().unless(self.signing_keys_why_not()),
            ];
            if let Some((repo, _)) = gitti_core::forge::self_hosted_server(&self.remotes) {
                items.push(MenuItem::new(sk::server_kind_label(&repo.host), |app: &mut App| app.server_kind()));
            }
            return items;
        };
        let offered = self
            .signing_key_offer()
            .is_some_and(|o| ObjectId::from_hex(sha.as_bytes()).is_ok_and(|id| id == o.id));
        if offered { vec![get()] } else { Vec::new() }
    }

    /// Asks the forge for the signer's keys (of `login`, else of the commit's accounts).
    pub fn get_signing_keys(&mut self, login: Option<String>) {
        let Some(KeyOffer { id, info, gh, email }) = self.signing_key_offer() else {
            self.set_info("The shown signature needs no keys from a forge");
            return;
        };
        self.set_info(match &login {
            Some(l) => format!("Asking {} for the signing keys of {l}…", gh.server()),
            None => format!("Asking {} ({}) for the signing keys…", gh.server(), gh.slug()),
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
                    // The forge as known now (a self-hosted one was just found out).
                    let forge = self.signing_key_offer().map_or("Server", |o| o.gh.forge_name());
                    self.ask_login(forge, why);
                }
                (Ok(Lookup::AskServer(q)), _) => {
                    self.status = None;
                    self.ask_server(q, true);
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

    fn ask_login(&mut self, forge: &str, why: String) {
        let field = format!("{forge} user name");
        self.modal = Some(
            Modal::input(format!("{forge} User"), vec![(field.as_str(), "")], |app, values| {
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

    /// What kind of server a self-hosted host is: a menu of the kinds (`q.why` says why the
    /// question), then the web address for a forge; remembered, and with `then_fetch` the
    /// keys are fetched.
    pub(super) fn ask_server(&mut self, q: ServerQuestion, then_fetch: bool) {
        let mut items = MenuItem::notes(&q.why, 60);
        items.push(MenuItem::separator());
        for (ix, (label, forge)) in sk::SERVER_KINDS.iter().enumerate() {
            let q = q.clone();
            let label = if ix == q.preselect() && q.known.is_some() { format!("{label} (now)") } else { label.to_string() };
            items.push(MenuItem::new(label, move |app: &mut App| match forge {
                Some(_) => app.ask_server_web(q, ix, then_fetch),
                None => app.remember_server(&q, ix, "", false),
            }));
        }
        self.modal = Some(Modal::menu(format!("What Is {}?", q.host), items));
    }

    /// The web address of the server `q` asks about, now said to be `SERVER_KINDS[kind]`.
    fn ask_server_web(&mut self, q: ServerQuestion, kind: usize, then_fetch: bool) {
        let web = q.web.clone();
        let live_q = q.clone();
        let ok = if then_fetch { "remember and get keys" } else { "remember" };
        let modal = Modal::input(format!("Web Address of {}", q.host), vec![("Web address", web.as_str())], move |app, values| {
            let web = values.first().cloned().unwrap_or_default();
            app.remember_server(&q, kind, &web, then_fetch);
        })
        .with_live(move |values, _| {
            let web = values.first().map(String::as_str).unwrap_or("");
            match live_q.plan(kind, web) {
                Ok(text) => crate::modal::Live { text, ok_label: ok.into(), ..Default::default() },
                Err(why) => crate::modal::Live { blocked: Some(why), ok_label: ok.into(), ..Default::default() },
            }
        });
        self.modal = Some(modal);
    }

    fn remember_server(&mut self, q: &ServerQuestion, kind: usize, web: &str, then_fetch: bool) {
        match q.remember(kind, web) {
            Ok(entry) if entry.forge.is_some() && then_fetch => self.get_signing_keys(None),
            Ok(entry) => self.set_info(match entry.forge {
                Some(f) => format!("{} is a {} server now", q.host, f.self_hosted_name()),
                None => format!("{} is a plain git server now: no signing keys are fetched from it", q.host),
            }),
            Err(why) => self.set_error(why),
        }
    }

    /// "Server Kind of …" (the Signature line's menu): what the first self-hosted remote's
    /// server is, changeable.
    pub(super) fn server_kind(&mut self) {
        let Some((repo, known)) = gitti_core::forge::self_hosted_server(&self.remotes) else {
            return;
        };
        let key_unknown = matches!(&self.signature, Some((_, Some(info))) if info.key_unknown());
        self.ask_server(ServerQuestion::about(&repo, known.as_ref()), key_unknown);
    }

    fn show_found_keys(&mut self, id: ObjectId, set: KeySet, target: ImportTarget) {
        let mut note = sk::plan_text(&set, &target);
        let title = format!("Signing Keys from {}", set.server);
        if set.keys.is_empty() {
            self.modal = Some(Modal::text(title, note));
            return;
        }
        note.push_str("\n\nspace ticks a key, ⏎ imports the ticked ones, esc cancels.");
        let labels: Vec<String> = set.keys.iter().map(ForgeKey::short_label).collect();
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
            title,
            Vec::new(),
            checks,
            move |app, _, on| {
                let chosen: Vec<ForgeKey> = keys
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

    fn import_signing_keys(&mut self, id: ObjectId, keys: Vec<ForgeKey>, target: ImportTarget) {
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
