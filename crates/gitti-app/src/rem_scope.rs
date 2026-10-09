//! A subtree with its own rem size. The window's rem follows the UI font
//! (`theme::sync_rem_size`); diff rows are sized by the diff font, so their gpui-component
//! buttons (whose height and text size are rems) take the rem of the diff font instead —
//! otherwise a diff zoomed apart from the UI clips them.

use gpui_kit::*;

pub struct RemScope {
    rem: Pixels,
    child: AnyElement,
}

pub fn rem_scope(rem: Pixels, child: impl IntoElement) -> RemScope {
    RemScope {
        rem,
        child: child.into_any_element(),
    }
}

impl IntoElement for RemScope {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for RemScope {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let child = &mut self.child;
        let id = window.with_rem_size(Some(self.rem), |window| child.request_layout(window, cx));
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let child = &mut self.child;
        window.with_rem_size(Some(self.rem), |window| child.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let child = &mut self.child;
        window.with_rem_size(Some(self.rem), |window| child.paint(window, cx));
    }
}
