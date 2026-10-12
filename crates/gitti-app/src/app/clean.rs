//! "Clean Untracked / Ignored Files…" (Repository menu, palette): the list is read in the
//! background (`Repo::clean_list`, two git processes), then one dialog — Untracked /
//! Ignored / both as options, a check box per entry shown for its option (untracked ticked,
//! ignored and nested repositories not), the live text says what OK deletes. The texts and
//! the work are `gitti_core::clean`'s, the same in gitty.

use std::rc::Rc;

use gitti_core::clean::{self, Choice, CleanList, Kind};
use gpui_kit::*;

use super::GittiApp;
use super::modal::{ModalLive, ModalSpec};

impl GittiApp {
    /// Reads what Clean can delete, then shows the dialog (or says there is nothing).
    pub(super) fn clean_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let job = cx.background_executor().spawn(async move { repo.clean_list() });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(list) if list.is_empty() => this.set_info(clean::NOTHING),
                    Ok(list) => this.clean_list_dialog(list, window, cx),
                    Err(e) => this.set_error(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn clean_list_dialog(&mut self, list: CleanList, window: &mut Window, cx: &mut Context<Self>) {
        let labels: Vec<String> = Choice::ALL.iter().map(|c| list.choice_label(*c)).collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let mut spec = ModalSpec::new(clean::TITLE)
            .message(list.message())
            .options(&labels, list.default_choice().index())
            .ok("Delete")
            .danger();
        for (i, e) in list.entries.iter().enumerate() {
            let options = match e.kind {
                Kind::Untracked => [Choice::Untracked.index(), Choice::Both.index()],
                Kind::Ignored => [Choice::Ignored.index(), Choice::Both.index()],
            };
            spec = spec.check(e.label(), e.ticked()).check_only_for(i, &options);
        }
        let list = Rc::new(list);
        let live = {
            let list = list.clone();
            move |res: &super::modal::ModalResult| {
                let l = list.live(Choice::from_index(res.option), &res.checks);
                ModalLive {
                    text: l.text,
                    ok_label: Some(l.ok_label),
                    blocked: l.blocked,
                    ..Default::default()
                }
            }
        };
        spec = spec.live(live);
        self.open_modal(spec, window, cx, move |this, res, w, cx| {
            let chosen = list.chosen(Choice::from_index(res.option), &res.checks);
            if chosen.is_empty() {
                return;
            }
            let list = (*list).clone();
            this.work_op_noted("Cleaning", None, w, cx, move |r| {
                Ok(Some(r.clean_chosen(&list, &chosen)?.note()))
            });
        });
    }
}
