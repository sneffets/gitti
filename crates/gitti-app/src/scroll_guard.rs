//! What a mouse press on a list's scrollbar strip does. Without this guard, on Linux /
//! Windows a click at the right edge of the commit list — meant to drag the panel narrower,
//! the cursor shows the resize arrows — would jump the history and select a commit.
//!
//! Why, in gpui-component 0.7 (`gpui-base` `scrollbar.rs` / `resizable/resize_handle.rs`):
//! - The resize handle between two panels straddles their seam: a 9 px band, 4 px of it
//!   (`HANDLE_PADDING`) over the left panel — exactly the right end of that panel's
//!   scrollbar track (16 px wide). The handle's hitbox is on top (it occludes, hence the
//!   cursor), but the scrollbar tests its press against its *bounds*, not its hitbox, so it
//!   takes the press under the handle too and jumps the list to the click (track click).
//! - The scrollbar listens for presses only while it is shown. Not shown yet (hover-to-show
//!   on Linux / Windows before its first frame, the overlay scrollbar on macOS once it
//!   faded) a press on the strip falls through to the row underneath and selects it.
//!
//! No patch of the framework: two invisible elements around the list's `Scrollbar` decide
//! per press, using the same geometry. gpui dispatches a press (bubble phase) from the
//! element painted last to the first, so:
//! - `over` (painted after the scrollbar, so before it — and after the resize handle of
//!   the next panel, which is painted later still): a press in the handle's band is the
//!   handle's alone — it has seen it already and starts its drag on the move —, and a press
//!   on the shown track beside the thumb does nothing (only the thumb drags). Both stop
//!   there.
//! - `under` (painted after the list, before the scrollbar, so after it): a press on the
//!   shown strip the scrollbar didn't take (thumb while its press listener isn't there
//!   yet) stops there instead of selecting a row.
//!
//! A press on the thumb goes to the scrollbar, which drags. Where the strip is not shown
//! (overlay scrollbars that faded out), presses belong to the rows as before.
//!
//! The geometry below mirrors the framework's (`ThumbGeometry`, constants `WIDTH`,
//! `MIN_THUMB_SIZE`, `THUMB_INSET`, `HANDLE_PADDING`); `tests/edges.rs` drives real
//! presses and drags, so a framework upgrade that moves any of it fails there.

use std::time::{Duration, Instant};

use gpui_kit::component::scroll::{ScrollbarHandle, ScrollbarMode};
use gpui_kit::*;

/// gpui-base `scrollbar.rs` `WIDTH`: the track's width.
pub const TRACK: f32 = 16.;
/// gpui-base `MIN_THUMB_SIZE`.
const MIN_THUMB: f32 = 48.;
/// gpui-base `THUMB_INSET` (also the gap right of the thumb's pressable area).
const INSET: f32 = 4.;
/// gpui-base `resizable` `HANDLE_PADDING`: how far a handle's band reaches into the panel
/// left of (above) its seam.
pub const HANDLE_BAND: f32 = 4.;
/// The overlay scrollbar (macOS "show when scrolling") stays this long after the last
/// scroll / hover over it (gpui-component `SCROLLBAR_IDLE`), then fades.
const OVERLAY_SHOWN: Duration = Duration::from_millis(2000);

/// What happens to a press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    /// Not ours: the next element (scrollbar, rows) gets it.
    Pass,
    /// Stops here.
    Stop,
}

/// Where a press landed, relative to the list's strip (pure, for unit tests).
#[derive(Debug, Clone, Copy)]
pub struct Strip {
    pub bounds: Bounds<Pixels>,
    /// Scroll offset (negative or zero, px) and the content's height.
    pub offset: f32,
    pub content: f32,
    /// The scrollbar is shown (it listens for presses).
    pub shown: bool,
    /// A resize handle's band covers the right edge.
    pub band_right: bool,
}

impl Strip {
    fn overflows(&self) -> bool {
        self.content > f32::from(self.bounds.size.height)
    }

    fn in_strip(&self, p: Point<Pixels>) -> bool {
        self.overflows()
            && self.bounds.contains(&p)
            && f32::from(self.bounds.right() - p.x) <= TRACK
    }

    fn in_band(&self, p: Point<Pixels>) -> bool {
        self.band_right
            && self.bounds.contains(&p)
            && f32::from(self.bounds.right() - p.x) <= HANDLE_BAND
    }

    /// The thumb's pressable area (y range), as the framework computes it, a pixel smaller
    /// on each side: a press we let through must land on the thumb, not jump beside it.
    pub fn thumb(&self) -> Option<(f32, f32)> {
        if !self.overflows() {
            return None;
        }
        let container = f32::from(self.bounds.size.height);
        let logical = (container / self.content * container)
            .max(MIN_THUMB)
            .min(container);
        let travel = container - logical;
        let extent = self.content - container;
        let start =
            f32::from(self.bounds.top()) + INSET + (-self.offset / extent).clamp(0., 1.) * travel;
        let len = logical - INSET * 2.;
        Some((start + 1., start + len - 1.))
    }

    fn on_thumb(&self, p: Point<Pixels>) -> bool {
        let from_right = f32::from(self.bounds.right() - p.x);
        let y = f32::from(p.y);
        from_right > INSET + 1.
            && from_right <= TRACK
            && self.thumb().is_some_and(|(a, b)| y >= a && y <= b)
    }

    /// The `over` element's decision (before the scrollbar).
    pub fn over(&self, p: Point<Pixels>) -> Press {
        if self.in_band(p) || (self.shown && self.in_strip(p) && !self.on_thumb(p)) {
            Press::Stop
        } else {
            Press::Pass
        }
    }

    /// The `under` element's decision (after the scrollbar, before the rows).
    pub fn under(&self, p: Point<Pixels>) -> Press {
        if self.shown && self.in_strip(p) {
            Press::Stop
        } else {
            Press::Pass
        }
    }
}

/// When the overlay scrollbar was last revealed (scroll, hover while shown).
#[derive(Default)]
struct Reveal {
    offset: Option<Pixels>,
    at: Option<Instant>,
}

/// The list, its scrollbar and the two guards, for a container's `.children(…)` (the
/// container is `relative`; list and scrollbar fill it). `band_right`: the panel's right
/// edge is a resize handle's seam (the next panel's handle reaches over it).
pub fn list_with_scrollbar<H: ScrollbarHandle + Clone + 'static>(
    id: &'static str,
    list: impl IntoElement,
    handle: &H,
    band_right: bool,
) -> [AnyElement; 4] {
    let [under, bar, over] = scrollbar(id, handle, band_right);
    [list.into_any_element(), under, bar, over]
}

/// [`list_with_scrollbar`] without the list: for after the list (and what is painted over
/// it) in the container's children.
pub fn scrollbar<H: ScrollbarHandle + Clone + 'static>(
    id: &'static str,
    handle: &H,
    band_right: bool,
) -> [AnyElement; 3] {
    [
        guard(id, handle.clone(), band_right, false),
        gpui_kit::component::scroll::Scrollbar::vertical(handle).into_any_element(),
        guard(id, handle.clone(), band_right, true),
    ]
}

fn guard<H: ScrollbarHandle + Clone + 'static>(
    id: &'static str,
    handle: H,
    band_right: bool,
    over: bool,
) -> AnyElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let mode = cx
                .try_global::<gpui_kit::base::Theme>()
                .map_or(ScrollbarMode::Scrolling, |t| t.scrollbar.mode());
            let reveal = window.use_keyed_state(
                ElementId::Name(format!("scroll-guard-{id}").into()),
                cx,
                |_, _| Reveal::default(),
            );
            let offset = handle.offset().y;
            let now = Instant::now();
            if over {
                reveal.update(cx, |r, _| {
                    if r.offset.is_some_and(|o| o != offset) {
                        r.at = Some(now);
                    }
                    r.offset = Some(offset);
                });
            }
            let shown = {
                let reveal = reveal.clone();
                move |cx: &App| match mode {
                    ScrollbarMode::Hover | ScrollbarMode::Always => true,
                    ScrollbarMode::Scrolling => reveal
                        .read(cx)
                        .at
                        .is_some_and(|t| t.elapsed() < OVERLAY_SHOWN),
                }
            };
            let strip = move |cx: &App| Strip {
                bounds,
                offset: f32::from(handle.offset().y),
                content: f32::from(handle.content_size().height),
                shown: shown(cx),
                band_right,
            };
            if over {
                // Hovering the shown overlay scrollbar keeps it shown (as the framework).
                let reveal = reveal.clone();
                let strip = strip.clone();
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                    let s = strip(cx);
                    if phase == DispatchPhase::Capture && s.shown && s.in_strip(e.position) {
                        reveal.update(cx, |r, _| r.at = Some(Instant::now()));
                    }
                });
            }
            window.on_mouse_event(move |e: &MouseDownEvent, phase, _, cx| {
                if phase != DispatchPhase::Bubble || !bounds.contains(&e.position) {
                    return;
                }
                let s = strip(cx);
                let press = if over {
                    s.over(e.position)
                } else {
                    s.under(e.position)
                };
                if press == Press::Stop {
                    cx.stop_propagation();
                }
            });
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{Press, Strip};
    use gpui_kit::{Bounds, point, px, size};

    fn strip(offset: f32, shown: bool) -> Strip {
        Strip {
            bounds: Bounds::new(point(px(100.), px(50.)), size(px(400.), px(500.))),
            offset,
            content: 10_000.,
            shown,
            band_right: true,
        }
    }

    #[gitti_test::test]
    fn presses_on_the_strip() {
        let s = strip(0., true);
        let (a, b) = s.thumb().unwrap();
        // 500 px of 10 000: the thumb is the minimum 48 px (40 pressable), at the top.
        assert_eq!((a, b), (55., 93.));
        let x_thumb = px(500. - 10.);
        let mid = px((a + b) / 2.);
        // Thumb: the scrollbar drags it.
        assert_eq!(s.over(point(x_thumb, mid)), Press::Pass);
        // Track beside the thumb: nothing (no jump), and no row.
        assert_eq!(s.over(point(x_thumb, px(300.))), Press::Stop);
        assert_eq!(s.under(point(x_thumb, px(300.))), Press::Stop);
        // The handle's band: its own, also at the thumb's height.
        assert_eq!(s.over(point(px(499.), mid)), Press::Stop);
        // Rows left of the strip.
        assert_eq!(s.over(point(px(300.), px(300.))), Press::Pass);
        assert_eq!(s.under(point(px(300.), px(300.))), Press::Pass);
        // Overlay scrollbar faded out: the strip is the rows' again; the band stays the
        // handle's.
        let hidden = strip(0., false);
        assert_eq!(hidden.over(point(x_thumb, px(300.))), Press::Pass);
        assert_eq!(hidden.under(point(x_thumb, px(300.))), Press::Pass);
        assert_eq!(hidden.over(point(px(499.), px(300.))), Press::Stop);
        // Scrolled to the end: the thumb sits at the bottom.
        let end = strip(-9500., true);
        let (a, b) = end.thumb().unwrap();
        assert_eq!((a, b), (550. - 48. + 5., 550. - 5.));
    }

    #[gitti_test::test]
    fn short_content_has_no_strip() {
        let mut s = strip(0., true);
        s.content = 400.;
        assert_eq!(s.over(point(px(490.), px(300.))), Press::Pass);
        assert_eq!(s.under(point(px(490.), px(300.))), Press::Pass);
        assert_eq!(s.over(point(px(499.), px(300.))), Press::Stop);
    }
}
