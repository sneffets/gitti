//! An allowed signers file for a repository without one (`gitti_core::allowed_signers`):
//! when the shown SSH signature can't be checked because `gpg.ssh.allowedSignersFile`
//! isn't set, the signature's menu and details window offer the file the user already has
//! — set globally (or in a file the global config includes), in another open tab's
//! repository, at a usual place — "for This Repository" or "for All Repositories"; the
//! dialog says what is written where. With the user's own SSH signing key they also offer
//! to create one that lists it. Without any known file they explain how to set one up.
//! After the write the signature is checked again (in every tab for a global one).

use std::path::PathBuf;

use gitti_core::ObjectId;
use gitti_core::Repo;
use gitti_core::allowed_signers::{self as al, Candidate, OwnKey, Scope, Situation};
use gitti_core::signing_keys::UserDirs;
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::*;

use super::modal::{ModalLive, ModalSpec};
use super::picker::PickerItem;
use super::{AppEvent, Detail, GittiApp};

thread_local! {
    /// The repositories of the open tabs (the workspace keeps it current).
    static OPEN_REPOS: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The workspace's tabs changed.
pub(crate) fn set_open_repos(repos: Vec<PathBuf>) {
    OPEN_REPOS.with(|r| *r.borrow_mut() = repos);
}

/// The repositories of the open tabs.
pub(super) fn open_repos() -> Vec<PathBuf> {
    OPEN_REPOS.with(|r| r.borrow().clone())
}

/// The shown commit's SSH signature can't be checked (no allowed signers file set); the
/// files that could be used, best first (none: explain how to set one up).
pub(super) struct SignersOffer {
    id: ObjectId,
    candidates: Vec<Candidate>,
    dirs: UserDirs,
    /// How the setting stands (unset, empty, a missing file; where).
    situation: Situation,
    /// The user's own SSH signing key, when no offered file lists it yet; whether it made
    /// the signature.
    own: Option<(OwnKey, bool)>,
}

impl SignersOffer {
    /// Looks for the files (in the background: reads config files, no git). `None`
    /// without a home directory.
    pub(super) fn find(id: ObjectId, repo: &Repo, others: &[PathBuf]) -> Option<Self> {
        let dirs = UserDirs::from_env()?;
        let candidates = al::candidates(&dirs, repo.workdir(), others);
        let situation = Situation::of(repo.workdir());
        let own = al::own_key(repo.workdir(), &dirs)
            .ok()
            .filter(|k| !al::lists_key(&candidates, k))
            .map(|k| {
                let signed = k.signed(repo, id);
                (k, signed)
            });
        Some(SignersOffer { id, candidates, dirs, situation, own })
    }
}

/// What a signature-menu entry opens.
#[derive(Clone, Copy)]
enum Entry {
    Use(Scope),
    Own,
    Help,
}

/// The palette entry / the details window's button.
pub(super) const USE_LABEL: &str = "Use an Allowed Signers File…";

/// The signature menu's entries when the offer applies: both scopes for the best file,
/// else how to set one up.
pub(super) fn menu_items(menu: PopupMenu, app: &WeakEntity<GittiApp>, cx: &App) -> PopupMenu {
    let Some(this) = app.upgrade() else {
        return menu;
    };
    let entries: Vec<(String, Entry)> = match this.read(cx).shown_signers_offer() {
        None => return menu,
        Some(o) => {
            let mut v: Vec<(String, Entry)> = match o.candidates.first() {
                Some(c) => [Scope::Repository, Scope::Global]
                    .into_iter()
                    .map(|s| (s.menu_label(c, &o.dirs), Entry::Use(s)))
                    .collect(),
                None => Vec::new(),
            };
            if o.own.is_some() {
                v.push((al::OWN_LABEL.to_string(), Entry::Own));
            }
            if o.candidates.is_empty() {
                v.push((al::HELP_LABEL.to_string(), Entry::Help));
            }
            v
        }
    };
    entries.into_iter().fold(menu, |menu, (label, entry)| {
        let app = app.clone();
        menu.item(super::menus::menu_entry(label).on_click(move |_, window, cx| {
            app.update(cx, |this, cx| match entry {
                Entry::Use(s) => this.allowed_signers_dialog(s, window, cx),
                Entry::Own => this.own_signers_dialog(window, cx),
                Entry::Help => this.allowed_signers_help(window, cx),
            })
            .ok();
        }))
    })
}

impl GittiApp {
    /// The offer, when it is about the shown commit.
    fn shown_signers_offer(&self) -> Option<&SignersOffer> {
        let Detail::Commit(cv) = &self.detail else {
            return None;
        };
        self.signers_offer.as_ref().filter(|o| o.id == cv.details.id)
    }

    /// The shown signature needs an allowed signers file: how many the user has
    /// (`None`: it doesn't need one).
    pub(super) fn signers_files_known(&self) -> Option<usize> {
        self.shown_signers_offer().map(|o| o.candidates.len())
    }

    /// The shown signature needs an allowed signers file and the user's own key could
    /// make one: `Some(true)` when that key made the signature.
    pub(super) fn own_signers_offered(&self) -> Option<bool> {
        self.shown_signers_offer()?.own.as_ref().map(|(_, signed)| *signed)
    }

    /// "Use an Allowed Signers File…": the dialog for this repository with the best file,
    /// else the one creating a file with the user's own key, else how to set one up.
    pub(super) fn use_allowed_signers_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let what = self
            .shown_signers_offer()
            .map(|o| (o.candidates.is_empty(), o.own.is_some()));
        match what {
            None => {
                self.set_info(
                    "Only for an SSH signature that can't be checked because no allowed signers \
                     file is set: show such a commit first",
                );
                cx.notify();
            }
            Some((false, _)) => self.allowed_signers_dialog(Scope::Repository, window, cx),
            Some((true, true)) => self.own_signers_dialog(window, cx),
            Some((true, false)) => self.allowed_signers_help(window, cx),
        }
    }

    /// The dialog: the file (the best one filled in, the others as buttons), this
    /// repository or all, and live what OK writes where (`allowed_signers::plan`).
    pub(super) fn allowed_signers_dialog(&mut self, scope: Scope, window: &mut Window, cx: &mut Context<Self>) {
        let Some(offer) = self.shown_signers_offer() else {
            return;
        };
        let (dirs, candidates, situation) = (offer.dirs.clone(), offer.candidates.clone(), offer.situation.clone());
        let first = candidates.first().map(|c| c.entry(&dirs)).unwrap_or_default();
        let mut spec = ModalSpec::new(al::TITLE)
            .field("Allowed signers file", first)
            .options(&Scope::LABELS, scope.index());
        if candidates.len() > 1 {
            for c in &candidates {
                let shown = c.entry(&dirs);
                spec = spec.choice(shown.clone(), 0, shown);
            }
        }
        let (live_dirs, live_situation) = (dirs.clone(), situation.clone());
        let spec = spec
            .live(move |res| {
                let entered = res.values.first().map(String::as_str).unwrap_or("");
                let plan = al::plan(entered, Scope::from_index(res.option), &candidates, &live_dirs, &live_situation);
                ModalLive {
                    text: plan.text,
                    ok_label: Some(plan.ok_label),
                    blocked: plan.blocked,
                    fills: Vec::new(),
                    fills_label: String::new(),
                }
            })
            .ok(match scope {
                Scope::Repository => "Use for This Repository",
                Scope::Global => "Use for All Repositories",
            });
        self.open_modal(spec, window, cx, move |this, res, _, cx| {
            let entered = res.values.first().cloned().unwrap_or_default();
            let (scope, dirs, situation) = (Scope::from_index(res.option), dirs.clone(), situation.clone());
            this.set_allowed_signers(scope, cx, move |repo| {
                repo.use_allowed_signers(&entered, scope, &dirs, &situation)
            });
        });
    }

    /// "Create Allowed Signers File with Your Key…": the file (the default one filled
    /// in), all repositories or this one, and live what OK writes where
    /// (`allowed_signers::plan_own`).
    pub(super) fn own_signers_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(offer) = self.shown_signers_offer() else {
            return;
        };
        let Some((own, _)) = offer.own.clone() else {
            return;
        };
        let (dirs, situation) = (offer.dirs.clone(), offer.situation.clone());
        let file = dirs.show(&dirs.default_signers_file());
        let (live_dirs, live_situation, live_own) = (dirs.clone(), situation.clone(), own.clone());
        let spec = ModalSpec::new(al::OWN_TITLE)
            .field("Allowed signers file", file)
            .options(&Scope::LABELS, Scope::Global.index())
            .live(move |res| {
                let entered = res.values.first().map(String::as_str).unwrap_or("");
                let plan = al::plan_own(entered, Scope::from_index(res.option), &live_own, &live_dirs, &live_situation);
                ModalLive {
                    text: plan.text,
                    ok_label: Some(plan.ok_label),
                    blocked: plan.blocked,
                    fills: Vec::new(),
                    fills_label: String::new(),
                }
            })
            .ok("Create and Use for All Repositories");
        self.open_modal(spec, window, cx, move |this, res, _, cx| {
            let entered = res.values.first().cloned().unwrap_or_default();
            let (scope, dirs, situation, own) = (Scope::from_index(res.option), dirs.clone(), situation.clone(), own.clone());
            this.set_allowed_signers(scope, cx, move |repo| {
                repo.create_own_signers(&entered, scope, &own, &dirs, &situation)
            });
        });
    }

    /// Writes the setting (`write`: `git config`, maybe the file first) in the background,
    /// then checks the signature again — in every tab when it is global.
    fn set_allowed_signers(
        &mut self,
        scope: Scope,
        cx: &mut Context<Self>,
        write: impl FnOnce(&Repo) -> anyhow::Result<String> + Send + 'static,
    ) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let task = cx.background_executor().spawn(async move { write(&repo) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(summary) => {
                        this.set_info(format!("{summary}; checking the signature again"));
                        this.reverify_shown_signature(cx);
                        if scope == Scope::Global {
                            cx.emit(AppEvent::SignersChanged);
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

    /// Verifies the shown commit's signature again (the allowed signers changed).
    pub(crate) fn reverify_shown_signature(&mut self, cx: &mut Context<Self>) {
        if let Detail::Commit(cv) = &self.detail
            && cv.sig_kind.is_some()
        {
            let id = cv.details.id;
            self.verify_signature(id, self.detail_generation, cx);
        }
    }

    /// No allowed signers file known: what to do, in plain words.
    pub(super) fn allowed_signers_help(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((dirs, own)) = self.shown_signers_offer().map(|o| (o.dirs.clone(), o.own.clone())) else {
            return;
        };
        let github = gitti_core::signing_keys::github_repo(&self.remotes).is_some();
        let spec = ModalSpec::new("Checking SSH Signatures").ok("Close").cancel(None);
        let help = al::setup_help(&dirs, github, own.as_ref().map(|(k, _)| k));
        self.show_text_modal_spec(spec, help, window, cx);
    }

    /// Palette: "Use an Allowed Signers File…".
    pub(super) fn signers_palette_items(&self) -> Vec<PickerItem> {
        vec![super::picker::item(USE_LABEL, "to check SSH signatures (gpg.ssh.allowedSignersFile)", |t, w, cx| {
            t.use_allowed_signers_file(w, cx)
        })]
    }
}
