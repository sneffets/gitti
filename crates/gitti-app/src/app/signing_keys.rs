//! "Get Signing Keys from GitHub…" (GitLab, Gitea, Forgejo, a self-hosted server: the label
//! names it, `signing_keys::action_label`) on the commit header's signature: offered when
//! the signature can't be verified because its key is unknown here and a remote is on a
//! forge (`gitti_core::signing_keys::offer`) — as a button of its details window, in the
//! commit's context menu (the shown commit, as gitty's commit menu) and in the signature's
//! context menu, which always has it: greyed with the reason when it doesn't apply (no
//! forge remote, the key is known). The server is asked on a background job — a
//! self-hosted one whose kind isn't known first what it is, or the user says
//! (`ask_server`; "Server Kind of …" in the signature's menu changes it later); the dialog
//! lists the keys found (the one that made the signature ticked) and says what importing
//! writes where (`signing_keys::plan_text`); after the import the signature is verified
//! again.

use gitti_core::ObjectId;
use gitti_core::signature::SignatureInfo;
use gitti_core::availability::NO_KEY_MISSING;
use gitti_core::signing_keys::{self as sk, ForgeKey, ForgeRepo, ImportTarget, Lookup, ServerQuestion};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::*;

use super::modal::ModalSpec;
use super::{Detail, GittiApp};

/// A button of the signature's details window.
type ExtraAction = fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>);

/// The shown commit's signature, when keys from its forge can help verify it.
pub(super) struct KeyOffer {
    id: ObjectId,
    info: SignatureInfo,
    gh: ForgeRepo,
    /// The committer's e-mail: the principal of new allowed-signers lines.
    email: String,
}

thread_local! {
    /// A right click on the commit header's signature, for the diff's menu builder.
    static SIGNATURE_CLICK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The right mouse button went down on the signature (capture phase: before the diff's
/// context menu, which builds its menu a frame later, sees the click).
pub(super) fn mark_signature_click() {
    SIGNATURE_CLICK.with(|c| c.set(true));
}

/// Whether the menu being built is the signature's (and forgets the click).
pub(super) fn take_signature_click() -> bool {
    SIGNATURE_CLICK.with(|c| c.replace(false))
}

/// The signature's context menu: its details, the forge's keys (greyed with the reason
/// when they don't apply), what a self-hosted server is, and an allowed signers file when
/// none is set.
pub(super) fn signature_menu(menu: PopupMenu, app: &WeakEntity<GittiApp>, cx: &App) -> PopupMenu {
    let (why, label, server) = app.upgrade().map_or((Some(NO_KEY_MISSING), sk::ACTION_LABEL.to_string(), None), |a| {
        let a = a.read(cx);
        let server = gitti_core::forge::self_hosted_server(&a.remotes).map(|(r, _)| r.host);
        (a.signing_keys_why_not(), a.signing_keys_label(), server)
    });
    let item = |label: String, f: fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>)| {
        let app = app.clone();
        // `menu_action`: the `menu:LABEL` script step can choose it.
        super::menus::menu_action(label, move |window, cx| {
            app.update(cx, |this, cx| f(this, window, cx)).ok();
        })
    };
    let menu = menu.item(item("Signature Details…".into(), |this, w, cx| {
        this.show_signature_details(w, cx)
    }));
    let label = match why {
        Some(why) => gitti_core::availability::with_reason(&label, why),
        None => label,
    };
    let mut menu = menu.item(
        item(label, |this, w, cx| this.get_signing_keys(None, w, cx)).disabled(why.is_some()),
    );
    if let Some(host) = server {
        menu = menu.item(item(sk::server_kind_label(&host), |this, w, cx| this.server_kind(w, cx)));
    }
    // No allowed signers file set: use one the user has, or how to set one up.
    super::signers::menu_items(menu, app, cx)
}

/// The commit list's menu of commit `rev`, when it is the shown one: what its signature's
/// menu offers to get it verified ("Get Signing Keys from GitHub…", an allowed signers
/// file) — only what applies, after a separator (as gitty's commit menu).
pub(super) fn commit_menu_items(menu: PopupMenu, app: &WeakEntity<GittiApp>, rev: &str, cx: &App) -> PopupMenu {
    let Some(this) = app.upgrade() else {
        return menu;
    };
    let (offered, signers) = {
        let a = this.read(cx);
        let rev = ObjectId::from_hex(rev.as_bytes()).ok();
        let shown = matches!(&a.detail, Detail::Commit(cv) if cv.compare.is_none() && Some(cv.details.id) == rev);
        if !shown {
            return menu;
        }
        (a.signing_key_offer().map(|o| sk::action_label(&o.gh)), a.signers_files_known().is_some())
    };
    if offered.is_none() && !signers {
        return menu;
    }
    let mut menu = menu.separator();
    if let Some(label) = offered {
        let app = app.clone();
        menu = menu.item(super::menus::menu_entry(label).on_click(move |_, window, cx| {
            app.update(cx, |this, cx| this.get_signing_keys(None, window, cx)).ok();
        }));
    }
    super::signers::menu_items(menu, app, cx)
}

impl GittiApp {
    /// See [`KeyOffer`]; no git runs (the remotes are the refresh's).
    pub(super) fn signing_key_offer(&self) -> Option<KeyOffer> {
        let Detail::Commit(cv) = &self.detail else {
            return None;
        };
        if cv.compare.is_some() {
            return None;
        }
        let (id, info) = self.signature.as_ref()?;
        let info = info.as_ref().filter(|_| *id == cv.details.id)?;
        let gh = sk::offer(info, &self.remotes)?;
        Some(KeyOffer {
            id: *id,
            info: info.clone(),
            gh,
            email: cv.details.recorded_committer_email().to_string(),
        })
    }

    /// The action's label: naming the shown signature's forge, else the first forge
    /// remote's, else the general one.
    pub(super) fn signing_keys_label(&self) -> String {
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

    /// The signature's details window: a button for each way to get it verified that
    /// applies — an allowed signers file (the user's, else a new one with the user's own
    /// key, else how to set one up) and "Get Signing Keys from <the forge>…".
    pub(super) fn show_signature_details(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, Some(info))) = &self.signature else {
            return;
        };
        let details = info.details();
        let mut spec = ModalSpec::new("Signature").ok("Close").cancel(None);
        // One button for an allowed signers file (an existing one beats a new one with
        // the user's own key, which beats how to set one up) and one for the keys from
        // the forge; the file comes first when it surely helps (the user has one, or the
        // user's own key made the signature).
        let files = self.signers_files_known();
        let own = self.own_signers_offered();
        let signers: Option<(&'static str, ExtraAction)> = if files.is_some_and(|n| n > 0) {
            Some((super::signers::USE_LABEL, |this, w, cx| this.use_allowed_signers_file(w, cx)))
        } else if own.is_some() {
            Some((gitti_core::allowed_signers::OWN_LABEL, |this, w, cx| this.own_signers_dialog(w, cx)))
        } else if files.is_some() {
            Some((gitti_core::allowed_signers::HELP_LABEL, |this, w, cx| this.allowed_signers_help(w, cx)))
        } else {
            None
        };
        let signers_first = files.is_some_and(|n| n > 0) || own == Some(true);
        let forge = self.signing_key_offer().map(|o| sk::action_label(&o.gh));
        if let Some((label, f)) = signers.filter(|_| signers_first) {
            spec = spec.extra(label, f);
        }
        if let Some(label) = forge {
            spec = spec.extra(label, |this, w, cx| this.get_signing_keys(None, w, cx));
        }
        if let Some((label, f)) = signers.filter(|_| !signers_first) {
            spec = spec.extra(label, f);
        }
        self.show_text_modal_spec(spec, details, window, cx);
    }

    /// Asks the forge for the signer's keys (of `login`, else of the commit's accounts) in
    /// the background, then shows what was found.
    pub(super) fn get_signing_keys(
        &mut self,
        login: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(offer), Some(repo)) = (self.signing_key_offer(), self.repo.clone()) else {
            self.set_info("The shown signature needs no keys from a forge");
            cx.notify();
            return;
        };
        self.set_info(match &login {
            Some(l) => format!("Asking {} for the signing keys of {l}…", offer.gh.server()),
            None => format!("Asking {} ({}) for the signing keys…", offer.gh.server(), offer.gh.slug()),
        });
        cx.notify();
        let KeyOffer { id, info, gh, email } = offer;
        let task = cx.background_executor().spawn(async move {
            let fetch = sk::default_fetcher();
            let found = repo.lookup_signing_keys(&*fetch, &gh, id, &info, login.as_deref());
            let target = sk::UserDirs::from_env().map(|d| repo.import_target(&email, &d));
            (found, target)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (found, target) = task.await;
            this.update_in(cx, |this, window, cx| {
                this.status = None;
                match (found, target) {
                    (Err(e), _) => this.set_error(e.to_string()),
                    (Ok(Lookup::AskLogin { why }), _) => {
                        // The forge as known now (a self-hosted one was just found out).
                        let forge = this.signing_key_offer().map_or("Server", |o| o.gh.forge_name());
                        this.ask_login(forge, why, window, cx)
                    }
                    (Ok(Lookup::AskServer(q)), _) => this.ask_server(q, true, window, cx),
                    (Ok(Lookup::Found(_)), None) => {
                        this.set_error("No home directory: nowhere to put the keys".into())
                    }
                    (Ok(Lookup::Found(set)), Some(target)) => {
                        this.show_found_keys(id, set, target, window, cx)
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The forge can't name the account: asks for a user name (`why` says why).
    fn ask_login(&mut self, forge: &str, why: String, window: &mut Window, cx: &mut Context<Self>) {
        let spec = ModalSpec::new(format!("{forge} User"))
            .message(why)
            .field(format!("{forge} user name"), "")
            .ok("Get Keys");
        self.open_modal(spec, window, cx, |this, r, w, cx| {
            let login = r.values.first().map(|v| v.trim().to_string()).unwrap_or_default();
            if login.is_empty() {
                this.set_info("No user name given: nothing fetched");
                cx.notify();
            } else {
                this.get_signing_keys(Some(login), w, cx)
            }
        });
    }

    /// What kind of server a self-hosted host is: the user says (`q.why` says why the
    /// question); OK remembers it and, when `then_fetch`, fetches the keys.
    pub(super) fn ask_server(&mut self, q: ServerQuestion, then_fetch: bool, window: &mut Window, cx: &mut Context<Self>) {
        let labels: Vec<&str> = sk::SERVER_KINDS.iter().map(|(l, _)| *l).collect();
        let forges: Vec<usize> = (0..sk::SERVER_KINDS.len()).filter(|i| sk::SERVER_KINDS[*i].1.is_some()).collect();
        let live_q = q.clone();
        let spec = ModalSpec::new(format!("What Is {}?", q.host))
            .message(q.why.clone())
            .options(&labels, q.preselect())
            .field("Web address", q.web.clone())
            .only_for(0, &forges)
            .live(move |r| {
                let web = r.values.first().map(String::as_str).unwrap_or("");
                match live_q.plan(r.option, web) {
                    Ok(text) => super::modal::ModalLive { text, ..Default::default() },
                    Err(why) => super::modal::ModalLive { blocked: Some(why), ..Default::default() },
                }
            })
            .ok(if then_fetch { "Remember and Get Keys" } else { "Remember" });
        self.open_modal(spec, window, cx, move |this, r, w, cx| {
            let web = r.values.first().cloned().unwrap_or_default();
            match q.remember(r.option, &web) {
                Ok(entry) if entry.forge.is_some() && then_fetch => this.get_signing_keys(None, w, cx),
                Ok(entry) => {
                    this.set_info(match entry.forge {
                        Some(f) => format!("{} is a {} server now", q.host, f.self_hosted_name()),
                        None => format!("{} is a plain git server now: no signing keys are fetched from it", q.host),
                    });
                    cx.notify();
                }
                Err(why) => {
                    this.set_error(why);
                    cx.notify();
                }
            }
        });
    }

    /// "Server Kind of …" (the signature's menu): what the first self-hosted remote's
    /// server is, changeable.
    pub(super) fn server_kind(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((repo, known)) = gitti_core::forge::self_hosted_server(&self.remotes) else {
            return;
        };
        let q = ServerQuestion::about(&repo, known.as_ref());
        // Keys are fetched right away when the shown signature wants them.
        let key_unknown = matches!(&self.signature, Some((_, Some(info))) if info.key_unknown());
        self.ask_server(q, key_unknown, window, cx);
    }

    /// The keys found: one checkbox each (the one that made the signature ticked), the
    /// plan in plain words; Import imports the ticked ones.
    fn show_found_keys(
        &mut self,
        id: ObjectId,
        set: sk::KeySet,
        target: ImportTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut spec =
            ModalSpec::new(format!("Signing Keys from {}", set.server)).message(sk::plan_text(&set, &target));
        for k in &set.keys {
            let usable = k.kind != gitti_core::signature::SigKind::Gpg || target.gpg_found;
            spec = spec.check(k.label(), k.signed_this && k.importable() && usable);
        }
        let forge = set.forge.name();
        spec = spec.extra(format!("Other {forge} User…"), move |this, w, cx| {
            this.ask_login(forge, format!("Whose keys? Type a {forge} user name."), w, cx)
        });
        if set.keys.is_empty() {
            spec = spec.ok("Close").cancel(None);
            self.open_modal(spec, window, cx, |_, _, _, _| {});
            return;
        }
        let keys = set.keys;
        self.open_modal(spec.ok("Import"), window, cx, move |this, r, _, cx| {
            let chosen: Vec<ForgeKey> = keys
                .into_iter()
                .zip(r.checks)
                .filter(|(k, on)| *on && k.importable())
                .map(|(k, _)| k)
                .collect();
            if chosen.is_empty() {
                this.set_info("No key ticked: nothing imported");
                cx.notify();
                return;
            }
            this.import_signing_keys(id, chosen, target, cx);
        });
    }

    /// Imports the chosen keys in the background, then verifies the signature again.
    fn import_signing_keys(
        &mut self,
        id: ObjectId,
        keys: Vec<ForgeKey>,
        target: ImportTarget,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        self.set_info("Importing the signing keys…");
        cx.notify();
        let task = cx
            .background_executor()
            .spawn(async move { repo.import_signing_keys(&keys, &target) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(summary) => {
                        this.set_info(format!("{summary}; checking the signature again"));
                        let shown = matches!(&this.detail, Detail::Commit(cv) if cv.details.id == id);
                        if shown {
                            this.verify_signature(id, this.detail_generation, cx);
                        }
                    }
                    Err(e) => this.set_error(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
