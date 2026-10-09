//! How the commit list lays out its rows, a setting shared by gitti and gitty: by the
//! list's width (Auto), always one line (subject | author | date), or always two lines
//! (subject; badges, author … date). The info columns are often too wide, so the switch
//! between one-line and two-line rows can also be made by hand.

use serde::{Deserialize, Serialize};

/// The commit list's row layout (stored as `commit_list_layout` in `gitti.db`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitListLayout {
    /// By the list's width: gitti switches to two lines below
    /// [`AUTO_TWO_LINES_BELOW_EM`]; gitty stays on one line and drops the author column
    /// in a narrow list (a terminal has few rows to spare).
    #[default]
    Auto,
    /// Subject, author and date side by side, whatever the width.
    OneLine,
    /// Subject on the first line; badges, author and date on the second.
    TwoLines,
}

/// gitti's Auto rule: a list narrower than this many UI-font ems gets two-line rows.
pub const AUTO_TWO_LINES_BELOW_EM: f32 = 40.;

impl CommitListLayout {
    pub const ALL: [Self; 3] = [Self::Auto, Self::OneLine, Self::TwoLines];

    /// The menu / palette name of the choice. "One-Line …" first, not "Commit List: One
    /// Line": the palettes' subsequence match finds it by "one line" then, before "Fewer
    /// Context Lines" (whose letters "commit" would otherwise use up).
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto Commit List Layout",
            Self::OneLine => "One-Line Commit List",
            Self::TwoLines => "Two-Line Commit List",
        }
    }

    /// The palette entry for this choice, the current one marked: "Two-Line Commit List
    /// (current)" (a state `palette::normalize` drops).
    pub fn palette_label(self, current: Self) -> String {
        let mark = if self == current { " (current)" } else { "" };
        format!("{}{mark}", self.label())
    }

    /// What the palette entry says it does.
    pub fn description(self) -> &'static str {
        match self {
            Self::Auto => "the rows fit the list's width",
            Self::OneLine => "subject | author | date in one row",
            Self::TwoLines => "subject; badges, author and date below",
        }
    }

    /// Whether the rows use two lines; `narrow` = the list is below the app's Auto
    /// threshold (gitty never passes `true`: its Auto stays on one line).
    pub fn two_lines(self, narrow: bool) -> bool {
        match self {
            Self::Auto => narrow,
            Self::OneLine => false,
            Self::TwoLines => true,
        }
    }

    /// The status message after choosing it.
    pub fn chosen_message(self) -> &'static str {
        match self {
            Self::Auto => "Commit list: one or two lines by its width",
            Self::OneLine => "Commit list: one line per commit",
            Self::TwoLines => "Commit list: two lines per commit",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CommitListLayout as L;

    #[gitti_test::test]
    fn two_lines_by_choice_and_width() {
        assert!(!L::Auto.two_lines(false) && L::Auto.two_lines(true));
        assert!(!L::OneLine.two_lines(true) && !L::OneLine.two_lines(false));
        assert!(L::TwoLines.two_lines(false) && L::TwoLines.two_lines(true));
    }

    #[gitti_test::test]
    fn stored_as_snake_case_names() {
        for (l, s) in [
            (L::Auto, "\"auto\""),
            (L::OneLine, "\"one_line\""),
            (L::TwoLines, "\"two_lines\""),
        ] {
            assert_eq!(serde_json::to_string(&l).unwrap(), s);
            assert_eq!(serde_json::from_str::<L>(s).unwrap(), l);
        }
    }

    #[gitti_test::test]
    fn palette_marks_the_current_choice() {
        assert_eq!(
            L::TwoLines.palette_label(L::TwoLines),
            "Two-Line Commit List (current)"
        );
        assert_eq!(L::Auto.palette_label(L::OneLine), "Auto Commit List Layout");
    }
}
