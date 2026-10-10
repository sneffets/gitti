//! "Get Signing Keys from GitHub…" on the commit header's signature: offered when
//! the signature can't be verified because its key is unknown here and a remote is on
//! GitHub (`gitti_core::signing_keys::offer`) — in the signature's context menu and as a
//! button of its details window. GitHub is asked on a background job; the dialog lists the
//! keys found (the one that made the signature ticked) and says what importing writes
//! where (`signing_keys::plan_text`); after the import the signature is verified again.

use gitti_core::ObjectId;
use gitti_core::signature::SignatureInfo;
use gitti_core::signing_keys::{self as sk, GitHubKey, GitHubRepo, ImportTarget, Lookup};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::*;

use super::modal::ModalSpec;
use super::{Detail, GittiApp};

/// The shown commit's signature, when keys from GitHub can help verify it.
pub(super) struct KeyOffer {
    id: ObjectId,
    info: SignatureInfo,
    gh: GitHubRepo,
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

/// The signature's context menu: its details, and the GitHub keys when offered.
pub(super) fn signature_menu(menu: PopupMenu, app: &WeakEntity<GittiApp>, cx: &App) -> PopupMenu {
    let offered = app
        .upgrade()
        .is_some_and(|a| a.read(cx).signing_key_offer().is_some());
    let item = |label: &'static str, f: fn(&mut GittiApp, &mut Window, &mut Context<GittiApp>)| {
        let app = app.clone();
        super::menus::menu_entry(label).on_click(move |_, window, cx| {
            app.update(cx, |this, cx| f(this, window, cx)).ok();
        })
    };
    let menu = menu.item(item("Signature Details…", |this, w, cx| {
        this.show_signature_details(w, cx)
    }));
    let menu = if offered {
        menu.item(item(sk::ACTION_LABEL, |this, w, cx| this.get_signing_keys(None, w, cx)))
    } else {
        menu
    };
    // No allowed signers file set: use one the user has, or how to set one up.
    super::signers::menu_items(menu, app, cx)
}

impl GittiApp {
    /// See [`KeyOffer`]; no git runs (the remotes are the refresh's).
    pub(super) fn signing_key_offer(&self) -> Option<KeyOffer> {
        let Detail::Commit(cv) = &self.detail else {
            return None;
        };
        let (id, info) = self.signature.as_ref()?;
        let info = info.as_ref().filter(|_| *id == cv.details.id)?;
        let gh = sk::offer(info, &self.remotes)?;
        Some(KeyOffer {
            id: *id,
            info: info.clone(),
            gh,
            email: cv.details.committer_email.clone(),
        })
    }

    /// The signature's details window (+ "Get Signing Keys from GitHub…" when offered).
    pub(super) fn show_signature_details(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, Some(info))) = &self.signature else {
            return;
        };
        let details = info.details();
        let mut spec = ModalSpec::new("Signature").ok("Close").cancel(None);
        // One button: an allowed signers file the user has beats a new one with the user's
        // own key when that key signed, then fetching keys, a file with the own key, how
        // to set one up.
        let files = self.signers_files_known();
        let own = self.own_signers_offered();
        let own_button = |spec: ModalSpec| {
            spec.extra(gitti_core::allowed_signers::OWN_LABEL, |this, w, cx| this.own_signers_dialog(w, cx))
        };
        if files.is_some_and(|n| n > 0) {
            spec = spec.extra(super::signers::USE_LABEL, |this, w, cx| this.use_allowed_signers_file(w, cx));
        } else if own == Some(true) {
            spec = own_button(spec);
        } else if self.signing_key_offer().is_some() {
            spec = spec.extra(sk::ACTION_LABEL, |this, w, cx| this.get_signing_keys(None, w, cx));
        } else if own.is_some() {
            spec = own_button(spec);
        } else if files.is_some() {
            spec = spec.extra(gitti_core::allowed_signers::HELP_LABEL, |this, w, cx| {
                this.allowed_signers_help(w, cx)
            });
        }
        self.show_text_modal_spec(spec, details, window, cx);
    }

    /// Asks GitHub for the signer's keys (of `login`, else of the commit's accounts) in the
    /// background, then shows what was found.
    pub(super) fn get_signing_keys(
        &mut self,
        login: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(offer), Some(repo)) = (self.signing_key_offer(), self.repo.clone()) else {
            self.set_info("The shown signature needs no keys from GitHub");
            cx.notify();
            return;
        };
        self.set_info(match &login {
            Some(l) => format!("Asking GitHub for the signing keys of {l}…"),
            None => format!("Asking GitHub ({}) for the signing keys…", offer.gh.slug()),
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
                    (Ok(Lookup::AskLogin { why }), _) => this.ask_github_login(why, window, cx),
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

    /// GitHub can't name the account: asks for a user name (`why` says why).
    fn ask_github_login(&mut self, why: String, window: &mut Window, cx: &mut Context<Self>) {
        let spec = ModalSpec::new("GitHub User")
            .message(why)
            .field("GitHub user name", "")
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
        let mut spec = ModalSpec::new("Signing Keys from GitHub").message(sk::plan_text(&set, &target));
        for k in &set.keys {
            let usable = k.kind != gitti_core::signature::SigKind::Gpg || target.gpg_found;
            spec = spec.check(k.label(), k.signed_this && k.importable() && usable);
        }
        spec = spec.extra("Other GitHub User…", |this, w, cx| {
            this.ask_github_login("Whose keys? Type a GitHub user name.".into(), w, cx)
        });
        if set.keys.is_empty() {
            spec = spec.ok("Close").cancel(None);
            self.open_modal(spec, window, cx, |_, _, _, _| {});
            return;
        }
        let keys = set.keys;
        self.open_modal(spec.ok("Import"), window, cx, move |this, r, _, cx| {
            let chosen: Vec<GitHubKey> = keys
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
        keys: Vec<GitHubKey>,
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
