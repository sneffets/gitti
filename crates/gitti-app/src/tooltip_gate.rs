//! No tooltips while a context menu or dropdown is open: neither over the menu nor
//! when the mouse moves over other elements meanwhile; after the menu closed they work as
//! before (a new hover waits the usual delay).
//!
//! Gated centrally, not per element: every menu is a gpui-component `PopupMenu` (context
//! menus, split-button dropdowns, submenus, the in-window menu bar) and every tooltip is a
//! gpui-component `Tooltip` view — both gpui's own `.tooltip(…)` on our elements and the
//! "managed" tooltips of buttons, which a per-window `TooltipOverlay` shows. gpui reports
//! each new entity of a type (`observe_new`), so:
//! - a new `PopupMenu` marks a menu as open until it is dismissed (a dismiss closes the whole
//!   menu chain) or released, and hides what tooltip is up;
//! - a `Tooltip` view made while a menu is open is invisible (style `visibility: hidden`),
//!   and stays so — the next hover makes a new one;
//! - a `TooltipOverlay` drops what it was about to show while a menu is open.
//!
//! No tooltips after the pointer left the window either: gpui's `.tooltip(…)` starts its
//! delay on hover and shows the tooltip when it runs out — without looking whether the
//! pointer is still there, and a pointer that leaves the window sends no last `MouseMove`,
//! only a `MouseExitEvent` the tooltip code doesn't listen to (and the window keeps the
//! last position inside, so the shown tooltip also stays). Seen on Linux when the pointer
//! left over the toolbar at the top. So a window the pointer left — or that was deactivated
//! — counts as "pointer outside" until the next `MouseMove` inside it: what is up or
//! pending there is hidden, and tooltip views made meanwhile are invisible, exactly as
//! while a menu is open. [`watcher`] (one per window root, it listens per frame) sees the
//! pointer events; the window's `TooltipOverlay` observes its activation.

use std::collections::HashSet;

use gpui_kit::base::TooltipOverlay;
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::*;

#[derive(Default)]
struct Gate {
    /// The open menus.
    menus: HashSet<EntityId>,
    /// Tooltip views (and their window), to hide the one that is up when a menu opens.
    tooltips: Vec<(EntityId, WeakEntity<Tooltip>, Option<WindowId>)>,
    /// The tooltip views made invisible.
    hidden: HashSet<EntityId>,
    overlays: Vec<(WeakEntity<TooltipOverlay>, Option<WindowId>)>,
    /// Windows the pointer left (or that were deactivated) and hasn't moved in since.
    outside: HashSet<WindowId>,
    /// Tooltip views made visible (for tests).
    shown: usize,
    /// The text of the last tooltip [`text`] built (for tests).
    last_text: Option<String>,
}

impl Gate {
    /// Whether tooltips in `window` stay invisible now.
    fn blocked(&self, window: Option<WindowId>) -> bool {
        !self.menus.is_empty() || window.is_some_and(|w| self.outside.contains(&w))
    }
}

impl Global for Gate {}

/// Installs the gate (once per app, before the first window).
pub fn init(cx: &mut App) {
    cx.set_global(Gate::default());
    cx.observe_new(
        |menu: &mut PopupMenu, window, cx: &mut Context<PopupMenu>| {
            let id = cx.entity_id();
            cx.global_mut::<Gate>().menus.insert(id);
            // A menu kept for the next time (the in-window menu bar) opens again by taking the
            // focus; it closed when the focus left it.
            if let Some(window) = window {
                let handle = menu.focus_handle(cx);
                cx.on_focus_in(&handle, window, move |_, _, cx| {
                    cx.global_mut::<Gate>().menus.insert(id);
                    hide_all(cx);
                })
                .detach();
                cx.on_focus_out(&handle, window, move |_, _, _, cx| {
                    cx.global_mut::<Gate>().menus.remove(&id);
                })
                .detach();
            }
            cx.subscribe_self(|_, _: &DismissEvent, cx| {
                // A dismiss closes the menu with its parents and submenus.
                cx.global_mut::<Gate>().menus.clear();
            })
            .detach();
            cx.on_release(move |_, cx| {
                if cx.has_global::<Gate>() {
                    cx.global_mut::<Gate>().menus.remove(&id);
                }
            })
            .detach();
            hide_all(cx);
        },
    )
    .detach();
    cx.observe_new(|tooltip: &mut Tooltip, window, cx: &mut Context<Tooltip>| {
        let (id, weak) = (cx.entity_id(), cx.entity().downgrade());
        let win = window.map(|w| w.window_handle().window_id());
        let gate = cx.global_mut::<Gate>();
        gate.tooltips.retain(|(_, t, _)| t.upgrade().is_some());
        let alive: HashSet<EntityId> = gate.tooltips.iter().map(|(id, _, _)| *id).collect();
        gate.hidden.retain(|id| alive.contains(id));
        gate.tooltips.push((id, weak, win));
        if gate.blocked(win) {
            gate.hidden.insert(id);
            hide(tooltip);
        } else {
            gate.shown += 1;
        }
    })
    .detach();
    cx.observe_new(
        |_: &mut TooltipOverlay, window, cx: &mut Context<TooltipOverlay>| {
            let weak = cx.entity().downgrade();
            let win = window.as_ref().map(|w| w.window_handle().window_id());
            let gate = cx.global_mut::<Gate>();
            gate.overlays.retain(|(o, _)| o.upgrade().is_some());
            gate.overlays.push((weak, win));
            cx.observe_self(move |overlay, cx| {
                if cx.global::<Gate>().blocked(win) {
                    overlay.hide(cx);
                }
            })
            .detach();
            // A deactivated window (another app or window in front) cancels what is
            // pending or up there; the next move of the pointer inside it allows them again.
            if let (Some(window), Some(win)) = (window, win) {
                cx.observe_window_activation(window, move |overlay, window, cx| {
                    if !window.is_window_active() {
                        overlay.hide(cx);
                        // Deferred: this overlay is being updated now.
                        cx.defer(move |cx| pointer_left(win, cx));
                    }
                })
                .detach();
            }
        },
    )
    .detach();
}

/// An invisible element for a window's root: it listens (per frame, as gpui wants) for the
/// pointer leaving the window and moving in it again.
pub fn watcher() -> impl IntoElement {
    canvas(
        |_, _, _| {},
        |_, _, window, _| {
            window.on_mouse_event(|_: &MouseExitEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture {
                    pointer_left(window.window_handle().window_id(), cx);
                }
            });
            window.on_mouse_event(|e: &MouseMoveEvent, phase, window, cx| {
                // Capture: before any element's hover handling of the same move.
                let size = window.viewport_size();
                let inside = e.position.x >= px(0.)
                    && e.position.y >= px(0.)
                    && e.position.x < size.width
                    && e.position.y < size.height;
                if phase == DispatchPhase::Capture
                    && inside
                    && let Some(gate) = cx.try_global::<Gate>()
                    && gate.outside.contains(&window.window_handle().window_id())
                {
                    let win = window.window_handle().window_id();
                    cx.global_mut::<Gate>().outside.remove(&win);
                }
            });
        },
    )
    .absolute()
    .size_0()
}

/// The pointer left window `win` (or it was deactivated): hide its tooltips, block new ones.
fn pointer_left(win: WindowId, cx: &mut App) {
    if !cx.has_global::<Gate>() {
        return;
    }
    if cx.global_mut::<Gate>().outside.insert(win) {
        hide_matching(cx, |w| w == Some(win));
    }
}

/// Whether a context menu / dropdown is open.
pub fn menu_open(cx: &App) -> bool {
    cx.try_global::<Gate>().is_some_and(|g| !g.menus.is_empty())
}

fn hide(tooltip: &mut Tooltip) {
    let style = tooltip.style();
    style.visibility = Some(Visibility::Hidden);
    style.display = Some(Display::None);
}

/// Hides every tooltip that is up or about to show.
fn hide_all(cx: &mut App) {
    hide_matching(cx, |_| true);
}

/// Hides the tooltips (up or about to show) of the windows `which` accepts.
fn hide_matching(cx: &mut App, which: impl Fn(Option<WindowId>) -> bool) {
    let gate = cx.global_mut::<Gate>();
    let tooltips: Vec<_> = gate
        .tooltips
        .iter()
        .filter(|(_, _, w)| which(*w))
        .filter_map(|(_, t, _)| t.upgrade())
        .collect();
    let overlays: Vec<_> = gate
        .overlays
        .iter()
        .filter(|(_, w)| which(*w))
        .filter_map(|(o, _)| o.upgrade())
        .collect();
    gate.hidden.extend(tooltips.iter().map(Entity::entity_id));
    for t in tooltips {
        t.update(cx, |t, cx| {
            hide(t);
            cx.notify();
        });
    }
    for o in overlays {
        o.update(cx, |o, cx| o.hide(cx));
    }
}

/// Tooltip views made visible so far, and the visible ones alive now (tests).
pub fn shown_tooltips(cx: &App) -> (usize, usize) {
    let Some(gate) = cx.try_global::<Gate>() else {
        return (0, 0);
    };
    let live = gate
        .tooltips
        .iter()
        .filter(|(id, t, _)| t.upgrade().is_some() && !gate.hidden.contains(id))
        .count();
    (gate.shown, live)
}

/// A plain text tooltip for `.tooltip(…)`, remembering its text when it is built (on
/// hover, after the delay) so tests can read what a hover showed ([`last_text`]).
pub fn text(tip: String) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |window, cx| {
        if cx.has_global::<Gate>() {
            cx.global_mut::<Gate>().last_text = Some(tip.clone());
        }
        Tooltip::new(tip.clone()).build(window, cx)
    }
}

/// The text of the last tooltip [`text`] built (tests).
pub fn last_text(cx: &App) -> Option<String> {
    cx.try_global::<Gate>().and_then(|g| g.last_text.clone())
}
