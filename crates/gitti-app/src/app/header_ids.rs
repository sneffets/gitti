//! The commit ids in the commit header: the commit's own (`header-id`) and each parent's
//! (`header-parent:N`), each with a context menu that copies it (`gitti_core::header_ids`).
//! The header lies inside the diff list, whose context menu opens on any right click in
//! it: like the signature (`signing_keys::mark_signature_click`), an id marks the click in
//! the capture phase and the diff's menu builder shows the id's menu instead.

use gitti_core::ObjectId;
use gitti_core::header_ids::{HeaderId, copy_entries};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::*;

use crate::probe::ProbeExt as _;

thread_local! {
    static ID_CLICK: std::cell::Cell<Option<IdClick>> =
        const { std::cell::Cell::new(None) };
}

/// A right click on an id of the header: the id, which one, how many parents.
pub(super) type IdClick = (ObjectId, HeaderId, usize);

/// The right click on an id the menu being built is for, if any (forgets it).
pub(super) fn take_id_click() -> Option<IdClick> {
    ID_CLICK.with(|c| c.take())
}

/// The id's context menu: copy it, full or short.
pub(super) fn id_menu(menu: PopupMenu, (id, which, parents): IdClick) -> PopupMenu {
    copy_entries(&id, which, parents)
        .into_iter()
        .fold(menu, |m, (label, text)| m.item(super::menus::copy_item(label, text)))
}

/// The id `id` as shown in the header (`text`), with its context menu.
pub(super) fn id_element(
    id: ObjectId,
    which: HeaderId,
    parents: usize,
    text: String,
) -> AnyElement {
    let (el_id, probe): (ElementId, String) = match which {
        HeaderId::Commit => ("header-id".into(), "header-id".into()),
        HeaderId::Parent(n) => (("header-parent", n).into(), format!("header-parent:{n}")),
    };
    div()
        .id(el_id)
        .probe(probe)
        .min_w_0()
        .truncate()
        .child(text)
        .capture_any_mouse_down(move |ev, _, _| {
            if ev.button == MouseButton::Right {
                ID_CLICK.with(|c| c.set(Some((id, which, parents))));
            }
        })
        .into_any_element()
}
