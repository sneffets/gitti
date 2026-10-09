//! Commit graph rows as box-drawing characters: two terminal columns per lane, the node
//! in the lane's first column. One text row per commit, so every segment of a
//! [`GraphRow`] (top half = into the node, bottom half = out of it) collapses into corner
//! glyphs plus horizontal runs.
//!
//! The pending merge's edges (working-directory row → `MERGE_HEAD` commits while a merge
//! is in progress, [`gitti_core::graph::Segment::pending`]) are drawn with dashed glyphs
//! (`╎`, `╌`) where there are some, and their cells are marked [`Cell::pending`] (drawn
//! dimmer), corners included.

use gitti_core::graph::{Edge, GraphRow, Half};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NodeStyle {
    Commit,
    Merge,
    Head,
    Worktree,
    Stash,
}

impl NodeStyle {
    fn glyph(self) -> char {
        match self {
            NodeStyle::Commit => '●',
            NodeStyle::Merge => '◆',
            NodeStyle::Head => '◉',
            NodeStyle::Worktree => '◌',
            NodeStyle::Stash => '■',
        }
    }
}

/// One terminal cell of the graph column; `color` is a lane palette index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub color: u8,
    /// Only lines of the pending merge pass here: drawn dimmer.
    pub pending: bool,
    /// Part of the edge the graph chooser's cursor is on ([`with_edge`]): drawn bold.
    pub strong: bool,
}

const BLANK: Cell = Cell {
    ch: ' ',
    color: 0,
    pending: false,
    strong: false,
};

/// Which kinds of line pass a cell: an edge that exists, one of the pending merge.
#[derive(Clone, Copy, Default)]
struct Kinds {
    solid: bool,
    dashed: bool,
}

impl Kinds {
    fn add(&mut self, pending: bool) {
        if pending {
            self.dashed = true;
        } else {
            self.solid = true;
        }
    }

    /// Only pending lines.
    fn pending(self) -> bool {
        self.dashed && !self.solid
    }
}

#[derive(Clone, Copy, Default)]
struct Lane {
    up: bool,
    down: bool,
    /// Joins the node horizontally from above (`╯`/`╰`).
    into_node: bool,
    /// Leaves the node horizontally downwards (`╮`/`╭`).
    out_of_node: bool,
    color: Option<u8>,
    /// Colour of a horizontal run crossing this lane.
    horizontal: Option<u8>,
    /// The lines through this cell: its own (vertical, corner) and runs crossing it.
    kinds: Kinds,
    run_kinds: Kinds,
}

/// Cells for `row`, `lanes` lanes wide (wider graphs are clipped).
pub fn cells(row: &GraphRow, lanes: usize, style: NodeStyle) -> Vec<Cell> {
    let lanes = lanes.max(1);
    let node = row.node as usize;
    let mut lane = vec![Lane::default(); lanes];
    let mut gaps: Vec<Option<u8>> = vec![None; lanes];
    let mut gap_kinds = vec![Kinds::default(); lanes];
    // Horizontal run between the node and lane `to`, drawn over lanes and gaps.
    let mut run = |lane: &mut [Lane], gaps: &mut [Option<u8>], to: usize, color: u8, pending: bool| {
        let (lo, hi) = if to < node { (to, node) } else { (node, to) };
        for l in lane.iter_mut().take(hi.min(lanes)).skip(lo + 1) {
            l.horizontal.get_or_insert(color);
            l.run_kinds.add(pending);
        }
        for (g, k) in gaps.iter_mut().zip(gap_kinds.iter_mut()).take(hi.min(lanes)).skip(lo) {
            g.get_or_insert(color);
            k.add(pending);
        }
    };
    for s in &row.segments {
        let (from, to) = (s.from as usize, s.to as usize);
        match s.half {
            Half::Top if from == to => {
                if let Some(l) = lane.get_mut(from) {
                    l.up = true;
                    l.color.get_or_insert(s.color);
                    l.kinds.add(s.pending);
                }
            }
            Half::Top => {
                if let Some(l) = lane.get_mut(from) {
                    l.into_node = true;
                    l.color = Some(s.color);
                    l.kinds.add(s.pending);
                }
                run(&mut lane, &mut gaps, from, s.color, s.pending);
            }
            Half::Bottom if from == to => {
                if let Some(l) = lane.get_mut(from) {
                    l.down = true;
                    l.color.get_or_insert(s.color);
                    l.kinds.add(s.pending);
                }
            }
            Half::Bottom => {
                if let Some(l) = lane.get_mut(to) {
                    l.out_of_node = true;
                    l.color.get_or_insert(s.color);
                    l.kinds.add(s.pending);
                }
                run(&mut lane, &mut gaps, to, s.color, s.pending);
            }
        }
    }
    let mut out = Vec::with_capacity(lanes * 2);
    for (i, l) in lane.iter().enumerate() {
        let color = l.color.or(l.horizontal).unwrap_or(0);
        let right = i > node;
        let kinds = Kinds {
            solid: l.kinds.solid || l.run_kinds.solid,
            dashed: l.kinds.dashed || l.run_kinds.dashed,
        };
        let pending = i != node && kinds.pending();
        let ch = if i == node {
            style.glyph()
        } else if l.into_node || l.out_of_node {
            // Corner where a horizontal run meets this lane; a tee if the lane also
            // continues vertically on the other side.
            let up = l.up || l.into_node;
            let down = l.down || l.out_of_node;
            // Another run continues past this lane.
            let through = l.horizontal.is_some();
            match (up, down, through, right) {
                (true, true, true, _) => '┼',
                (true, true, false, true) => '┤',
                (true, true, false, false) => '├',
                (true, false, true, _) => '┴',
                (true, false, false, true) => '╯',
                (true, false, false, false) => '╰',
                (false, _, true, _) => '┬',
                (false, _, false, true) => '╮',
                (false, _, false, false) => '╭',
            }
        } else {
            match (l.up, l.down, l.horizontal.is_some()) {
                (true, true, true) => '┼',
                (true, true, false) if pending => '╎',
                (true, true, false) => '│',
                (true, false, _) => '╵',
                (false, true, _) => '╷',
                (false, false, true) if pending => '╌',
                (false, false, true) => '─',
                (false, false, false) => ' ',
            }
        };
        let color = if i == node {
            row.color
        } else if matches!(ch, '─' | '╌') {
            l.horizontal.unwrap_or(color)
        } else {
            color
        };
        out.push(Cell {
            ch,
            color,
            pending,
            strong: false,
        });
        out.push(match gaps[i] {
            Some(c) => {
                let pending = gap_kinds[i].pending();
                Cell {
                    ch: if pending { '╌' } else { '─' },
                    color: c,
                    pending,
                    strong: false,
                }
            }
            None => BLANK,
        });
    }
    out
}

/// Cells of the second text row of a two-line commit row: every lane that goes
/// on below the commit, as a vertical (dashed if only the pending merge's lines pass).
pub fn below(row: &GraphRow, lanes: usize) -> Vec<Cell> {
    let lanes = lanes.max(1);
    let mut out = vec![BLANK; lanes * 2];
    let mut kinds = vec![Kinds::default(); lanes];
    for s in row.segments.iter().filter(|s| s.half == Half::Bottom) {
        let l = s.to as usize;
        if l >= lanes {
            continue;
        }
        kinds[l].add(s.pending);
        if out[l * 2].ch == ' ' {
            out[l * 2] = Cell {
                ch: '│',
                color: s.color,
                pending: false,
                strong: false,
            };
        }
    }
    for (l, k) in kinds.iter().enumerate() {
        if out[l * 2].ch != ' ' && k.pending() {
            out[l * 2].ch = '╎';
            out[l * 2].pending = true;
        }
    }
    out
}

/// [`cells`] (or [`below`], `second_line`) of row `ix` with `edge`'s parts heavy: the
/// cells the edge alone would draw become heavy glyphs ([`heavy`]) and [`Cell::strong`];
/// the node too where the edge starts or ends.
pub fn with_edge(
    row: &GraphRow,
    ix: usize,
    edge: &Edge,
    lanes: usize,
    style: NodeStyle,
    second_line: bool,
) -> Vec<Cell> {
    let draw = |r: &GraphRow| {
        if second_line {
            below(r, lanes)
        } else {
            cells(r, lanes, style)
        }
    };
    let mut out = draw(row);
    let only = GraphRow {
        segments: row
            .segments
            .iter()
            .filter(|s| edge.has(ix, row.node, s))
            .copied()
            .collect(),
        ..row.clone()
    };
    if only.segments.is_empty() {
        return out;
    }
    let node = row.node as usize * 2;
    for (i, (cell, e)) in out.iter_mut().zip(draw(&only)).enumerate() {
        if !second_line && i == node {
            cell.strong = ix == edge.child || ix == edge.parent;
        } else if e.ch != ' ' {
            cell.ch = heavy(cell.ch, e.ch);
            cell.color = e.color;
            cell.pending = false;
            cell.strong = true;
        }
    }
    out
}

/// The heavy form of `ch` where line `edge` (what the edge alone draws in the cell) is
/// the heavy part: a crossing gets only the edge's direction heavy.
fn heavy(ch: char, edge: char) -> char {
    match (ch, edge) {
        ('┼', '│' | '╎') => '╂',
        ('┼', '─' | '╌') => '┿',
        ('├', '│') => '┠',
        ('┤', '│') => '┨',
        ('┴', '─') => '┷',
        ('┬', '─') => '┯',
        // The edge turns here, another line runs on to the other side.
        ('┴', '╯') => '┹',
        ('┴', '╰') => '┺',
        ('┬', '╮') => '┱',
        ('┬', '╭') => '┲',
        _ => match ch {
            '│' | '╎' => '┃',
            '─' | '╌' => '━',
            '╭' => '┏',
            '╮' => '┓',
            '╯' => '┛',
            '╰' => '┗',
            '├' => '┣',
            '┤' => '┫',
            '┼' => '╋',
            '┴' => '┻',
            '┬' => '┳',
            '╵' => '╹',
            '╷' => '╻',
            other => other,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitti_core::graph::Segment;

    /// Under a merge node with a branch going off to lane 1 and a lane passing
    /// at 2, the second line continues lanes 0, 1 and 2; a lane ending at the node
    /// doesn't; the pending merge's lane is dashed.
    #[gitti_test::test]
    fn below_continues_the_lanes_going_down() {
        let row = GraphRow {
            node: 0,
            color: 0,
            width: 4,
            segments: vec![
                seg(Half::Top, 0, 0),
                seg(Half::Top, 3, 0),
                seg(Half::Bottom, 0, 0),
                seg(Half::Bottom, 0, 1),
                seg(Half::Top, 2, 2),
                seg(Half::Bottom, 2, 2),
                dashed(Half::Bottom, 0, 3),
            ],
        };
        let text: String = below(&row, 4).iter().map(|c| c.ch).collect();
        assert_eq!(text.trim_end(), "│ │ │ ╎");
        let bare = GraphRow { node: 0, color: 0, width: 1, segments: vec![seg(Half::Top, 0, 0)] };
        assert_eq!(below(&bare, 1).iter().map(|c| c.ch).collect::<String>().trim_end(), "");
    }

    /// The edge to the chooser's candidate is drawn heavy — the merge's
    /// branch-off to lane 1 here, the lane passing through below, never the other lines.
    #[gitti_test::test]
    fn the_chosen_edge_is_heavy() {
        let merge = GraphRow {
            node: 0,
            color: 0,
            width: 3,
            segments: vec![
                seg(Half::Top, 0, 0),
                seg(Half::Bottom, 0, 0),
                seg(Half::Bottom, 0, 1),
                seg(Half::Top, 2, 2),
                seg(Half::Bottom, 2, 2),
            ],
        };
        let edge = Edge { child: 4, parent: 9, lane: 1 };
        let text = |c: &[Cell]| c.iter().map(|c| c.ch).collect::<String>();
        let at_child = with_edge(&merge, 4, &edge, 3, NodeStyle::Merge, false);
        assert_eq!(text(&at_child).trim_end(), "◆━┓ │");
        assert!(at_child[0].strong && at_child[2].strong && !at_child[4].strong);
        // Further down the lane passes (a crossing run: only the vertical is heavy).
        let crossing = GraphRow {
            node: 0,
            color: 0,
            width: 3,
            segments: vec![
                seg(Half::Top, 0, 0),
                seg(Half::Top, 1, 1),
                seg(Half::Bottom, 1, 1),
                seg(Half::Top, 2, 0),
            ],
        };
        let mid = with_edge(&crossing, 6, &edge, 3, NodeStyle::Commit, false);
        assert_eq!(text(&mid).trim_end(), "●─╂─╯");
        assert!(!mid[0].strong);
        // Another row: nothing heavy.
        let other = with_edge(&merge, 12, &edge, 3, NodeStyle::Merge, false);
        assert!(other.iter().all(|c| !c.strong), "{}", text(&other));
        assert_eq!(text(&with_edge(&merge, 4, &edge, 3, NodeStyle::Merge, true)).trim_end(), "│ ┃ │");
    }

    fn seg(half: Half, from: u16, to: u16) -> Segment {
        Segment {
            half,
            from,
            to,
            color: from as u8,
            pending: false,
        }
    }

    fn dashed(half: Half, from: u16, to: u16) -> Segment {
        Segment {
            pending: true,
            ..seg(half, from, to)
        }
    }

    fn text(row: &GraphRow, lanes: usize) -> String {
        cells(row, lanes, NodeStyle::Commit)
            .iter()
            .map(|c| c.ch)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[gitti_test::test]
    fn straight_line() {
        let row = GraphRow {
            node: 0,
            color: 0,
            width: 2,
            segments: vec![
                seg(Half::Top, 0, 0),
                seg(Half::Bottom, 0, 0),
                seg(Half::Top, 1, 1),
                seg(Half::Bottom, 1, 1),
            ],
        };
        assert_eq!(text(&row, 2), "● │");
    }

    #[gitti_test::test]
    fn merge_opens_lane_and_branch_joins() {
        // Merge commit: second parent opens lane 1.
        let merge = GraphRow {
            node: 0,
            color: 0,
            width: 2,
            segments: vec![
                seg(Half::Top, 0, 0),
                seg(Half::Bottom, 0, 0),
                seg(Half::Bottom, 0, 1),
            ],
        };
        assert_eq!(text(&merge, 2), "●─╮");
        // Fork point: lane 1 converges into the node in lane 0.
        let fork = GraphRow {
            node: 0,
            color: 0,
            width: 2,
            segments: vec![
                seg(Half::Top, 0, 0),
                seg(Half::Top, 1, 0),
                seg(Half::Bottom, 0, 0),
            ],
        };
        assert_eq!(text(&fork, 2), "●─╯");
        // Crossing a pass-through lane.
        let cross = GraphRow {
            node: 0,
            color: 0,
            width: 3,
            segments: vec![
                seg(Half::Top, 0, 0),
                seg(Half::Top, 1, 1),
                seg(Half::Top, 2, 0),
                seg(Half::Bottom, 0, 0),
                seg(Half::Bottom, 1, 1),
            ],
        };
        assert_eq!(text(&cross, 3), "●─┼─╯");
        // Merge whose second parent joins a lane that passes through.
        let join = GraphRow {
            node: 0,
            color: 0,
            width: 2,
            segments: vec![
                seg(Half::Top, 0, 0),
                seg(Half::Top, 1, 1),
                seg(Half::Bottom, 0, 0),
                seg(Half::Bottom, 0, 1),
                seg(Half::Bottom, 1, 1),
            ],
        };
        assert_eq!(text(&join, 2), "●─┤");
        // Octopus-like stash row: two new lanes, the first one crossed by the second run.
        let stash = GraphRow {
            node: 0,
            color: 0,
            width: 3,
            segments: vec![
                seg(Half::Bottom, 0, 0),
                seg(Half::Bottom, 0, 1),
                seg(Half::Bottom, 0, 2),
            ],
        };
        assert_eq!(text(&stash, 3), "●─┬─╮");
    }

    #[gitti_test::test]
    fn pending_merge_is_dashed() {
        // The working-directory row merging lane 1 (pending) into HEAD in lane 0.
        let wt = GraphRow {
            node: 0,
            color: 0,
            width: 2,
            segments: vec![seg(Half::Bottom, 0, 0), dashed(Half::Bottom, 0, 1)],
        };
        assert_eq!(text(&wt, 2), "●╌╮");
        let wt_cells = cells(&wt, 2, NodeStyle::Commit);
        assert_eq!(
            wt_cells.iter().map(|c| c.pending).collect::<Vec<_>>(),
            [false, true, true, false]
        );
        // A row below: the pending lane passes by.
        let below = GraphRow {
            node: 0,
            color: 0,
            width: 2,
            segments: vec![
                seg(Half::Top, 0, 0),
                seg(Half::Bottom, 0, 0),
                dashed(Half::Top, 1, 1),
                dashed(Half::Bottom, 1, 1),
            ],
        };
        assert_eq!(text(&below, 2), "● ╎");
        // A real run crossing the pending lane stays solid.
        let cross = GraphRow {
            node: 0,
            color: 0,
            width: 3,
            segments: vec![
                seg(Half::Top, 0, 0),
                dashed(Half::Top, 1, 1),
                dashed(Half::Bottom, 1, 1),
                seg(Half::Top, 2, 0),
                seg(Half::Bottom, 0, 0),
            ],
        };
        assert_eq!(text(&cross, 3), "●─┼─╯");
        assert!(cells(&cross, 3, NodeStyle::Commit).iter().all(|c| !c.pending));
    }
}
