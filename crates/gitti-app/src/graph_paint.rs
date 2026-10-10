//! Painting of one commit-graph row. Every row is painted independently from its
//! precomputed [`GraphRow`], so only visible rows cost anything.
//!
//! While a merge is in progress the working-directory row is the pending merge commit:
//! its edges to the `MERGE_HEAD` commits ([`Segment::pending`]) are dashed, its node a
//! dashed ring.

use gitti_core::graph::{Edge, GraphRow, Half, Segment};
use gpui_kit::*;

pub const LANE_W: f32 = 13.0;
pub const GRAPH_PAD: f32 = 8.0;
/// The graph column never gets wider than this; more lanes are drawn closer together.
const MAX_GRAPH_W: f32 = 16.0 * LANE_W;
const MIN_LANE_W: f32 = 4.0;
/// Dashes of a pending edge: about this period (px), this much of it drawn. Every
/// half-row segment gets a whole number of periods, starting and ending with half a gap,
/// so the dashes run on evenly from row to row whatever the row height.
const DASH_PERIOD: f32 = 5.5;
const DASH_FILL: f32 = 0.55;
/// Points a curved half-row segment is flattened to for dashing.
const CURVE_STEPS: usize = 12;

/// Horizontal geometry of the graph column for `lanes` occupied lanes.
#[derive(Clone, Copy)]
struct Lanes {
    /// Lanes that fit (the rest is clipped at the right edge).
    shown: u16,
    lane_w: f32,
}

impl Lanes {
    fn new(lanes: u16) -> Self {
        let lanes = lanes.max(1);
        let lane_w = (MAX_GRAPH_W / lanes as f32).clamp(MIN_LANE_W, LANE_W);
        let shown = lanes.min((MAX_GRAPH_W / lane_w).floor() as u16).max(1);
        Self { shown, lane_w }
    }

    fn width(&self) -> Pixels {
        px(self.shown as f32 * self.lane_w + GRAPH_PAD)
    }

    fn x(&self, origin: Pixels, lane: u16) -> Pixels {
        origin + px(GRAPH_PAD / 2.0 + self.lane_w * (lane.min(self.shown - 1) as f32 + 0.5))
    }

    /// Node radii shrink with the lane spacing.
    fn radius(&self, r: f32) -> f32 {
        r.min(self.lane_w * 0.45).max(2.0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NodeStyle {
    Commit,
    Merge,
    Head,
    Worktree,
    /// The working-directory row while a merge is in progress (the pending merge commit).
    PendingMerge,
    Stash,
}

/// What the graph navigation chooser marks in a row.
#[derive(Clone, Copy, Default)]
pub struct NavMark {
    /// One of the chooser's candidates: a halo around its node.
    pub candidate: bool,
    /// The candidate under the chooser's cursor: a strong halo.
    pub focused: bool,
    /// The edge from the selected commit to that candidate: drawn thick where it
    /// crosses this row.
    pub edge: Option<Edge>,
}

/// Stroke of an edge, and of the edge the chooser's cursor is on.
const EDGE_W: f32 = 1.5;
const FOCUSED_EDGE_W: f32 = 3.5;

/// The width of the graph cell for `lanes` occupied lanes ([`graph_cell`]).
pub fn graph_width(lanes: u16) -> Pixels {
    Lanes::new(lanes).width()
}

/// The graph cell of commit-list row `ix` (`ix` names its layout probes).
pub fn graph_cell(
    ix: usize,
    row: GraphRow,
    lanes: u16,
    style: NodeStyle,
    bg: Hsla,
    colors: [Hsla; 8],
    nav: NavMark,
) -> impl IntoElement {
    let lanes = Lanes::new(lanes);
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            paint_row(ix, &row, lanes, style, bg, &colors, nav.edge, bounds, window);
            if nav.candidate {
                paint_candidate_ring(ix, &row, lanes, &colors, nav.focused, bounds, window);
            }
        },
    )
    .w(lanes.width())
    .h_full()
    .flex_none()
}

#[allow(clippy::too_many_arguments)]
fn paint_row(
    ix: usize,
    row: &GraphRow,
    lanes: Lanes,
    style: NodeStyle,
    bg: Hsla,
    colors: &[Hsla; 8],
    edge: Option<Edge>,
    b: Bounds<Pixels>,
    window: &mut Window,
) {
    let lane_color = |i: u8| colors[i as usize % colors.len()];
    let x = |lane: u16| lanes.x(b.origin.x, lane);
    let top = b.origin.y;
    let mid = b.origin.y + b.size.height / 2.0;
    let bottom = b.origin.y + b.size.height;
    let h4 = b.size.height / 4.0;

    for seg in &row.segments {
        if seg.from >= lanes.shown && seg.to >= lanes.shown {
            continue;
        }
        let (y0, y1) = match seg.half {
            Half::Top => (top, mid),
            Half::Bottom => (mid, bottom),
        };
        let (x0, x1) = (x(seg.from), x(seg.to));
        let on_edge = edge.is_some_and(|e| e.has(ix, row.node, seg));
        let mut p = PathBuilder::stroke(px(if on_edge { FOCUSED_EDGE_W } else { EDGE_W }));
        if seg.pending {
            dash(&mut p, &segment_points(x0, x1, y0, y1, h4));
            probe_dash(ix, seg, x0, x1, y0, y1, window);
        } else {
            p.move_to(point(x0, y0));
            if x0 == x1 {
                p.line_to(point(x1, y1));
            } else {
                p.cubic_bezier_to(point(x1, y1), point(x0, y0 + h4), point(x1, y1 - h4));
            }
        }
        if let Ok(path) = p.build() {
            window.paint_path(path, lane_color(seg.color));
        }
    }

    if row.node >= lanes.shown {
        return;
    }
    let color = lane_color(row.color);
    let (r, fill_color, border) = match style {
        NodeStyle::Commit => (4.0, color, 0.0),
        NodeStyle::Merge => (3.5, color, 0.0),
        NodeStyle::Head => (5.0, bg, 2.0),
        NodeStyle::Worktree | NodeStyle::PendingMerge => (5.0, bg, 1.5),
        NodeStyle::Stash => (4.0, bg, 1.5),
    };
    let border_style = if style == NodeStyle::PendingMerge {
        BorderStyle::Dashed
    } else {
        BorderStyle::default()
    };
    let c = point(x(row.node), mid);
    let r = px(lanes.radius(r));
    let bounds = Bounds::new(point(c.x - r, c.y - r), size(r * 2.0, r * 2.0));
    // Stashes get a square node, everything else a circle.
    let corner = if style == NodeStyle::Stash {
        px(1.5)
    } else {
        r
    };
    window.paint_quad(quad(
        bounds,
        corner,
        fill_color,
        px(border),
        color,
        border_style,
    ));
}

/// The points of a half-row segment from `(x0, y0)` to `(x1, y1)`: a straight line, or
/// the curve [`paint_row`] draws for a lane change, flattened.
fn segment_points(x0: Pixels, x1: Pixels, y0: Pixels, y1: Pixels, h4: Pixels) -> Vec<Point<Pixels>> {
    if x0 == x1 {
        return vec![point(x0, y0), point(x1, y1)];
    }
    let (p0, c0, c1, p1) = (
        point(x0, y0),
        point(x0, y0 + h4),
        point(x1, y1 - h4),
        point(x1, y1),
    );
    (0..=CURVE_STEPS)
        .map(|i| {
            let t = i as f32 / CURVE_STEPS as f32;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            point(
                p0.x * a + c0.x * b + c1.x * c + p1.x * d,
                p0.y * a + c0.y * b + c1.y * c + p1.y * d,
            )
        })
        .collect()
}

/// Adds the dashes along the polyline `pts` to `p` (one sub-path each): a whole number
/// of [`DASH_PERIOD`]s fitted to its length, each dash centred in its period.
fn dash(p: &mut PathBuilder, pts: &[Point<Pixels>]) {
    let len = |a: Point<Pixels>, b: Point<Pixels>| {
        let (dx, dy) = (f32::from(b.x - a.x), f32::from(b.y - a.y));
        (dx * dx + dy * dy).sqrt()
    };
    // Arc length at every point.
    let mut at = Vec::with_capacity(pts.len());
    let mut total = 0.0;
    for (i, &q) in pts.iter().enumerate() {
        if i > 0 {
            total += len(pts[i - 1], q);
        }
        at.push(total);
    }
    if total <= 0.0 {
        return;
    }
    // The point at arc length `s`.
    let point_at = |s: f32| {
        let i = at.partition_point(|&a| a < s).clamp(1, pts.len() - 1);
        let (a, b) = (pts[i - 1], pts[i]);
        let span = at[i] - at[i - 1];
        let t = if span > 0.0 { (s - at[i - 1]) / span } else { 0.0 };
        point(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
    };
    let n = (total / DASH_PERIOD).round().max(1.0);
    let period = total / n;
    let on = period * DASH_FILL;
    for k in 0..n as usize {
        let start = k as f32 * period + (period - on) / 2.0;
        let end = start + on;
        p.move_to(point_at(start));
        for (i, &q) in pts.iter().enumerate() {
            if at[i] > start && at[i] < end {
                p.line_to(q);
            }
        }
        p.line_to(point_at(end));
    }
}

/// Layout probe `graph-dash:ROW:FROM-TO` over a dashed segment (tests check it lies in
/// its row).
fn probe_dash(
    ix: usize,
    seg: &Segment,
    x0: Pixels,
    x1: Pixels,
    y0: Pixels,
    y1: Pixels,
    window: &Window,
) {
    if !crate::probe::enabled() {
        return;
    }
    let (left, right) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
    let bounds = Bounds::new(
        point(left - px(0.75), y0),
        size(right - left + px(1.5), y1 - y0),
    );
    crate::probe::record(
        format!("graph-dash:{ix}:{}-{}", seg.from, seg.to),
        bounds,
        window.content_mask().bounds,
    );
}

/// A halo around the node of a row offered by the graph navigation chooser; a stronger
/// one on the candidate under the chooser's cursor (probe `graph-focus:{ix}`).
fn paint_candidate_ring(
    ix: usize,
    row: &GraphRow,
    lanes: Lanes,
    colors: &[Hsla; 8],
    focused: bool,
    b: Bounds<Pixels>,
    window: &mut Window,
) {
    if row.node >= lanes.shown {
        return;
    }
    let color = colors[row.color as usize % colors.len()];
    let c = point(
        lanes.x(b.origin.x, row.node),
        b.origin.y + b.size.height / 2.0,
    );
    let (r, fill, border) = if focused {
        (px(9.0), 0.5, px(3.0))
    } else {
        (px(8.0), 0.2, px(1.5))
    };
    let bounds = Bounds::new(point(c.x - r, c.y - r), size(r * 2.0, r * 2.0));
    window.paint_quad(quad(
        bounds,
        r,
        color.opacity(fill),
        border,
        color,
        BorderStyle::default(),
    ));
    if focused {
        crate::probe::record(format!("graph-focus:{ix}"), bounds, window.content_mask().bounds);
    }
}
