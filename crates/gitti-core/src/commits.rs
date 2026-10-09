//! The history's commits stored compactly: arrays per field instead of a struct
//! with its own parent `Vec` per row, so a history of linux.git (~1.4M commits) costs
//! tens of bytes per commit instead of hundreds.
//!
//! - ids back to back in one buffer, 20 (SHA-1) or 32 (SHA-256) bytes each;
//! - parents as row numbers in one flat array with offsets per row; a parent that is not
//!   loaded (beyond the quick history's newest commits, a missing object) is an
//!   [`EXTERNAL`] key into a buffer of such ids;
//! - commit times as `u32` seconds after the oldest one (`i64` when the span needs it);
//! - stash rows as a short sorted list;
//! - the id → row lookup as an open-addressing table of row numbers that compares the
//!   ids in the buffer (no ids of its own).
//!
//! Row 0 is always the working-directory row (null id); its parents (HEAD and, during a
//! merge, the `MERGE_HEAD` commits) belong to each [`Commits`], the rest is a [`Table`]
//! shared by the histories updated from one another ([`crate::HistoryLoad::SameCommits`]).

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use gix::ObjectId;
use gix::hash::Kind;

use crate::graph;

/// Parent key bit: not a row but the `key & !EXTERNAL`th id of [`Table::external`] (or,
/// past those, of [`Commits::wt_external`]).
pub(crate) const EXTERNAL: u32 = 1 << 31;

/// Builder key bit: a row of the previous table (a prepend update).
const PREV: u32 = 1 << 30;

/// An empty slot of [`IdIndex`].
const EMPTY: u32 = u32::MAX;

/// [`Table::marks`] value of a stash helper row ("index on …", "untracked files on …").
const HELPER: u32 = u32::MAX;

fn id_hash(id: &[u8]) -> u64 {
    let n = id.len();
    let a = u64::from_le_bytes(id[..8].try_into().expect("8 bytes"));
    let b = u64::from_le_bytes(id[n - 8..].try_into().expect("8 bytes"));
    (a ^ b.rotate_left(29)).wrapping_mul(0x9e37_79b9_7f4a_7c15)
}

/// id → row: linear probing over row numbers; the ids are those of the rows in the
/// buffer the index belongs to.
#[derive(Debug, Clone, Default)]
struct IdIndex {
    /// Power-of-two length (or empty); [`EMPTY`] or a row.
    slots: Vec<u32>,
    len: usize,
}

impl IdIndex {
    fn start(&self, id: &[u8]) -> usize {
        let bits = self.slots.len().trailing_zeros();
        (id_hash(id) >> (64 - bits)) as usize
    }

    /// `Ok(row)` of `id`, or `Err(free slot)` where it would go.
    fn find(&self, ids: &[u8], hl: usize, id: &[u8]) -> Result<u32, usize> {
        if self.slots.is_empty() || id.len() != hl {
            return Err(usize::MAX);
        }
        let mask = self.slots.len() - 1;
        let mut i = self.start(id);
        loop {
            let s = self.slots[i];
            if s == EMPTY {
                return Err(i);
            }
            let r = s as usize * hl;
            if &ids[r..r + hl] == id {
                return Ok(s);
            }
            i = (i + 1) & mask;
        }
    }

    fn get(&self, ids: &[u8], hl: usize, id: &[u8]) -> Option<u32> {
        self.find(ids, hl, id).ok()
    }

    /// Adds `row` (its id is in `ids`); false when the id is there already.
    fn insert(&mut self, ids: &[u8], hl: usize, row: u32) -> bool {
        if (self.len + 1) * 4 > self.slots.len() * 3 {
            self.grow(ids, hl);
        }
        let r = row as usize * hl;
        match self.find(ids, hl, &ids[r..r + hl]) {
            Ok(_) => false,
            Err(i) => {
                self.slots[i] = row;
                self.len += 1;
                true
            }
        }
    }

    fn grow(&mut self, ids: &[u8], hl: usize) {
        let size = (self.slots.len() * 2).max(16);
        let old = std::mem::replace(&mut self.slots, vec![EMPTY; size]);
        let mask = self.slots.len() - 1;
        for row in old.into_iter().filter(|&s| s != EMPTY) {
            let r = row as usize * hl;
            let mut i = self.start(&ids[r..r + hl]);
            while self.slots[i] != EMPTY {
                i = (i + 1) & mask;
            }
            self.slots[i] = row;
        }
    }

    /// Every row number mapped through `f` (the ids moved with their rows).
    fn remap(&mut self, f: impl Fn(u32) -> u32) {
        for s in self.slots.iter_mut().filter(|s| **s != EMPTY) {
            *s = f(*s);
        }
    }
}

/// Commit times: `u32` offsets from the oldest when they fit (always, in practice).
#[derive(Debug, Clone)]
enum Times {
    Narrow { base: i64, offsets: Vec<u32> },
    Wide(Vec<i64>),
}

impl Default for Times {
    fn default() -> Self {
        Times::Narrow {
            base: 0,
            offsets: Vec::new(),
        }
    }
}

impl Times {
    /// The times of `n` rows from `time(row)` (called twice per row); row 0 (the
    /// working-directory row) is not asked.
    fn from_fn(n: usize, time: impl Fn(usize) -> i64) -> Times {
        let (lo, hi) = (1..n).map(&time).fold((i64::MAX, i64::MIN), |(lo, hi), t| {
            (lo.min(t), hi.max(t))
        });
        if n <= 1 {
            return Times::Narrow {
                base: 0,
                offsets: vec![0; n],
            };
        }
        if (hi as i128 - lo as i128) <= u32::MAX as i128 {
            let offsets = (0..n)
                .map(|r| if r == 0 { 0 } else { (time(r) - lo) as u32 })
                .collect();
            Times::Narrow { base: lo, offsets }
        } else {
            Times::Wide((0..n).map(|r| if r == 0 { 0 } else { time(r) }).collect())
        }
    }

    fn get(&self, row: usize) -> i64 {
        match self {
            Times::Narrow { base, offsets } => base + offsets[row] as i64,
            Times::Wide(t) => t[row],
        }
    }

    fn heap_bytes(&self) -> usize {
        match self {
            Times::Narrow { offsets, .. } => offsets.capacity() * 4,
            Times::Wide(t) => t.capacity() * 8,
        }
    }
}

/// The commits of a history without the working-directory row's parents; immutable,
/// shared between histories with the same commits.
#[derive(Debug, Clone)]
pub(crate) struct Table {
    kind: Kind,
    /// Bytes per id (20 or 32).
    hl: usize,
    /// Row `r`'s id is `ids[r * hl..][..hl]`; row 0's is null.
    ids: Vec<u8>,
    /// Row 0's is a placeholder ([`Commits::commit_time`] gives `i64::MAX`).
    times: Times,
    /// Row `r`'s parents are `parents[pstart[r]..pstart[r + 1]]`.
    pstart: Vec<u32>,
    /// Row numbers, or [`EXTERNAL`] keys.
    parents: Vec<u32>,
    /// Ids of parents that are not rows, `hl` bytes each.
    external: Vec<u8>,
    /// `(row, n)` of `stash@{n}` rows and `(row, HELPER)` of stash helpers, by row.
    marks: Vec<(u32, u32)>,
    index: IdIndex,
}

impl Default for Table {
    fn default() -> Self {
        Table {
            kind: Kind::Sha1,
            hl: Kind::Sha1.len_in_bytes(),
            ids: Vec::new(),
            times: Times::default(),
            pstart: Vec::new(),
            parents: Vec::new(),
            external: Vec::new(),
            marks: Vec::new(),
            index: IdIndex::default(),
        }
    }
}

impl Table {
    fn len(&self) -> usize {
        self.ids.len() / self.hl
    }

    fn id_bytes(&self, row: usize) -> &[u8] {
        &self.ids[row * self.hl..(row + 1) * self.hl]
    }

    fn keys(&self, row: usize) -> &[u32] {
        match (self.pstart.get(row), self.pstart.get(row + 1)) {
            (Some(&a), Some(&b)) => &self.parents[a as usize..b as usize],
            _ => &[],
        }
    }

    fn external_count(&self) -> u32 {
        (self.external.len() / self.hl) as u32
    }

    fn row_of(&self, id: &[u8]) -> Option<usize> {
        self.index.get(&self.ids, self.hl, id).map(|r| r as usize)
    }

    fn mark(&self, row: usize) -> Option<u32> {
        self.marks
            .binary_search_by_key(&(row as u32), |&(r, _)| r)
            .ok()
            .map(|i| self.marks[i].1)
    }

    fn heap_bytes(&self) -> usize {
        self.ids.capacity()
            + self.times.heap_bytes()
            + self.pstart.capacity() * 4
            + self.parents.capacity() * 4
            + self.external.capacity()
            + self.marks.capacity() * 8
            + self.index.slots.capacity() * 4
    }
}

/// Collects commits in any order (a walk), then sorts them into a [`Table`]
/// ([`Builder::assemble`]).
pub(crate) struct Builder {
    kind: Kind,
    hl: usize,
    ids: Vec<u8>,
    times: Vec<i64>,
    pstart: Vec<u32>,
    /// Parent ids, `hl` bytes each, resolved to rows in [`Builder::assemble`].
    pids: Vec<u8>,
    marks: Vec<(u32, u32)>,
    index: IdIndex,
}

impl Builder {
    pub(crate) fn new(kind: Kind) -> Builder {
        Builder {
            kind,
            hl: kind.len_in_bytes(),
            ids: Vec::new(),
            times: Vec::new(),
            pstart: vec![0],
            pids: Vec::new(),
            marks: Vec::new(),
            index: IdIndex::default(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.times.len()
    }

    pub(crate) fn contains(&self, id: &gix::oid) -> bool {
        self.index.get(&self.ids, self.hl, id.as_bytes()).is_some()
    }

    /// Adds a commit; false (and nothing added) when it is there already or its id is of
    /// another hash kind.
    pub(crate) fn push<P: AsRef<gix::oid>>(
        &mut self,
        id: &gix::oid,
        parents: impl IntoIterator<Item = P>,
        time: i64,
    ) -> bool {
        if id.kind() != self.kind {
            return false;
        }
        let row = self.len() as u32;
        self.ids.extend_from_slice(id.as_bytes());
        if !self.index.insert(&self.ids, self.hl, row) {
            self.ids.truncate(row as usize * self.hl);
            return false;
        }
        for p in parents {
            let p = p.as_ref();
            if p.kind() == self.kind {
                self.pids.extend_from_slice(p.as_bytes());
            }
        }
        self.times.push(time);
        self.pstart.push((self.pids.len() / self.hl) as u32);
        true
    }

    /// Like [`Builder::push`], for a `stash@{n}` row (`Some(n)`) or a stash helper.
    pub(crate) fn push_stash(
        &mut self,
        id: &gix::oid,
        parents: &[ObjectId],
        time: i64,
        stash: Option<usize>,
    ) {
        let row = self.len() as u32;
        if self.push(id, parents, time) {
            self.marks.push((row, stash.map_or(HELPER, |n| n as u32)));
        }
    }

    /// The parents of the last commit pushed.
    pub(crate) fn last_parents(&self) -> impl Iterator<Item = &gix::oid> {
        let n = self.pstart.len();
        let (a, b) = (self.pstart[n.saturating_sub(2)], self.pstart[n - 1]);
        self.pids[a as usize * self.hl..b as usize * self.hl]
            .chunks(self.hl)
            .map(gix::oid::from_bytes_unchecked)
    }

    /// Sorts the commits (`topo` or date order) into a table, on top of `prev`'s rows when
    /// given (a prepend update: the new commits all sort above them). Row 0 is the
    /// working-directory row.
    pub(crate) fn assemble(self, prev: Option<&Table>, topo: bool) -> Arc<Table> {
        let Builder {
            kind,
            hl,
            ids,
            times,
            pstart,
            pids,
            marks,
            mut index,
        } = self;
        let nf = times.len();
        if let Some(prev) = prev
            && prev.hl != hl
        {
            // Another hash kind than the previous rows: those can't be kept.
            return Builder {
                kind,
                hl,
                ids,
                times,
                pstart,
                pids,
                marks,
                index,
            }
            .assemble(None, topo);
        }
        let prev_rows = prev.map_or(0, |p| p.len().saturating_sub(1));
        // Parent ids → keys: a new commit (its builder index), a previous row (PREV), or
        // not loaded (EXTERNAL).
        let mut external: Vec<u8> = prev.map(|p| p.external.clone()).unwrap_or_default();
        let mut new_external: HashMap<&[u8], u32> = HashMap::new();
        let mut keys: Vec<u32> = Vec::with_capacity(pids.len() / hl);
        for p in pids.chunks(hl) {
            let key = if let Some(i) = index.get(&ids, hl, p) {
                i
            } else if let Some(r) = prev.and_then(|t| t.row_of(p)) {
                PREV | r as u32
            } else {
                let next = (external.len() / hl) as u32;
                *new_external.entry(p).or_insert_with(|| {
                    external.extend_from_slice(p);
                    next
                }) | EXTERNAL
            };
            keys.push(key);
        }
        drop(new_external);
        drop(pids);
        let fresh_parents = |i: usize| &keys[pstart[i] as usize..pstart[i + 1] as usize];

        // Display order of the new commits (their parents among them only).
        let mut order = graph::sort_order(
            nf,
            |i| {
                fresh_parents(i)
                    .iter()
                    .filter(|&&k| k & (PREV | EXTERNAL) == 0)
                    .map(|&k| k as usize)
            },
            |i| times[i],
            topo,
        );
        if order.len() < nf {
            // A cycle (not in a real repository): keep its commits, at the end.
            let mut placed = vec![false; nf];
            for &i in &order {
                placed[i as usize] = true;
            }
            order.extend((0..nf as u32).filter(|&i| !placed[i as usize]));
        }
        let mut new_row = vec![0u32; nf];
        for (pos, &i) in order.iter().enumerate() {
            new_row[i as usize] = pos as u32 + 1;
        }
        let shift = nf as u32;
        let map_key = |k: u32| {
            if k & EXTERNAL != 0 {
                k
            } else if k & PREV != 0 {
                (k & !PREV) + shift
            } else {
                new_row[k as usize]
            }
        };

        let total = 1 + nf + prev_rows;
        let mut out_ids = Vec::with_capacity(total * hl);
        out_ids.resize(hl, 0);
        for &i in &order {
            out_ids.extend_from_slice(&ids[i as usize * hl..(i as usize + 1) * hl]);
        }
        if let Some(p) = prev {
            out_ids.extend_from_slice(&p.ids[hl.min(p.ids.len())..]);
        }
        drop(ids);
        let out_times = Times::from_fn(total, |r| match r {
            0 => 0,
            r if r <= nf => times[order[r - 1] as usize],
            r => prev.map_or(0, |p| p.times.get(r - nf)),
        });

        let prev_links = prev.map_or(0, |p| p.parents.len());
        let mut out_pstart = Vec::with_capacity(total + 1);
        let mut out_parents = Vec::with_capacity(keys.len() + prev_links);
        out_pstart.extend([0, 0]);
        for &i in &order {
            out_parents.extend(fresh_parents(i as usize).iter().map(|&k| map_key(k)));
            out_pstart.push(out_parents.len() as u32);
        }
        if let Some(p) = prev {
            for r in 1..p.len() {
                out_parents.extend(
                    p.keys(r)
                        .iter()
                        .map(|&k| if k & EXTERNAL != 0 { k } else { k + shift }),
                );
                out_pstart.push(out_parents.len() as u32);
            }
        }

        let mut out_marks: Vec<(u32, u32)> = marks
            .iter()
            .map(|&(i, m)| (new_row[i as usize], m))
            .chain(prev.into_iter().flat_map(|p| p.marks.iter().map(|&(r, m)| (r + shift, m))))
            .collect();
        out_marks.sort_unstable();

        // The index: the builder's rows renumbered (the ids moved with them); on top of
        // previous rows, theirs shifted down plus the new ones.
        let index = match prev {
            None => {
                index.remap(|i| new_row[i as usize]);
                index
            }
            Some(p) => {
                drop(index);
                let mut index = p.index.clone();
                index.remap(|r| r + shift);
                for row in 1..=nf {
                    index.insert(&out_ids, hl, row as u32);
                }
                index
            }
        };
        out_ids.shrink_to_fit();
        out_parents.shrink_to_fit();
        external.shrink_to_fit();
        Arc::new(Table {
            kind,
            hl,
            ids: out_ids,
            times: out_times,
            pstart: out_pstart,
            parents: out_parents,
            external,
            marks: out_marks,
            index,
        })
    }
}

/// The rows of a [`crate::History`]: row 0 is the working-directory row (null id; HEAD
/// and the pending merge's `MERGE_HEAD` commits as parents), then the commits in display
/// order.
#[derive(Debug, Clone, Default)]
pub struct Commits {
    table: Arc<Table>,
    /// Keys of row 0's parents.
    wt: Box<[u32]>,
    /// Row 0's parents that are not rows (keys from [`Table::external`]'s end on).
    wt_external: Box<[ObjectId]>,
}

impl Commits {
    /// The rows of `table` with HEAD and the `MERGE_HEAD` commits as row 0's parents;
    /// also the merge heads that are loaded, other than HEAD, once each (the pending
    /// merge's parents after HEAD).
    pub(crate) fn new(
        table: Arc<Table>,
        head: Option<ObjectId>,
        merge_heads: &[ObjectId],
    ) -> (Commits, Vec<ObjectId>) {
        let mut kept: Vec<ObjectId> = Vec::new();
        for m in merge_heads {
            if head.is_some()
                && Some(*m) != head
                && table.row_of(m.as_bytes()).is_some()
                && !kept.contains(m)
            {
                kept.push(*m);
            }
        }
        let mut wt = Vec::new();
        let mut wt_external = Vec::new();
        for id in head.iter().chain(&kept) {
            match table.row_of(id.as_bytes()) {
                Some(r) => wt.push(r as u32),
                None => {
                    wt.push(EXTERNAL | (table.external_count() + wt_external.len() as u32));
                    wt_external.push(*id);
                }
            }
        }
        let commits = Commits {
            table,
            wt: wt.into(),
            wt_external: wt_external.into(),
        };
        (commits, kept)
    }

    pub(crate) fn table(&self) -> &Arc<Table> {
        &self.table
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The hash kind of the ids (SHA-1 or SHA-256).
    pub fn hash_kind(&self) -> Kind {
        self.table.kind
    }

    /// Row `row`, if there is one.
    pub fn get(&self, row: usize) -> Option<CommitInfo<'_>> {
        (row < self.len()).then(|| CommitInfo {
            id: self.id_at(row),
            parents: Parents {
                commits: self,
                keys: self.keys(row),
            },
            commit_time: self.commit_time(row),
            stash: self.table.mark(row).filter(|&m| m != HELPER).map(|m| m as usize),
            stash_helper: self.table.mark(row) == Some(HELPER),
        })
    }

    /// Row `row`; panics when there is none.
    pub fn at(&self, row: usize) -> CommitInfo<'_> {
        self.get(row)
            .unwrap_or_else(|| panic!("row {row} of {} commits", self.len()))
    }

    /// All rows, from row 0 on.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = CommitInfo<'_>> + ExactSizeIterator + '_ {
        (0..self.len()).map(|r| self.at(r))
    }

    /// The id of `row`.
    pub fn id(&self, row: usize) -> Option<ObjectId> {
        (row < self.len()).then(|| self.id_at(row))
    }

    fn id_at(&self, row: usize) -> ObjectId {
        gix::oid::from_bytes_unchecked(self.table.id_bytes(row)).to_owned()
    }

    /// Committer time of `row` (`i64::MAX` for the working-directory row).
    pub fn commit_time(&self, row: usize) -> i64 {
        if row == 0 {
            i64::MAX
        } else {
            self.table.times.get(row)
        }
    }

    /// The row of commit `id` (never the working-directory row).
    pub fn row_of(&self, id: &gix::oid) -> Option<usize> {
        self.table.row_of(id.as_bytes())
    }

    /// Parent keys of `row`: row numbers, or [`EXTERNAL`] for parents that aren't rows.
    pub(crate) fn keys(&self, row: usize) -> &[u32] {
        if row == 0 { &self.wt } else { self.table.keys(row) }
    }

    /// The commit a parent key stands for.
    pub(crate) fn key_id(&self, key: u32) -> ObjectId {
        if key & EXTERNAL == 0 {
            return self.id_at(key as usize);
        }
        let j = key & !EXTERNAL;
        let n = self.table.external_count();
        if j < n {
            let hl = self.table.hl;
            let j = j as usize;
            gix::oid::from_bytes_unchecked(&self.table.external[j * hl..(j + 1) * hl]).to_owned()
        } else {
            self.wt_external[(j - n) as usize]
        }
    }

    /// Bytes on the heap: the arrays (their capacity), the shared table counted in full.
    pub fn heap_bytes(&self) -> usize {
        self.table.heap_bytes() + self.wt.len() * 4 + self.wt_external.len() * std::mem::size_of::<ObjectId>()
    }
}

/// One row of [`Commits`] (a view; the data lives in the arrays).
#[derive(Debug, Clone, Copy)]
pub struct CommitInfo<'a> {
    pub id: ObjectId,
    pub parents: Parents<'a>,
    /// Committer time, used for ordering.
    pub commit_time: i64,
    /// `Some(n)` for `stash@{n}` rows.
    pub stash: Option<usize>,
    /// Stash helper commit ("index on …" / "untracked files on …").
    pub stash_helper: bool,
}

/// The parents of a row, in order (first parent first), loaded or not.
#[derive(Clone, Copy)]
pub struct Parents<'a> {
    commits: &'a Commits,
    keys: &'a [u32],
}

impl<'a> Parents<'a> {
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = ObjectId> + ExactSizeIterator + 'a {
        let (commits, keys) = (self.commits, self.keys);
        keys.iter().map(move |&k| commits.key_id(k))
    }

    pub fn first(&self) -> Option<ObjectId> {
        self.keys.first().map(|&k| self.commits.key_id(k))
    }

    pub fn get(&self, i: usize) -> Option<ObjectId> {
        self.keys.get(i).map(|&k| self.commits.key_id(k))
    }

    pub fn contains(&self, id: &ObjectId) -> bool {
        self.iter().any(|p| p == *id)
    }

    /// The rows of the parents that are loaded, in parent order.
    pub fn rows(&self) -> impl Iterator<Item = usize> + 'a {
        self.keys
            .iter()
            .filter(|&&k| k & EXTERNAL == 0)
            .map(|&k| k as usize)
    }

    pub fn to_vec(&self) -> Vec<ObjectId> {
        self.iter().collect()
    }
}

impl<'a> IntoIterator for Parents<'a> {
    type Item = ObjectId;
    type IntoIter = Box<dyn DoubleEndedIterator<Item = ObjectId> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}

impl<'a> IntoIterator for &Parents<'a> {
    type Item = ObjectId;
    type IntoIter = Box<dyn DoubleEndedIterator<Item = ObjectId> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}

impl fmt::Debug for Parents<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl PartialEq for Parents<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl PartialEq<[ObjectId]> for Parents<'_> {
    fn eq(&self, other: &[ObjectId]) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter().copied())
    }
}

impl<const N: usize> PartialEq<[ObjectId; N]> for Parents<'_> {
    fn eq(&self, other: &[ObjectId; N]) -> bool {
        *self == other[..]
    }
}

impl PartialEq<Vec<ObjectId>> for Parents<'_> {
    fn eq(&self, other: &Vec<ObjectId>) -> bool {
        *self == other[..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(kind: Kind, n: u32) -> ObjectId {
        let mut b = vec![0u8; kind.len_in_bytes()];
        // Ids that differ in a few bytes only (worst case for a weak hash).
        b[..4].copy_from_slice(&n.to_be_bytes());
        ObjectId::from_bytes_or_panic(&b)
    }

    /// A chain c0 ← c1 ← … with a merge of a side commit, built in walk order.
    fn build(kind: Kind, n: u32) -> (Arc<Table>, Vec<ObjectId>) {
        let ids: Vec<ObjectId> = (1..=n).map(|i| id(kind, i)).collect();
        let side = id(kind, 1_000_000);
        let missing = id(kind, 2_000_000);
        let mut b = Builder::new(kind);
        // Newest first (a walk from the tip); the side branch hangs on c1 and is merged
        // by the tip; the root's parent is not loaded.
        for i in (0..n as usize).rev() {
            let mut parents = vec![if i == 0 { missing } else { ids[i - 1] }];
            if i + 1 == n as usize {
                parents.push(side);
            }
            assert!(b.push(&ids[i], &parents, 1000 + i as i64));
            assert!(!b.push(&ids[i], &parents, 0), "a commit is added once");
        }
        b.push(&side, [ids[1]], 1001);
        b.push_stash(&id(kind, 3_000_000), &[ids[n as usize - 1]], 5000, Some(0));
        (b.assemble(None, true), ids)
    }

    #[gitti_test::test]
    fn compact_rows_of_both_hash_kinds() {
        for kind in [Kind::Sha1, Kind::Sha256] {
            let (table, ids) = build(kind, 5000);
            let head = ids[4999];
            let (c, kept) = Commits::new(table, Some(head), &[]);
            assert!(kept.is_empty());
            assert_eq!(c.len(), 5000 + 3, "worktree, commits, side, stash");
            assert_eq!(c.hash_kind(), kind);
            assert!(c.at(0).id.is_null() && c.at(0).id.kind() == kind);
            assert_eq!(c.at(0).parents, [head]);
            assert_eq!(c.at(0).commit_time, i64::MAX);
            // Every id found at its row, children before parents.
            for row in 1..c.len() {
                let info = c.at(row);
                assert_eq!(c.row_of(&info.id), Some(row));
                for p in info.parents.rows() {
                    assert!(p > row, "parent below child");
                }
            }
            assert_eq!(c.row_of(&id(kind, 777_777)), None);
            assert_eq!(c.row_of(&ObjectId::null(kind)), None, "not the worktree row");
            let stash = c.iter().position(|r| r.stash == Some(0)).unwrap();
            assert_eq!(c.at(stash).commit_time, 5000);
            let tip = c.row_of(&head).unwrap();
            let t = c.at(tip);
            assert_eq!(t.parents.len(), 2);
            assert_eq!(t.parents.first(), Some(ids[4998]));
            assert_eq!(t.parents.get(1), Some(id(kind, 1_000_000)));
            assert_eq!(t.commit_time, 1000 + 4999);
            // The root's parent isn't loaded: kept as an id, not a row.
            let root = c.row_of(&ids[0]).unwrap();
            assert_eq!(c.at(root).parents, [id(kind, 2_000_000)]);
            assert_eq!(c.at(root).parents.rows().count(), 0);
            // Compact: well under 100 bytes per commit.
            let per = c.heap_bytes() / c.len();
            assert!(per < 60, "{kind:?}: {per} bytes per commit");
        }
    }

    #[gitti_test::test]
    fn prepend_onto_previous_rows() {
        let kind = Kind::Sha1;
        let (table, ids) = build(kind, 300);
        let mut b = Builder::new(kind);
        let (n1, n2) = (id(kind, 9001), id(kind, 9002));
        b.push(&n2, [n1], 9002);
        b.push(&n1, [ids[299]], 9001);
        let merged = b.assemble(Some(&table), true);
        let (c, _) = Commits::new(merged, Some(n2), &[]);
        assert_eq!(c.len(), table.len() + 2);
        assert_eq!((c.at(1).id, c.at(2).id), (n2, n1));
        assert_eq!(c.at(2).parents, [ids[299]]);
        let below = table.row_of(ids[299].as_bytes()).unwrap() + 2;
        assert_eq!(c.at(2).parents.rows().collect::<Vec<_>>(), [below]);
        for row in 1..c.len() {
            assert_eq!(c.row_of(&c.at(row).id), Some(row));
        }
        let (old, _) = Commits::new(table.clone(), Some(ids[299]), &[]);
        for row in 1..old.len() {
            let (a, b) = (old.at(row), c.at(row + 2));
            assert_eq!((a.id, a.parents.to_vec(), a.commit_time, a.stash), (b.id, b.parents.to_vec(), b.commit_time, b.stash));
        }
    }

    #[gitti_test::test]
    fn wide_times_and_unloaded_head() {
        let kind = Kind::Sha1;
        let mut b = Builder::new(kind);
        let (a, c) = (id(kind, 1), id(kind, 2));
        b.push(&c, [a], i64::MAX / 2);
        b.push(&a, std::iter::empty::<ObjectId>(), -5);
        let (commits, _) = Commits::new(b.assemble(None, false), Some(id(kind, 99)), &[c, c, a]);
        assert_eq!(commits.at(1).commit_time, i64::MAX / 2);
        assert_eq!(commits.at(2).commit_time, -5);
        assert_eq!(commits.at(0).parents, [id(kind, 99), c, a]);
        assert_eq!(commits.at(0).parents.rows().collect::<Vec<_>>(), [1, 2]);
    }
}
