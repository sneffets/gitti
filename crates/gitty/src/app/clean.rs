//! "Clean Untracked / Ignored Files…" (palette): the list is read in the background
//! (`Repo::clean_list`, two git processes); a menu chooses Untracked / Ignored / both
//! (gitti: the dialog's options), then a checklist of those entries — untracked ticked,
//! ignored and nested repositories not — whose live line says what Enter deletes. The
//! texts and the work are `gitti_core::clean`'s, the same as gitti's.

use std::rc::Rc;

use gitti_core::clean::{self, Choice, CleanList};

use super::{App, Msg};
use crate::modal::{Live, MenuItem, Modal};

/// Width of the menu's explanation lines.
const NOTE_WIDTH: usize = 64;

impl App {
    /// Reads what Clean can delete, then asks which kind (or says there is nothing).
    pub fn clean_dialog(&mut self) {
        let repo = self.repo.clone();
        self.start_load(
            move || repo.clean_list(),
            move |result| {
                Msg::Apply(Box::new(move |app: &mut App| match result {
                    Ok(list) if list.is_empty() => app.set_info(clean::NOTHING),
                    Ok(list) => app.clean_choice_menu(list),
                    Err(e) => app.set_error(format!("{e:#}")),
                }))
            },
        );
    }

    /// Untracked / Ignored / both, each with its count (greyed when it has none).
    fn clean_choice_menu(&mut self, list: CleanList) {
        let list = Rc::new(list);
        let mut items = MenuItem::notes(
            "What to delete? You tick the entries next. Deleted files don't go to the trash.",
            NOTE_WIDTH,
        );
        items.push(MenuItem::separator());
        for (choice, key) in Choice::ALL.into_iter().zip(['u', 'i', 'b']) {
            let l = list.clone();
            let none = list.shown(choice).is_empty();
            items.push(
                MenuItem::new(format!("{}…", list.choice_label(choice)), move |app: &mut App| {
                    app.clean_checklist(&l, choice)
                })
                .key(key)
                .unless(none.then_some("none of this kind")),
            );
        }
        self.modal = Some(Modal::menu(clean::TITLE, items));
    }

    /// The checklist of the entries `choice` shows; Enter deletes the ticked ones.
    fn clean_checklist(&mut self, list: &Rc<CleanList>, choice: Choice) {
        let rows = Rc::new(list.shown(choice));
        let labels: Vec<(String, bool)> = rows
            .iter()
            .map(|&i| (list.entries[i].label(), list.entries[i].ticked()))
            .collect();
        let checks: Vec<(&str, bool)> = labels.iter().map(|(l, on)| (l.as_str(), *on)).collect();
        // The ticks of the shown rows as one per entry (the rest unticked).
        let ticks = {
            let (rows, n) = (rows.clone(), list.entries.len());
            Rc::new(move |flags: &[bool]| {
                let mut t = vec![false; n];
                for (&i, &on) in rows.iter().zip(flags) {
                    t[i] = on;
                }
                t
            })
        };
        let submit = {
            let (list, ticks) = (list.clone(), ticks.clone());
            move |app: &mut App, _: Vec<String>, flags: Vec<bool>| {
                let chosen = list.chosen(choice, &ticks(&flags));
                if chosen.is_empty() {
                    return;
                }
                let list = (*list).clone();
                app.work_op_noted("Cleaning", None, move |r| {
                    Ok(Some(r.clean_chosen(&list, &chosen)?.note()))
                });
            }
        };
        let live = {
            let list = list.clone();
            move |_: &[String], flags: &[bool]| {
                let l = list.live(choice, &ticks(flags));
                Live {
                    text: l.text,
                    ok_label: l.ok_label,
                    blocked: l.blocked,
                    ..Default::default()
                }
            }
        };
        self.modal = Some(
            Modal::input_checks(clean::TITLE, Vec::new(), checks, submit)
                .with_note(list.message())
                .with_live(live),
        );
    }
}
