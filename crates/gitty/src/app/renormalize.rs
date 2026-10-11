//! "Renormalize Line Endings…" (palette, and the menu of a `.gitattributes` file), as
//! gitti's: the dialog says what it does and what follows; the work and the texts are
//! `gitti_core::renormalize`'s.

use gitti_core::renormalize::{self, Plan};

use super::{After, App, Modal};
use crate::modal::Live;

impl App {
    /// The dialog; Renormalize runs `git add --renormalize` (without the files with
    /// unstaged changes unless the box is ticked).
    pub fn renormalize_dialog(&mut self) {
        let plan = Plan::of(&self.working);
        // The palette greys it then (`availability`); the file menu's entry says why.
        if plan.conflicted > 0 {
            self.set_info(renormalize::CONFLICTS);
            return;
        }
        let run = |app: &mut App, leave_out: Vec<String>| {
            app.run_op("Renormalize line endings", false, After::Nothing, move |r| {
                Ok(r.renormalize(&leave_out)?.note())
            });
        };
        let Some(label) = plan.include_label() else {
            let leave_out = plan.leave_out(false);
            self.modal = Some(Modal::confirm(
                renormalize::TITLE,
                plan.detail(),
                renormalize::BUTTON,
                move |app: &mut App| run(app, leave_out),
            ));
            return;
        };
        let detail = plan.detail();
        let live = |_: &[String], _: &[bool]| Live {
            ok_label: renormalize::BUTTON.to_string(),
            ..Default::default()
        };
        self.modal = Some(
            Modal::input_checks(renormalize::TITLE, Vec::new(), vec![(label.as_str(), false)], move |app, _, checks| {
                let include = checks.first().copied().unwrap_or(false);
                run(app, plan.leave_out(include))
            })
            .with_note(detail)
            .with_live(live),
        );
    }
}
