//! The commit ids in the commit header: the commit's own (`header-id`) and each parent's
//! (`header-parent:N`), each with a context menu that copies it (`gitti_core::header_ids`).
//! The author / date / committer lines are selectable text instead
//! (`text_sel::Area::Header`, [`header_lines`]).
//! The header lies inside the diff list, whose context menu opens on any right click in
//! it: like the signature (`signing_keys::mark_signature_click`), an id marks the click in
//! the capture phase and the diff's menu builder shows the id's menu instead. The same
//! for the "Refs" row's badges (`commit_list::ref_badge`): their ref's menu.

use gitti_core::ObjectId;
use gitti_core::header_ids::{HeaderId, copy_entries};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::*;

use crate::probe::ProbeExt as _;

/// The commit header's selectable lines (`text_sel::Area::Header`, by index): the author
/// ("Name <email>"), the date, and the committer when it isn't the author.
pub(super) fn header_lines(d: &gitti_core::CommitDetails) -> Vec<String> {
    let mut lines = vec![
        format!("{} <{}>", d.author, d.author_email),
        super::format_time_full(d.author_time),
    ];
    if d.committer != d.author || d.committer_email != d.author_email {
        lines.push(format!("{} <{}>", d.committer, d.committer_email));
    }
    lines
}

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

thread_local! {
    static REF_CLICK: std::cell::RefCell<Option<gitti_core::RefInfo>> =
        const { std::cell::RefCell::new(None) };
}

/// The right mouse button went down on ref `r`'s badge in the header's "Refs" row.
pub(super) fn mark_ref_click(r: gitti_core::RefInfo) {
    REF_CLICK.with(|c| *c.borrow_mut() = Some(r));
}

/// The ref whose header badge the menu being built is for, if any (forgets it).
pub(super) fn take_ref_click() -> Option<gitti_core::RefInfo> {
    REF_CLICK.with(|c| c.borrow_mut().take())
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
