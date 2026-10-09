//! Unusual repository shapes and histories: octopus / criss-cross merges, orphan
//! branches, random DAGs, commit-graph files (fresh, stale, split, corrupt), shallow and
//! partial clones, replace refs, odd commit metadata, odd ref names, detached / unborn
//! HEAD, stashes, repository discovery (`.git` files, submodules, bare repos, `GIT_DIR`
//! in the environment) and blob reads. Fixture repositories are built with the git CLI
//! (`git fast-import` for big histories), so these run on every OS.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gitti_core::blobs::{MAX_BLOB_BYTES, Version};
use gitti_core::graph::{self, GraphRow, Half};
use gitti_core::patch::{PatchMode, Selection};
use gitti_core::{History, LogOrder, ObjectId, RefKind, Repo};

// ------------------------------------------------------------------ fixtures

fn git_cmd(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir)
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "T")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "T")
        .env("GIT_COMMITTER_EMAIL", "t@example.com");
    cmd
}

fn sh_env(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> String {
    let out = git_cmd(dir, args)
        .envs(env.iter().copied())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn sh(dir: &Path, args: &[&str]) -> String {
    sh_env(dir, args, &[])
}

fn sh_stdin(dir: &Path, args: &[&str], input: &[u8]) -> String {
    let mut child = git_cmd(dir, args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn oid(hex: &str) -> ObjectId {
    ObjectId::from_hex(hex.trim().as_bytes()).unwrap()
}

fn rev(dir: &Path, r: &str) -> ObjectId {
    oid(&sh(dir, &["rev-parse", r]))
}

/// `git init` in `<tmp>/<name>` with a local identity and stable settings.
fn init_in(root: &Path, name: &str, extra: &[&str]) -> PathBuf {
    let p = root.join(name);
    fs::create_dir_all(&p).unwrap();
    let mut args = vec!["init", "-q", "-b", "main"];
    args.extend_from_slice(extra);
    sh(&p, &args);
    sh(&p, &["config", "user.name", "T"]);
    sh(&p, &["config", "user.email", "t@example.com"]);
    sh(&p, &["config", "core.autocrlf", "false"]);
    // Our fixtures decide themselves when a commit-graph exists.
    sh(&p, &["config", "gc.auto", "0"]);
    sh(&p, &["config", "fetch.writeCommitGraph", "false"]);
    p
}

fn new_repo(extra: &[&str]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let p = init_in(dir.path(), "repo", extra);
    (dir, p)
}

/// One commit of a synthetic history: parents are indices of earlier commits.
#[derive(Clone, Debug)]
struct Spec {
    parents: Vec<usize>,
    time: i64,
}

/// A chain of `n` commits, one second apart from `t0`.
fn linear(n: usize, t0: i64) -> Vec<Spec> {
    (0..n)
        .map(|i| Spec {
            parents: if i == 0 { Vec::new() } else { vec![i - 1] },
            time: t0 + i as i64,
        })
        .collect()
}

fn dates(s: &str) -> [(&'static str, &str); 2] {
    [("GIT_AUTHOR_DATE", s), ("GIT_COMMITTER_DATE", s)]
}

fn spec(parents: &[usize], time: i64) -> Spec {
    Spec {
        parents: parents.to_vec(),
        time,
    }
}

/// Writes `specs` (parents first) with one `git fast-import` and points `branches` at
/// them; returns the commit ids in spec order. Every commit changes `f<i>.txt` so no two
/// commits collide.
fn import(dir: &Path, specs: &[Spec], branches: &[(&str, usize)]) -> Vec<ObjectId> {
    let mut s = String::new();
    for (i, c) in specs.iter().enumerate() {
        assert!(c.parents.iter().all(|&p| p < i), "parents first");
        let msg = format!("commit {i}\n");
        s.push_str("reset refs/gitti-import/tmp\ncommit refs/gitti-import/tmp\n");
        s.push_str(&format!("mark :{}\n", i + 1));
        s.push_str(&format!("author T <t@example.com> {} +0000\n", c.time));
        s.push_str(&format!("committer T <t@example.com> {} +0000\n", c.time));
        s.push_str(&format!("data {}\n{msg}", msg.len()));
        if let Some(first) = c.parents.first() {
            s.push_str(&format!("from :{}\n", first + 1));
        }
        for p in c.parents.iter().skip(1) {
            s.push_str(&format!("merge :{}\n", p + 1));
        }
        let body = format!("{i}\n");
        s.push_str(&format!(
            "M 644 inline f{i}.txt\ndata {}\n{body}\n",
            body.len()
        ));
    }
    for (name, i) in branches {
        s.push_str(&format!("reset refs/heads/{name}\nfrom :{}\n\n", i + 1));
    }
    let marks = dir.join(".git").join("gitti-marks");
    let marks_arg = format!("--export-marks={}", marks.display());
    sh_stdin(dir, &["fast-import", "--quiet", &marks_arg], s.as_bytes());
    sh(dir, &["update-ref", "-d", "refs/gitti-import/tmp"]);
    let text = fs::read_to_string(&marks).unwrap();
    fs::remove_file(&marks).unwrap();
    let mut ids = vec![None; specs.len()];
    for line in text.lines() {
        let (mark, hex) = line.split_once(' ').unwrap();
        let n: usize = mark.trim_start_matches(':').parse().unwrap();
        ids[n - 1] = Some(oid(hex));
    }
    // Check out `main` if there is one (the working tree matches HEAD).
    if branches.iter().any(|(n, _)| *n == "main") {
        sh(dir, &["reset", "-q", "--hard", "main"]);
    }
    ids.into_iter().map(Option::unwrap).collect()
}

/// Tiny deterministic PRNG (xorshift64).
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// A random DAG: mostly linear chains, merges (2–5 parents, incl. criss-crosses), a few
/// new roots, clock skew and identical timestamps. Branches on every commit without a
/// child; `main` on the last one.
fn random_dag(n: usize, seed: u64) -> (Vec<Spec>, Vec<(String, usize)>) {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut specs: Vec<Spec> = Vec::with_capacity(n);
    for i in 0..n {
        let parents: Vec<usize> = if i == 0 {
            Vec::new()
        } else {
            let k = match rng.below(100) {
                0..=2 => 0,
                3..=74 => 1,
                75..=91 => 2,
                92..=96 => 3,
                97..=98 => 4,
                _ => 5,
            };
            let mut ps = Vec::new();
            for _ in 0..k {
                let window = i.min(40);
                let p = i - 1 - rng.below(window);
                if !ps.contains(&p) {
                    ps.push(p);
                }
            }
            ps
        };
        let base = 1_600_000_000 + i as i64 * 60;
        let time = match rng.below(20) {
            0 => base - 3600 * 24,           // clock skew: older than its parents
            1 if i > 0 => specs[i - 1].time, // identical timestamp
            _ => base,
        };
        specs.push(Spec { parents, time });
    }
    let mut has_child = vec![false; n];
    for s in &specs {
        for &p in &s.parents {
            has_child[p] = true;
        }
    }
    let mut branches: Vec<(String, usize)> = (0..n - 1)
        .filter(|&i| !has_child[i])
        .map(|i| (format!("b{i}"), i))
        .collect();
    branches.push(("main".into(), n - 1));
    (specs, branches)
}

fn import_named(dir: &Path, specs: &[Spec], branches: &[(String, usize)]) -> Vec<ObjectId> {
    let b: Vec<(&str, usize)> = branches.iter().map(|(n, i)| (n.as_str(), *i)).collect();
    import(dir, specs, &b)
}

// ------------------------------------------------------------------ checks

/// Commit → parents as git's revision walk sees them (grafts, shallow and replace refs
/// applied), over branches, tags, remotes and HEAD.
fn git_structure(dir: &Path) -> HashMap<ObjectId, Vec<ObjectId>> {
    let mut args = vec!["rev-list", "--parents", "--branches", "--tags", "--remotes"];
    let unborn = git_cmd(dir, &["rev-parse", "-q", "--verify", "HEAD"])
        .output()
        .map(|o| !o.status.success())
        .unwrap();
    if !unborn {
        args.push("HEAD");
    }
    sh(dir, &args)
        .lines()
        .map(|l| {
            let mut ids = l.split_whitespace().map(oid);
            let id = ids.next().unwrap();
            (id, ids.collect())
        })
        .collect()
}

/// Our structure (without the working-directory row and stash rows).
fn our_structure(h: &History) -> HashMap<ObjectId, Vec<ObjectId>> {
    h.commits
        .iter()
        .enumerate()
        .filter(|(i, c)| !h.is_worktree_row(*i) && c.stash.is_none() && !c.stash_helper)
        .map(|(_, c)| (c.id, c.parents.to_vec()))
        .collect()
}

fn assert_same_structure(dir: &Path, h: &History) {
    let ours = our_structure(h);
    let git = git_structure(dir);
    let missing: Vec<_> = git.keys().filter(|k| !ours.contains_key(*k)).collect();
    let extra: Vec<_> = ours.keys().filter(|k| !git.contains_key(*k)).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "missing {missing:?}, extra {extra:?}"
    );
    for (id, parents) in &git {
        assert_eq!(&ours[id], parents, "parents of {id}");
    }
}

fn same_rows(a: &[GraphRow], b: &[GraphRow]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            (x.node, x.color, x.width, &x.segments) == (y.node, y.color, y.width, &y.segments)
        })
}

/// Lane invariants of a laid-out history:
/// - children above parents;
/// - every parent edge leaves its child's node, runs straight down one lane without
///   crossing another commit's node and ends in its parent's node (or runs to the bottom
///   when the parent is not loaded), in one colour;
/// - the lanes open below a row are exactly the lanes of edges still pending there
///   (lanes are released), and the top half of a row continues the bottom of the one above;
/// - every segment lies inside the row's width;
/// - replaying from the checkpoints gives the same rows as a full replay for any range.
fn check_graph(h: &History, seed: u64) {
    let n = h.commits.len();
    assert_eq!(h.graph.len(), n);
    let rows = h.graph.rows(0..n);
    assert_eq!(rows.len(), n);
    let row_of = |id: &ObjectId| -> Option<usize> {
        // The working-directory row is not in the index.
        h.row_of(id)
    };

    // Lanes open in the bottom half of each row, by edges.
    let mut expected_bottom: Vec<BTreeSet<u16>> = vec![BTreeSet::new(); n];
    let mut ending: Vec<BTreeSet<u16>> = vec![BTreeSet::new(); n];
    for (r, c) in h.commits.iter().enumerate() {
        let row = &rows[r];
        assert!(row.node < row.width, "row {r}: node outside width");
        for s in &row.segments {
            assert!(
                s.from < row.width && s.to < row.width,
                "row {r}: segment outside width"
            );
        }
        let extra: Vec<_> = row
            .segments
            .iter()
            .filter(|s| s.half == Half::Bottom && s.from == row.node && s.to != row.node)
            .collect();
        assert_eq!(
            extra.len(),
            c.parents.len().saturating_sub(1),
            "row {r}: one bottom segment per extra parent"
        );
        for (k, p) in c.parents.iter().enumerate() {
            let (lane, color) = if k == 0 {
                let s = row
                    .segments
                    .iter()
                    .find(|s| s.half == Half::Bottom && s.from == row.node && s.to == row.node)
                    .unwrap_or_else(|| panic!("row {r}: first parent leaves the node"));
                (row.node, s.color)
            } else {
                (extra[k - 1].to, extra[k - 1].color)
            };
            let target = row_of(&p);
            if let Some(t) = target {
                assert!(t > r, "row {r}: parent {p} at row {t} is above its child");
            }
            let end = target.unwrap_or(n);
            expected_bottom[r].insert(lane);
            for (k, row_k) in rows.iter().enumerate().take(end).skip(r + 1) {
                assert_ne!(
                    row_k.node, lane,
                    "row {r}: edge to {p} runs through the node of row {k}"
                );
                let top = row_k
                    .segments
                    .iter()
                    .find(|s| s.half == Half::Top && s.from == lane && s.to == lane);
                assert_eq!(
                    top.map(|s| s.color),
                    Some(color),
                    "row {k}: lane {lane} passes (top)"
                );
                let bottom = row_k
                    .segments
                    .iter()
                    .find(|s| s.half == Half::Bottom && s.from == lane && s.to == lane);
                assert_eq!(
                    bottom.map(|s| s.color),
                    Some(color),
                    "row {k}: lane {lane} passes"
                );
                expected_bottom[k].insert(lane);
            }
            if let Some(t) = target {
                let into = rows[t]
                    .segments
                    .iter()
                    .find(|s| s.half == Half::Top && s.from == lane && s.to == rows[t].node);
                assert_eq!(
                    into.map(|s| s.color),
                    Some(color),
                    "row {t}: lane {lane} converges"
                );
                ending[t].insert(lane);
            }
        }
    }
    for r in 0..n {
        let bottom: BTreeSet<u16> = rows[r]
            .segments
            .iter()
            .filter(|s| s.half == Half::Bottom)
            .map(|s| s.to)
            .collect();
        assert_eq!(bottom, expected_bottom[r], "row {r}: open lanes");
        let top: BTreeSet<u16> = rows[r]
            .segments
            .iter()
            .filter(|s| s.half == Half::Top)
            .map(|s| s.from)
            .collect();
        let above = if r == 0 {
            BTreeSet::new()
        } else {
            expected_bottom[r - 1].clone()
        };
        assert_eq!(top, above, "row {r}: top half continues the row above");
        // One colour per lane end point.
        let mut colors: HashMap<u16, u8> = HashMap::new();
        for s in rows[r].segments.iter().filter(|s| s.half == Half::Bottom) {
            assert_eq!(
                *colors.entry(s.to).or_insert(s.color),
                s.color,
                "row {r}: lane {} colours",
                s.to
            );
        }
        // Exactly the lanes of edges to this commit converge into its node.
        let converging: BTreeSet<u16> = rows[r]
            .segments
            .iter()
            .filter(|s| s.half == Half::Top && s.to == rows[r].node)
            .map(|s| s.from)
            .collect();
        assert_eq!(converging, ending[r], "row {r}: converging lanes");
    }

    // Checkpoint replay at arbitrary ranges.
    let mut rng = Rng(seed | 1);
    for _ in 0..40 {
        let a = rng.below(n + 1);
        let len = rng.below(200) + 1;
        let b = (a + len).min(n);
        let part = h.graph.rows(a..b);
        assert!(
            same_rows(&part, &rows[a..b]),
            "range {a}..{b} differs from a full replay"
        );
    }
    for a in [0, 63, 64, 65, 127, 128, n.saturating_sub(1)] {
        let b = (a + 3).min(n);
        if a < b {
            assert!(
                same_rows(&h.graph.rows(a..b), &rows[a..b]),
                "range {a}..{b}"
            );
        }
    }
}

fn check_history(dir: &Path, h: &History, seed: u64) {
    assert_same_structure(dir, h);
    check_graph(h, seed);
}

fn history(repo: &Repo, order: LogOrder) -> History {
    repo.history(order).unwrap()
}

/// Same rows, ids, parents and graph.
fn assert_same_history(a: &History, b: &History) {
    let ids = |h: &History| {
        h.commits
            .iter()
            .map(|c| (c.id, c.parents.to_vec()))
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(a), ids(b));
    let n = a.commits.len();
    assert!(same_rows(&a.graph.rows(0..n), &b.graph.rows(0..n)));
}

/// git writes the commit-graph read-only.
fn replace_file(path: &Path, data: &[u8]) {
    let _ = fs::remove_file(path);
    fs::write(path, data).unwrap();
}

fn commit_graph_file(dir: &Path) -> PathBuf {
    dir.join(".git/objects/info/commit-graph")
}

// ------------------------------------------------------------------ graph shapes

#[test]
fn octopus_merges_with_three_to_five_parents() {
    let (_t, p) = new_repo(&[]);
    // root, five side branches, then octopus merges of 3, 4 and 5 parents.
    let mut specs = vec![spec(&[], 1000)];
    for i in 1..=5 {
        specs.push(spec(&[0], 1000 + i * 10));
    }
    specs.push(spec(&[1, 2, 3], 2000)); // 6
    specs.push(spec(&[6, 4, 5, 2], 2100)); // 7 (2 already merged)
    specs.push(spec(&[0], 2150)); // 8
    specs.push(spec(&[7, 8, 3, 1, 5], 2200)); // 9
    let ids = import(&p, &specs, &[("main", 9)]);
    let repo = Repo::open(&p).unwrap();
    for order in [LogOrder::Topo, LogOrder::Date] {
        let h = history(&repo, order);
        check_history(&p, &h, 1);
        let top = h.row_of(&ids[9]).unwrap();
        assert_eq!(h.parent_rows(top).len(), 5);
        let rows = h.graph.rows(top..top + 1);
        let fan = rows[0]
            .segments
            .iter()
            .filter(|s| s.half == Half::Bottom && s.from == rows[0].node && s.to != rows[0].node)
            .count();
        assert_eq!(fan, 4, "an octopus opens / joins one lane per extra parent");
        // The root's children: all six commits that have it as first parent.
        let root = h.row_of(&ids[0]).unwrap();
        assert_eq!(h.child_rows(root).len(), 6);
    }
}

#[test]
fn criss_cross_merges() {
    let (_t, p) = new_repo(&[]);
    // a — b1 — m1 — m3      b1/c1 merged into each other twice (criss-cross).
    //   \     ×     ×
    //    c1 — m2 — m4
    let specs = vec![
        spec(&[], 100),
        spec(&[0], 200),    // 1 b1
        spec(&[0], 210),    // 2 c1
        spec(&[1, 2], 300), // 3 m1
        spec(&[2, 1], 310), // 4 m2
        spec(&[3, 4], 400), // 5 m3
        spec(&[4, 3], 410), // 6 m4
        spec(&[5, 6], 500), // 7 final merge
    ];
    let ids = import(&p, &specs, &[("main", 7), ("side", 6)]);
    let repo = Repo::open(&p).unwrap();
    for order in [LogOrder::Topo, LogOrder::Date] {
        let h = history(&repo, order);
        check_history(&p, &h, 2);
        assert_eq!(h.commits.len(), 1 + ids.len());
    }
    // Two merge bases: git sees a criss-cross.
    assert_eq!(
        sh(&p, &["merge-base", "--all", "main~1^1", "main~1^2"])
            .lines()
            .count(),
        2
    );
}

#[test]
fn many_parallel_branches_merge_back() {
    let (_t, p) = new_repo(&[]);
    // 30 branches fork from the root, each three commits long, merged back one by one.
    let mut specs = vec![spec(&[], 1)];
    let mut tips = Vec::new();
    for b in 0..30i64 {
        let mut prev = 0;
        for k in 0..3 {
            specs.push(spec(&[prev], 10 + b * 3 + k));
            prev = specs.len() - 1;
        }
        tips.push(prev);
    }
    let mut main = 0;
    for (k, &t) in tips.iter().enumerate() {
        specs.push(spec(&[main, t], 1000 + k as i64));
        main = specs.len() - 1;
    }
    import(&p, &specs, &[("main", main)]);
    let repo = Repo::open(&p).unwrap();
    for order in [LogOrder::Topo, LogOrder::Date] {
        let h = history(&repo, order);
        check_history(&p, &h, 3);
        assert!(h.graph.max_lanes() >= 2);
    }
    // Date order interleaves the branches: up to 31 lanes are open at once.
    let h = history(&repo, LogOrder::Date);
    let n = h.commits.len();
    let widest = h.graph.rows(0..n).iter().map(|r| r.width).max().unwrap();
    assert!(
        widest > 20,
        "date order keeps the parallel branches side by side ({widest})"
    );
}

#[test]
fn orphan_branches_and_unrelated_merge() {
    let (_t, p) = new_repo(&[]);
    let specs = vec![
        spec(&[], 100),     // 0 root A
        spec(&[0], 110),    // 1
        spec(&[], 120),     // 2 root B (orphan branch "docs")
        spec(&[2], 130),    // 3
        spec(&[], 140),     // 4 root C (orphan "pages", never merged)
        spec(&[1, 3], 150), // 5 merge of the unrelated history B into A
        spec(&[5], 160),    // 6
        spec(&[4], 170),    // 7
    ];
    let ids = import(&p, &specs, &[("main", 6), ("docs", 3), ("pages", 7)]);
    let repo = Repo::open(&p).unwrap();
    for order in [LogOrder::Topo, LogOrder::Date] {
        let h = history(&repo, order);
        check_history(&p, &h, 4);
        let roots: Vec<_> = h
            .commits
            .iter()
            .skip(1)
            .filter(|c| c.parents.is_empty())
            .map(|c| c.id)
            .collect();
        assert_eq!(roots.len(), 3);
        for r in [ids[0], ids[2], ids[4]] {
            assert!(roots.contains(&r));
        }
        // A root ends its lane: nothing continues below it in that lane.
        let n = h.commits.len();
        let rows = h.graph.rows(0..n);
        for root in [ids[0], ids[2], ids[4]] {
            let r = h.row_of(&root).unwrap();
            assert!(
                !rows[r].segments.iter().any(|s| s.half == Half::Bottom
                    && s.from == rows[r].node
                    && s.to == rows[r].node),
                "root at row {r} has no line below"
            );
        }
    }
}

#[test]
fn very_long_linear_history() {
    let (_t, p) = new_repo(&[]);
    let n = 5000;
    let specs = linear(n, 1_000_000);
    import(&p, &specs, &[("main", n - 1)]);
    let repo = Repo::open(&p).unwrap();
    let h = history(&repo, LogOrder::Topo);
    assert_eq!(h.commits.len(), n + 1);
    check_history(&p, &h, 5);
    let rows = h.graph.rows(0..n + 1);
    assert!(rows.iter().all(|r| r.node == 0 && r.width == 1));
    assert_eq!(h.graph.max_lanes(), 1);
}

#[test]
fn random_dags_keep_the_lane_invariants() {
    // Real repositories (sorting + loading) for a few seeds…
    for seed in 1..=4u64 {
        let (_t, p) = new_repo(&[]);
        let (specs, branches) = random_dag(400, seed);
        import_named(&p, &specs, &branches);
        let repo = Repo::open(&p).unwrap();
        for order in [LogOrder::Topo, LogOrder::Date] {
            let h = history(&repo, order);
            check_history(&p, &h, seed);
        }
    }
}

#[test]
fn random_dags_layout_only() {
    // …and the pure layout for many more (no git): rows in creation order reversed are a
    // valid display order (children before parents).
    for seed in 1..=60u64 {
        let (specs, _) = random_dag(300, seed * 31);
        let ids: Vec<ObjectId> = (0..specs.len())
            .map(|i| {
                let mut b = [0u8; 20];
                b[..8].copy_from_slice(&(i as u64 + 1).to_be_bytes());
                ObjectId::from_bytes_or_panic(&b)
            })
            .collect();
        let parents: Vec<Vec<ObjectId>> = specs
            .iter()
            .map(|s| s.parents.iter().map(|&p| ids[p]).collect())
            .collect();
        let items: Vec<(usize, Vec<ObjectId>)> = (0..specs.len())
            .rev()
            .map(|i| (i, parents[i].clone()))
            .collect();
        let sorted: Vec<usize> = graph::sort_commits(
            items,
            |c| ids[c.0],
            |c| c.1.as_slice(),
            |c| specs[c.0].time,
            seed % 2 == 0,
        )
        .into_iter()
        .map(|c| c.0)
        .collect();
        let g = graph::layout(sorted.iter().map(|&i| (ids[i], parents[i].as_slice())));
        let pos: HashMap<ObjectId, usize> = sorted
            .iter()
            .enumerate()
            .map(|(r, &i)| (ids[i], r))
            .collect();
        // Every edge: straight lane from child to parent, no foreign node on the way.
        let n = sorted.len();
        let rows = g.rows(0..n);
        for (r, &i) in sorted.iter().enumerate() {
            for (k, p) in parents[i].iter().enumerate() {
                let t = pos[p];
                assert!(t > r, "seed {seed}: parent below child");
                let lane = if k == 0 {
                    rows[r].node
                } else {
                    rows[r]
                        .segments
                        .iter()
                        .filter(|s| {
                            s.half == Half::Bottom && s.from == rows[r].node && s.to != rows[r].node
                        })
                        .nth(k - 1)
                        .unwrap()
                        .to
                };
                for row in &rows[r + 1..t] {
                    assert_ne!(row.node, lane, "seed {seed}: edge through a foreign node");
                }
                assert!(
                    rows[t]
                        .segments
                        .iter()
                        .any(|s| s.half == Half::Top && s.from == lane && s.to == rows[t].node)
                );
            }
        }
        let last = &rows[n - 1];
        assert!(
            last.segments.iter().all(|s| s.half == Half::Top),
            "seed {seed}: no lane stays open below the last root"
        );
        let mut rng = Rng(seed);
        for _ in 0..20 {
            let a = rng.below(n);
            let b = (a + rng.below(150) + 1).min(n);
            assert!(
                same_rows(&g.rows(a..b), &rows[a..b]),
                "seed {seed}: {a}..{b}"
            );
        }
    }
}

// ------------------------------------------------------------------ commit-graph file

#[test]
fn commit_graph_file_fresh_stale_split_and_corrupt() {
    let (_t, p) = new_repo(&[]);
    let (specs, branches) = random_dag(300, 77);
    let ids = import_named(&p, &specs, &branches);
    let repo = Repo::open(&p).unwrap();

    let plain = history(&repo, LogOrder::Date);
    assert!(!plain.used_commit_graph);
    check_history(&p, &plain, 6);

    // Fresh commit-graph: same history, from the file.
    repo.write_commit_graph().unwrap();
    assert!(commit_graph_file(&p).exists());
    let cg = history(&repo, LogOrder::Date);
    assert!(cg.used_commit_graph);
    assert_same_history(&plain, &cg);

    // Stale: new commits (incl. a merge of an old commit) are not in the file.
    let tip = rev(&p, "main");
    let old = ids[100];
    let tree = sh(&p, &["rev-parse", "main^{tree}"]);
    let a = sh_env(
        &p,
        &["commit-tree", &tree, "-p", &tip.to_string(), "-m", "new 1"],
        &dates("@1700000000 +0000"),
    );
    let m = sh_env(
        &p,
        &[
            "commit-tree",
            &tree,
            "-p",
            &a,
            "-p",
            &old.to_string(),
            "-m",
            "new merge",
        ],
        &dates("@1700000100 +0000"),
    );
    sh(&p, &["update-ref", "refs/heads/main", &m]);
    sh(&p, &["branch", "-f", "fresh", &a]);
    let stale = history(&repo, LogOrder::Date);
    assert!(stale.used_commit_graph);
    check_history(&p, &stale, 7);
    fs::rename(commit_graph_file(&p), p.join(".git/cg-backup")).unwrap();
    let stale_plain = history(&repo, LogOrder::Date);
    assert!(!stale_plain.used_commit_graph);
    assert_same_history(&stale, &stale_plain);
    // The quick partial history agrees too.
    fs::rename(p.join(".git/cg-backup"), commit_graph_file(&p)).unwrap();
    let quick = repo.history_newest(LogOrder::Date, 50).unwrap();
    assert!(quick.partial && quick.used_commit_graph);
    fs::remove_file(commit_graph_file(&p)).unwrap();
    let quick_plain = repo.history_newest(LogOrder::Date, 50).unwrap();
    assert_same_history(&quick, &quick_plain);

    // Split chain: base layer + an incremental layer after more commits.
    sh(&p, &["commit-graph", "write", "--reachable", "--split"]);
    let b = sh_env(
        &p,
        &["commit-tree", &tree, "-p", &m, "-m", "after split"],
        &dates("@1700000200 +0000"),
    );
    sh(&p, &["update-ref", "refs/heads/main", &b]);
    sh(
        &p,
        &["commit-graph", "write", "--reachable", "--split=no-merge"],
    );
    let chain = p.join(".git/objects/info/commit-graphs/commit-graph-chain");
    assert!(chain.exists());
    assert!(
        fs::read_to_string(&chain).unwrap().lines().count() >= 2,
        "two layers"
    );
    let split = history(&repo, LogOrder::Date);
    assert!(split.used_commit_graph);
    check_history(&p, &split, 8);
    fs::remove_dir_all(p.join(".git/objects/info/commit-graphs")).unwrap();
    let split_plain = history(&repo, LogOrder::Date);
    assert_same_history(&split, &split_plain);

    // Corrupt or truncated file: the history still loads (from the object database).
    repo.write_commit_graph().unwrap();
    let good = fs::read(commit_graph_file(&p)).unwrap();
    for bad in [
        b"garbage, not a commit-graph".to_vec(),
        good[..good.len() / 2].to_vec(),
        Vec::new(),
    ] {
        replace_file(&commit_graph_file(&p), &bad);
        let h = history(&repo, LogOrder::Date);
        assert_same_history(&h, &split_plain);
    }

    // core.commitGraph=false: the file is ignored.
    replace_file(&commit_graph_file(&p), &good);
    sh(&p, &["config", "core.commitGraph", "false"]);
    let off = history(&repo, LogOrder::Date);
    assert!(!off.used_commit_graph);
    assert_same_history(&off, &split_plain);
}

#[test]
fn partial_history_is_the_newest_slice_of_the_full_one() {
    let (_t, p) = new_repo(&[]);
    // Distinct, monotonic timestamps: date order is unambiguous.
    let mut specs = vec![spec(&[], 1000)];
    let mut rng = Rng(99);
    for i in 1..400usize {
        let a = i - 1;
        let parents = if i > 5 && rng.below(5) == 0 {
            vec![a, i - 2 - rng.below(4)]
        } else {
            vec![a]
        };
        specs.push(Spec {
            parents,
            time: 1000 + i as i64 * 10,
        });
    }
    import(&p, &specs, &[("main", 399), ("old", 250)]);
    let repo = Repo::open(&p).unwrap();
    let full = history(&repo, LogOrder::Date);
    assert!(!full.partial);
    for limit in [1, 10, 64, 65, 200, 399, 400, 1000] {
        let quick = repo.history_newest(LogOrder::Date, limit).unwrap();
        assert!(quick.partial);
        let got: Vec<ObjectId> = quick.commits.iter().skip(1).map(|c| c.id).collect();
        let want: Vec<ObjectId> = full
            .commits
            .iter()
            .skip(1)
            .take(limit)
            .map(|c| c.id)
            .collect();
        assert_eq!(got, want, "limit {limit}");
        // Lanes of parents that are not loaded yet run to the bottom.
        check_graph(&quick, limit as u64);
    }
    // Topo order: the partial set is the newest commits, children above parents.
    let quick = repo.history_newest(LogOrder::Topo, 120).unwrap();
    let mut by_time: Vec<_> = full
        .commits
        .iter()
        .skip(1)
        .map(|c| (c.commit_time, c.id))
        .collect();
    by_time.sort_by(|a, b| b.cmp(a));
    let newest: HashSet<ObjectId> = by_time.iter().take(120).map(|(_, id)| *id).collect();
    let got: HashSet<ObjectId> = quick.commits.iter().skip(1).map(|c| c.id).collect();
    assert_eq!(got, newest);
    check_graph(&quick, 9);
}

// ------------------------------------------------------------------ commit metadata

/// Writes a raw commit object and returns its id.
fn raw_commit(dir: &Path, body: &[u8]) -> ObjectId {
    oid(&sh_stdin(
        dir,
        &[
            "hash-object",
            "-t",
            "commit",
            "-w",
            "--stdin",
            "--literally",
        ],
        body,
    ))
}

#[test]
fn lazy_meta_of_odd_commits() {
    let (_t, p) = new_repo(&[]);
    let tree = sh(&p, &["mktree"]);
    let mut prev: Option<ObjectId> = None;
    let mut ids = Vec::new();
    let long = "x".repeat(10_000);
    let cases: Vec<(String, String, String)> = vec![
        // (author line, committer line, message)
        (
            "A <a@x> 1000000000 +0000".into(),
            "C <c@x> 1000000000 +0000".into(),
            "".into(),
        ),
        (
            "A <a@x> 1000000001 +0000".into(),
            "C <c@x> 1000000001 +0000".into(),
            "no newline".into(),
        ),
        (
            "A <a@x> 1000000002 +0000".into(),
            "C <c@x> 1000000002 +0000".into(),
            "\n\nleading blank lines\n".into(),
        ),
        (
            "A <a@x> 1000000003 +0000".into(),
            "C <c@x> 1000000003 +0000".into(),
            "first\nsecond line of the subject\n\nbody\n".into(),
        ),
        (
            "A <a@x> 1000000004 +0000".into(),
            "C <c@x> 1000000004 +0000".into(),
            format!("{long}\n"),
        ),
        // Identical timestamps.
        (
            "A <a@x> 1000000004 +0000".into(),
            "C <c@x> 1000000004 +0000".into(),
            "same time\n".into(),
        ),
        // Before 1970, and far in the future; odd time zones.
        (
            "Old <o@x> -86400 -0130".into(),
            "C <c@x> 1000000005 +0000".into(),
            "author before 1970\n".into(),
        ),
        (
            "Future <f@x> 4102444800 +1400".into(),
            "C <c@x> 1000000006 +0000".into(),
            "year 2100\n".into(),
        ),
        // Committer differs from the author.
        (
            "Ada Lovelace <ada@example.com> 1000000007 +0100".into(),
            "Bot <bot@ci> 1000000107 -0700".into(),
            "committed by someone else\n".into(),
        ),
        (
            "Ünïcödé 名前 <u@x> 1000000008 +0000".into(),
            "C <c@x> 1000000008 +0000".into(),
            "subject with ünïcödé ✓\n".into(),
        ),
        (
            "NoEmail <> 1000000009 +0000".into(),
            "C <c@x> 1000000009 +0000".into(),
            "empty email\n".into(),
        ),
    ];
    for (author, committer, message) in &cases {
        let mut body = format!("tree {tree}\n");
        if let Some(p) = prev {
            body.push_str(&format!("parent {p}\n"));
        }
        body.push_str(&format!(
            "author {author}\ncommitter {committer}\n\n{message}"
        ));
        let id = raw_commit(&p, body.as_bytes());
        ids.push(id);
        prev = Some(id);
    }
    sh(
        &p,
        &["update-ref", "refs/heads/main", &prev.unwrap().to_string()],
    );
    let repo = Repo::open(&p).unwrap();
    for order in [LogOrder::Topo, LogOrder::Date] {
        let h = history(&repo, order);
        check_history(&p, &h, 10);
        for id in &ids {
            let row = h.row_of(id).unwrap();
            let m = h.meta(row);
            let git = sh(
                &p,
                &[
                    "log",
                    "-1",
                    "--format=%an%x00%ae%x00%at%x00%s",
                    &id.to_string(),
                ],
            );
            let f: Vec<&str> = git.split('\0').collect();
            assert_eq!(m.author, f[0], "{id}");
            assert_eq!(m.email, f[1], "{id}");
            // git prints no date before 1970 (we do).
            if !f[2].is_empty() {
                assert_eq!(m.time.to_string(), f[2], "{id}");
            }
            assert_eq!(m.summary, f[3].trim(), "{id}: summary like git's %s");
            let d = repo.commit_details(*id).unwrap();
            assert_eq!(d.author, m.author);
        }
        // The working-directory row has empty meta; rows past the end too.
        assert!(h.meta(0).summary.is_empty());
        assert!(h.meta(h.commits.len() + 5).summary.is_empty());
    }
    let h = history(&repo, LogOrder::Date);
    let m = h.meta(h.row_of(&ids[6]).unwrap());
    assert_eq!((m.time, m.tz_offset), (-86400, -(90 * 60)));
    let d = repo.commit_details(ids[8]).unwrap();
    assert_eq!(
        (d.author.as_str(), d.committer.as_str()),
        ("Ada Lovelace", "Bot")
    );
    assert_eq!((d.author_time, d.commit_time), (1000000007, 1000000107));
    assert_eq!(h.meta(h.row_of(&ids[4]).unwrap()).summary.len(), 10_000);
}

#[test]
fn extreme_commit_times_with_and_without_commit_graph() {
    // Committer times before 1970, at 0, beyond 2^32 and beyond the commit-graph's 34 bits.
    let (_t, p) = new_repo(&[]);
    let tree = sh(&p, &["mktree"]);
    let mut tips = Vec::new();
    for (k, t) in [
        "-1000000",
        "0",
        "1",
        "4294967297",
        "17179869184",
        "99999999999",
        "1500000000",
    ]
    .iter()
    .enumerate()
    {
        let body = format!(
            "tree {tree}\nauthor A <a@x> {t} +0000\ncommitter C <c@x> {t} +0000\n\nat {t} ({k})\n"
        );
        let id = raw_commit(&p, body.as_bytes());
        sh(
            &p,
            &["update-ref", &format!("refs/heads/t{k}"), &id.to_string()],
        );
        tips.push(id);
    }
    sh(&p, &["update-ref", "refs/heads/main", &tips[6].to_string()]);
    let repo = Repo::open(&p).unwrap();
    let plain = history(&repo, LogOrder::Date);
    check_graph(&plain, 21);
    assert_eq!(plain.commits.len(), 8);
    let times: Vec<i64> = plain
        .commits
        .iter()
        .skip(1)
        .map(|c| c.commit_time)
        .collect();
    let mut sorted = times.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(times, sorted, "date order, newest first");
    assert_eq!(plain.meta(plain.row_of(&tips[0]).unwrap()).time, -1000000);
    repo.write_commit_graph().unwrap();
    let cg = history(&repo, LogOrder::Date);
    assert!(cg.used_commit_graph);
    check_graph(&cg, 22);
    // The commit-graph file stores committer times in 34 bits (like git, which reads
    // them from there too): times in 0..2^34 are exact, others wrap.
    let time_of = |h: &History, id: &ObjectId| h.commits.at(h.row_of(id).unwrap()).commit_time;
    for (k, id) in tips.iter().enumerate() {
        let t = time_of(&plain, id);
        if (0..1 << 34).contains(&t) {
            assert_eq!(time_of(&cg, id), t, "tip {k}");
        } else {
            assert_eq!(time_of(&cg, id), t.rem_euclid(1 << 34), "tip {k} wraps");
        }
    }
}

#[test]
fn latin1_commit_message_is_decoded() {
    let (_t, p) = new_repo(&[]);
    let tree = sh(&p, &["mktree"]);
    let mut body = format!("tree {tree}\nauthor ").into_bytes();
    // Latin-1 author name and message: "José", "Café für Jürgen".
    body.extend_from_slice(
        b"Jos\xe9 <j@x> 1000000000 +0000\ncommitter C <c@x> 1000000000 +0000\n\
          encoding ISO-8859-1\n\nCaf\xe9 f\xfcr J\xfcrgen\n\nK\xf6rper\n",
    );
    let id = raw_commit(&p, &body);
    sh(&p, &["update-ref", "refs/heads/main", &id.to_string()]);
    // git itself re-encodes to UTF-8.
    assert_eq!(
        sh(&p, &["log", "-1", "--format=%s|%an"]),
        "Café für Jürgen|José"
    );
    let repo = Repo::open(&p).unwrap();
    let h = history(&repo, LogOrder::Topo);
    let m = h.meta(1);
    assert_eq!(m.summary, "Café für Jürgen");
    assert_eq!(m.author, "José");
    let d = repo.commit_details(id).unwrap();
    assert_eq!(d.message, "Café für Jürgen\n\nKörper\n");
    assert_eq!(d.author, "José");
}

#[test]
fn commit_made_with_i18n_commit_encoding() {
    // The usual way such commits come about: i18n.commitEncoding + a Latin-1 message file.
    let (_t, p) = new_repo(&[]);
    sh(&p, &["config", "i18n.commitEncoding", "ISO-8859-1"]);
    fs::write(p.join("msg"), b"Gr\xfc\xdfe\n").unwrap();
    fs::write(p.join("a.txt"), "a\n").unwrap();
    sh(&p, &["add", "a.txt"]);
    sh(&p, &["commit", "-q", "-F", "msg"]);
    let repo = Repo::open(&p).unwrap();
    let h = history(&repo, LogOrder::Topo);
    assert_eq!(h.meta(1).summary, "Grüße");
}

// ------------------------------------------------------------------ clones

/// A source repository with some history and two branches.
fn source_repo(root: &Path) -> PathBuf {
    let p = init_in(root, "source", &[]);
    let (specs, branches) = random_dag(60, 5);
    import_named(&p, &specs, &branches);
    p
}

fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

#[test]
fn shallow_clone_shows_grafted_commits_as_roots() {
    let t = tempfile::tempdir().unwrap();
    let src = source_repo(t.path());
    for depth in ["1", "3"] {
        let name = format!("shallow{depth}");
        sh(
            t.path(),
            &[
                "clone",
                "-q",
                "--no-single-branch",
                "--depth",
                depth,
                &file_url(&src),
                &name,
            ],
        );
        let p = t.path().join(&name);
        assert!(p.join(".git/shallow").exists());
        let repo = Repo::open(&p).unwrap();
        for order in [LogOrder::Topo, LogOrder::Date] {
            let h = history(&repo, order);
            // Like `git log`: the boundary commits have no parents (no dangling lanes).
            check_history(&p, &h, 11);
            let quick = repo.history_newest(order, 5).unwrap();
            check_graph(&quick, 12);
        }
        let h = history(&repo, LogOrder::Topo);
        let n = h.commits.len();
        let last = &h.graph.rows(n - 1..n)[0];
        assert!(
            last.segments.iter().all(|s| s.half == Half::Top),
            "no lane below the last row"
        );
        assert!(repo.status().unwrap().entries.is_empty());
        let _ = h.meta(1);
    }
}

#[test]
fn partial_clone_without_trees_opens() {
    let t = tempfile::tempdir().unwrap();
    let src = source_repo(t.path());
    sh(&src, &["config", "uploadpack.allowFilter", "true"]);
    sh(&src, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
    sh(
        t.path(),
        &["clone", "-q", "--filter=tree:0", &file_url(&src), "partial"],
    );
    let p = t.path().join("partial");
    let repo = Repo::open(&p).unwrap();
    let h = history(&repo, LogOrder::Topo);
    check_history(&p, &h, 13);
    assert!(!h.meta(1).summary.is_empty());
    assert!(repo.status().unwrap().entries.is_empty());
    // An old file version is fetched lazily from the promisor remote.
    let old = h.commits.at(h.commits.len() - 1).id.to_string();
    let got = repo
        .read_versions(&[(Version::Rev(old), "f0.txt".into())])
        .unwrap();
    assert_eq!(got[0].as_deref(), Some("0\n"));
}

#[test]
#[ignore = "refs/replace is ignored (gix 0.88 inverts core.useReplaceRefs: replacements are off by default)"]
fn replace_refs_are_honoured_like_git() {
    let (_t, p) = new_repo(&[]);
    let specs = vec![
        spec(&[], 100),
        spec(&[0], 200),
        spec(&[1], 300),
        spec(&[], 400), // 3: an unrelated root
        spec(&[3], 500),
    ];
    let ids = import(&p, &specs, &[("main", 2), ("other", 4)]);
    // Graft "other"'s root onto main: git log now shows one history.
    sh(
        &p,
        &[
            "replace",
            "--graft",
            &ids[3].to_string(),
            &ids[2].to_string(),
        ],
    );
    let repo = Repo::open(&p).unwrap();
    for with_graph in [false, true] {
        if with_graph {
            repo.write_commit_graph().unwrap();
        }
        let h = history(&repo, LogOrder::Topo);
        assert_same_structure(&p, &h);
        check_graph(&h, 14);
        // refs/replace/* is not a decoration.
        assert!(
            h.refs_by_commit
                .values()
                .flatten()
                .all(|r| !r.full_name.starts_with("refs/replace/"))
        );
    }
}

// ------------------------------------------------------------------ SHA-256

#[test]
fn sha256_repository_opens_or_fails_cleanly() {
    let (_t, p) = new_repo(&["--object-format=sha256"]);
    fs::write(p.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    sh(&p, &["add", "a.txt"]);
    sh(&p, &["commit", "-qm", "first"]);
    // Defined behaviour today: gix is built without SHA-256 support, so opening reports
    // that instead of panicking or showing a broken history.
    match Repo::open(&p) {
        Err(e) => assert!(
            e.to_string()
                .starts_with("SHA-256 repositories are not supported"),
            "{e}"
        ),
        Ok(repo) => {
            // If it opens, it must work end to end.
            let h = history(&repo, LogOrder::Topo);
            assert_eq!(h.commits.len(), 2);
            assert_eq!(h.meta(1).summary, "first");
        }
    }
}

#[test]
#[ignore = "SHA-256 repositories are not supported (gix built with `sha1` only)"]
fn sha256_repository_works() {
    let (_t, p) = new_repo(&["--object-format=sha256"]);
    fs::write(p.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    sh(&p, &["add", "a.txt"]);
    sh(&p, &["commit", "-qm", "first"]);
    let repo = Repo::open(&p).unwrap();
    let h = history(&repo, LogOrder::Topo);
    assert_eq!(h.commits.len(), 2);
    assert_eq!(h.meta(1).summary, "first");
    fs::write(p.join("a.txt"), "one\n2\nthree\nfour\n").unwrap();
    let state = repo.working_state().unwrap();
    let d = state.unstaged[0].diff.clone().unwrap();
    repo.apply_selection(&d, &Selection::whole_hunk(0), PatchMode::Stage)
        .unwrap();
    assert_eq!(repo.working_state().unwrap().staged.len(), 1);
}

// ------------------------------------------------------------------ refs

#[test]
fn odd_ref_names_packed_loose_symbolic_and_broken() {
    let (_t, p) = new_repo(&[]);
    let specs = linear(4, 100);
    let ids = import(&p, &specs, &[("main", 3)]);
    let names = [
        "feature/deep/nested/x",
        "fix-ü-分支",
        "user@host",
        "v1.2.3",
        "release/1.0.x",
        "dash-",
        "UPPER",
        "upper2",
    ];
    for n in names {
        sh(&p, &["branch", n, &ids[1].to_string()]);
    }
    sh(&p, &["tag", "tag/with/slashes", &ids[0].to_string()]);
    sh(&p, &["tag", "ünï-tag", &ids[2].to_string()]);
    // Packed and loose ref of the same name: the loose one wins.
    sh(&p, &["branch", "both", &ids[0].to_string()]);
    sh(&p, &["pack-refs", "--all"]);
    assert!(!p.join(".git/refs/heads/both").exists());
    fs::write(p.join(".git/refs/heads/both"), format!("{}\n", ids[2])).unwrap();
    assert_eq!(rev(&p, "both"), ids[2]);
    // Symbolic ref to a branch.
    sh(&p, &["symbolic-ref", "refs/heads/alias", "refs/heads/main"]);
    // Remote-tracking branches with a remote HEAD, and a remote with dots in its name.
    sh(
        &p,
        &[
            "update-ref",
            "refs/remotes/my.remote/feature/a",
            &ids[1].to_string(),
        ],
    );
    sh(
        &p,
        &[
            "update-ref",
            "refs/remotes/origin/main",
            &ids[3].to_string(),
        ],
    );
    sh(
        &p,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    // Broken refs: an unknown object and garbage content are skipped.
    fs::write(
        p.join(".git/refs/heads/missing"),
        format!("{}\n", "1".repeat(40)),
    )
    .unwrap();
    fs::write(p.join(".git/refs/heads/garbage"), "not a ref at all\n").unwrap();

    let repo = Repo::open(&p).unwrap();
    let refs = repo.refs().unwrap();
    let find = |short: &str| refs.iter().find(|r| r.short_name == short);
    for n in names {
        let r = find(n).unwrap_or_else(|| panic!("{n} listed"));
        assert_eq!((r.kind, r.target), (RefKind::LocalBranch, ids[1]), "{n}");
        assert_eq!(r.full_name, format!("refs/heads/{n}"));
    }
    assert_eq!(
        find("both").unwrap().target,
        ids[2],
        "loose wins over packed"
    );
    assert_eq!(refs.iter().filter(|r| r.short_name == "both").count(), 1);
    assert_eq!(find("alias").map(|r| r.target), Some(ids[3]));
    assert_eq!(find("tag/with/slashes").map(|r| r.kind), Some(RefKind::Tag));
    assert_eq!(find("ünï-tag").map(|r| r.target), Some(ids[2]));
    assert_eq!(
        find("my.remote/feature/a").map(|r| r.kind),
        Some(RefKind::RemoteBranch)
    );
    assert!(find("origin/HEAD").is_none(), "remote HEAD is not listed");
    assert!(find("missing").is_none() && find("garbage").is_none());
    // Sorted by kind, then name.
    let keys: Vec<_> = refs
        .iter()
        .map(|r| (r.kind, r.short_name.clone()))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);

    let h = history(&repo, LogOrder::Topo);
    check_graph(&h, 15);
    assert_eq!(h.commits.len(), 5);
    let deco: Vec<_> = h.refs_by_commit[&ids[1]]
        .iter()
        .map(|r| r.short_name.as_str())
        .collect();
    for n in names {
        assert!(deco.contains(&n), "{n} decorates its commit");
    }
    assert_eq!(repo.head().unwrap().branch.as_deref(), Some("main"));
    // Ops keep working with such names.
    repo.create_branch("neu/ö@x", &ids[0].to_string(), true)
        .unwrap();
    assert_eq!(repo.head().unwrap().branch.as_deref(), Some("neu/ö@x"));
}

#[test]
fn tags_on_tags_and_on_non_commits() {
    let (_t, p) = new_repo(&[]);
    fs::write(p.join("a.txt"), "a\n").unwrap();
    sh(&p, &["add", "a.txt"]);
    sh(&p, &["commit", "-qm", "one"]);
    let c = rev(&p, "HEAD");
    sh(&p, &["tag", "-a", "inner", "-m", "inner"]);
    sh(&p, &["tag", "-a", "outer", "inner", "-m", "tag of a tag"]);
    sh(
        &p,
        &[
            "tag",
            "-a",
            "outermost",
            "outer",
            "-m",
            "tag of a tag of a tag",
        ],
    );
    let tree = sh(&p, &["rev-parse", "HEAD^{tree}"]);
    let blob = sh(&p, &["rev-parse", "HEAD:a.txt"]);
    sh(&p, &["tag", "light-tree", &tree]);
    sh(&p, &["tag", "light-blob", &blob]);
    sh(&p, &["tag", "-a", "annotated-tree", &tree, "-m", "a tree"]);
    sh(&p, &["tag", "-a", "annotated-blob", &blob, "-m", "a blob"]);
    let repo = Repo::open(&p).unwrap();
    let refs = repo.refs().unwrap();
    let target = |n: &str| refs.iter().find(|r| r.short_name == n).map(|r| r.target);
    for n in ["inner", "outer", "outermost"] {
        assert_eq!(target(n), Some(c), "{n} peels to the commit");
    }
    assert_eq!(target("light-tree"), Some(oid(&tree)));
    assert_eq!(target("annotated-blob"), Some(oid(&blob)));
    let h = history(&repo, LogOrder::Topo);
    assert_eq!(h.commits.len(), 2, "trees and blobs are not history tips");
    check_graph(&h, 16);
    let deco: Vec<_> = h.refs_by_commit[&c]
        .iter()
        .map(|r| r.short_name.as_str())
        .collect();
    assert!(deco.contains(&"outermost") && deco.contains(&"main"));
}

#[test]
fn detached_and_unborn_head() {
    // Empty repository: only the working-directory row.
    let (_t, p) = new_repo(&[]);
    let repo = Repo::open(&p).unwrap();
    let head = repo.head().unwrap();
    assert_eq!((head.branch.as_deref(), head.id), (Some("main"), None));
    let h = history(&repo, LogOrder::Topo);
    assert_eq!(h.commits.len(), 1);
    assert!(h.commits.at(0).parents.is_empty());
    check_graph(&h, 17);
    assert!(repo.refs().unwrap().is_empty());
    assert!(
        repo.history_newest(LogOrder::Date, 10)
            .unwrap()
            .commits
            .len()
            == 1
    );
    // The first commit can be staged and made from there.
    fs::write(p.join("a.txt"), "one\n").unwrap();
    let state = repo.working_state().unwrap();
    assert_eq!(state.unstaged.len(), 1);
    repo.stage_paths(&["a.txt"]).unwrap();
    repo.unstage_paths(&["a.txt"]).unwrap();
    repo.stage_paths(&["a.txt"]).unwrap();
    repo.commit("first", false).unwrap();
    let h = history(&repo, LogOrder::Topo);
    assert_eq!(h.commits.len(), 2);
    assert_eq!(h.meta(1).summary, "first");

    // Detached HEAD on a commit no branch points at: still in the history.
    fs::write(p.join("a.txt"), "two\n").unwrap();
    sh(&p, &["commit", "-qam", "second"]);
    sh(&p, &["checkout", "-q", "--detach", "HEAD"]);
    sh(
        &p,
        &["commit", "-q", "--allow-empty", "-m", "detached work"],
    );
    let detached = rev(&p, "HEAD");
    let head = repo.head().unwrap();
    assert_eq!((head.branch, head.id), (None, Some(detached)));
    let h = history(&repo, LogOrder::Topo);
    assert_eq!(h.commits.at(0).parents, vec![detached]);
    assert_eq!(h.row_of(&detached).unwrap(), 1);
    check_history(&p, &h, 18);

    // Unborn orphan branch in a repository with history: the other commits stay.
    sh(&p, &["checkout", "-q", "--orphan", "fresh"]);
    let head = repo.head().unwrap();
    assert_eq!((head.branch.as_deref(), head.id), (Some("fresh"), None));
    let h = history(&repo, LogOrder::Topo);
    assert!(h.commits.at(0).parents.is_empty());
    assert_eq!(
        h.commits.len(),
        3,
        "main's two commits; the detached one is unreachable now"
    );
    check_graph(&h, 19);
    // Everything of main is staged in the orphan branch's first commit-to-be.
    assert_eq!(repo.working_state().unwrap().staged.len(), 1);
}

#[test]
fn stashes_in_the_history() {
    let (_t, p) = new_repo(&[]);
    fs::write(p.join("a.txt"), "a\n").unwrap();
    sh(&p, &["add", "a.txt"]);
    sh(&p, &["commit", "-qm", "base"]);
    // Stash 1: tracked change only (2 parents).
    fs::write(p.join("a.txt"), "a1\n").unwrap();
    sh(&p, &["stash", "push", "-q", "-m", "tracked"]);
    // Stash 0: staged + untracked (3 parents).
    fs::write(p.join("a.txt"), "a2\n").unwrap();
    sh(&p, &["add", "a.txt"]);
    fs::write(p.join("new.txt"), "n\n").unwrap();
    sh(&p, &["stash", "push", "-q", "-u", "-m", "with untracked"]);
    sh(&p, &["commit", "-q", "--allow-empty", "-m", "after"]);
    let repo = Repo::open(&p).unwrap();
    for order in [LogOrder::Topo, LogOrder::Date] {
        let h = history(&repo, order);
        assert_same_structure(&p, &h);
        check_graph(&h, 20);
        let stash_rows: Vec<_> = h.commits.iter().filter_map(|c| c.stash).collect();
        assert_eq!(stash_rows.len(), 2);
        assert!(stash_rows.contains(&0) && stash_rows.contains(&1));
        let helpers = h.commits.iter().filter(|c| c.stash_helper).count();
        assert_eq!(
            helpers, 3,
            "index commit of both stashes + untracked commit"
        );
        let s0 = rev(&p, "stash@{0}");
        assert_eq!(h.commits.at(h.row_of(&s0).unwrap()).parents.len(), 3);
        assert_eq!(h.refs_by_commit[&s0][0].kind, RefKind::Stash);
    }
    // The quick first screen leaves stashes out.
    let quick = repo.history_newest(LogOrder::Date, 100).unwrap();
    assert!(
        quick
            .commits
            .iter()
            .all(|c| c.stash.is_none() && !c.stash_helper)
    );
}

// ------------------------------------------------------------------ opening / discovery

#[test]
fn open_from_subdirectories_and_the_git_dir() {
    let (_t, p) = new_repo(&[]);
    fs::create_dir_all(p.join("a/b/c")).unwrap();
    fs::write(p.join("a/b/c/f.txt"), "x\n").unwrap();
    sh(&p, &["add", "-A"]);
    sh(&p, &["commit", "-qm", "one"]);
    let root = Repo::open(&p).unwrap().workdir().to_path_buf();
    for sub in ["a", "a/b/c", ".git", ".git/objects"] {
        let repo = Repo::open(p.join(sub)).unwrap_or_else(|e| panic!("{sub}: {e}"));
        assert_eq!(repo.workdir(), root, "{sub}");
        assert_eq!(repo.history(LogOrder::Topo).unwrap().commits.len(), 2);
    }
    // Status from a subdirectory lists repository-relative paths.
    fs::write(p.join("a/b/c/f.txt"), "y\n").unwrap();
    let repo = Repo::open(p.join("a/b")).unwrap();
    assert_eq!(repo.status().unwrap().entries[0].path, "a/b/c/f.txt");
    // Not a repository / missing directory: errors.
    let plain = tempfile::tempdir().unwrap();
    assert!(Repo::open(plain.path()).is_err());
    assert!(Repo::open(plain.path().join("does/not/exist")).is_err());
}

#[test]
fn git_file_with_separate_git_dir() {
    let t = tempfile::tempdir().unwrap();
    let store = t.path().join("store.git");
    let wt = t.path().join("work tree");
    fs::create_dir_all(&wt).unwrap();
    sh(
        &wt,
        &[
            "init",
            "-q",
            "-b",
            "main",
            "--separate-git-dir",
            &store.to_string_lossy(),
        ],
    );
    sh(&wt, &["config", "core.autocrlf", "false"]);
    assert!(wt.join(".git").is_file());
    fs::write(wt.join("a.txt"), "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n").unwrap();
    sh(&wt, &["add", "a.txt"]);
    sh(&wt, &["commit", "-qm", "one"]);
    let repo = Repo::open(wt.join(".")).unwrap();
    assert_eq!(
        repo.git_dir().unwrap(),
        gitti_core::paths::canonical(&store)
    );
    assert_eq!(repo.common_dir(), gitti_core::paths::canonical(&store));
    assert_eq!(history(&repo, LogOrder::Topo).commits.len(), 2);
    // Stage one hunk.
    fs::write(wt.join("a.txt"), "1!\n2\n3\n4\n5\n6\n7\n8\n9\n10!\n").unwrap();
    let state = repo.working_state().unwrap();
    let d = state.unstaged[0].diff.clone().unwrap();
    assert_eq!(d.hunks.len(), 2);
    repo.apply_selection(&d, &Selection::whole_hunk(1), PatchMode::Stage)
        .unwrap();
    let state = repo.working_state().unwrap();
    assert_eq!((state.staged.len(), state.unstaged.len()), (1, 1));
    assert!(repo.index_stamp().is_some());
}

#[test]
fn submodule_opens_as_its_own_repository() {
    let t = tempfile::tempdir().unwrap();
    let lib = init_in(t.path(), "lib", &[]);
    fs::write(lib.join("lib.txt"), "lib\n").unwrap();
    sh(&lib, &["add", "-A"]);
    sh(&lib, &["commit", "-qm", "lib one"]);
    let sup = init_in(t.path(), "super", &[]);
    fs::write(sup.join("top.txt"), "top\n").unwrap();
    sh(&sup, &["add", "-A"]);
    sh(&sup, &["commit", "-qm", "super one"]);
    sh(
        &sup,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            &file_url(&lib),
            "deps/lib",
        ],
    );
    sh(&sup, &["commit", "-qm", "add lib"]);
    let sub = sup.join("deps/lib");
    assert!(sub.join(".git").is_file());

    let inner = Repo::open(&sub).unwrap();
    assert_eq!(inner.workdir(), gitti_core::paths::canonical(&sub));
    let h = history(&inner, LogOrder::Topo);
    assert_eq!(h.meta(1).summary, "lib one");
    assert!(
        inner
            .git_dir()
            .unwrap()
            .ends_with(Path::new("modules").join("deps").join("lib"))
    );
    fs::write(sub.join("lib.txt"), "lib changed\n").unwrap();
    assert_eq!(inner.status().unwrap().entries[0].path, "lib.txt");

    let outer = Repo::open(&sup).unwrap();
    let h = history(&outer, LogOrder::Topo);
    assert_eq!(h.commits.len(), 3);
    // The modified submodule shows as one entry of the superproject.
    assert!(
        outer
            .status()
            .unwrap()
            .entries
            .iter()
            .any(|e| e.path == "deps/lib")
    );
    // A version read at a commit where the path is a submodule is not a file.
    let got = outer
        .read_versions(&[(Version::Rev("HEAD".into()), "deps/lib".into())])
        .unwrap();
    assert_eq!(got, vec![None]);
}

#[test]
fn bare_repository_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let bare = t.path().join("bare.git");
    fs::create_dir_all(&bare).unwrap();
    sh(&bare, &["init", "-q", "--bare"]);
    let err = Repo::open(&bare).unwrap_err();
    // Typed, so the apps say "Bare repository" (not "Not a git repository") and offer no
    // "Initialize Repository Here".
    let kind = err.downcast_ref::<gitti_core::OpenError>().cloned();
    assert_eq!(kind, Some(gitti_core::OpenError::Bare(gitti_core::paths::canonical(&bare))));
    assert_eq!(kind.unwrap().title(), "Bare repository");
    let msg = err.to_string();
    assert!(msg.contains("is a bare repository") && msg.contains("worktree add"), "{msg}");
    let err = Repo::open(bare.join("refs")).unwrap_err().to_string();
    assert!(err.contains("bare"), "{err}");
}

/// Environment of a git hook / alias: `GIT_DIR` and friends point at another repository.
const LEAK_VARS: [&str; 4] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
];

#[test]
fn git_dir_in_the_environment_does_not_leak() {
    let t = tempfile::tempdir().unwrap();
    let ours = init_in(t.path(), "ours", &[]);
    fs::write(ours.join("ours.txt"), "ours\n").unwrap();
    sh(&ours, &["add", "-A"]);
    sh(&ours, &["commit", "-qm", "our commit"]);
    fs::write(ours.join("ours.txt"), "changed\n").unwrap();
    let other = init_in(t.path(), "other", &[]);
    fs::write(other.join("other.txt"), "other\n").unwrap();
    sh(&other, &["add", "-A"]);
    sh(&other, &["commit", "-qm", "their commit"]);
    // The check runs in a child process (the environment is process-wide), once per
    // variable and once with all of them.
    let value = |var: &str| match var {
        "GIT_WORK_TREE" => other.clone(),
        "GIT_INDEX_FILE" => other.join(".git").join("index"),
        _ => other.join(".git"),
    };
    let exe = std::env::current_exe().unwrap();
    let mut failures = Vec::new();
    let sets: Vec<Vec<&str>> = LEAK_VARS
        .iter()
        .map(|v| vec![*v])
        .chain([LEAK_VARS.to_vec()])
        .collect();
    for vars in sets {
        let mut cmd = Command::new(&exe);
        cmd.args([
            "--exact",
            "repo_shapes::env_leak_child",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("GITTI_R24E_REPO", &ours);
        for v in &vars {
            cmd.env(v, value(v));
        }
        let out = cmd.output().unwrap();
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if !(out.status.success() && text.contains("1 passed")) {
            failures.push(format!("{vars:?}: {text}"));
        }
        // The child stages a file: reset for the next run.
        sh(&ours, &["reset", "-q"]);
        sh(&other, &["reset", "-q"]);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
#[ignore = "helper of git_dir_in_the_environment_does_not_leak (runs in a child process)"]
fn env_leak_child() {
    let Some(path) = std::env::var_os("GITTI_R24E_REPO") else {
        return;
    };
    let repo = Repo::open(&path).unwrap();
    let h = repo.history(LogOrder::Topo).unwrap();
    assert_eq!(h.meta(1).summary, "our commit");
    let status = repo.status().unwrap();
    let paths: Vec<_> = status.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, vec!["ours.txt"]);
    let state = repo.working_state().unwrap();
    assert_eq!(state.unstaged.len(), 1);
    repo.stage_paths(&["ours.txt"]).unwrap();
    assert_eq!(repo.working_state().unwrap().staged.len(), 1);
    let got = repo
        .read_versions(&[(Version::Rev("HEAD".into()), "ours.txt".into())])
        .unwrap();
    assert_eq!(got[0].as_deref(), Some("ours\n"));
    assert_eq!(repo.head_message().unwrap().trim(), "our commit");
}

// ------------------------------------------------------------------ blobs

#[test]
fn reading_huge_binary_and_lfs_pointer_files_at_a_commit() {
    let (_t, p) = new_repo(&[]);
    let huge: String = "0123456789abcdef\n".repeat(MAX_BLOB_BYTES / 17 + 10);
    assert!(huge.len() > MAX_BLOB_BYTES);
    let limit: String = "x".repeat(MAX_BLOB_BYTES - 1) + "\n";
    let mut binary = b"PNG\x00\x01\x02".to_vec();
    binary.extend(std::iter::repeat_n(0xffu8, 5000));
    let late_nul = {
        let mut v = "text\n".repeat(2000).into_bytes();
        v.push(0);
        v
    };
    let lfs = "version https://git-lfs.github.com/spec/v1\noid sha256:4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393\nsize 12345\n";
    fs::write(p.join("huge.txt"), &huge).unwrap();
    fs::write(p.join("limit.txt"), &limit).unwrap();
    fs::write(p.join("image.png"), &binary).unwrap();
    fs::write(p.join("late-nul.txt"), &late_nul).unwrap();
    fs::write(p.join("model.bin"), lfs).unwrap();
    fs::write(
        p.join(".gitattributes"),
        "model.bin filter=lfs diff=lfs merge=lfs -text\n",
    )
    .unwrap();
    fs::write(p.join("with space.txt"), "spaced\n").unwrap();
    fs::write(p.join("latin1.txt"), b"caf\xe9\n").unwrap();
    fs::write(p.join("crlf.txt"), "a\r\nb\r\n").unwrap();
    sh(&p, &["add", "-A"]);
    sh(&p, &["commit", "-qm", "files"]);
    let repo = Repo::open(&p).unwrap();
    let at = |path: &str| (Version::Rev("HEAD".into()), path.to_string());
    let requests = vec![
        at("huge.txt"),
        at("missing.txt"),
        at("image.png"),
        at("model.bin"),
        at("limit.txt"),
        at("with space.txt"),
        (Version::Rev("HEAD".into()), "".into()), // the root tree
        at("late-nul.txt"),
        at("latin1.txt"),
        at("crlf.txt"),
        (Version::Index, "with space.txt".into()),
        (Version::Worktree, "huge.txt".into()),
        (Version::Rev("no-such-rev".into()), "a.txt".into()),
        at("model.bin"),
    ];
    let got = repo.read_versions(&requests).unwrap();
    assert_eq!(got.len(), requests.len());
    assert_eq!(got[0], None, "over the size limit");
    assert_eq!(got[1], None, "missing");
    assert_eq!(got[2], None, "binary");
    assert_eq!(
        got[3].as_deref(),
        Some(lfs),
        "an LFS pointer is shown as its text"
    );
    assert_eq!(
        got[4].as_deref(),
        Some(limit.as_str()),
        "exactly at the limit"
    );
    assert_eq!(got[5].as_deref(), Some("spaced\n"));
    assert_eq!(got[6], None, "a tree is not a file");
    assert!(
        got[7].is_some(),
        "a NUL after the first 8000 bytes is not sniffed"
    );
    assert_eq!(got[8].as_deref(), Some("caf\u{fffd}\n"));
    assert_eq!(got[9].as_deref(), Some("a\r\nb\r\n"));
    assert_eq!(got[10].as_deref(), Some("spaced\n"));
    assert_eq!(got[11], None, "working-tree file over the limit");
    assert_eq!(got[12], None, "unknown revision");
    assert_eq!(
        got[13].as_deref(),
        Some(lfs),
        "batch stays in step after misses"
    );
}
