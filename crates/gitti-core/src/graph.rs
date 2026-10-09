//! Commit graph lane layout.
//!
//! The layout is computed once for the whole history but stored compactly: per row only
//! its *events* (node lane, lanes that end in it, lanes opened for extra parents) plus a
//! snapshot of all lane colours every [`CHECKPOINT`] rows. [`Graph::rows`] replays the
//! events from the nearest snapshot to materialise the [`GraphRow`]s of the visible range,
//! so memory stays proportional to the number of commits, not commits × lanes.
//!
//! While a merge is in progress the working-directory row is laid out as the pending merge
//! commit ([`layout_pending_merge`]): its edges to the `MERGE_HEAD` commits are *pending*
//! ([`Segment::pending`], painted dashed) until they reach their commit.

use std::collections::{BTreeSet, HashMap};
use std::hash::Hash;
use std::ops::Range;

use gix::ObjectId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Half {
    /// From the top edge of the row to its centre.
    Top,
    /// From the centre of the row to its bottom edge.
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub half: Half,
    pub from: u16,
    pub to: u16,
    pub color: u8,
    /// Part of an edge of the pending merge (working-directory row → a `MERGE_HEAD`
    /// commit) that doesn't exist yet: painted dashed.
    pub pending: bool,
}

/// One materialised row, ready to paint.
#[derive(Debug, Clone, Default)]
pub struct GraphRow {
    pub node: u16,
    pub color: u8,
    /// Number of lanes that are occupied in this row (for sizing the graph column).
    pub width: u16,
    pub segments: Vec<Segment>,
}

/// One edge of the graph: from `child`'s node down to its parent at row `parent`, in
/// `lane` between them — to highlight it (the graph navigation chooser's candidate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub child: usize,
    pub parent: usize,
    pub lane: u16,
}

impl Edge {
    /// Whether `seg` of row `row` (its node in lane `node`) is a part of the edge. A lane
    /// another child of the same parent joined is shared on the way down.
    pub fn has(&self, row: usize, node: u16, seg: &Segment) -> bool {
        if row == self.child {
            seg.half == Half::Bottom && seg.from == node && seg.to == self.lane
        } else if row == self.parent {
            seg.half == Half::Top && seg.from == self.lane && seg.to == node
        } else {
            row > self.child && row < self.parent && seg.from == self.lane && seg.to == self.lane
        }
    }
}

pub const PALETTE_SIZE: u8 = 8;

/// Bit in a lane's colour byte (layout state, events, checkpoints) marking a lane of the
/// pending merge; colours are below [`PALETTE_SIZE`], so it is free.
const PENDING: u8 = 0x80;

/// `(colour, pending)` of a lane's colour byte.
fn split(c: u8) -> (u8, bool) {
    (c & !PENDING, c & PENDING != 0)
}

/// Rows between two lane snapshots.
const CHECKPOINT: usize = 64;

/// Bit in [`RowEvents::color`]: the commit has parents (its lane continues below it).
const HAS_PARENTS: u8 = 0x40;

/// A lane slot without a lane in [`Graph::lane_colors`].
const NO_LANE: u8 = 0xff;

/// What happens in one row (8 bytes). The lists it refers to are replayed in order from a
/// checkpoint, so a row stores only their lengths.
#[derive(Debug, Clone, Copy, Default)]
struct RowEvents {
    node: u16,
    /// Lanes waiting for this commit (they converge into the node). The first of them is
    /// the node's own lane; the others are the next `conv - 1` entries of
    /// [`Graph::converging`].
    conv: u16,
    /// Parents 2..n: the next `extra` entries of [`Graph::extra`].
    extra: u16,
    /// Palette index, plus [`HAS_PARENTS`].
    color: u8,
}

/// Where replaying starts: the lanes at the top of row `k * CHECKPOINT` and the list
/// positions of that row.
#[derive(Debug, Clone, Copy, Default)]
struct Checkpoint {
    /// Start of this snapshot in [`Graph::lane_colors`] (it ends where the next begins).
    lanes: u32,
    converging: u32,
    extra: u32,
}

/// Compact lane layout of a whole history.
#[derive(Debug, Clone, Default)]
pub struct Graph {
    rows: Vec<RowEvents>,
    /// Lanes converging into a row's node besides its own lane, row after row.
    converging: Vec<u16>,
    /// `(lane, colour)` of parents 2..n, row after row.
    extra: Vec<(u16, u8)>,
    checkpoints: Vec<Checkpoint>,
    /// The lane colours of every checkpoint, back to back ([`NO_LANE`]: a free slot).
    lane_colors: Vec<u8>,
}

/// Replay position: the next entries of [`Graph::converging`] and [`Graph::extra`].
#[derive(Clone, Copy)]
struct Cursor {
    conv: usize,
    extra: usize,
}

impl Graph {
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Bytes on the heap (capacity of every array).
    pub fn heap_bytes(&self) -> usize {
        use std::mem::size_of;
        self.rows.capacity() * size_of::<RowEvents>()
            + self.converging.capacity() * size_of::<u16>()
            + self.extra.capacity() * size_of::<(u16, u8)>()
            + self.checkpoints.capacity() * size_of::<Checkpoint>()
            + self.lane_colors.capacity()
    }

    /// The lane slots of checkpoint `k`.
    fn snapshot(&self, k: usize) -> &[u8] {
        let start = self.checkpoints[k].lanes as usize;
        let end = self
            .checkpoints
            .get(k + 1)
            .map_or(self.lane_colors.len(), |c| c.lanes as usize);
        &self.lane_colors[start..end]
    }

    /// Most lanes open at any checkpoint (an estimate of the graph's width).
    pub fn max_lanes(&self) -> usize {
        (0..self.checkpoints.len())
            .map(|k| self.snapshot(k).len())
            .max()
            .unwrap_or(0)
    }

    /// Lane of the node in `row`.
    pub fn node(&self, row: usize) -> Option<u16> {
        self.rows.get(row).map(|r| r.node)
    }

    /// Palette index of the node in `row`.
    pub fn color(&self, row: usize) -> Option<u8> {
        self.rows.get(row).map(|r| r.color & !HAS_PARENTS)
    }

    /// The edge from `child` to its parent number `parent_ix` (0 = first), which sits at
    /// row `parent`; `None` when the rows don't fit that (no such parent, not below).
    pub fn edge(&self, child: usize, parent: usize, parent_ix: usize) -> Option<Edge> {
        let ev = self.rows.get(child)?;
        if parent <= child || parent >= self.rows.len() {
            return None;
        }
        let lane = if parent_ix == 0 {
            (ev.color & HAS_PARENTS != 0).then_some(ev.node)?
        } else {
            if parent_ix > ev.extra as usize {
                return None;
            }
            // Where `child`'s extra parents start: from its checkpoint on.
            let cp = child / CHECKPOINT;
            let start = self.checkpoints[cp].extra as usize
                + self.rows[cp * CHECKPOINT..child]
                    .iter()
                    .map(|r| r.extra as usize)
                    .sum::<usize>();
            self.extra[start + parent_ix - 1].0
        };
        Some(Edge { child, parent, lane })
    }

    /// Materialises the rows in `range` (clamped to the graph).
    pub fn rows(&self, range: Range<usize>) -> Vec<GraphRow> {
        let end = range.end.min(self.rows.len());
        if range.start >= end {
            return Vec::new();
        }
        let cp = range.start / CHECKPOINT;
        let mut lanes: Vec<Option<u8>> = self
            .snapshot(cp)
            .iter()
            .map(|&c| (c != NO_LANE).then_some(c))
            .collect();
        let at = self.checkpoints[cp];
        let mut cursor = Cursor {
            conv: at.converging as usize,
            extra: at.extra as usize,
        };
        let mut out = Vec::with_capacity(end - range.start);
        for r in cp * CHECKPOINT..end {
            let row = self.replay(r, &mut lanes, &mut cursor, r >= range.start);
            if let Some(row) = row {
                out.push(row);
            }
        }
        out
    }

    /// Advances `lanes` over row `r`; returns the row's segments if `emit`.
    fn replay(
        &self,
        r: usize,
        lanes: &mut Vec<Option<u8>>,
        cursor: &mut Cursor,
        emit: bool,
    ) -> Option<GraphRow> {
        let ev = self.rows[r];
        let others = ev.conv.saturating_sub(1) as usize;
        let conv_rest = &self.converging[cursor.conv..cursor.conv + others];
        cursor.conv += others;
        let extra = &self.extra[cursor.extra..cursor.extra + ev.extra as usize];
        cursor.extra += ev.extra as usize;
        let converges = |i: u16| (ev.conv > 0 && i == ev.node) || conv_rest.contains(&i);
        let has_parents = ev.color & HAS_PARENTS != 0;
        let color = ev.color & !HAS_PARENTS;
        let node = ev.node as usize;
        let mut row = emit.then(|| GraphRow {
            node: ev.node,
            color,
            width: 0,
            segments: Vec::new(),
        });

        // Top half: waiting lanes converge into the node, all others pass through.
        if let Some(row) = row.as_mut() {
            for (i, lane) in lanes.iter().enumerate() {
                if let Some(c) = lane {
                    let to = if converges(i as u16) { ev.node } else { i as u16 };
                    let (color, pending) = split(*c);
                    row.segments.push(Segment {
                        half: Half::Top,
                        from: i as u16,
                        to,
                        color,
                        pending,
                    });
                }
            }
        }
        if ev.conv > 0
            && let Some(l) = lanes.get_mut(node)
        {
            *l = None;
        }
        for &i in conv_rest {
            if let Some(l) = lanes.get_mut(i as usize) {
                *l = None;
            }
        }
        // Lanes that continue through this row (not ending in this commit).
        let through: Vec<bool> = lanes.iter().map(Option::is_some).collect();

        // Bottom half: first parent continues in the node lane; other parents join an
        // existing lane or open a new one.
        let set = |lanes: &mut Vec<Option<u8>>, i: usize, c: Option<u8>| {
            if lanes.len() <= i {
                lanes.resize(i + 1, None);
            }
            lanes[i] = c;
        };
        if has_parents {
            set(lanes, node, Some(color));
        }
        for &(j, c) in extra {
            set(lanes, j as usize, Some(c));
            if let Some(row) = row.as_mut() {
                let (color, pending) = split(c);
                row.segments.push(Segment {
                    half: Half::Bottom,
                    from: ev.node,
                    to: j,
                    color,
                    pending,
                });
            }
        }
        if let Some(row) = row.as_mut() {
            for (i, lane) in lanes.iter().enumerate() {
                if let Some(c) = lane {
                    let straight = if i == node {
                        has_parents
                    } else {
                        through.get(i).copied().unwrap_or(false)
                    };
                    if straight {
                        let (color, pending) = split(*c);
                        row.segments.push(Segment {
                            half: Half::Bottom,
                            from: i as u16,
                            to: i as u16,
                            color,
                            pending,
                        });
                    }
                }
            }
            let max_seg = row
                .segments
                .iter()
                .map(|s| s.from.max(s.to))
                .max()
                .unwrap_or(0);
            row.width = max_seg.max(row.node) + 1;
        }
        while lanes.last().is_some_and(|l| l.is_none()) {
            lanes.pop();
        }
        row
    }
}

/// Lays out rows given as `(id, parents)` in display order (children before parents).
/// `K` identifies a commit: an [`ObjectId`], or a row key of the history's compact rows.
pub fn layout<'a, K: Copy + Eq + Hash + 'a>(rows: impl IntoIterator<Item = (K, &'a [K])>) -> Graph {
    layout_with(rows, false)
}

/// Like [`layout`], with row 0 as the pending merge commit (the working-directory row
/// while a merge is in progress): the edges to its parents after the first (the
/// `MERGE_HEAD` commits) are [`Segment::pending`] until they reach their commit, or
/// until a real commit's edge joins them on the way.
pub fn layout_pending_merge<'a, K: Copy + Eq + Hash + 'a>(
    rows: impl IntoIterator<Item = (K, &'a [K])>,
) -> Graph {
    layout_with(rows, true)
}

fn layout_with<'a, K: Copy + Eq + Hash + 'a>(
    rows: impl IntoIterator<Item = (K, &'a [K])>,
    pending_merge: bool,
) -> Graph {
    let mut g = Graph::default();
    // Lane state: which commit each lane waits for, and its colour.
    let mut lanes: Vec<Option<(K, u8)>> = Vec::new();
    // Commit → lanes waiting for it.
    let mut waiting: HashMap<K, Vec<u16>> = HashMap::new();
    // Unused lanes below `lanes.len()`.
    let mut free: BTreeSet<u16> = BTreeSet::new();
    let mut next_color: u8 = 0;
    let mut alloc_color = || {
        let c = next_color;
        next_color = (next_color + 1) % PALETTE_SIZE;
        c
    };
    let take_slot = |lanes: &mut Vec<Option<(K, u8)>>, free: &mut BTreeSet<u16>| match free
        .pop_first()
    {
        Some(i) => i as usize,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    };

    for (r, (id, parents)) in rows.into_iter().enumerate() {
        if r % CHECKPOINT == 0 {
            g.checkpoints.push(Checkpoint {
                lanes: g.lane_colors.len() as u32,
                converging: g.converging.len() as u32,
                extra: g.extra.len() as u32,
            });
            g.lane_colors
                .extend(lanes.iter().map(|l| l.map_or(NO_LANE, |(_, c)| c)));
        }
        let mut conv = waiting.remove(&id).unwrap_or_default();
        conv.sort_unstable();
        // The commit sits in the first lane waiting for it, else in a free lane.
        let (node, color) = match conv.first() {
            Some(&i) => (
                i as usize,
                split(lanes[i as usize].expect("waiting lane").1).0,
            ),
            None => (take_slot(&mut lanes, &mut free), alloc_color()),
        };
        for &i in &conv {
            lanes[i as usize] = None;
            if i as usize != node {
                free.insert(i);
            }
        }
        // The first converging lane is the node's: only the others are stored.
        g.converging.extend_from_slice(conv.get(1..).unwrap_or_default());

        let extra_start = g.extra.len();
        match parents.first() {
            Some(first) => {
                lanes[node] = Some((*first, color));
                waiting.entry(*first).or_default().push(node as u16);
            }
            None => {
                lanes[node] = None;
                free.insert(node as u16);
            }
        }
        let pending = if pending_merge && r == 0 { PENDING } else { 0 };
        for p in parents.iter().skip(1) {
            let existing = waiting.get(p).and_then(|v| v.iter().min().copied());
            let (j, c) = match existing {
                Some(j) => {
                    // A real edge joins the lane: from here on it exists (not pending).
                    let lane = lanes[j as usize].as_mut().expect("waiting lane");
                    lane.1 = split(lane.1).0 | pending;
                    (j, lane.1)
                }
                None => {
                    let j = take_slot(&mut lanes, &mut free);
                    let c = alloc_color() | pending;
                    lanes[j] = Some((*p, c));
                    waiting.entry(*p).or_default().push(j as u16);
                    (j as u16, c)
                }
            };
            g.extra.push((j, c));
        }
        // Keep the lane vector short: drop trailing free lanes.
        while lanes.last().is_some_and(|l| l.is_none()) {
            lanes.pop();
            free.remove(&(lanes.len() as u16));
        }
        g.rows.push(RowEvents {
            node: node as u16,
            conv: conv.len() as u16,
            extra: (g.extra.len() - extra_start) as u16,
            color: color | if parents.is_empty() { 0 } else { HAS_PARENTS },
        });
    }
    g.rows.shrink_to_fit();
    g.converging.shrink_to_fit();
    g.extra.shrink_to_fit();
    g.checkpoints.shrink_to_fit();
    g.lane_colors.shrink_to_fit();
    g
}

/// Order commits for display. `topo` mimics `git log --topo-order`, otherwise
/// `--date-order` (children always before parents in both cases).
pub fn sort_commits<T>(
    items: Vec<T>,
    id: impl Fn(&T) -> ObjectId,
    parents: impl Fn(&T) -> &[ObjectId],
    time: impl Fn(&T) -> i64,
    topo: bool,
) -> Vec<T> {
    let index: HashMap<ObjectId, usize> =
        items.iter().enumerate().map(|(i, c)| (id(c), i)).collect();
    let order = sort_order(
        items.len(),
        |i| parents(&items[i]).iter().filter_map(|p| index.get(p).copied()),
        |i| time(&items[i]),
        topo,
    );
    let mut slots: Vec<Option<T>> = items.into_iter().map(Some).collect();
    order.into_iter().filter_map(|i| slots[i as usize].take()).collect()
}

/// The display order of `n` commits given by index: `parents(i)` yields the indices of
/// the parents of `i` among them, `time(i)` its commit time. See [`sort_commits`].
pub(crate) fn sort_order<I: Iterator<Item = usize>>(
    n: usize,
    parents: impl Fn(usize) -> I,
    time: impl Fn(usize) -> i64,
    topo: bool,
) -> Vec<u32> {
    let mut indegree = vec![0u32; n];
    for i in 0..n {
        for p in parents(i) {
            indegree[p] += 1;
        }
    }
    let mut order: Vec<u32> = Vec::with_capacity(n);
    if topo {
        // Initial tips newest first, used as a stack.
        let mut tips: Vec<usize> = (0..n).filter(|&i| indegree[i] == 0).collect();
        tips.sort_by_key(|&i| time(i));
        let mut stack = tips; // newest at the end → popped first
        while let Some(i) = stack.pop() {
            order.push(i as u32);
            for p in parents(i) {
                indegree[p] -= 1;
                if indegree[p] == 0 {
                    stack.push(p);
                }
            }
        }
    } else {
        use std::collections::BinaryHeap;
        let mut heap: BinaryHeap<(i64, std::cmp::Reverse<usize>)> = (0..n)
            .filter(|&i| indegree[i] == 0)
            .map(|i| (time(i), std::cmp::Reverse(i)))
            .collect();
        while let Some((_, std::cmp::Reverse(i))) = heap.pop() {
            order.push(i as u32);
            for p in parents(i) {
                indegree[p] -= 1;
                if indegree[p] == 0 {
                    heap.push((time(p), std::cmp::Reverse(p)));
                }
            }
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An edge is the child's bottom segment into its lane, the lane straight down, the
    /// parent's top segment out of it — for first and other parents, across checkpoints.
    #[gitti_test::test]
    fn an_edge_runs_from_the_child_down_its_lane_into_the_parent() {
        // Rows: 0 merge(1, 70), 1..70 a chain on the first lane, 70 side, 71 base of both
        // (side's parent and the chain's end).
        let mut rows: Vec<(u32, Vec<u32>)> = vec![(0, vec![1, 70])];
        rows.extend((1..70).map(|r| (r, vec![r + 1])));
        rows[69].1 = vec![71];
        // Row 66 merges side too (after the checkpoint at 64): it joins row 0's lane.
        rows[66].1 = vec![67, 70];
        rows.push((70, vec![71]));
        rows.push((71, vec![]));
        let g = layout(rows.iter().map(|(k, p)| (*k, p.as_slice())));
        let edge = g.edge(0, 70, 1).expect("second parent");
        assert_ne!(edge.lane, g.node(0).unwrap());
        let painted = g.rows(0..72);
        for (r, row) in painted.iter().enumerate() {
            let hits = row.segments.iter().filter(|s| edge.has(r, row.node, s)).count();
            let want = match r {
                0 | 70 => 1,
                1..70 => 2,
                _ => 0,
            };
            assert_eq!(hits, want, "row {r}: {row:?}");
        }
        // The first parent: the node's own lane, one row.
        let first = g.edge(0, 1, 0).unwrap();
        assert_eq!(first.lane, g.node(0).unwrap());
        let hits = |r: usize| painted[r].segments.iter().filter(|s| first.has(r, painted[r].node, s)).count();
        assert_eq!((hits(0), hits(1), hits(2)), (1, 1, 0));
        // An extra parent after a checkpoint: row 66 joins the same lane.
        let late = g.edge(66, 70, 1).unwrap();
        assert_eq!(late.lane, edge.lane);
        assert!(painted[66].segments.iter().any(|s| late.has(66, painted[66].node, s)));
        assert_eq!(g.edge(0, 70, 2), None);
        assert_eq!(g.edge(5, 3, 0), None);
    }

    fn oid(n: u8) -> ObjectId {
        let mut b = [0u8; 20];
        b[19] = n;
        ObjectId::from_bytes_or_panic(&b)
    }

    /// The original (uncompressed) layout, as reference for the compact one.
    fn reference_layout<'a>(
        rows: impl IntoIterator<Item = (ObjectId, &'a [ObjectId])>,
    ) -> Vec<GraphRow> {
        let mut lanes: Vec<Option<(ObjectId, u8)>> = Vec::new();
        let mut next_color: u8 = 0;
        let mut alloc_color = || {
            let c = next_color;
            next_color = (next_color + 1) % PALETTE_SIZE;
            c
        };
        let mut out = Vec::new();

        for (id, parents) in rows {
            let mut row = GraphRow::default();

            // Where does this commit sit? The first lane waiting for it, else the first free lane.
            let hit = lanes.iter().position(|l| l.is_some_and(|(o, _)| o == id));
            let (node, color) = match hit {
                Some(i) => (i, lanes[i].expect("hit").1),
                None => {
                    let i = reference_free_slot(&mut lanes);
                    (i, alloc_color())
                }
            };
            row.node = node as u16;
            row.color = color;

            // Lanes that continue through this row (not ending in this commit).
            let through: Vec<bool> = lanes
                .iter()
                .map(|l| l.is_some_and(|(o, _)| o != id))
                .collect();

            // Top half: waiting lanes converge into the node, all others pass through.
            for (i, lane) in lanes.iter().enumerate() {
                if let Some((o, c)) = lane {
                    let to = if *o == id { node } else { i };
                    row.segments.push(Segment {
                        half: Half::Top,
                        from: i as u16,
                        to: to as u16,
                        color: *c,
                        pending: false,
                    });
                }
            }
            for lane in lanes.iter_mut() {
                if lane.is_some_and(|(o, _)| o == id) {
                    *lane = None;
                }
            }

            // Bottom half: first parent continues in the node lane; other parents join an
            // existing lane waiting for them or open a new one.
            if let Some(first) = parents.first() {
                lanes[node] = Some((*first, color));
            }
            for p in parents.iter().skip(1) {
                let target = match lanes.iter().position(|l| l.is_some_and(|(o, _)| o == *p)) {
                    Some(j) => j,
                    None => {
                        let j = reference_free_slot(&mut lanes);
                        lanes[j] = Some((*p, alloc_color()));
                        j
                    }
                };
                row.segments.push(Segment {
                    half: Half::Bottom,
                    from: node as u16,
                    to: target as u16,
                    color: lanes[target].expect("set").1,
                    pending: false,
                });
            }
            for (i, lane) in lanes.iter().enumerate() {
                if let Some((_, c)) = lane {
                    if i == node && !parents.is_empty() {
                        row.segments.push(Segment {
                            half: Half::Bottom,
                            from: node as u16,
                            to: node as u16,
                            color: *c,
                            pending: false,
                        });
                    } else if i != node && through.get(i).copied().unwrap_or(false) {
                        row.segments.push(Segment {
                            half: Half::Bottom,
                            from: i as u16,
                            to: i as u16,
                            color: *c,
                            pending: false,
                        });
                    }
                }
            }
            while lanes.last().is_some_and(|l| l.is_none()) {
                lanes.pop();
            }
            let max_seg = row
                .segments
                .iter()
                .map(|s| s.from.max(s.to))
                .max()
                .unwrap_or(0);
            row.width = max_seg.max(row.node) + 1;
            out.push(row);
        }
        out
    }

    fn reference_free_slot(lanes: &mut Vec<Option<(ObjectId, u8)>>) -> usize {
        match lanes.iter().position(Option::is_none) {
            Some(i) => i,
            None => {
                lanes.push(None);
                lanes.len() - 1
            }
        }
    }

    /// Random DAG in display order: row i's parents are rows > i.
    fn random_history(n: usize, seed: u64) -> Vec<(ObjectId, Vec<ObjectId>)> {
        let mut state = seed;
        let mut rnd = move |m: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % m as u64) as usize
        };
        let id = |i: usize| {
            let mut b = [0u8; 20];
            b[..8].copy_from_slice(&(i as u64 + 1).to_be_bytes());
            ObjectId::from_bytes_or_panic(&b)
        };
        (0..n)
            .map(|i| {
                let left = n - i - 1;
                let parents = if left == 0 {
                    vec![]
                } else {
                    let k = match rnd(20) {
                        0 => 0,
                        1..=3 => 2.min(left),
                        4 => 3.min(left),
                        _ => 1,
                    };
                    let mut ps: Vec<usize> = (0..k).map(|_| i + 1 + rnd(left.min(30))).collect();
                    ps.dedup();
                    ps.into_iter().map(id).collect()
                };
                (id(i), parents)
            })
            .collect()
    }

    #[gitti_test::test]
    fn compact_layout_matches_reference() {
        for seed in 1..40u64 {
            let h = random_history(700, seed * 7919);
            let items = || h.iter().map(|(i, p)| (*i, p.as_slice()));
            let reference = reference_layout(items());
            let g = layout(items());
            // Whole range, and ranges starting between checkpoints.
            for range in [0..h.len(), 100..160, 63..65, 650..700] {
                let rows = g.rows(range.clone());
                assert_eq!(rows.len(), range.len());
                for (r, row) in range.clone().zip(&rows) {
                    let want = &reference[r];
                    assert_eq!(
                        (row.node, row.color, row.width, &row.segments),
                        (want.node, want.color, want.width, &want.segments),
                        "seed {seed} row {r}"
                    );
                }
            }
        }
    }

    #[gitti_test::test]
    fn linear_history_stays_in_lane_zero() {
        let (a, b, c) = (oid(1), oid(2), oid(3));
        let rows = [(c, vec![b]), (b, vec![a]), (a, vec![])];
        let g = layout(rows.iter().map(|(i, p)| (*i, p.as_slice()))).rows(0..3);
        assert!(g.iter().all(|r| r.node == 0 && r.width == 1));
        assert!(g[2].segments.iter().all(|s| s.half == Half::Top));
    }

    #[gitti_test::test]
    fn merge_opens_and_closes_lane() {
        // m merges x (side) into b; both descend from a
        let (a, b, x, m) = (oid(1), oid(2), oid(3), oid(4));
        let rows = [(m, vec![b, x]), (x, vec![a]), (b, vec![a]), (a, vec![])];
        let g = layout(rows.iter().map(|(i, p)| (*i, p.as_slice()))).rows(0..4);
        assert_eq!(g[0].node, 0);
        assert!(g[0].segments.contains(&Segment {
            half: Half::Bottom,
            from: 0,
            to: 1,
            color: g[0].segments.iter().find(|s| s.to == 1).unwrap().color,
            pending: false,
        }));
        assert_eq!(g[1].node, 1);
        assert_eq!(g[2].node, 0);
        assert_eq!(g[3].node, 0);
        // both lanes converge into a
        let tops: Vec<_> = g[3]
            .segments
            .iter()
            .filter(|s| s.half == Half::Top)
            .map(|s| (s.from, s.to))
            .collect();
        assert_eq!(tops, vec![(0, 0), (1, 0)]);
    }

    /// `(half, from, to, pending)` of a row's segments, sorted.
    fn segs(row: &GraphRow) -> Vec<(Half, u16, u16, bool)> {
        let mut v: Vec<_> = row
            .segments
            .iter()
            .map(|s| (s.half, s.from, s.to, s.pending))
            .collect();
        v.sort_by_key(|&(h, f, t, p)| (h == Half::Bottom, f, t, p));
        v
    }

    #[gitti_test::test]
    fn pending_merge_edge_until_merge_head() {
        use Half::{Bottom as B, Top as T};
        // w = working directory merging m into h; both descend from a.
        let (a, h, m, w) = (oid(1), oid(2), oid(3), oid(4));
        let rows = [(w, vec![h, m]), (h, vec![a]), (m, vec![a]), (a, vec![])];
        let items = || rows.iter().map(|(i, p)| (*i, p.as_slice()));
        let g = layout_pending_merge(items()).rows(0..4);
        assert_eq!(segs(&g[0]), [(B, 0, 0, false), (B, 0, 1, true)]);
        assert_eq!(
            segs(&g[1]),
            [(T, 0, 0, false), (T, 1, 1, true), (B, 0, 0, false), (B, 1, 1, true)]
        );
        // MERGE_HEAD's commit: the dashed lane ends in it, its own parent edge is real.
        assert_eq!(g[2].node, 1);
        assert_eq!(
            segs(&g[2]),
            [(T, 0, 0, false), (T, 1, 1, true), (B, 0, 0, false), (B, 1, 1, false)]
        );
        assert!(g[3].segments.iter().all(|s| !s.pending));
        // The colour doesn't carry the mark.
        assert!(g.iter().all(|r| r.color < PALETTE_SIZE));
        assert!(g.iter().flat_map(|r| &r.segments).all(|s| s.color < PALETTE_SIZE));
        // Without the merge: the same geometry, nothing pending.
        let plain = layout(items()).rows(0..4);
        for (p, q) in plain.iter().zip(&g) {
            assert!(p.segments.iter().all(|s| !s.pending));
            let strip = |r: &GraphRow| -> Vec<_> { segs(r).into_iter().map(|s| (s.0, s.1, s.2)).collect() };
            assert_eq!((p.node, p.color, strip(p)), (q.node, q.color, strip(q)));
        }
    }

    #[gitti_test::test]
    fn pending_lane_across_checkpoints_and_octopus() {
        // w merges m1 and m2 (octopus) into a 100-commit first-parent chain h0..h99.
        let id = |n: u32| {
            let mut b = [0u8; 20];
            b[16..].copy_from_slice(&n.to_be_bytes());
            ObjectId::from_bytes_or_panic(&b)
        };
        let (w, m1, m2, root) = (id(1000), id(1001), id(1002), id(1003));
        let mut rows = vec![(w, vec![id(0), m1, m2])];
        for i in 0..100 {
            rows.push((id(i), vec![id(i + 1)]));
        }
        rows.pop();
        rows.push((id(99), vec![root]));
        rows.push((m1, vec![root]));
        rows.push((m2, vec![root]));
        rows.push((root, vec![]));
        let g = layout_pending_merge(rows.iter().map(|(i, p)| (*i, p.as_slice())));
        let first = &g.rows(0..1)[0];
        let out: Vec<_> = first.segments.iter().filter(|s| s.from != s.to).collect();
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|s| s.pending));
        // Replayed from a checkpoint: lanes 1 and 2 still dashed, lane 0 solid.
        for row in g.rows(70..72) {
            for s in &row.segments {
                assert_eq!(s.pending, s.from != 0, "{s:?}");
            }
        }
        let n = rows.len();
        assert!(g.rows(n - 1..n)[0].segments.iter().all(|s| !s.pending));
    }

    #[gitti_test::test]
    fn real_merge_of_merge_head_ends_the_dashes() {
        use Half::{Bottom as B, Top as T};
        // x (another branch) already merged m: below x the edge to m exists.
        let (a, h, m, x, w) = (oid(1), oid(2), oid(3), oid(4), oid(5));
        let rows = [
            (w, vec![h, m]),
            (x, vec![h, m]),
            (h, vec![a]),
            (m, vec![a]),
            (a, vec![]),
        ];
        let g = layout_pending_merge(rows.iter().map(|(i, p)| (*i, p.as_slice()))).rows(0..5);
        assert_eq!(g[1].node, 2);
        assert_eq!(
            segs(&g[1]),
            [
                (T, 0, 0, false),
                (T, 1, 1, true),
                (B, 0, 0, false),
                (B, 1, 1, false),
                (B, 2, 1, false),
                (B, 2, 2, false),
            ]
        );
        assert!(g[2..].iter().flat_map(|r| &r.segments).all(|s| !s.pending));
    }

    #[gitti_test::test]
    fn topo_sort_children_first() {
        let (a, b, x, m) = (oid(1), oid(2), oid(3), oid(4));
        // (id, parents, time) deliberately shuffled, with a skewed clock on x
        let items = vec![
            (a, vec![], 1i64),
            (x, vec![a], 10),
            (m, vec![b, x], 5),
            (b, vec![a], 3),
        ];
        for topo in [true, false] {
            let sorted = sort_commits(items.clone(), |c| c.0, |c| c.1.as_slice(), |c| c.2, topo);
            let pos = |o| sorted.iter().position(|c| c.0 == o).unwrap();
            assert!(pos(m) < pos(x) && pos(m) < pos(b) && pos(x) < pos(a) && pos(b) < pos(a));
        }
    }
}
