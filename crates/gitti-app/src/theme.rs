//! Our own palette (dark bluish grey) and persisted user settings.

use gpui_kit::*;
use serde::{Deserialize, Serialize};

fn rgb_hex(hex: u32) -> Hsla {
    rgb(hex).into()
}

#[derive(Clone)]
pub struct Palette {
    pub bg_toolbar: Hsla,
    pub bg_sidebar: Hsla,
    pub bg_list: Hsla,
    pub bg_list_hover: Hsla,
    pub bg_list_selected: Hsla,
    pub bg_diff: Hsla,
    pub bg_section: Hsla,
    pub bg_file_header: Hsla,
    pub bg_hunk_header: Hsla,
    pub bg_hunk_gap: Hsla,
    pub bg_gutter: Hsla,
    pub bg_added: Hsla,
    pub bg_removed: Hsla,
    pub bg_added_gutter: Hsla,
    pub bg_removed_gutter: Hsla,
    pub bg_line_selected: Hsla,
    /// Character selection (copy) — translucent, so added/removed backgrounds show through.
    pub text_selection: Hsla,
    pub bg_modal: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub text_strong: Hsla,
    pub text_muted: Hsla,
    pub text_faint: Hsla,
    pub text_added: Hsla,
    pub text_removed: Hsla,
    pub accent: Hsla,
    pub accent_text: Hsla,
    pub danger: Hsla,
    pub warning: Hsla,
    pub success: Hsla,
    pub tag: Hsla,
    pub remote: Hsla,
    /// Compare mode (banner, marked base row): purple, unlike any commit view colour.
    pub compare: Hsla,
    pub bg_compare: Hsla,
    pub lanes: [Hsla; 8],
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            bg_toolbar: rgb_hex(0x1e252c),
            bg_sidebar: rgb_hex(0x272f37),
            bg_list: rgb_hex(0x2f3741),
            bg_list_hover: rgb_hex(0x353e49),
            bg_list_selected: rgb_hex(0x414c5a),
            bg_diff: rgb_hex(0x33404e),
            bg_section: rgb_hex(0x28323d),
            bg_file_header: rgb_hex(0x4d5f70),
            bg_hunk_header: rgb_hex(0x3f4f5e),
            bg_hunk_gap: rgb_hex(0x28323d),
            bg_gutter: rgb_hex(0x3b4957),
            bg_added: rgb_hex(0x36757e),
            bg_removed: rgb_hex(0x7a4450),
            bg_added_gutter: rgb_hex(0x305d67),
            bg_removed_gutter: rgb_hex(0x5f3a43),
            bg_line_selected: rgb_hex(0x4a70a8),
            text_selection: rgb_hex(0x7fb0ff).opacity(0.45),
            bg_modal: rgb_hex(0x384452),
            border: rgb_hex(0x1b222a),
            text: rgb_hex(0xd4d9e1),
            text_strong: rgb_hex(0xf2f4f7),
            text_muted: rgb_hex(0x9aa3ae),
            text_faint: rgb_hex(0x76808b),
            text_added: rgb_hex(0xe3f4f2),
            text_removed: rgb_hex(0xf6e4e7),
            accent: rgb_hex(0x7aaee8),
            accent_text: rgb_hex(0x10141a),
            danger: rgb_hex(0xe5737f),
            warning: rgb_hex(0xe5c07b),
            success: rgb_hex(0x86c79a),
            tag: rgb_hex(0xe5c07b),
            remote: rgb_hex(0x9aa5b4),
            compare: rgb_hex(0xb18be8),
            bg_compare: rgb_hex(0x463d5e),
            lanes: [
                rgb_hex(0x7aaee8),
                rgb_hex(0xe57fb1),
                rgb_hex(0x86c79a),
                rgb_hex(0xe5a45f),
                rgb_hex(0xb18be8),
                rgb_hex(0x5fc4cf),
                rgb_hex(0xe5d06f),
                rgb_hex(0xe5737f),
            ],
        }
    }
}

impl Global for Palette {}

/// Corner radius of the label boxes — ref badges (branches, tags, worktrees, the merge /
/// compare marks) and the count / stat pills of the diff — the only rounded boxes. Everything
/// else — menus, buttons, inputs, tooltips, dialogs, pickers — is square, through the
/// component theme's zero radius ([`apply_to_components`]) and no `rounded_*` on our own
/// elements. Same as gpui's `rounded_sm` (scales with the UI font).
pub const BADGE_RADIUS: Rems = Rems(0.125);

/// Aligns gpui-component's theme (buttons, menus, popovers) with our palette. Its dark
/// theme uses near-black hover / pressed colours, which are invisible on our blue-grey
/// panels — buttons felt dead.
pub fn apply_to_components(p: &Palette, cx: &mut App) {
    gpui_kit::component::Theme::update(cx, |t| {
        // Square corners on every kit component: buttons, inputs, popup / context /
        // menu-bar menus, tooltips, checkboxes, kbd chips; `radius_full` (switch thumbs,
        // dots) follows a zero `radius` too.
        t.radius = px(0.);
        t.radius_lg = px(0.);
        let c = &mut t.colors;
        // Ghost buttons hover with `accent` at 50 %, menus highlight items with it.
        c.accent = p.text_strong.opacity(0.38);
        c.accent_foreground = p.text_strong;
        // Pressed ghost / default buttons.
        c.button_active = p.accent.opacity(0.55);
        c.button = p.bg_file_header;
        c.button_hover = rgb_hex(0x6b8190);
        c.button_foreground = p.text_strong;
        c.secondary = p.bg_file_header;
        c.secondary_hover = rgb_hex(0x6b8190);
        c.secondary_active = p.accent.opacity(0.55);
        c.secondary_foreground = p.text_strong;
        c.popover = p.bg_modal;
        c.popover_foreground = p.text;
        c.border = p.border;
        // Code editor (merge result): gutter and current line in our colours, not black.
        let mut ht = (*t.highlight_theme).clone();
        ht.style.editor_gutter_background = Some(p.bg_gutter);
        ht.style.editor_active_line = Some(p.bg_list_hover);
        ht.style.editor_line_number = Some(p.text_faint);
        ht.style.editor_active_line_number = Some(p.text_strong);
        t.highlight_theme = std::sync::Arc::new(ht);
    });
}

pub trait PaletteExt {
    fn palette(&self) -> &Palette;
}

impl PaletteExt for App {
    fn palette(&self) -> &Palette {
        self.global::<Palette>()
    }
}

/// Text edge smoothing (Settings ▸ Appearance), applied app-wide through gpui's
/// `TextRenderingMode` ([`apply_text_antialiasing`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextAntialiasing {
    /// The system's choice: ClearType when Windows' font smoothing is ClearType, subpixel
    /// on Linux where the GPU can blend it, grayscale on macOS.
    #[default]
    System,
    /// Subpixel (ClearType-style, coloured edges); ignored where gpui can't draw it (macOS).
    Subpixel,
    /// Grayscale: no colour fringes.
    Grayscale,
}

impl TextAntialiasing {
    pub const ALL: [TextAntialiasing; 3] = [Self::System, Self::Subpixel, Self::Grayscale];

    pub fn id(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Subpixel => "subpixel",
            Self::Grayscale => "grayscale",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Subpixel if cfg!(windows) => "ClearType",
            Self::Subpixel => "Subpixel",
            Self::Grayscale => "Grayscale",
        }
    }

    fn mode(self) -> TextRenderingMode {
        match self {
            Self::System => TextRenderingMode::PlatformDefault,
            Self::Subpixel => TextRenderingMode::Subpixel,
            Self::Grayscale => TextRenderingMode::Grayscale,
        }
    }
}

/// Sets gpui's text rendering mode for every window and redraws them when it changed. The
/// glyph atlas is keyed by the mode, so the next frame rasterizes with the new one.
pub fn apply_text_antialiasing(aa: TextAntialiasing, cx: &mut App) {
    if cx.text_rendering_mode() != aa.mode() {
        cx.set_text_rendering_mode(aa.mode());
        cx.refresh_windows();
    }
}

pub const DEFAULT_UI_FONT: f32 = 13.0;
pub const DEFAULT_DIFF_FONT: f32 = 13.0;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub ui_font_size: f32,
    pub diff_font_size: f32,
    /// Text edge smoothing: system / subpixel (ClearType) / grayscale.
    pub text_antialiasing: TextAntialiasing,
    pub topo_order: bool,
    /// Soft-wrap long lines in the diff view.
    pub wrap_lines: bool,
    /// Tree-sitter syntax highlighting in diffs.
    pub syntax_highlight: bool,
    /// `git diff -w` for display (staging maps selections onto the exact diff).
    pub ignore_whitespace: bool,
    /// Bottom panel with executed git commands.
    pub show_git_log: bool,
    /// Also list read-only queries (status, diff, …) in the git log.
    pub log_show_reads: bool,
    /// Mirror the git command log to a file per repository.
    pub persist_git_log: bool,
    /// Context lines around changes in diffs (`-U`).
    pub diff_context: u32,
    /// "Open in Editor" command (`gitti_core::editor`); empty = system default app.
    /// Merge editor: only the result (ours / theirs folded to a bar on top).
    pub merge_single_view: bool,
    pub editor_command: String,
    /// "Open in Terminal" command (`gitti_core::desktop`); empty = platform default.
    pub terminal_command: String,
    /// Sidebar shown (View ▸ Toggle Sidebar).
    pub show_sidebar: bool,
    /// Spell checking in the message editors (`spell`, `app/spell_ui.rs`).
    pub spell_check: bool,
    /// Intra-line diff marks: lightening in percent (`gitti_core::intraline::lift`), 0 = off.
    pub intraline_strength: u32,
    /// Commit list rows: by width, one line or two lines (shared with gitty).
    pub commit_list_layout: gitti_core::list_layout::CommitListLayout,
    /// Pane sizes as fractions, shared with gitty (`gitti_core::store::PaneLayout`).
    pub pane_sidebar: f32,
    pub pane_list: f32,
    pub pane_log: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ui_font_size: DEFAULT_UI_FONT,
            diff_font_size: DEFAULT_DIFF_FONT,
            text_antialiasing: TextAntialiasing::System,
            topo_order: true,
            wrap_lines: false,
            syntax_highlight: true,
            ignore_whitespace: false,
            show_git_log: false,
            log_show_reads: false,
            persist_git_log: true,
            diff_context: 3,
            merge_single_view: false,
            editor_command: String::new(),
            terminal_command: String::new(),
            show_sidebar: true,
            spell_check: true,
            intraline_strength: gitti_core::intraline::DEFAULT_STRENGTH,
            commit_list_layout: gitti_core::list_layout::CommitListLayout::Auto,
            pane_sidebar: 0.,
            pane_list: 0.,
            pane_log: 0.,
        }
    }
}

impl Settings {
    /// From the settings database (`store`), field by field over the defaults.
    pub fn load() -> Self {
        crate::store::load_settings()
    }

    /// Height of a commit list / sidebar row.
    pub fn row_h(&self) -> Pixels {
        px((self.ui_font_size * 1.85).round())
    }

    /// Height of a diff row.
    pub fn diff_row_h(&self) -> Pixels {
        px((self.diff_font_size * 1.6).round())
    }

    /// Line height of commit messages (view and editor): tighter than a diff row, prose
    /// reads better that way.
    pub fn message_line_h(&self) -> Pixels {
        px((self.diff_font_size * 1.35).round())
    }

    pub fn ui(&self) -> Pixels {
        px(self.ui_font_size)
    }

    pub fn ui_small(&self) -> Pixels {
        px((self.ui_font_size - 2.0).max(8.0))
    }

    pub fn diff(&self) -> Pixels {
        px(self.diff_font_size)
    }

    /// Small text inside diff rows (headers, notes): rows are diff-font tall.
    pub fn diff_small(&self) -> Pixels {
        px((self.diff_font_size - 2.0).max(8.0))
    }

    /// The window's rem size: gpui's 16 px at the default UI font, scaled with it.
    pub fn rem(&self) -> Pixels {
        px((16.0 * self.ui_font_size / DEFAULT_UI_FONT).round())
    }

    /// The rem size inside diff rows (`rem_scope`), scaled with the diff font.
    pub fn diff_rem(&self) -> Pixels {
        px((16.0 * self.diff_font_size / DEFAULT_DIFF_FONT).round())
    }

    pub fn zoom(&mut self, delta: f32) {
        self.ui_font_size = (self.ui_font_size + delta).clamp(9.0, 28.0);
        self.diff_font_size = (self.diff_font_size + delta).clamp(9.0, 32.0);
    }

    pub fn zoom_diff(&mut self, delta: f32) {
        self.diff_font_size = (self.diff_font_size + delta).clamp(9.0, 32.0);
    }

    pub fn reset_zoom(&mut self) {
        self.ui_font_size = DEFAULT_UI_FONT;
        self.diff_font_size = DEFAULT_DIFF_FONT;
    }
}

/// gpui-component sizes its widgets in rems (buttons, the in-window menu bar and its
/// menus, context menus, checkboxes, popovers), and its root sets the window's rem size
/// from `Theme::font_size` every frame — so they ignored the UI font size. Called
/// by each window root's render, which runs after that and before layout: this frame uses
/// the new size, the theme keeps it for the next ones (and the other windows).
pub fn sync_rem_size(s: &Settings, window: &mut Window, cx: &mut App) {
    let rem = s.rem();
    if gpui_kit::component::Theme::global(cx).font_size != rem {
        gpui_kit::component::Theme::global_mut(cx).font_size = rem;
    }
    window.set_rem_size(rem);
}

impl Settings {
    pub fn diff_options(&self) -> gitti_core::DiffOptions {
        gitti_core::DiffOptions {
            ignore_whitespace: self.ignore_whitespace,
            context_lines: self.diff_context,
        }
    }
}

pub use gitti_core::paths::log_file_for;

/// How the `secondary` modifier is written in hints.
pub const MOD_KEY: &str = if cfg!(target_os = "macos") {
    "⌘"
} else {
    "Ctrl+"
};

/// Key hints are written with macOS symbols; spelled out elsewhere.
/// Diff font size keys as shown (macOS: ⌃⌘, since ⌥⌘± is the system zoom when that's on).
pub const DIFF_ZOOM_KEYS: &str = if cfg!(target_os = "macos") {
    "⌃⌘+ / ⌃⌘−"
} else {
    "⌘⌥+ / ⌘⌥−"
};

pub fn keys_hint(mac: &str) -> String {
    if cfg!(target_os = "macos") {
        return mac.to_string();
    }
    mac.replace(['⌘', '⌃'], "Ctrl+")
        .replace('⌥', "Alt+")
        .replace('⇧', "Shift+")
        .replace('↩', "Enter")
}

pub fn mono_font() -> &'static str {
    if cfg!(target_os = "macos") {
        "Menlo"
    } else if cfg!(target_os = "windows") {
        "Consolas"
    } else {
        "DejaVu Sans Mono"
    }
}

#[cfg(test)]
mod tests {
    use super::Settings;
    use gitti_core::store::{PaneLayout, SharedSettings};

    #[gitti_test::test]
    fn shared_settings_match_gitti_settings() {
        // gitty stores these under the same keys; names and defaults must agree.
        let ours = serde_json::to_value(Settings::default()).unwrap();
        for shared in [
            serde_json::to_value(SharedSettings::default()).unwrap(),
            serde_json::to_value(PaneLayout::default()).unwrap(),
        ] {
            for (key, value) in shared.as_object().unwrap() {
                assert_eq!(ours.get(key), Some(value), "{key}");
            }
        }
    }
}
