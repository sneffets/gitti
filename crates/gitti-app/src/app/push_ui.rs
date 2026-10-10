//! Push with options and the palette's second steps for remotes: the main Push (toolbar,
//! ⌘⌥↑, `P`), "Push…" → a remote / all remotes / Options… (the dialog), "Pull from…" and
//! "Fetch from…" → a remote. The logic (refspecs, defaults, results per remote) is
//! `gitti_core::push`, shared with gitty.

use gitti_core::RefKind;
use gitti_core::overwrite::{Overwrite, PushRejected, Source, WayOut};
use gitti_core::pull::{DivergentPull, PullStrategy};
use gitti_core::push::{
    PUSH_CHOICES_KEY, PullBlock, PushBlock, PushChoices, PushOptions, PushTags, QuickPush,
    Upstream, pull_block, push_block, quick_pushes,
};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;

use super::GittiApp;
use super::modal::{ModalLive, ModalSpec};
use super::picker::PickerItem;

impl GittiApp {
    /// The main Push (toolbar, menu bar, ⌘⌥↑ / `P`, palette, the current branch's menu):
    /// the current branch to its upstream. Without one no git runs — "Push to…" opens
    /// with the remotes (the chosen one becomes the upstream) and Set Upstream… (not
    /// `-u` to the default remote: that would ask for a passphrase before the user chose
    /// anything). Decided from the last refresh, as Pull; `Repo::push_current` checks once
    /// more (locally) if that is stale.
    pub(super) fn push_main(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let branch = self.head.branch.clone();
        let has_upstream = branch.as_deref().is_some_and(|b| self.has_upstream(b));
        match push_block(branch.as_deref(), has_upstream, !self.remotes.is_empty()) {
            // Behind its upstream (amended / rebased after the push, or someone pushed):
            // git would refuse — the ways out right away, read locally (no network).
            None if branch.as_deref().is_some_and(|b| self.behind_upstream(b)) => {
                self.git_op("Push", window, cx, |r| r.push_diverged())
            }
            None => self.git_push_op("Push", false, window, cx, |r| r.push_current()),
            Some(block) => self.push_blocked(block, window, cx),
        }
    }

    /// The main Push can't run: says why; without an upstream, "Push to…" opens.
    pub(super) fn push_blocked(
        &mut self,
        block: PushBlock,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A user operation all the same: earlier warnings go.
        self.user_op_started();
        match block {
            PushBlock::Detached | PushBlock::NoRemote => self.set_error(block.to_string()),
            PushBlock::NoUpstream { .. } => {
                self.set_info(block.to_string());
                self.open_push_step(window, cx);
            }
        }
        cx.notify();
    }

    /// Whether local `branch` is behind its upstream, from the last refresh (no git).
    fn behind_upstream(&self, branch: &str) -> bool {
        self.tracking
            .iter()
            .any(|t| t.branch == branch && t.upstream.is_some() && t.behind > 0)
    }

    /// Whether local `branch` has an upstream, from the last refresh (no git).
    fn has_upstream(&self, branch: &str) -> bool {
        self.tracking
            .iter()
            .any(|t| t.branch == branch && t.upstream.is_some())
    }

    /// The plain Pull (toolbar, menu bar, `p`, the current branch's menu): without an
    /// upstream (or with a detached HEAD) no git runs at all — before, `git pull` fetched
    /// first (asking for a passphrase) and only then said there is no tracking
    /// information. "Pull from…" opens instead. Decided from the last refresh; `Repo::pull`
    /// checks once more (locally) if that is stale.
    pub(super) fn pull_main(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let branch = self.head.branch.clone();
        let has_upstream = branch.as_deref().is_some_and(|b| self.has_upstream(b));
        match pull_block(branch.as_deref(), has_upstream) {
            None => self.git_op("Pull", window, cx, |r| r.pull_saved()),
            Some(block) => self.pull_blocked(block, window, cx),
        }
    }

    /// The plain Pull can't run: says why; without an upstream, "Pull from…" (a remote,
    /// or Set Upstream…) opens.
    pub(super) fn pull_blocked(
        &mut self,
        block: PullBlock,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A user operation all the same: earlier warnings go.
        self.user_op_started();
        match block {
            PullBlock::Detached => self.set_error(block.to_string()),
            PullBlock::NoUpstream { .. } => {
                self.set_info(block.to_string());
                self.open_pull_step(window, cx);
            }
        }
        cx.notify();
    }

    /// The branches diverged and no pull strategy is configured or remembered: Merge /
    /// Rebase / Fast-forward only, "Remember for this repository" (gitti's settings, not
    /// the git config), then the pull again with that option.
    pub(super) fn pull_diverged(
        &mut self,
        d: DivergentPull,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.user_op_started();
        self.set_info(d.to_string());
        let labels: Vec<&str> = PullStrategy::ALL.iter().map(|s| s.label()).collect();
        let spec = ModalSpec::new("Pull — branches have diverged")
            .message(d.describe())
            .options(&labels, 0)
            .check("Remember for this repository", false)
            .ok("Pull");
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let strategy = PullStrategy::ALL[res.option.min(PullStrategy::ALL.len() - 1)];
            let remember = res.checks.first().copied().unwrap_or(false);
            let d = d.clone();
            this.git_op("Pull", w, cx, move |r| {
                if remember {
                    gitti_core::pull::save_strategy(r.workdir(), strategy);
                }
                d.retry(r, strategy)
            })
        });
        cx.notify();
    }

    /// "Pull with…" (menu bar, palette): the plain Pull with a strategy chosen for it —
    /// once, or remembered for this repository — or the remembered one forgotten (the
    /// next divergent pull asks again). Without an upstream as the plain Pull.
    pub(super) fn open_pull_with_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let branch = self.head.branch.clone();
        let has_upstream = branch.as_deref().is_some_and(|b| self.has_upstream(b));
        if let Some(block) = pull_block(branch.as_deref(), has_upstream) {
            self.pull_blocked(block, window, cx);
            return;
        }
        let Some(workdir) = self.repo.as_ref().map(|r| r.workdir().to_path_buf()) else {
            return;
        };
        let saved = gitti_core::pull::saved_strategy(&workdir);
        let mut items = Vec::new();
        for remember in [false, true] {
            for strategy in PullStrategy::ALL {
                let (label, detail) = if remember {
                    (
                        format!("{}, remember for this repository", strategy.label()),
                        format!("{} now and whenever git asks", strategy.flag()),
                    )
                } else {
                    (strategy.label().to_string(), format!("{} · this pull", strategy.flag()))
                };
                items.push(super::picker::item(label, detail, move |t, w, cx| {
                    t.git_op("Pull", w, cx, move |r| {
                        if remember {
                            gitti_core::pull::save_strategy(r.workdir(), strategy);
                        }
                        r.pull_chosen(strategy)
                    })
                }));
            }
        }
        if let Some(s) = saved {
            items.push(super::picker::item(
                format!("Forget the remembered strategy ({})", s.label()),
                "the next divergent pull asks again",
                move |t, _, cx| {
                    gitti_core::pull::forget_strategy(&workdir);
                    t.set_info("Pull strategy forgotten — the next divergent pull asks again");
                    cx.notify();
                },
            ));
        }
        let title = gitti_core::pull::pull_with_title(saved);
        self.open_picker(&title, items, window, cx);
    }

    fn remote_names(&self) -> Vec<String> {
        self.remotes.iter().map(|r| r.name.clone()).collect()
    }

    /// The upstream of local `branch`, from the last refresh (no git).
    fn upstream_of(&self, branch: &str) -> Option<Upstream> {
        let short = self
            .tracking
            .iter()
            .find(|t| t.branch == branch)?
            .upstream
            .clone()?;
        Upstream::parse(&short, &self.remote_names())
    }

    /// The quick pushes of the current branch (palette step, Push ▾).
    pub(super) fn quick_pushes(&self) -> Vec<QuickPush> {
        let Some(branch) = self.head.branch.clone() else {
            return Vec::new();
        };
        quick_pushes(&branch, self.upstream_of(&branch).as_ref(), &self.remote_names())
    }

    fn run_push(&mut self, opts: PushOptions, window: &mut Window, cx: &mut Context<Self>) {
        let no_verify = opts.no_verify;
        self.git_push_op("Push", no_verify, window, cx, move |r| r.push_with(&opts));
    }

    /// A remote's menu ▸ "Push <branch> to <remote>": the current branch to `remote`
    /// (`-u` when it has no upstream yet, as the palette's quick push does).
    pub(super) fn push_current_to(
        &mut self,
        remote: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(branch) = self.head.branch.clone() else {
            self.set_info("HEAD is detached — no branch to push");
            cx.notify();
            return;
        };
        let opts = PushOptions::to_remote(&branch, &remote, self.upstream_of(&branch).as_ref());
        self.run_push(opts, window, cx);
    }

    /// "Push…" in the palette (and the main Push without an upstream): the remotes to push
    /// the current branch to, Set Upstream… when it has none, then Options….
    pub(super) fn open_push_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut items: Vec<PickerItem> = self
            .quick_pushes()
            .into_iter()
            .map(|q| {
                let opts = q.opts;
                super::picker::item(q.label, q.detail, move |t, w, cx| {
                    t.run_push(opts.clone(), w, cx)
                })
            })
            .collect();
        // Without an upstream: a remote above (it becomes the upstream), or an existing
        // remote branch as the upstream.
        let no_upstream = self
            .head
            .branch
            .clone()
            .filter(|b| !self.has_upstream(b));
        if let Some(b) = &no_upstream {
            let b2 = b.clone();
            items.push(super::picker::item(
                "Set Upstream…",
                format!("a remote branch for {b}"),
                move |t, w, cx| t.open_upstream_picker(b2.clone(), w, cx),
            ));
        }
        items.push(super::picker::item(
            "Options…",
            "remotes, branch name, upstream, force, tags, hooks",
            |t, w, cx| t.push_dialog(None, w, cx),
        ));
        let placeholder = match (&self.head.branch, &no_upstream) {
            (Some(b), Some(_)) => format!("Push {b} to… (no upstream yet)"),
            (Some(b), None) => format!("Push {b} to…"),
            (None, _) => "Push (HEAD is detached) — Options…".to_string(),
        };
        self.open_picker(&placeholder, items, window, cx);
    }

    /// "Pull from…" in the palette: pull the current branch from one remote. Without an
    /// upstream: the remotes, then Set Upstream… instead of the plain Pull.
    pub(super) fn open_pull_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let branch = self.head.branch.clone().unwrap_or_default();
        let upstream = self.upstream_of(&branch);
        let mut items = Vec::new();
        if let Some(u) = &upstream {
            items.push(super::picker::item(
                "Pull",
                format!("from the upstream {}", u.short()),
                |t, w, cx| t.pull_main(w, cx),
            ));
        }
        let mut remotes = self.remote_names();
        if let Some(u) = &upstream {
            remotes.sort_by_key(|r| *r != u.remote);
        }
        for remote in remotes {
            let rbranch = match &upstream {
                Some(u) if u.remote == remote => u.branch.clone(),
                _ => branch.clone(),
            };
            let rm = remote.clone();
            items.push(super::picker::item(
                format!("Pull from {remote}"),
                format!("{remote}/{rbranch}"),
                move |t, w, cx| {
                    let rm = rm.clone();
                    t.git_op("Pull", w, cx, move |r| r.pull_from_saved(&rm))
                },
            ));
        }
        let title = if upstream.is_none() && !branch.is_empty() {
            let b = branch.clone();
            items.push(super::picker::item(
                "Set Upstream…",
                format!("a remote branch for {branch}"),
                move |t, w, cx| t.open_upstream_picker(b.clone(), w, cx),
            ));
            format!("Pull {branch} from… (no upstream yet)")
        } else {
            "Pull from…".to_string()
        };
        self.open_picker(&title, items, window, cx);
    }

    /// "Fetch from…" in the palette: one remote or all.
    pub(super) fn open_fetch_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut items = vec![super::picker::item(
            "Fetch All Remotes",
            "git fetch --all --prune",
            |t, w, cx| t.git_op("Fetch", w, cx, |r| r.fetch()),
        )];
        for remote in self.remote_names() {
            let rm = remote.clone();
            items.push(super::picker::item(
                format!("Fetch {remote}"),
                "git fetch --prune",
                move |t, w, cx| {
                    let rm = rm.clone();
                    t.git_op("Fetch", w, cx, move |r| r.fetch_remote(&rm))
                },
            ));
        }
        self.open_picker("Fetch from…", items, window, cx);
    }

    /// The Push… dialog for `branch` (default: the current one). The choices are
    /// remembered per repository (not force).
    pub(super) fn push_dialog(
        &mut self,
        branch: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let Some(branch) = branch.or_else(|| self.head.branch.clone()) else {
            self.set_error("HEAD is detached — no branch to push (switch to a branch first)".into());
            cx.notify();
            return;
        };
        let remotes = self.remote_names();
        if remotes.is_empty() {
            self.set_error("No remote to push to — add one first (Add Remote…)".into());
            cx.notify();
            return;
        }
        let remembered: Option<PushChoices> =
            gitti_core::store::load_repo_pref(repo.workdir(), PUSH_CHOICES_KEY);
        let upstream = self.upstream_of(&branch);
        let d = PushOptions::defaults(&branch, upstream.as_ref(), &remotes, None, remembered.as_ref());
        // What the user sees now: a force push pins its lease to it.
        let seen: Vec<_> = self
            .refs
            .iter()
            .filter(|r| r.kind == RefKind::RemoteBranch)
            .cloned()
            .collect();
        let mut spec = ModalSpec::new(format!("Push {branch}"))
            .message(match &upstream {
                Some(u) => format!("Upstream: {}. Several remotes = one push each, in order.", u.short()),
                None => "No upstream yet. Several remotes = one push each, in order.".to_string(),
            })
            .field("Local branch", branch.clone())
            .field("Remote branch (empty = same name)", d.remote_branch.clone());
        for r in &remotes {
            let url = self
                .remotes
                .iter()
                .find(|x| &x.name == r)
                .map(|x| x.push_url.clone())
                .unwrap_or_default();
            spec = spec.check(format!("Push to {r}  {url}"), d.remotes.contains(r));
        }
        let spec = spec
            .check("Set upstream (-u, on the first remote)", d.set_upstream)
            .check("Force with lease (--force-with-lease)", false)
            .check("Push tags of these commits (--follow-tags)", d.tags == PushTags::Follow)
            .check("Push all tags (--tags)", d.tags == PushTags::All)
            .check(self.no_verify_label(), d.no_verify)
            .ok("Push");
        let n = remotes.len();
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let chosen: Vec<String> = remotes
                .iter()
                .zip(&res.checks)
                .filter(|(_, on)| **on)
                .map(|(r, _)| r.clone())
                .collect();
            let c = &res.checks[n..];
            let mut opts = PushOptions {
                remotes: chosen,
                branch: res.values[0].trim().to_string(),
                remote_branch: res.values[1].trim().to_string(),
                set_upstream: c[0],
                force_with_lease: c[1],
                tags: match (c[2], c[3]) {
                    (_, true) => PushTags::All,
                    (true, false) => PushTags::Follow,
                    _ => PushTags::None,
                },
                no_verify: c[4],
                leases: Default::default(),
            };
            opts.pin_leases(&seen);
            if opts.remotes.is_empty() {
                this.set_error("Push: no remote chosen".into());
                cx.notify();
                return;
            }
            if let Some(repo) = this.repo.clone() {
                gitti_core::store::save_repo_pref(repo.workdir(), PUSH_CHOICES_KEY, &opts.choices());
            }
            if opts.force_with_lease {
                this.plan_force_push(opts, w, cx);
            } else {
                this.run_push(opts, w, cx);
            }
        });
    }

    /// Push…'s "Force with lease": reads (in the background, local git only) what each
    /// remote's branch loses, then confirms like Push with Lease… — the leases pinned to
    /// what the list shows — and pushes.
    fn plan_force_push(&mut self, opts: PushOptions, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let task = cx.background_executor().spawn(async move { repo.force_push_plan(opts) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(fp) => {
                    let spec = ModalSpec::new(fp.title())
                        .message(fp.describe())
                        .ok(fp.ok_label())
                        .danger();
                    let opts = fp.opts;
                    this.open_modal(spec, window, cx, move |this, _, w, cx| this.run_push(opts, w, cx));
                    cx.notify();
                }
                Err(e) => {
                    this.set_error(format!("{e:#}"));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// git refused a push (or the main Push knew it would): says why in plain words and
    /// offers the ways out — Pull (merge / rebase), Push with Lease… (what the remote
    /// loses, then the push pinned to it), Fetch, Then Decide…. A refusal by the server
    /// itself (protected branch, hook) is an error naming it: its settings decide.
    pub(super) fn push_rejected(
        &mut self,
        rej: PushRejected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.user_op_started();
        let ways = rej.ways_out();
        if ways.is_empty() {
            let details = if rej.output.trim().is_empty() {
                rej.describe()
            } else {
                format!("{rej}\n\n{}", rej.output.trim_end())
            };
            self.op_failed("Push", details);
            self.set_error(rej.to_string());
            cx.notify();
            return;
        }
        self.set_info(rej.to_string());
        let labels: Vec<String> = ways.iter().map(|w| rej.way_label(*w)).collect();
        let options: Vec<&str> = labels.iter().map(String::as_str).collect();
        let live_labels = labels.clone();
        let spec = ModalSpec::new(rej.title())
            .message(rej.describe())
            .options(&options, 0)
            .ok(labels[0].clone())
            .live(move |res| ModalLive {
                ok_label: live_labels.get(res.option).cloned(),
                ..Default::default()
            });
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let way = ways[res.option.min(ways.len() - 1)];
            this.push_way_out(rej.clone(), way, w, cx)
        });
        cx.notify();
    }

    fn push_way_out(
        &mut self,
        rej: PushRejected,
        way: WayOut,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match way {
            WayOut::Pull(strategy) => {
                self.git_op("Pull", window, cx, move |r| rej.pull(r, strategy))
            }
            WayOut::PushWithLease => match rej.plan {
                Some(plan) => self.overwrite_dialog(plan, window, cx),
                None => self.push_way_out(rej, WayOut::FetchThenDecide, window, cx),
            },
            WayOut::FetchThenDecide => self.run_op(
                "Fetch",
                true,
                window,
                cx,
                move |r| rej.fetch_then_plan(r),
                |this, plan, w, cx| this.overwrite_dialog(plan, w, cx),
            ),
        }
    }

    /// The confirmation of an overwrite of a remote branch: what moves, what it loses
    /// (with authors), that the lease is pinned to what is listed; then the push.
    pub(super) fn overwrite_dialog(
        &mut self,
        ov: Overwrite,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let spec = ModalSpec::new(ov.title())
            .message(ov.describe())
            .ok(ov.ok_label())
            .danger();
        self.open_modal(spec, window, cx, move |this, _, w, cx| {
            let ov = ov.clone();
            this.git_push_op("Push with lease", ov.no_verify, w, cx, move |r| r.overwrite(&ov))
        });
        cx.notify();
    }

    /// Reads (in the background, local git only) what overwriting `<remote>/<branch>`
    /// with `source` does — the lease pinned to the remote-tracking commit shown now —
    /// then [`Self::overwrite_dialog`].
    pub(super) fn plan_overwrite(
        &mut self,
        remote: String,
        branch: String,
        source: Source,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let lease = gitti_core::push::Lease::seen(&self.refs, &remote, &branch);
        let short = format!("{remote}/{branch}");
        let local = self
            .tracking
            .iter()
            .find(|t| t.upstream.as_deref() == Some(short.as_str()))
            .map(|t| t.branch.clone());
        // A force push of a branch without upstream sets it (as Push does).
        let set_upstream = matches!(&source, Source::Branch(b) if !self.has_upstream(b));
        let task = cx.background_executor().spawn(async move {
            let mut plan = repo.overwrite_plan(&remote, &branch, source, lease, local.as_deref())?;
            plan.set_upstream = set_upstream;
            anyhow::Ok(plan)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(plan) => this.overwrite_dialog(plan, window, cx),
                Err(e) => {
                    this.set_error(format!("{e:#}"));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// "Reset <remote>/<branch> to Here…": `remote_branch` (`origin/main`, as shown) onto
    /// commit `rev` (hex) — the local branch stays as it is.
    pub(super) fn reset_remote_branch(
        &mut self,
        remote_branch: String,
        rev: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(u) = Upstream::parse(&remote_branch, &self.remote_names()) else {
            self.set_error(format!("{remote_branch}: no such remote"));
            cx.notify();
            return;
        };
        self.plan_overwrite(u.remote, u.branch, Source::Commit(rev), window, cx);
    }

    /// The upstream of the current branch (`origin/main`), from the last refresh.
    pub(super) fn head_upstream(&self) -> Option<String> {
        let b = self.head.branch.as_deref()?;
        self.tracking
            .iter()
            .find(|t| t.branch == b)?
            .upstream
            .clone()
    }

    /// The remote branches to reset to commit `rev`: the current branch's upstream first.
    pub(super) fn reset_remote_picker(
        &mut self,
        rev: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let upstream = self.head_upstream();
        let mut names: Vec<String> = self
            .refs
            .iter()
            .filter(|r| r.kind == RefKind::RemoteBranch)
            .map(|r| r.short_name.clone())
            .collect();
        if names.is_empty() {
            self.set_info("No remote branches — fetch or push one first");
            cx.notify();
            return;
        }
        names.sort_by_key(|n| Some(n) != upstream.as_ref());
        let short = rev[..rev.len().min(7)].to_string();
        let items = names
            .into_iter()
            .map(|n| {
                let detail = if Some(&n) == upstream.as_ref() {
                    "upstream of the current branch".to_string()
                } else {
                    String::new()
                };
                let (n2, rev) = (n.clone(), rev.clone());
                super::picker::item(format!("Reset {n} to {short}…"), detail, move |t, w, cx| {
                    t.reset_remote_branch(n2.clone(), rev.clone(), w, cx)
                })
            })
            .collect();
        self.open_picker(&format!("Reset which remote branch to {short}?"), items, window, cx);
    }

    /// The Push ▾ menu of the toolbar: the quick pushes, then Options….
    pub(super) fn push_menu(menu: PopupMenu, app: &WeakEntity<GittiApp>, cx: &App) -> PopupMenu {
        let Some(this) = app.upgrade() else {
            return menu;
        };
        let pushes = this.read(cx).quick_pushes();
        let mut menu = menu;
        for q in pushes {
            let (app, opts) = (app.clone(), q.opts);
            menu = menu.item(
                PopupMenuItem::new(format!("{}  {}", q.label, q.detail)).on_click(move |_, w, cx| {
                    let opts = opts.clone();
                    app.update(cx, |t, cx| t.run_push(opts, w, cx)).ok();
                }),
            );
        }
        let app = app.clone();
        menu.separator().item(PopupMenuItem::new("Push Options…").on_click(move |_, w, cx| {
            app.update(cx, |t, cx| t.push_dialog(None, w, cx)).ok();
        }))
    }
}
