//! Show Tag: a tag's details in a text window — lightweight or annotated, the annotated
//! one's tagger, date, message and verified signature ([`gitti_core::tag`]).

use gpui_kit::*;

use super::GittiApp;

impl GittiApp {
    /// Reads tag `name` in the background (one `for-each-ref`, a `verify-tag` if signed)
    /// and shows it in a text window.
    pub(super) fn show_tag(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let read = name.clone();
        let task = cx
            .background_executor()
            .spawn(async move { repo.tag_details(&read) });
        cx.spawn_in(window, async move |this, cx| {
            let details = task.await;
            this.update_in(cx, |this, window, cx| match details {
                Ok(d) => this.show_text_modal(&format!("Tag {name}"), d.describe(), window, cx),
                Err(e) => {
                    this.set_error(format!("Show Tag {name}: {e:#}"));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}
