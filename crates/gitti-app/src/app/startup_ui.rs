//! Start-up timings in gitti: a tab reports when its repository is first shown,
//! Help ▸ Start-up Timings… shows the kept starts, a slow start says so once. The
//! recording itself is `gitti_core::startup`.

use gpui_kit::*;

use super::GittiApp;
use super::repo_actions::AppEvent;

/// Where a slow start's notice points to.
pub(crate) const WHERE: &str = "Help ▸ Start-up Timings…";

impl GittiApp {
    /// The first read of this tab is done (shown, or failed): counted for the start.
    pub(super) fn report_startup_tab(&self, failed: bool, cx: &mut Context<Self>) {
        let Some(repo) = &self.repo else { return };
        if let Some(record) =
            gitti_core::startup::tab_ready(&repo.name(), repo.command_log(), failed)
        {
            cx.emit(AppEvent::StartupDone(record));
        }
    }

    /// A slow start, said once in the status line.
    pub fn show_startup_notice(&mut self, text: String, cx: &mut Context<Self>) {
        self.set_info(text);
        cx.notify();
    }

    pub(super) fn show_startup_timings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let records = gitti_core::startup::load();
        let text = gitti_core::startup::format_all(&records, &local_time);
        self.show_text_modal("Start-up Timings", text, window, cx);
    }
}

/// `2026-10-06 09:12:03` in local time.
fn local_time(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_default()
}
