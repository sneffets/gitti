//! gitti's palette (dark bluish grey) as 24-bit terminal colours.
//! Keep the values in sync with `gitti-app/src/theme.rs`.
//!
//! Terminals without 24-bit colour ([`ColorDepth`]) get every frame mapped after drawing
//! ([`reduce`]): to the nearest xterm-256 colour, or with 16 colours to a fixed colour per
//! palette role (selection blue, added green, removed red, …) and by hue / lightness for
//! the rest (blends, syntax). `NO_COLOR` is honoured by crossterm (no colour at all).

use std::collections::HashMap;
use std::sync::OnceLock;

use gitti_core::syntax::Category;
use ratatui::buffer::Buffer;
use ratatui::style::Color;

const fn hex(v: u32) -> Color {
    Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

#[derive(Clone, Debug)]
pub struct Palette {
    pub bg_toolbar: Color,
    pub bg_sidebar: Color,
    pub bg_list: Color,
    pub bg_list_selected: Color,
    pub bg_diff: Color,
    pub bg_section: Color,
    pub bg_file_header: Color,
    pub bg_hunk_header: Color,
    pub bg_gutter: Color,
    pub bg_added: Color,
    pub bg_removed: Color,
    pub bg_added_gutter: Color,
    pub bg_removed_gutter: Color,
    /// Intra-line diff: the changed characters inside a removed / added line.
    pub bg_line_selected: Color,
    pub bg_modal: Color,
    pub border: Color,
    pub text: Color,
    pub text_strong: Color,
    pub text_muted: Color,
    pub text_faint: Color,
    pub text_added: Color,
    pub text_removed: Color,
    pub accent: Color,
    pub accent_text: Color,
    pub danger: Color,
    pub warning: Color,
    pub success: Color,
    pub tag: Color,
    pub remote: Color,
    /// Compare mode (banner, marked base row) — same as gitti.
    pub compare: Color,
    pub bg_compare: Color,
    pub lanes: [Color; 8],
    /// What the terminal shows; frames are mapped down to it ([`reduce`]).
    pub depth: ColorDepth,
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            bg_toolbar: hex(0x1e252c),
            bg_sidebar: hex(0x272f37),
            bg_list: hex(0x2f3741),
            bg_list_selected: hex(0x414c5a),
            bg_diff: hex(0x33404e),
            bg_section: hex(0x28323d),
            bg_file_header: hex(0x4d5f70),
            bg_hunk_header: hex(0x3f4f5e),
            bg_gutter: hex(0x3b4957),
            bg_added: hex(0x36757e),
            bg_removed: hex(0x7a4450),
            bg_added_gutter: hex(0x305d67),
            bg_removed_gutter: hex(0x5f3a43),
            bg_line_selected: hex(0x4a70a8),
            bg_modal: hex(0x384452),
            border: hex(0x1b222a),
            text: hex(0xd4d9e1),
            text_strong: hex(0xf2f4f7),
            text_muted: hex(0x9aa3ae),
            text_faint: hex(0x76808b),
            text_added: hex(0xe3f4f2),
            text_removed: hex(0xf6e4e7),
            accent: hex(0x7aaee8),
            accent_text: hex(0x10141a),
            danger: hex(0xe5737f),
            warning: hex(0xe5c07b),
            success: hex(0x86c79a),
            tag: hex(0xe5c07b),
            remote: hex(0x9aa5b4),
            compare: hex(0xb18be8),
            bg_compare: hex(0x463d5e),
            lanes: [
                hex(0x7aaee8),
                hex(0xe57fb1),
                hex(0x86c79a),
                hex(0xe5a45f),
                hex(0xb18be8),
                hex(0x5fc4cf),
                hex(0xe5d06f),
                hex(0xe5737f),
            ],
            depth: ColorDepth::TrueColor,
        }
    }

    /// Every colour of the palette, in field order (lanes last).
    fn colors(&self) -> Vec<Color> {
        let mut v = vec![
            self.bg_toolbar,
            self.bg_sidebar,
            self.bg_list,
            self.bg_list_selected,
            self.bg_diff,
            self.bg_section,
            self.bg_file_header,
            self.bg_hunk_header,
            self.bg_gutter,
            self.bg_added,
            self.bg_removed,
            self.bg_added_gutter,
            self.bg_removed_gutter,
            self.bg_line_selected,
            self.bg_modal,
            self.border,
            self.text,
            self.text_strong,
            self.text_muted,
            self.text_faint,
            self.text_added,
            self.text_removed,
            self.accent,
            self.accent_text,
            self.danger,
            self.warning,
            self.success,
            self.tag,
            self.remote,
            self.compare,
            self.bg_compare,
        ];
        v.extend(self.lanes);
        v
    }

    /// The palette's roles in 16 colours (same field order as [`Self::colors`]). Dark
    /// backgrounds are black, the bright background variants are avoided where the
    /// difference matters (the linux console has only 8 background colours).
    fn ansi16() -> Vec<Color> {
        use Color::*;
        vec![
            Black,        // bg_toolbar
            Black,        // bg_sidebar
            Black,        // bg_list
            Blue,         // bg_list_selected
            Black,        // bg_diff
            Black,        // bg_section
            DarkGray,     // bg_file_header
            DarkGray,     // bg_hunk_header
            Black,        // bg_gutter
            Green,        // bg_added
            Red,          // bg_removed
            Green,        // bg_added_gutter
            Red,          // bg_removed_gutter
            Blue,         // bg_line_selected
            Black,        // bg_modal
            DarkGray,     // border
            Gray,         // text
            White,        // text_strong
            Gray,         // text_muted
            DarkGray,     // text_faint
            White,        // text_added
            White,        // text_removed
            LightBlue,    // accent
            Black,        // accent_text
            LightRed,     // danger
            LightYellow,  // warning
            LightGreen,   // success
            LightYellow,  // tag
            Gray,         // remote
            LightMagenta, // compare
            Magenta,      // bg_compare
            LightBlue,    // lanes
            LightMagenta,
            LightGreen,
            Yellow,
            Magenta,
            LightCyan,
            LightYellow,
            LightRed,
        ]
    }

    pub fn lane(&self, color: u8) -> Color {
        self.lanes[color as usize % self.lanes.len()]
    }

    /// Syntax colours: gpui-component's default dark highlight theme, which gitti uses.
    pub fn syntax(&self, c: Category) -> Option<Color> {
        Some(match c {
            Category::Comment => hex(0x9d9d9d),
            Category::String => hex(0xa3e09f),
            Category::Keyword => hex(0x87b1f6),
            Category::Function => hex(0xb3c5f3),
            Category::Type => hex(0xcba6f7),
            Category::Number | Category::Constant | Category::Heading => hex(0xcc9e00),
            Category::Property => hex(0xbcc4e0),
            Category::Tag => hex(0x419cff),
            Category::Attribute => hex(0x7faef9),
            Category::Operator | Category::Punctuation => return None,
        })
    }
}

/// `fg` over `bg` with opacity `a` (terminals have no alpha).
pub fn blend(fg: Color, bg: Color, a: f32) -> Color {
    match (fg, bg) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let mix = |x: u8, y: u8| (x as f32 * a + y as f32 * (1.0 - a)).round() as u8;
            Color::Rgb(mix(r1, r2), mix(g1, g2), mix(b1, b2))
        }
        _ => fg,
    }
}

/// How many colours the terminal can show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorDepth {
    /// 24-bit colour: the palette as is.
    #[default]
    TrueColor,
    /// xterm's 256 colours.
    Ansi256,
    /// The 16 system colours (linux console, `*-16color` terminals).
    Ansi16,
}

impl ColorDepth {
    /// The real terminal's depth (scripted runs stay 24-bit unless `GITTY_COLORS` says
    /// otherwise, so tests see the palette's values).
    pub fn detect(scripted: bool) -> Self {
        Self::from_env(|k| std::env::var(k).ok(), scripted)
    }

    /// `GITTY_COLORS` (`truecolor` / `256` / `16`) wins; then `COLORTERM=truecolor|24bit`
    /// → 24-bit; `TERM=linux` / `*-16color` / `*-8color` → 16; `TERM=*-direct`, Windows and
    /// a few terminals known for 24-bit (`TERM_PROGRAM`) → 24-bit; anything else → 256.
    pub fn from_env(var: impl Fn(&str) -> Option<String>, scripted: bool) -> Self {
        let lower = |k: &str| var(k).unwrap_or_default().trim().to_ascii_lowercase();
        match lower("GITTY_COLORS").as_str() {
            "truecolor" | "24bit" | "24" => return Self::TrueColor,
            "256" => return Self::Ansi256,
            "16" | "8" => return Self::Ansi16,
            _ if scripted => return Self::TrueColor,
            _ => {}
        }
        if matches!(lower("COLORTERM").as_str(), "truecolor" | "24bit") {
            return Self::TrueColor;
        }
        let term = lower("TERM");
        if matches!(term.as_str(), "linux" | "ansi" | "cons25")
            || term.ends_with("-16color")
            || term.ends_with("-8color")
        {
            return Self::Ansi16;
        }
        if term.ends_with("-direct") || cfg!(windows) {
            return Self::TrueColor;
        }
        match var("TERM_PROGRAM").as_deref() {
            Some("iTerm.app" | "WezTerm" | "vscode" | "ghostty") => Self::TrueColor,
            _ => Self::Ansi256,
        }
    }

    /// `c` as this depth shows it (only RGB colours change).
    pub fn map(self, c: Color) -> Color {
        let Color::Rgb(r, g, b) = c else {
            return c;
        };
        match self {
            Self::TrueColor => c,
            Self::Ansi256 => Color::Indexed(to_256(r, g, b)),
            Self::Ansi16 => roles16().get(&c).copied().unwrap_or_else(|| to_16(r, g, b)),
        }
    }
}

/// Maps a drawn frame down to `depth` (after drawing, so blends and syntax colours are
/// covered as well).
pub fn reduce(buf: &mut Buffer, depth: ColorDepth) {
    if depth == ColorDepth::TrueColor {
        return;
    }
    for cell in &mut buf.content {
        let (fg, bg) = (depth.map(cell.fg), depth.map(cell.bg));
        // Two nearby shades can fall on one colour: the text gets a readable one instead.
        cell.fg = if fg == bg && !cell.symbol().trim().is_empty() {
            contrast(bg)
        } else {
            fg
        };
        cell.bg = bg;
    }
}

/// A text colour readable on `bg` (a mapped colour): grey on dark grey, white on the other
/// dark colours, black on light ones.
fn contrast(bg: Color) -> Color {
    use Color::*;
    let light = match bg {
        Indexed(i) => indexed_lightness(i) >= 128,
        _ => matches!(
            bg,
            Gray | White | LightRed | LightGreen | LightYellow | LightBlue | LightMagenta
                | LightCyan | Yellow | Cyan
        ),
    };
    match bg {
        _ if light => match bg {
            Indexed(_) => Indexed(235),
            _ => Black,
        },
        DarkGray => Gray,
        Indexed(_) => Indexed(252),
        _ => White,
    }
}

/// Rough lightness (0–255) of an xterm-256 colour.
fn indexed_lightness(i: u8) -> u8 {
    const LEVELS: [u16; 6] = [0, 95, 135, 175, 215, 255];
    match i {
        0 | 1 | 2 | 4 | 5 | 8 => 64,
        0..16 => 192,
        16..232 => {
            let n = usize::from(i - 16);
            let (r, g, b) = (LEVELS[n / 36], LEVELS[n / 6 % 6], LEVELS[n % 6]);
            ((r.max(g).max(b) + r.min(g).min(b)) / 2) as u8
        }
        _ => 8 + 10 * (i - 232),
    }
}

/// The dark palette's colours → their 16-colour role (the first role wins for colours
/// shared by several).
fn roles16() -> &'static HashMap<Color, Color> {
    static ROLES: OnceLock<HashMap<Color, Color>> = OnceLock::new();
    ROLES.get_or_init(|| {
        let mut m = HashMap::new();
        for (from, to) in Palette::dark().colors().into_iter().zip(Palette::ansi16()) {
            m.entry(from).or_insert(to);
        }
        m
    })
}

/// The nearest xterm-256 colour among the 6×6×6 cube (16–231) and the grey ramp
/// (232–255); the 16 system colours are left out (terminal themes change them).
pub fn to_256(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let level = |v: u8| match v {
        0..48 => 0,
        48..115 => 1,
        _ => (usize::from(v) - 35) / 40,
    };
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let dist = |(x, y, z): (u8, u8, u8)| {
        let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
        d(x, r) + d(y, g) + d(z, b)
    };
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    let avg = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    let k = (avg.saturating_sub(3) / 10).min(23) as u8;
    let grey = 8 + 10 * k;
    if dist((grey, grey, grey)) < dist(cube) {
        232 + k
    } else {
        (16 + 36 * ri + 6 * gi + bi) as u8
    }
}

/// A colour that is no palette role, in 16 colours: greys by lightness, the rest by hue
/// (bright variants for light colours).
pub fn to_16(r: u8, g: u8, b: u8) -> Color {
    use Color::*;
    let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let light = (max + min) / 2;
    let chroma = max - min;
    if chroma < 40 {
        return match light {
            ..60 => Black,
            60..140 => DarkGray,
            140..210 => Gray,
            _ => White,
        };
    }
    let hue = if max == r {
        60 * (g - b) / chroma
    } else if max == g {
        120 + 60 * (b - r) / chroma
    } else {
        240 + 60 * (r - g) / chroma
    }
    .rem_euclid(360);
    let bright = light >= 128;
    let pick = |dark, light| if bright { light } else { dark };
    match hue {
        0..30 | 330.. => pick(Red, LightRed),
        30..90 => pick(Yellow, LightYellow),
        90..150 => pick(Green, LightGreen),
        150..200 => pick(Cyan, LightCyan),
        200..260 => pick(Blue, LightBlue),
        _ => pick(Magenta, LightMagenta),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[gitti_test::test]
    fn depth_from_the_environment() {
        use ColorDepth::*;
        let d = |pairs: &[(&str, &str)]| ColorDepth::from_env(env(pairs), false);
        assert_eq!(d(&[("COLORTERM", "truecolor"), ("TERM", "xterm-256color")]), TrueColor);
        assert_eq!(d(&[("COLORTERM", "24bit")]), TrueColor);
        assert_eq!(d(&[("TERM", "xterm-direct")]), TrueColor);
        assert_eq!(d(&[("TERM", "linux")]), Ansi16);
        assert_eq!(d(&[("TERM", "xterm-16color")]), Ansi16);
        assert_eq!(d(&[("TERM", "rxvt-8color")]), Ansi16);
        assert_eq!(d(&[("TERM", "xterm-16color"), ("GITTY_COLORS", "256")]), Ansi256);
        assert_eq!(d(&[("COLORTERM", "truecolor"), ("GITTY_COLORS", "16")]), Ansi16);
        if !cfg!(windows) {
            assert_eq!(d(&[("TERM", "xterm-256color")]), Ansi256);
            assert_eq!(d(&[("TERM", "screen-256color")]), Ansi256);
            assert_eq!(d(&[]), Ansi256);
            assert_eq!(d(&[("TERM_PROGRAM", "iTerm.app")]), TrueColor);
        }
        // Scripted runs: 24-bit unless asked.
        let s = |pairs: &[(&str, &str)]| ColorDepth::from_env(env(pairs), true);
        assert_eq!(s(&[("TERM", "linux")]), TrueColor);
        assert_eq!(s(&[("GITTY_COLORS", "16")]), Ansi16);
    }

    #[gitti_test::test]
    fn xterm_256() {
        assert_eq!(to_256(0, 0, 0), 16);
        assert_eq!(to_256(255, 255, 255), 231);
        assert_eq!(to_256(255, 0, 0), 196);
        assert_eq!(to_256(0x87, 0xaf, 0xff), 111);
        assert_eq!(to_256(128, 128, 128), 244);
        assert_eq!(to_256(8, 8, 8), 232);
        assert_eq!(to_256(238, 238, 238), 255);
    }

    /// The backgrounds that tell rows apart stay apart in 256 colours.
    #[gitti_test::test]
    fn palette_in_256_keeps_its_distinctions() {
        let p = Palette::dark();
        let m = |c| ColorDepth::Ansi256.map(c);
        let rows = [p.bg_list, p.bg_list_selected, p.bg_added, p.bg_removed, p.bg_line_selected];
        for (i, a) in rows.iter().enumerate() {
            for b in &rows[i + 1..] {
                assert_ne!(m(*a), m(*b), "{a:?} / {b:?}");
            }
        }
        assert_ne!(m(p.bg_diff), m(p.bg_list_selected));
        assert_ne!(m(p.text), m(p.bg_list));
    }

    #[gitti_test::test]
    fn palette_in_16_colours() {
        let p = Palette::dark();
        let m = |c| ColorDepth::Ansi16.map(c);
        assert_eq!(m(p.bg_list), Color::Black);
        assert_eq!(m(p.bg_list_selected), Color::Blue);
        assert_eq!(m(p.bg_added), Color::Green);
        assert_eq!(m(p.bg_removed), Color::Red);
        assert_eq!(m(p.accent), Color::LightBlue);
        assert_eq!(m(p.tag), Color::LightYellow);
        // Blends: the diff cursor's tint and an inactive selection are visible on black.
        assert_eq!(m(blend(p.text_strong, p.bg_diff, 0.14)), Color::DarkGray);
        assert_eq!(m(blend(p.bg_list_selected, p.bg_list, 0.55)), Color::DarkGray);
        // A ref badge: its text stays readable on its tinted background.
        let badge = m(blend(p.accent, p.bg_list, 0.2));
        assert_ne!(badge, m(p.accent));
        // Syntax colours by hue.
        assert_eq!(m(p.syntax(Category::String).unwrap()), Color::LightGreen);
        assert_eq!(m(p.syntax(Category::Keyword).unwrap()), Color::LightBlue);
        assert_eq!(m(p.syntax(Category::Comment).unwrap()), Color::Gray);
        // Non-RGB colours pass through.
        assert_eq!(m(Color::Reset), Color::Reset);
        assert_eq!(Palette::dark().colors().len(), Palette::ansi16().len());
    }

    #[gitti_test::test]
    fn reduce_maps_every_cell() {
        use ratatui::layout::Rect;
        use ratatui::style::Style;
        let p = Palette::dark();
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
        buf.set_style(buf.area, Style::new().fg(p.text).bg(p.bg_added));
        let mut b16 = buf.clone();
        reduce(&mut buf, ColorDepth::TrueColor);
        assert_eq!(buf[(1, 0)].bg, p.bg_added);
        reduce(&mut b16, ColorDepth::Ansi16);
        assert!(b16.content.iter().all(|c| c.fg == Color::Gray && c.bg == Color::Green));
        // Faint text on a slightly lifted background: both dark grey in 16 colours, the
        // text gets grey; blank cells keep theirs.
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        let faint_on_lift = Style::new().fg(p.text_faint).bg(blend(p.text_strong, p.bg_diff, 0.2));
        buf.set_style(buf.area, faint_on_lift);
        buf[(0, 0)].set_symbol("x");
        let mut b256 = buf.clone();
        reduce(&mut buf, ColorDepth::Ansi16);
        assert_eq!((buf[(0, 0)].fg, buf[(0, 0)].bg), (Color::Gray, Color::DarkGray));
        assert_eq!(buf[(1, 0)].fg, Color::DarkGray);
        reduce(&mut b256, ColorDepth::Ansi256);
        assert_ne!(b256[(0, 0)].fg, b256[(0, 0)].bg);
        assert_eq!(contrast(Color::Indexed(250)), Color::Indexed(235));
        assert_eq!(contrast(Color::Indexed(236)), Color::Indexed(252));
        assert_eq!(contrast(Color::LightYellow), Color::Black);
        assert_eq!(contrast(Color::Red), Color::White);
    }
}
