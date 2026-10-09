//! Menus fit the window. gpui-component makes a menu with more than 20 items scrollable
//! (the in-window menu bar's menus: `with_menu_items`) and caps it at min(half the window,
//! 450 px), so long menus scrolled although the window had room — and one with 20 entries
//! or fewer ran out of a small window. Every `PopupMenu` is scrollable with the main
//! window's height (minus the room above and below it) as its cap instead: it scrolls only
//! when the window really is too small.
//!
//! The menu bar builds its menus outside a window context and keeps them for the next
//! time, so the cap comes from the workspace's render ([`track_window`]): new menus get
//! the current one, live ones a new one whenever the window's height changes.

use gpui_kit::component::menu::PopupMenu;
use gpui_kit::*;

/// Room kept free around a menu, in rem. Small: a menu taller than the room below its
/// anchor is moved up (gpui snaps it into the window), over the menu bar if need be.
const MARGIN_REM: f32 = 1.;

#[derive(Default)]
struct MenuFit {
    cap: Option<Pixels>,
    menus: Vec<WeakEntity<PopupMenu>>,
}

impl Global for MenuFit {}

/// Installs the hook on new menus (once per app, before the first window).
pub fn init(cx: &mut App) {
    cx.set_global(MenuFit::default());
    cx.observe_new(|menu: &mut PopupMenu, _, cx: &mut Context<PopupMenu>| {
        let weak = cx.entity().downgrade();
        let fit = cx.global_mut::<MenuFit>();
        fit.menus.retain(|m| m.upgrade().is_some());
        fit.menus.push(weak);
        if let Some(cap) = fit.cap {
            set_cap(menu, cap);
        }
    })
    .detach();
}

/// Called by the main window's root on every render: a changed height re-caps every live
/// menu.
pub fn track_window(window: &Window, cx: &mut App) {
    let cap = (window.viewport_size().height - window.rem_size() * MARGIN_REM).max(px(120.));
    let Some(fit) = cx.try_global::<MenuFit>() else {
        return;
    };
    if fit.cap == Some(cap) {
        return;
    }
    let menus = fit.menus.clone();
    cx.global_mut::<MenuFit>().cap = Some(cap);
    for menu in menus.iter().filter_map(WeakEntity::upgrade) {
        menu.update(cx, |m, cx| {
            set_cap(m, cap);
            cx.notify();
        });
    }
}

fn set_cap(menu: &mut PopupMenu, cap: Pixels) {
    // `scrollable` / `max_h` are builders (take the menu by value) and the fields are
    // private: move the menu out and back. Nothing in between can panic (they only set the
    // fields).
    // SAFETY: `menu` is valid for reads and writes, the value read is written back before
    // anything else can observe it, and the moved-out copy is consumed by the builders.
    unsafe {
        let owned = std::ptr::read(menu);
        std::ptr::write(menu, owned.scrollable(true).max_h(cap));
    }
}
