//! The mark right of a repository tab's name (`gitti_core::tab_marks`): a filled circle
//! while the working tree has uncommitted changes, else a hollow one while the branch has
//! commits that are not pushed; the tooltip names both when both hold. Read from what the
//! tab's refreshes keep current anyway — its file watcher runs also while the tab is in
//! the background — so drawing them costs no git process.

use std::sync::Arc;

use gitti_core::WorkingState;
use gitti_core::tab_marks::{Mark, Unpushed, describe_uncommitted, mark};
use gpui_kit::component::h_flex;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::GittiApp;
use crate::probe::ProbeExt as _;
use crate::theme::Palette;

/// What one tab's marks show.
#[derive(Clone)]
pub struct TabMarks {
    working: Arc<WorkingState>,
    unpushed: Option<Unpushed>,
}

impl GittiApp {
    /// This repository's marks for its tab (nothing before its first read).
    pub fn tab_marks(&self) -> TabMarks {
        TabMarks {
            working: self.working.clone(),
            unpushed: self.unpushed.clone(),
        }
    }
}

impl TabMarks {
    /// The one mark shown: uncommitted outranks unpushed.
    pub fn mark(&self) -> Option<Mark> {
        mark(&self.working.status, self.unpushed.as_ref())
    }

    pub fn any(&self) -> bool {
        self.mark().is_some()
    }

    /// The mark as text ("●", "○"; empty without one): the tab list menu, tests.
    pub fn glyphs(&self) -> &'static str {
        self.mark().map_or("", Mark::glyph)
    }

    /// What they mean, one line each (the tooltip).
    pub fn describe(&self) -> String {
        let mut lines = Vec::new();
        lines.extend(describe_uncommitted(&self.working.status));
        lines.extend(self.unpushed.as_ref().map(Unpushed::describe));
        lines.join("\n")
    }

    /// The circle of tab `ix` (probes `tab-marks:N` and `tab-dirty:N` or `tab-unpushed:N`),
    /// sized to the UI font; `None` without one. It never shrinks: the name left of it
    /// truncates first.
    pub fn element(&self, ix: usize, font_size: f32, p: &Palette) -> Option<AnyElement> {
        let mark = self.mark()?;
        // About the height of a lowercase letter; a thicker ring from 10 px on.
        let d = (font_size * 0.55).round().max(6.);
        let ring = |div: Div| if d >= 10. { div.border_2() } else { div.border_1() };
        let tip = self.describe();
        Some(
            h_flex()
                .id(("tab-marks", ix))
                .probe(format!("tab-marks:{ix}"))
                .flex_none()
                .items_center()
                .when(mark == Mark::Uncommitted, |el| {
                    el.child(
                        div()
                            .probe(format!("tab-dirty:{ix}"))
                            .flex_none()
                            .size(px(d))
                            .rounded_full()
                            .bg(p.warning),
                    )
                })
                .when(mark == Mark::Unpushed, |el| {
                    el.child(
                        ring(div())
                            .probe(format!("tab-unpushed:{ix}"))
                            .flex_none()
                            .size(px(d))
                            .rounded_full()
                            .border_color(p.accent),
                    )
                })
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .into_any_element(),
        )
    }
}
