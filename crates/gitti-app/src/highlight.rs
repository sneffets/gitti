//! Syntax highlighting with gitti-core's engine (`gitti_core::syntax`, shared with gitty):
//! whole file versions are parsed, so multi-line constructs that start outside a hunk are
//! still coloured correctly; the result is a [`Category`] per range and line, coloured here
//! with [`syntax_style`]. The merge editor's result pane gets the same engine through
//! [`input_highlighter_factory`].

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gitti_core::syntax::{Category, LineCategories};
use gpui_kit::base::input::{
    EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter,
    InputHighlighterFactory,
};
use gpui_kit::component::Rope;
use gpui_kit::*;

/// Highlighted ranges of one line: byte ranges within the raw line text.
pub type LineStyles = LineCategories;

/// Per-line styles of a whole file (index = line number - 1).
pub type FileStyles = Arc<Vec<LineStyles>>;

/// Old and new side of one file in a diff.
#[derive(Clone, Default)]
pub struct FileHighlight {
    pub old: Option<FileStyles>,
    pub new: Option<FileStyles>,
}

/// Tree-sitter language name for a path (shared with gitty).
pub use gitti_core::syntax::language_for_path;

/// Highlights `text` and returns the categories of every line (none for an unsupported
/// language).
pub fn highlight_text(text: &str, lang: &str) -> Vec<LineStyles> {
    gitti_core::syntax::highlight(lang, text).unwrap_or_default()
}

/// How a category looks: gpui-component's default dark syntax theme, which gitti used
/// before it shared gitty's engine (gitty's `Palette::syntax` has the same colours).
pub fn syntax_style(c: Category) -> Option<HighlightStyle> {
    let color = |hex: u32| HighlightStyle {
        color: Some(rgb(hex).into()),
        ..Default::default()
    };
    Some(match c {
        Category::Comment => color(0x9d9d9d),
        Category::String => color(0xa3e09f),
        Category::Keyword => color(0x87b1f6),
        Category::Function => color(0xb3c5f3),
        Category::Type => color(0xcba6f7),
        Category::Number | Category::Constant => color(0xcc9e00),
        Category::Heading => HighlightStyle {
            font_weight: Some(FontWeight::SEMIBOLD),
            ..color(0xcc9e00)
        },
        Category::Property => color(0xbcc4e0),
        Category::Tag => color(0x419cff),
        Category::Attribute => color(0x7faef9),
        Category::Operator | Category::Punctuation => return None,
    })
}

/// Categories → highlight styles (uncoloured categories dropped).
pub fn styled(ranges: Vec<(Range<usize>, Category)>) -> Vec<(Range<usize>, HighlightStyle)> {
    ranges
        .into_iter()
        .filter_map(|(r, c)| Some((r, syntax_style(c)?)))
        .collect()
}

/// Caches highlight results by (language, text) so refreshes don't re-parse unchanged files.
#[derive(Default)]
pub struct HighlightCache {
    map: HashMap<u64, FileStyles>,
}

impl HighlightCache {
    const MAX_ENTRIES: usize = 400;

    pub fn key(lang: &str, text: &str) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        lang.hash(&mut h);
        text.hash(&mut h);
        h.finish()
    }

    pub fn get(&self, key: u64) -> Option<FileStyles> {
        self.map.get(&key).cloned()
    }

    pub fn insert(&mut self, key: u64, styles: FileStyles) {
        if self.map.len() >= Self::MAX_ENTRIES {
            self.map.clear();
        }
        self.map.insert(key, styles);
    }
}

/// Maps the raw-line styles onto the displayed segment of that line (tabs expanded to four
/// spaces, wrapped part `[char_start, char_end)` of the expanded text).
pub fn segment_styles(
    raw: &str,
    styles: &LineStyles,
    char_start: usize,
    char_end: usize,
) -> Vec<(Range<usize>, HighlightStyle)> {
    styled(gitti_core::syntax::segment_ranges(
        raw, styles, char_start, char_end,
    ))
}

/// For the merge editor's code editor (`EditorState::set_highlighter_factory`): highlights
/// with gitti-core instead of gpui-component's highlighter.
pub fn input_highlighter_factory() -> InputHighlighterFactory {
    Rc::new(|lang: &str| {
        Some(Box::new(CoreHighlighter {
            lang: SharedString::from(lang.to_string()),
            runs: Vec::new(),
        }) as Box<dyn InputHighlighter>)
    })
}

/// [`InputHighlighter`] on `gitti_core::syntax`: the whole text is re-highlighted after
/// every change (merge results are small; a 2000-line file takes a few ms).
struct CoreHighlighter {
    lang: SharedString,
    /// Coloured runs over the whole text (byte offsets), in order.
    runs: Vec<(Range<usize>, HighlightStyle)>,
}

impl CoreHighlighter {
    fn set_text(&mut self, text: &str) {
        self.runs.clear();
        let mut start = 0;
        for (line, cats) in text.split('\n').zip(highlight_text(text, &self.lang)) {
            for (r, style) in styled(cats) {
                self.runs.push((start + r.start..start + r.end, style));
            }
            start += line.len() + 1;
        }
    }
}

impl InputHighlighter for CoreHighlighter {
    fn language(&self) -> SharedString {
        self.lang.clone()
    }

    fn update(
        &mut self,
        _edit: Option<InputEdit>,
        text: &Rope,
        _folding: bool,
        _window: &mut Window,
        _cx: &mut Context<EditorState>,
    ) {
        self.set_text(&text.to_string());
    }

    fn styles(
        &self,
        range: &Range<usize>,
        _resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        // Ordered, non-overlapping runs covering all of `range` (gaps unstyled).
        let mut out = Vec::new();
        let mut at = range.start;
        let first = self.runs.partition_point(|(r, _)| r.end <= range.start);
        for (r, style) in &self.runs[first..] {
            if r.start >= range.end {
                break;
            }
            let (a, b) = (r.start.max(range.start), r.end.min(range.end));
            if a > at {
                out.push((at..a, HighlightStyle::default()));
            }
            out.push((a..b, *style));
            at = b;
        }
        if at < range.end {
            out.push((at..range.end, HighlightStyle::default()));
        }
        out
    }

    fn fold_ranges(&self, _text: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{CoreHighlighter, HighlightStyle, highlight_text, language_for_path, segment_styles};
    use gitti_core::syntax::Category;

    #[gitti_test::test]
    fn languages() {
        assert_eq!(language_for_path("src/main.rs"), Some("rust"));
        assert_eq!(language_for_path("a/CMakeLists.txt"), Some("cmake"));
        assert_eq!(language_for_path("Cargo.lock"), Some("toml"));
        assert_eq!(language_for_path("README"), None);
        assert_eq!(language_for_path("include/foo.h"), Some("cpp"));
        assert_eq!(language_for_path("shaders/blit.metal"), Some("cpp"));
        assert_eq!(language_for_path("a.frag"), Some("glsl"));
    }

    #[gitti_test::test]
    fn segment_mapping_with_tabs_and_wrap() {
        // raw "\tfn x" with "fn" (bytes 1..3) a keyword
        let styles = vec![(1..3, Category::Keyword)];
        // whole line displayed: "    fn x" → "fn" at display bytes 4..6
        assert_eq!(segment_styles("\tfn x", &styles, 0, usize::MAX)[0].0, 4..6);
        // second wrapped part starting at display char 5 ("n x") → "n" at 0..1
        assert_eq!(segment_styles("\tfn x", &styles, 5, usize::MAX)[0].0, 0..1);
        // punctuation has no colour: no style at all
        assert!(segment_styles("{", &vec![(0..1, Category::Punctuation)], 0, 9).is_empty());
    }

    #[gitti_test::test]
    fn rust_gets_highlighted_per_line() {
        let lines = highlight_text("/* a\n b */\nfn main() {}\n", "rust");
        assert!(lines.len() >= 3);
        // the block comment covers both of its lines
        assert!(!lines[0].is_empty() && !lines[1].is_empty(), "{lines:?}");
        assert!(!lines[2].is_empty());
    }

    #[gitti_test::test]
    fn editor_runs_cover_the_requested_range() {
        let mut h = CoreHighlighter {
            lang: "rust".into(),
            runs: Vec::new(),
        };
        let text = "fn a() {}\n// c\n";
        h.set_text(text);
        struct NoStyles;
        impl gpui_kit::base::input::HighlightStyleResolver for NoStyles {
            fn style(&self, _: &str) -> Option<HighlightStyle> {
                None
            }
        }
        let range = 1..text.len() - 1;
        let runs = gpui_kit::base::input::InputHighlighter::styles(&h, &range, &NoStyles);
        assert_eq!(runs.first().unwrap().0.start, range.start);
        assert_eq!(runs.last().unwrap().0.end, range.end);
        assert!(runs.windows(2).all(|w| w[0].0.end == w[1].0.start), "{runs:?}");
        // "fn" (keyword) is cut at the range start; the comment is coloured.
        assert!(runs.iter().any(|(r, s)| r.start >= 10 && s.color.is_some()));
    }
}
