//! How the sidebar | commit list | detail row answers a drag of its two edges, as in gitty
//! (where the list's width is a share of the whole row): the sidebar's right edge moves the
//! list — it keeps its width, the detail gives way —, the list's right edge resizes it.
//!
//! gpui-base's resize group hands what a panel gains or gives up to its neighbour, so a drag
//! of the sidebar's edge would resize the list. The row keeps gpui-base's handles (their
//! band, cursor and drag; `scroll_guard` mirrors their geometry) and corrects the sizes per
//! frame instead: a press on the sidebar's edge notes the list's width
//! ([`PaneStates::note_press`]); while that holds, a frame whose list came out at another
//! width lays the panels out with the noted one ([`PaneStates::keep_list_width`]) before
//! the layout, so no frame shows the list at another width. The release stores the same
//! sizes ([`PaneStates::stored_main_sizes`]).

use gpui_kit::*;

use super::PaneStates;

/// gpui-base's `PANEL_MIN_SIZE`: the narrowest a commit list or detail gets.
const PANE_MIN: f32 = 100.;

/// How far from the sidebar's edge a press counts as one on its handle: the handle's band
/// reaches `scroll_guard::HANDLE_BAND` (4 px) left of the seam and 5 px right of it.
const EDGE_REACH: f32 = 8.;

impl PaneStates {
    /// A left press at `x` (from the row's left edge) while the sidebar is shown: on the
    /// sidebar's edge it notes the list's width for the drag that may follow; anywhere else
    /// it ends the last one's correction.
    pub(super) fn note_press(&self, x: f32, cx: &App) {
        let sizes = self.main.read(cx).sizes();
        let keep = match sizes[..] {
            [side, list, _] if (x - f32::from(side)).abs() <= EDGE_REACH => Some(f32::from(list)),
            _ => None,
        };
        self.keep_list.set(keep);
    }

    /// While the sidebar's edge is dragged and the drag moved the list's edge as well: this
    /// frame's sidebar and list sizes with the list back at its width from the press. The
    /// resize state is cleared, so the panels lay out with these as their initial sizes
    /// (as in `rescale_panes`); the detail takes the rest.
    pub(super) fn keep_list_width(&self, total: f32, cx: &mut App) -> Option<(f32, f32)> {
        let keep = self.keep_list.get()?;
        let (side, list) = match self.main.read(cx).sizes()[..] {
            [side, list, _] => (f32::from(side), f32::from(list)),
            _ => return None,
        };
        let want = kept_list(side, keep, total);
        if (list - want).abs() < 0.5 {
            return None;
        }
        self.main.update(cx, |s, _| s.clear());
        Some((side, want))
    }

    /// The sidebar and list sizes a finished drag stores: the list at its noted width when
    /// the drag was the sidebar edge's (the release may come before the frame that
    /// corrects the last move).
    pub(super) fn stored_main_sizes(&self, sizes: &[f32], total: f32) -> Option<(f32, f32)> {
        let [side, list, _] = sizes[..] else {
            return None;
        };
        let list = self.keep_list.get().map_or(list, |keep| kept_list(side, keep, total));
        Some((side, list))
    }
}

/// `list` as wide as a row of `total` leaves next to a `side` wide sidebar and the
/// narrowest detail; never below the narrowest list.
fn kept_list(side: f32, list: f32, total: f32) -> f32 {
    list.min(total - side - PANE_MIN).max(PANE_MIN)
}

#[cfg(test)]
mod tests {
    use super::kept_list;

    #[gitti_test::test]
    fn the_list_keeps_its_width_while_the_detail_has_room() {
        assert_eq!(kept_list(230., 640., 1500.), 640.);
        assert_eq!(kept_list(900., 640., 1500.), 500.);
        assert_eq!(kept_list(1400., 640., 1500.), 100.);
    }
}
