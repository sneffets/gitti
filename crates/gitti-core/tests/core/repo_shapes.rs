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

use crate::fixture_kit::{object_id, write_objects, write_ref};

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

/// `git init` in `<tmp>/<name>` with a local identity and stable settings. The refs in
/// files whatever git's default (git 3.0: reftable): these shapes write loose refs,
/// `packed-refs` and `HEAD` by hand.
fn init_in(root: &Path, name: &str, extra: &[&str]) -> PathBuf {
    let p = root.join(name);
    fs::create_dir_all(&p).unwrap();
    let mut args = crate::fixture_kit::FILES_REF_FORMAT.to_vec();
    args.extend(["init", "-q", "-b", "main"]);
    args.extend_from_slice(extra);
    sh(&p, &args);
    // Config written directly: five `git config` processes cost ~0.6 s on Windows. Our
    // fixtures decide themselves when a commit-graph exists (gc.auto, writeCommitGraph).
    let config = p.join(".git").join("config");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str(
        "[user]\n\tname = T\n\temail = t@example.com\n[core]\n\tautocrlf = false\n\
         [gc]\n\tauto = 0\n[fetch]\n\twriteCommitGraph = false\n",
    );
    fs::write(&config, text).unwrap();
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

fn spec(parents: &[usize], time: i64) -> Spec {
    Spec {
        parents: parents.to_vec(),
        time,
    }
}

/// Writes `specs` (parents first) with one `git fast-import` and points `branches` at
/// them; returns the commit ids in spec order. Commit `i` writes `i` to `f<i % 16>.txt`.
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
        // Sixteen files in turn: trees stay small however long the history (every commit
        // is unique by its message anyway).
        let body = format!("{i}\n");
        s.push_str(&format!(
            "M 644 inline f{}.txt\ndata {}\n{body}\n",
            i % 16,
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

// ------------------------------------------------------------------ graph shapes

#[gitti_test::test]
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

#[gitti_test::test]
fn random_dags_layout_only() {
    // …and the pure layout for many more (no git): rows in creation order reversed are a
    // valid display order (children before parents). 15 DAGs daily, 60 with
    // `GITTI_TEST_FULL=1`: 0.08 s alone, but up to 2.6 s for 60 in the suite (the parallel
    // tests' git starts fork this process and stall its allocations).
    let seeds = if std::env::var_os("GITTI_TEST_FULL").is_some() { 60 } else { 15 };
    for seed in 1..=seeds {
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

/// Workflow: CM-20
#[gitti_test::test]
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

/// Workflow: CM-20
#[gitti_test::test]
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

#[gitti_test::test]
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

// SHA-256 repositories: `sha256.rs`.

// ------------------------------------------------------------------ refs

// ------------------------------------------------------------------ opening / discovery

#[gitti_test::test]
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

#[gitti_test::test]
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

#[gitti_test::test]
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

/// A folder that is not there (a drive not connected) is not "not a repository": typed,
/// so gitti keeps a saved tab on it, offers no "Initialize Repository Here" (it would
/// create the folder where the drive mounts) and reads it once it is back. No git process.
#[gitti_test::test]
fn missing_folder_is_its_own_error() {
    let t = tempfile::tempdir().unwrap();
    let gone = t.path().join("drive").join("repo");
    let err = Repo::open(&gone).unwrap_err();
    let kind = err.downcast_ref::<gitti_core::OpenError>().cloned();
    assert_eq!(kind, Some(gitti_core::OpenError::FolderMissing(gone.clone())));
    assert_eq!(kind.unwrap().title(), "Folder not found");
    let msg = err.to_string();
    assert!(msg.contains("does not exist") && msg.contains("not connected"), "{msg}");
}


/// A repository kept apart from its work tree ("dotfiles": `core.worktree` names the
/// folder, which has no `.git`): opening the git dir is refused with how to link them —
/// git run in the work tree (as gitti / gitty do) would not find it; linked by a `.git`
/// file it opens from either side. Started with `GIT_DIR` / `GIT_WORK_TREE` naming it while
/// the folder leads nowhere (or elsewhere), the start is refused; a path given that is not
/// `GIT_WORK_TREE` (a hook starting the app for another repository) ignores them.
#[gitti_test::test]
fn separate_git_dir_with_a_work_tree() {
    use gitti_core::OpenError;
    use gitti_core::open_check::LaunchEnv;
    use gitti_core::paths::canonical;
    let t = tempfile::tempdir().unwrap();
    let (git_dir, home) = (t.path().join("cfg"), t.path().join("home"));
    fs::create_dir_all(&home).unwrap();
    crate::fx::bare(&git_dir);
    let config = git_dir.join("config");
    fs::write(
        &config,
        format!(
            "[core]\n\trepositoryformatversion = 0\n\tbare = false\n\tworktree = {}\n",
            home.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    let (git_dir, home) = (canonical(&git_dir), canonical(&home));
    let err = Repo::open(&git_dir).unwrap_err();
    let want = OpenError::WorkTreeElsewhere { git_dir: git_dir.clone(), work_tree: home.clone() };
    assert_eq!(err.downcast_ref::<OpenError>(), Some(&want), "{err:#}");
    assert!(err.to_string().contains("gitdir: "), "{err}");

    // Started in home with the variables: refused while home leads nowhere.
    let env = LaunchEnv::from_values(None, &home, Some(git_dir.as_os_str()), Some(".".as_ref()))
        .unwrap();
    assert_eq!((env.path.as_path(), env.work_tree.as_deref()), (home.as_path(), Some(home.as_path())));
    let problem = env.problem(None).unwrap();
    assert!(problem.to_string().contains("GIT_DIR="), "{problem}");
    // A hook of another repository starting the app for `ours`: the path wins.
    let ours = t.path().join("ours");
    assert_eq!(LaunchEnv::from_values(Some(&ours), &home, Some(git_dir.as_os_str()), None), None);
    assert_eq!(
        LaunchEnv::from_values(Some(&home), t.path(), Some(git_dir.as_os_str()), Some(home.as_os_str()))
            .map(|e| e.path),
        Some(home.clone())
    );

    // Linked by a .git file: opens from both sides, the variables agree.
    fs::write(home.join(".git"), format!("gitdir: {}\n", git_dir.display())).unwrap();
    for open in [&git_dir, &home] {
        let r = Repo::open(open).unwrap();
        assert_eq!((r.workdir(), r.git_dir()), (home.as_path(), Some(git_dir.clone())));
    }
    assert_eq!(env.problem(Some(&Repo::open(&home).unwrap())), None);
}

#[gitti_test::test]
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

// ------------------------------------------------------------------ fast-import fixtures

fn rev(dir: &Path, r: &str) -> ObjectId {
    oid(&sh(dir, &["rev-parse", r]))
}

/// A chain of `n` commits, one second apart from `t0`.
fn linear(n: usize, t0: i64) -> Vec<Spec> {
    (0..n)
        .map(|i| Spec { parents: if i == 0 { Vec::new() } else { vec![i - 1] }, time: t0 + i as i64 })
        .collect()
}

fn import_named(dir: &Path, specs: &[Spec], branches: &[(String, usize)]) -> Vec<ObjectId> {
    let b: Vec<(&str, usize)> = branches.iter().map(|(n, i)| (n.as_str(), *i)).collect();
    import(dir, specs, &b)
}

/// Same rows, ids, parents and graph.
fn assert_same_history(a: &History, b: &History) {
    let ids = |h: &History| h.commits.iter().map(|c| (c.id, c.parents.to_vec())).collect::<Vec<_>>();
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

/// Raw commits on top of each other (`parent` first): one `git hash-object` for all.
fn raw_commits(dir: &Path, tree: ObjectId, parent: Option<ObjectId>, heads: &[(&str, &str, String)]) -> Vec<ObjectId> {
    let mut prev = parent;
    let mut bodies = Vec::new();
    for (author, committer, message) in heads {
        let mut body = format!("tree {tree}\n");
        if let Some(p) = prev {
            body.push_str(&format!("parent {p}\n"));
        }
        body.push_str(&format!("author {author}\ncommitter {committer}\n\n{message}"));
        prev = Some(object_id("commit", body.as_bytes()));
        bodies.push(body.into_bytes());
    }
    write_objects(dir, "commit", &bodies)
}

/// The empty tree, written (one git process).
fn empty_tree(dir: &Path) -> ObjectId {
    write_objects(dir, "tree", &[Vec::new()])[0]
}

#[gitti_test::test]
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
        assert_eq!(h.child_rows(h.row_of(&ids[0]).unwrap()).len(), 6);
    }
}

#[gitti_test::test]
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
    let bases = sh(&p, &["merge-base", "--all", &ids[5].to_string(), &ids[6].to_string()]);
    assert_eq!(bases.lines().count(), 2);
}

#[gitti_test::test]
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
        let roots: Vec<_> = h.commits.iter().skip(1).filter(|c| c.parents.is_empty()).map(|c| c.id).collect();
        assert_eq!(roots.len(), 3);
        // A root ends its lane: nothing continues below it in that lane.
        let rows = h.graph.rows(0..h.commits.len());
        for root in [ids[0], ids[2], ids[4]] {
            assert!(roots.contains(&root));
            let r = h.row_of(&root).unwrap();
            assert!(
                !rows[r].segments.iter().any(|s| s.half == Half::Bottom && s.from == rows[r].node && s.to == rows[r].node),
                "root at row {r} has no line below"
            );
        }
    }
}

/// Workflow: BG-03
#[gitti_test::test]
fn very_long_linear_history() {
    let (_t, p) = new_repo(&[]);
    let n = 2000;
    import(&p, &linear(n, 1_000_000), &[("main", n - 1)]);
    let repo = Repo::open(&p).unwrap();
    let h = history(&repo, LogOrder::Topo);
    assert_eq!(h.commits.len(), n + 1);
    check_history(&p, &h, 5);
    let rows = h.graph.rows(0..n + 1);
    assert!(rows.iter().all(|r| r.node == 0 && r.width == 1));
    assert_eq!(h.graph.max_lanes(), 1);
}

/// The loaded history stays compact — 5000 commits (eight branches, a merge in
/// twenty, empty trees: one `git fast-import`) cost well under 100 bytes each in rows,
/// id lookup and graph layout (it was ~300). 47 bytes at 5000 as at 20 000, which took
/// 2.4 s under the full suite's load.
/// Workflow: BG-03
#[gitti_test::test]
fn history_rows_are_compact() {
    let (_t, p) = new_repo(&[]);
    let n = 5_000;
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut tips: [Option<usize>; 8] = [None; 8];
    let mut s = String::with_capacity(n * 100);
    for i in 1..=n {
        let b = if i < 32 { 0 } else { rng.below(8) };
        s.push_str(&format!(
            "commit refs/heads/b{b}\nmark :{i}\ncommitter T <t@x> {} +0000\ndata 0\n",
            1_000_000 + i * 60
        ));
        match tips[b] {
            Some(t) => s.push_str(&format!("from :{t}\n")),
            None if i > 1 => s.push_str(&format!("from :{}\n", i - 1)),
            None => {}
        }
        let o = rng.below(8);
        if i >= 32
            && o != b
            && rng.below(20) == 0
            && let Some(t) = tips[o]
        {
            s.push_str(&format!("merge :{t}\n"));
        }
        s.push('\n');
        tips[b] = Some(i);
    }
    sh_stdin(&p, &["fast-import", "--quiet"], s.as_bytes());
    let repo = Repo::open(&p).unwrap();
    let h = history(&repo, LogOrder::Topo);
    assert_eq!(h.commits.len(), n + 1);
    let per_commit = h.heap_bytes() as f64 / h.commits.len() as f64;
    assert!(per_commit < 70.0, "{per_commit:.1} bytes per commit");
    // Spot checks of the compact rows: lookups, parents, children above parents.
    for row in (1..=n).step_by(997) {
        let c = h.commits.at(row);
        assert_eq!(h.row_of(&c.id), Some(row));
        assert!(h.parent_rows(row).iter().all(|&p| p > row));
        assert_eq!(h.parent_rows(row).len(), c.parents.len());
    }
}

/// Real repositories (sorting + loading) for a few small random DAGs; the pure layout of
/// many more is `random_dags_layout_only`.
#[gitti_test::test]
fn random_dags_keep_the_lane_invariants() {
    for seed in 1..=2u64 {
        let (_t, p) = new_repo(&[]);
        let (specs, branches) = random_dag(200, seed);
        import_named(&p, &specs, &branches);
        let repo = Repo::open(&p).unwrap();
        for order in [LogOrder::Topo, LogOrder::Date] {
            check_history(&p, &history(&repo, order), seed);
        }
    }
}

/// A fresh commit-graph file gives the same history; a stale one (new commits, one
/// merging an old commit) too, also for the quick partial history.
#[gitti_test::test]
fn commit_graph_file_fresh_and_stale() {
    let (_t, p) = new_repo(&[]);
    let (specs, branches) = random_dag(150, 77);
    let ids = import_named(&p, &specs, &branches);
    let repo = Repo::open(&p).unwrap();
    let plain = history(&repo, LogOrder::Date);
    assert!(!plain.used_commit_graph);
    check_graph(&plain, 6);

    repo.write_commit_graph().unwrap();
    assert!(commit_graph_file(&p).exists());
    let cg = history(&repo, LogOrder::Date);
    assert!(cg.used_commit_graph);
    assert_same_history(&plain, &cg);

    // Stale: new commits (incl. a merge of an old commit) are not in the file.
    let tree = empty_tree(&p);
    let tip = *ids.last().unwrap();
    let a_body = format!("tree {tree}\nparent {tip}\nauthor T <t@x> 1700000000 +0000\ncommitter T <t@x> 1700000000 +0000\n\nnew 1\n");
    let a = object_id("commit", a_body.as_bytes());
    let m_body = format!(
        "tree {tree}\nparent {a}\nparent {}\nauthor T <t@x> 1700000100 +0000\ncommitter T <t@x> 1700000100 +0000\n\nnew merge\n",
        ids[100]
    );
    let m = write_objects(&p, "commit", &[a_body.into_bytes(), m_body.into_bytes()])[1];
    write_ref(&p, "refs/heads/main", &m.to_string());
    write_ref(&p, "refs/heads/fresh", &a.to_string());
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
}

/// A split chain (base layer + an incremental one), a corrupt or truncated file and
/// `core.commitGraph=false`: always the history of the object database.
#[gitti_test::test]
fn commit_graph_split_corrupt_and_disabled() {
    let (_t, p) = new_repo(&[]);
    let (specs, branches) = random_dag(150, 78);
    let ids = import_named(&p, &specs, &branches);
    let repo = Repo::open(&p).unwrap();
    sh(&p, &["commit-graph", "write", "--reachable", "--split"]);
    let tree = empty_tree(&p);
    let b = raw_commits(&p, tree, Some(*ids.last().unwrap()), &[(
        "T <t@x> 1700000200 +0000",
        "T <t@x> 1700000200 +0000",
        "after split\n".into(),
    )])[0];
    write_ref(&p, "refs/heads/main", &b.to_string());
    sh(&p, &["commit-graph", "write", "--reachable", "--split=no-merge"]);
    let chain = p.join(".git/objects/info/commit-graphs/commit-graph-chain");
    assert!(fs::read_to_string(&chain).unwrap().lines().count() >= 2, "two layers");
    let split = history(&repo, LogOrder::Date);
    assert!(split.used_commit_graph);
    check_history(&p, &split, 8);
    fs::remove_dir_all(p.join(".git/objects/info/commit-graphs")).unwrap();
    let split_plain = history(&repo, LogOrder::Date);
    assert_same_history(&split, &split_plain);

    // Corrupt or truncated file: the history still loads (from the object database).
    repo.write_commit_graph().unwrap();
    let good = fs::read(commit_graph_file(&p)).unwrap();
    for bad in [b"garbage, not a commit-graph".to_vec(), good[..good.len() / 2].to_vec(), Vec::new()] {
        replace_file(&commit_graph_file(&p), &bad);
        assert_same_history(&history(&repo, LogOrder::Date), &split_plain);
    }
    // core.commitGraph=false: the file is ignored.
    replace_file(&commit_graph_file(&p), &good);
    crate::fixture_kit::append_config(&p, "[core]\n\tcommitGraph = false\n");
    let off = history(&repo, LogOrder::Date);
    assert!(!off.used_commit_graph);
    assert_same_history(&off, &split_plain);
}

/// Workflow: BG-03
#[gitti_test::test]
fn partial_history_is_the_newest_slice_of_the_full_one() {
    let (_t, p) = new_repo(&[]);
    // Distinct, monotonic timestamps: date order is unambiguous.
    let n = 200usize;
    let mut specs = vec![spec(&[], 1000)];
    let mut rng = Rng(99);
    for i in 1..n {
        let parents = if i > 5 && rng.below(5) == 0 { vec![i - 1, i - 2 - rng.below(4)] } else { vec![i - 1] };
        specs.push(Spec { parents, time: 1000 + i as i64 * 10 });
    }
    import(&p, &specs, &[("main", n - 1), ("old", 120)]);
    let repo = Repo::open(&p).unwrap();
    let full = history(&repo, LogOrder::Date);
    assert!(!full.partial);
    for limit in [1, 10, 64, 65, n - 1, n, 1000] {
        let quick = repo.history_newest(LogOrder::Date, limit).unwrap();
        assert!(quick.partial);
        let got: Vec<ObjectId> = quick.commits.iter().skip(1).map(|c| c.id).collect();
        let want: Vec<ObjectId> = full.commits.iter().skip(1).take(limit).map(|c| c.id).collect();
        assert_eq!(got, want, "limit {limit}");
        // Lanes of parents that are not loaded yet run to the bottom.
        check_graph(&quick, limit as u64);
    }
    // Topo order: the partial set is the newest commits, children above parents.
    let quick = repo.history_newest(LogOrder::Topo, 120).unwrap();
    let mut by_time: Vec<_> = full.commits.iter().skip(1).map(|c| (c.commit_time, c.id)).collect();
    by_time.sort_by(|a, b| b.cmp(a));
    let newest: HashSet<ObjectId> = by_time.iter().take(120).map(|(_, id)| *id).collect();
    let got: HashSet<ObjectId> = quick.commits.iter().skip(1).map(|c| c.id).collect();
    assert_eq!(got, newest);
    check_graph(&quick, 9);
}

/// Odd commit metadata read lazily per row: empty / unterminated / blank-led / two-line /
/// huge subjects, identical times, times before 1970 and far ahead, odd time zones, a
/// committer other than the author, Unicode names, an empty email — like git shows it.
#[gitti_test::test]
fn lazy_meta_of_odd_commits() {
    let (_t, p) = new_repo(&[]);
    let tree = empty_tree(&p);
    let long = "x".repeat(10_000);
    let c = |n: i64| format!("C <c@x> {n} +0000");
    let a = |n: i64| format!("A <a@x> {n} +0000");
    let cases: Vec<(String, String, String)> = vec![
        (a(1000000000), c(1000000000), "".into()),
        (a(1000000001), c(1000000001), "no newline".into()),
        (a(1000000002), c(1000000002), "\n\nleading blank lines\n".into()),
        (a(1000000003), c(1000000003), "first\nsecond line of the subject\n\nbody\n".into()),
        (a(1000000004), c(1000000004), format!("{long}\n")),
        (a(1000000004), c(1000000004), "same time\n".into()),
        ("Old <o@x> -86400 -0130".into(), c(1000000005), "author before 1970\n".into()),
        ("Future <f@x> 4102444800 +1400".into(), c(1000000006), "year 2100\n".into()),
        (
            "Ada Lovelace <ada@example.com> 1000000007 +0100".into(),
            "Bot <bot@ci> 1000000107 -0700".into(),
            "committed by someone else\n".into(),
        ),
        ("Ünïcödé 名前 <u@x> 1000000008 +0000".into(), c(1000000008), "subject with ünïcödé ✓\n".into()),
        ("NoEmail <> 1000000009 +0000".into(), c(1000000009), "empty email\n".into()),
    ];
    let heads: Vec<(&str, &str, String)> = cases.iter().map(|(a, c, m)| (a.as_str(), c.as_str(), m.clone())).collect();
    let ids = raw_commits(&p, tree, None, &heads);
    write_ref(&p, "refs/heads/main", &ids.last().unwrap().to_string());
    // git's view of all of them in one call.
    let out = crate::fixture_kit::sh(&p, &["log", "--format=%H%x00%an%x00%ae%x00%at%x00%s%x01", "main"]);
    let git: HashMap<ObjectId, Vec<String>> = out
        .split('\u{1}')
        .map(|r| r.trim_start_matches('\n'))
        .filter(|r| !r.is_empty())
        .map(|r| {
            let f: Vec<String> = r.split('\0').map(String::from).collect();
            (oid(&f[0]), f[1..].to_vec())
        })
        .collect();
    assert_eq!(git.len(), ids.len());
    let repo = Repo::open(&p).unwrap();
    for order in [LogOrder::Topo, LogOrder::Date] {
        let h = history(&repo, order);
        check_graph(&h, 10);
        for id in &ids {
            let m = h.meta(h.row_of(id).unwrap());
            let f = &git[id];
            assert_eq!((m.author.as_str(), m.email.as_str()), (f[0].as_str(), f[1].as_str()), "{id}");
            // git prints no date before 1970 (we do).
            if !f[2].is_empty() {
                assert_eq!(m.time.to_string(), f[2], "{id}");
            }
            assert_eq!(m.summary, f[3].trim(), "{id}: summary like git's %s");
        }
        // The working-directory row has empty meta; rows past the end too.
        assert!(h.meta(0).summary.is_empty());
        assert!(h.meta(h.commits.len() + 5).summary.is_empty());
    }
    let h = history(&repo, LogOrder::Date);
    let m = h.meta(h.row_of(&ids[6]).unwrap());
    assert_eq!((m.time, m.tz_offset), (-86400, -(90 * 60)));
    assert_eq!(h.meta(h.row_of(&ids[4]).unwrap()).summary.len(), 10_000);
    let d = repo.commit_details(ids[8]).unwrap();
    assert_eq!((d.author.as_str(), d.committer.as_str()), ("Ada Lovelace", "Bot"));
    assert_eq!((d.author_time, d.commit_time), (1000000007, 1000000107));
    assert_eq!(repo.commit_details(ids[9]).unwrap().author, "Ünïcödé 名前");
}

#[gitti_test::test]
fn extreme_commit_times_with_and_without_commit_graph() {
    // Committer times before 1970, at 0, beyond 2^32 and beyond the commit-graph's 34 bits.
    let (_t, p) = new_repo(&[]);
    let tree = empty_tree(&p);
    let times = ["-1000000", "0", "1", "4294967297", "17179869184", "99999999999", "1500000000"];
    let bodies: Vec<Vec<u8>> = times
        .iter()
        .enumerate()
        .map(|(k, t)| format!("tree {tree}\nauthor A <a@x> {t} +0000\ncommitter C <c@x> {t} +0000\n\nat {t} ({k})\n").into_bytes())
        .collect();
    let tips = write_objects(&p, "commit", &bodies);
    for (k, id) in tips.iter().enumerate() {
        write_ref(&p, &format!("refs/heads/t{k}"), &id.to_string());
    }
    write_ref(&p, "refs/heads/main", &tips[6].to_string());
    let repo = Repo::open(&p).unwrap();
    let plain = history(&repo, LogOrder::Date);
    check_graph(&plain, 21);
    assert_eq!(plain.commits.len(), 8);
    let got: Vec<i64> = plain.commits.iter().skip(1).map(|c| c.commit_time).collect();
    let mut sorted = got.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(got, sorted, "date order, newest first");
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
        let want = if (0..1 << 34).contains(&t) { t } else { t.rem_euclid(1 << 34) };
        assert_eq!(time_of(&cg, id), want, "tip {k}");
    }
}

/// A source repository with some history and several branches.
fn source_repo(root: &Path) -> PathBuf {
    let p = init_in(root, "source", &[]);
    let (specs, branches) = random_dag(60, 5);
    import_named(&p, &specs, &branches);
    p
}

fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') { format!("file://{s}") } else { format!("file:///{s}") }
}

#[gitti_test::test]
fn shallow_clone_shows_grafted_commits_as_roots() {
    let t = tempfile::tempdir().unwrap();
    let src = source_repo(t.path());
    for depth in ["1", "3"] {
        let name = format!("shallow{depth}");
        sh(t.path(), &["clone", "-q", "--no-single-branch", "--depth", depth, &file_url(&src), &name]);
        let p = t.path().join(&name);
        assert!(p.join(".git/shallow").exists());
        let repo = Repo::open(&p).unwrap();
        for order in [LogOrder::Topo, LogOrder::Date] {
            let h = history(&repo, order);
            // Like `git log`: the boundary commits have no parents (no dangling lanes).
            check_history(&p, &h, 11);
            check_graph(&repo.history_newest(order, 5).unwrap(), 12);
        }
        let h = history(&repo, LogOrder::Topo);
        let n = h.commits.len();
        let last = &h.graph.rows(n - 1..n)[0];
        assert!(last.segments.iter().all(|s| s.half == Half::Top), "no lane below the last row");
        assert!(!h.meta(1).summary.is_empty());
    }
}

#[gitti_test::test]
fn partial_clone_without_trees_opens() {
    let t = tempfile::tempdir().unwrap();
    let src = source_repo(t.path());
    crate::fixture_kit::append_config(&src, "[uploadpack]\n\tallowFilter = true\n\tallowAnySHA1InWant = true\n");
    sh(t.path(), &["clone", "-q", "--filter=tree:0", &file_url(&src), "partial"]);
    let p = t.path().join("partial");
    let repo = Repo::open(&p).unwrap();
    let h = history(&repo, LogOrder::Topo);
    check_history(&p, &h, 13);
    assert!(!h.meta(1).summary.is_empty());
    assert!(repo.status().unwrap().entries.is_empty());
    // An old file version is fetched lazily from the promisor remote.
    let old = h.commits.at(h.commits.len() - 1).id.to_string();
    let got = repo.read_versions(&[(Version::Rev(old), "f0.txt".into())]).unwrap();
    assert_eq!(got[0].as_deref(), Some("0\n"));
}

#[gitti_test::test]
fn odd_ref_names_packed_loose_symbolic_and_broken() {
    let (_t, p) = new_repo(&[]);
    let ids = import(&p, &linear(4, 100), &[("main", 3)]);
    let names = ["feature/deep/nested/x", "fix-ü-分支", "user@host", "v1.2.3", "release/1.0.x", "dash-", "UPPER", "upper2"];
    for n in names {
        write_ref(&p, &format!("refs/heads/{n}"), &ids[1].to_string());
    }
    write_ref(&p, "refs/tags/tag/with/slashes", &ids[0].to_string());
    write_ref(&p, "refs/tags/ünï-tag", &ids[2].to_string());
    // Packed and loose ref of the same name: the loose one wins.
    fs::write(
        p.join(".git/packed-refs"),
        format!("# pack-refs with: peeled fully-peeled sorted \n{} refs/heads/both\n", ids[0]),
    )
    .unwrap();
    write_ref(&p, "refs/heads/both", &ids[2].to_string());
    assert_eq!(rev(&p, "both"), ids[2], "git agrees");
    // Symbolic ref to a branch; remote-tracking branches with a remote HEAD, and a remote
    // with dots in its name.
    write_ref(&p, "refs/heads/alias", "ref: refs/heads/main");
    write_ref(&p, "refs/remotes/my.remote/feature/a", &ids[1].to_string());
    write_ref(&p, "refs/remotes/origin/main", &ids[3].to_string());
    write_ref(&p, "refs/remotes/origin/HEAD", "ref: refs/remotes/origin/main");
    // Broken refs: an unknown object and garbage content are skipped.
    write_ref(&p, "refs/heads/missing", &"1".repeat(40));
    write_ref(&p, "refs/heads/garbage", "not a ref at all");

    let repo = Repo::open(&p).unwrap();
    let refs = repo.refs().unwrap();
    let find = |short: &str| refs.iter().find(|r| r.short_name == short);
    for n in names {
        let r = find(n).unwrap_or_else(|| panic!("{n} listed"));
        assert_eq!((r.kind, r.target), (RefKind::LocalBranch, ids[1]), "{n}");
        assert_eq!(r.full_name, format!("refs/heads/{n}"));
    }
    assert_eq!(find("both").unwrap().target, ids[2], "loose wins over packed");
    assert_eq!(refs.iter().filter(|r| r.short_name == "both").count(), 1);
    assert_eq!(find("alias").map(|r| r.target), Some(ids[3]));
    assert_eq!(find("tag/with/slashes").map(|r| r.kind), Some(RefKind::Tag));
    assert_eq!(find("ünï-tag").map(|r| r.target), Some(ids[2]));
    assert_eq!(find("my.remote/feature/a").map(|r| r.kind), Some(RefKind::RemoteBranch));
    assert!(find("origin/HEAD").is_none(), "remote HEAD is not listed");
    assert!(find("missing").is_none() && find("garbage").is_none());
    // Sorted by kind, then name.
    let keys: Vec<_> = refs.iter().map(|r| (r.kind, r.short_name.clone())).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);

    let h = history(&repo, LogOrder::Topo);
    check_graph(&h, 15);
    assert_eq!(h.commits.len(), 5);
    let deco: Vec<_> = h.refs_by_commit[&ids[1]].iter().map(|r| r.short_name.as_str()).collect();
    for n in names {
        assert!(deco.contains(&n), "{n} decorates its commit");
    }
    assert_eq!(repo.head().unwrap().branch.as_deref(), Some("main"));
    // Ops keep working with such names.
    repo.create_branch("neu/ö@x", &ids[0].to_string(), true).unwrap();
    assert_eq!(repo.head().unwrap().branch.as_deref(), Some("neu/ö@x"));
}

/// Workflow: TG-06
#[gitti_test::test]
fn tags_on_tags_and_on_non_commits() {
    use crate::fixture_kit::{History, f, repo_with};
    let mut hist = History::new();
    let one = hist.commit("main", &[], "one", &[("a.txt", f("a\n"))]);
    let (_t, p, cids) = repo_with(&hist);
    let c = cids[one];
    let blob = object_id("blob", b"a\n");
    let mut tree_body = b"100644 a.txt\0".to_vec();
    tree_body.extend_from_slice(blob.as_bytes());
    let tree = object_id("tree", &tree_body);
    // Annotated tags (also of tags, trees and blobs) as raw objects, one git call.
    let tag = |target: ObjectId, kind: &str, name: &str, msg: &str| {
        format!("object {target}\ntype {kind}\ntag {name}\ntagger T <t@x> 1700000000 +0000\n\n{msg}\n")
    };
    let inner = tag(c, "commit", "inner", "inner");
    let outer = tag(object_id("tag", inner.as_bytes()), "tag", "outer", "tag of a tag");
    let outermost = tag(object_id("tag", outer.as_bytes()), "tag", "outermost", "tag of a tag of a tag");
    let bodies = [inner, outer, outermost, tag(tree, "tree", "annotated-tree", "a tree"), tag(blob, "blob", "annotated-blob", "a blob")];
    let names = ["inner", "outer", "outermost", "annotated-tree", "annotated-blob"];
    let tags = write_objects(&p, "tag", &bodies.map(String::into_bytes));
    let mut refs: Vec<(String, String)> =
        names.iter().zip(&tags).map(|(name, id)| (format!("refs/tags/{name}"), id.to_string())).collect();
    refs.push(("refs/tags/light-tree".into(), tree.to_string()));
    refs.push(("refs/tags/light-blob".into(), blob.to_string()));
    let refs: Vec<(&str, String)> = refs.iter().map(|(n, id)| (n.as_str(), id.clone())).collect();
    crate::fixture_kit::update_refs(&p, &refs);
    assert_eq!(rev(&p, "outermost^{commit}"), c, "git peels it too");

    let repo = Repo::open(&p).unwrap();
    let refs = repo.refs().unwrap();
    let target = |n: &str| refs.iter().find(|r| r.short_name == n).map(|r| r.target);
    for n in ["inner", "outer", "outermost"] {
        assert_eq!(target(n), Some(c), "{n} peels to the commit");
    }
    assert_eq!(target("light-tree"), Some(tree));
    assert_eq!(target("annotated-blob"), Some(blob));
    let h = history(&repo, LogOrder::Topo);
    assert_eq!(h.commits.len(), 2, "trees and blobs are not history tips");
    check_graph(&h, 16);
    let deco: Vec<_> = h.refs_by_commit[&c].iter().map(|r| r.short_name.as_str()).collect();
    assert!(deco.contains(&"outermost") && deco.contains(&"main"));
}

/// Workflow: BR-16
#[gitti_test::test]
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
    assert_eq!(repo.history_newest(LogOrder::Date, 10).unwrap().commits.len(), 1);
    // The first commit can be staged and made from there.
    fs::write(p.join("a.txt"), "one\n").unwrap();
    assert_eq!(repo.working_state().unwrap().unstaged.len(), 1);
    repo.stage_paths(&["a.txt"]).unwrap();
    repo.commit("first", false).unwrap();
    let h = history(&repo, LogOrder::Topo);
    assert_eq!(h.commits.len(), 2);
    assert_eq!(h.meta(1).summary, "first");

    // Detached HEAD on a commit no branch points at: still in the history.
    fs::write(p.join("a.txt"), "two\n").unwrap();
    sh(&p, &["commit", "-qam", "second"]);
    sh(&p, &["checkout", "-q", "--detach", "HEAD"]);
    sh(&p, &["commit", "-q", "--allow-empty", "-m", "detached work"]);
    let head = repo.head().unwrap();
    let detached = head.id.unwrap();
    assert_eq!(head.branch, None);
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
    assert_eq!(h.commits.len(), 3, "main's two commits; the detached one is unreachable now");
    check_graph(&h, 19);
    // Everything of main is staged in the orphan branch's first commit-to-be.
    assert_eq!(repo.working_state().unwrap().staged.len(), 1);
}

#[gitti_test::test]
fn stashes_in_the_history() {
    let (_t, p) = new_repo(&[]);
    import(&p, &[spec(&[], 100)], &[("main", 0)]);
    // Stash 1: tracked change only (2 parents).
    fs::write(p.join("f0.txt"), "a1\n").unwrap();
    sh(&p, &["stash", "push", "-q", "-m", "tracked"]);
    // Stash 0: staged + untracked (3 parents).
    fs::write(p.join("f0.txt"), "a2\n").unwrap();
    sh(&p, &["add", "f0.txt"]);
    fs::write(p.join("new.txt"), "n\n").unwrap();
    sh(&p, &["stash", "push", "-q", "-u", "-m", "with untracked"]);
    sh(&p, &["commit", "-q", "--allow-empty", "-m", "after"]);
    let s0 = rev(&p, "stash@{0}");
    let repo = Repo::open(&p).unwrap();
    for order in [LogOrder::Topo, LogOrder::Date] {
        let h = history(&repo, order);
        assert_same_structure(&p, &h);
        check_graph(&h, 20);
        let stash_rows: Vec<_> = h.commits.iter().filter_map(|c| c.stash).collect();
        assert_eq!(stash_rows.len(), 2);
        assert!(stash_rows.contains(&0) && stash_rows.contains(&1));
        let helpers = h.commits.iter().filter(|c| c.stash_helper).count();
        assert_eq!(helpers, 3, "index commit of both stashes + untracked commit");
        assert_eq!(h.commits.at(h.row_of(&s0).unwrap()).parents.len(), 3);
        assert_eq!(h.refs_by_commit[&s0][0].kind, RefKind::Stash);
    }
    // The quick first screen leaves stashes out.
    let quick = repo.history_newest(LogOrder::Date, 100).unwrap();
    assert!(quick.commits.iter().all(|c| c.stash.is_none() && !c.stash_helper));
}

/// A submodule's directory opens as its own repository (`.git` file into the
/// superproject's `.git/modules`); the superproject sees it as one entry. The layout is
/// written by hand like `git submodule add` leaves it (see `submodules.rs`).
#[gitti_test::test]
fn submodule_opens_as_its_own_repository() {
    use crate::fixture_kit::{E, History, f};
    let t = tempfile::tempdir().unwrap();
    let sup = gitti_core::paths::canonical(t.path()).join("super");
    // Files: a ref folder is removed and the submodule's git dir written by hand.
    crate::fixture_kit::init_files(&sup);
    let mut hist = History::new();
    let lib = hist.commit("refs/gitti-test/lib", &[], "lib one", &[("lib.txt", f("lib\n"))]);
    let one = hist.commit("main", &[], "super one", &[("top.txt", f("top\n"))]);
    hist.commit("main", &[one], "add lib", &[
        (".gitmodules", f("[submodule \"deps/lib\"]\n\tpath = deps/lib\n\turl = ../lib\n")),
        ("deps/lib", E::Gitlink(lib)),
    ]);
    let ids = hist.import(&sup);
    fs::remove_dir_all(sup.join(".git/refs/gitti-test")).unwrap();
    let config = fs::read_to_string(sup.join(".git/config")).unwrap();
    let module = sup.join(".git/modules/deps/lib");
    crate::fixture_kit::write_git_dir(
        &module,
        &config,
        "ref: refs/heads/main",
        "[core]\n\tworktree = ../../../../deps/lib\n",
        &[("refs/heads/main", ids[lib].to_string())],
        Some(&sup.join(".git/objects")),
    );
    let sub = sup.join("deps/lib");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join(".git"), "gitdir: ../../.git/modules/deps/lib\n").unwrap();
    sh(&sup, &["reset", "-q", "--hard"]);
    sh(&sub, &["reset", "-q", "--hard"]);

    let inner = Repo::open(&sub).unwrap();
    assert_eq!(inner.workdir(), gitti_core::paths::canonical(&sub));
    let h = history(&inner, LogOrder::Topo);
    assert_eq!(h.meta(1).summary, "lib one");
    assert!(inner.git_dir().unwrap().ends_with(Path::new("modules").join("deps").join("lib")));
    fs::write(sub.join("lib.txt"), "lib changed\n").unwrap();
    assert_eq!(inner.status().unwrap().entries[0].path, "lib.txt");

    let outer = Repo::open(&sup).unwrap();
    assert_eq!(history(&outer, LogOrder::Topo).commits.len(), 3);
    // The modified submodule shows as one entry of the superproject.
    assert!(outer.status().unwrap().entries.iter().any(|e| e.path == "deps/lib"));
    // A version read at a commit where the path is a submodule is not a file.
    let got = outer.read_versions(&[(Version::Rev("HEAD".into()), "deps/lib".into())]).unwrap();
    assert_eq!(got, vec![None]);
}

/// Environment of a git hook / alias: `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE` and
/// `GIT_COMMON_DIR` point at another repository — none of them reaches gitti's reads and
/// writes (checked in a child process, the environment is process-wide; all four at once).
#[gitti_test::test]
fn git_dir_in_the_environment_does_not_leak() {
    use crate::fixture_kit::{History, f, repo_with};
    let mut hist = History::new();
    hist.commit("main", &[], "our commit", &[("ours.txt", f("ours\n"))]);
    let (_t1, ours, _) = repo_with(&hist);
    fs::write(ours.join("ours.txt"), "changed\n").unwrap();
    let mut hist = History::new();
    hist.commit("main", &[], "their commit", &[("other.txt", f("other\n"))]);
    let (_t2, other, _) = repo_with(&hist);
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "repo_shapes::env_leak_child", "--ignored", "--nocapture", "--test-threads=1"])
        .env("GITTI_R24E_REPO", &ours)
        .env("GIT_DIR", other.join(".git"))
        .env("GIT_WORK_TREE", &other)
        .env("GIT_INDEX_FILE", other.join(".git").join("index"))
        .env("GIT_COMMON_DIR", other.join(".git"))
        .output()
        .unwrap();
    let text = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && text.contains("1 passed"), "{text}");
}

#[gitti_test::test]
fn reading_huge_binary_and_lfs_pointer_files_at_a_commit() {
    use crate::fixture_kit::{E, History, f};
    let (_t, p) = new_repo(&[]);
    let huge: String = "0123456789abcdef\n".repeat(MAX_BLOB_BYTES / 17 + 10);
    assert!(huge.len() > MAX_BLOB_BYTES);
    let limit: String = "x".repeat(MAX_BLOB_BYTES - 1) + "\n";
    let mut binary = b"PNG\x00\x01\x02".to_vec();
    binary.extend(std::iter::repeat_n(0xffu8, 5000));
    let mut late_nul = "text\n".repeat(2000).into_bytes();
    late_nul.push(0);
    let lfs = "version https://git-lfs.github.com/spec/v1\noid sha256:4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393\nsize 12345\n";
    let mut hist = History::new();
    hist.commit("main", &[], "files", &[
        ("huge.txt", f(&huge)),
        ("limit.txt", f(&limit)),
        ("image.png", E::File(&binary)),
        ("late-nul.txt", E::File(&late_nul)),
        ("model.bin", f(lfs)),
        (".gitattributes", f("model.bin filter=lfs diff=lfs merge=lfs -text\n")),
        ("with space.txt", f("spaced\n")),
        ("latin1.txt", E::File(b"caf\xe9\n")),
        ("crlf.txt", f("a\r\nb\r\n")),
    ]);
    hist.import(&p);
    // The index (no checkout); the one working-tree file read below.
    sh(&p, &["read-tree", "HEAD"]);
    fs::write(p.join("huge.txt"), &huge).unwrap();
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
    assert_eq!(got[3].as_deref(), Some(lfs), "an LFS pointer is shown as its text");
    assert_eq!(got[4].as_deref(), Some(limit.as_str()), "exactly at the limit");
    assert_eq!(got[5].as_deref(), Some("spaced\n"));
    assert_eq!(got[6], None, "a tree is not a file");
    assert!(got[7].is_some(), "a NUL after the first 8000 bytes is not sniffed");
    // Not UTF-8: decoded per line like the diff rows, Latin-1 here.
    assert_eq!(got[8].as_deref(), Some("café\n"));
    assert_eq!(got[9].as_deref(), Some("a\r\nb\r\n"));
    assert_eq!(got[10].as_deref(), Some("spaced\n"));
    assert_eq!(got[11], None, "working-tree file over the limit");
    assert_eq!(got[12], None, "unknown revision");
    assert_eq!(got[13].as_deref(), Some(lfs), "batch stays in step after misses");
}

// ------------------------------------------------------------------ history updates

/// `random_dag` with distinct commit times (ties would let a full load break them
/// differently from the reused rows — both valid orders).
fn distinct_dag(n: usize, seed: u64) -> (Vec<Spec>, Vec<(String, usize)>) {
    let (mut specs, branches) = random_dag(n, seed);
    let mut used = HashSet::new();
    for s in &mut specs {
        while !used.insert(s.time) {
            s.time += 1;
        }
    }
    (specs, branches)
}

/// A history update equals a full load of the same state; `how` is the way it came about.
fn expect_update(repo: &Repo, prev: &History, how: gitti_core::HistoryLoad) -> History {
    let order = prev.order();
    let h = repo.history_update(prev, order).unwrap().expect("a new history");
    assert_eq!(h.load, how);
    let full = history(repo, order);
    assert_same_history(&h, &full);
    let decorations = |h: &History| {
        let mut v: Vec<(ObjectId, String)> = h
            .refs_by_commit
            .iter()
            .flat_map(|(id, refs)| refs.iter().map(move |r| (*id, r.full_name.clone())))
            .collect();
        v.sort();
        v
    };
    assert_eq!(decorations(&h), decorations(&full));
    h
}

/// A ref change reloads the history only as far as needed — not at all when
/// nothing it shows moved, the same rows for new refs / a moved HEAD, the new commits
/// only when they came on top — and always gives what a full load gives.
/// Workflow: EX-22
#[gitti_test::test]
fn history_updates_reuse_what_did_not_change() {
    use gitti_core::HistoryLoad::{Full, Prepended, SameCommits};
    let (_t, p) = new_repo(&[]);
    let (specs, branches) = distinct_dag(60, 5);
    let ids = import_named(&p, &specs, &branches);
    let repo = Repo::open(&p).unwrap();
    let newest = specs.iter().map(|s| s.time).max().unwrap();
    // Every commit the steps below add, written at once (one git process): per order, two
    // on the first row, a fast-forward, one on an old commit, one older than the rest.
    let tree = empty_tree(&p);
    let first_row = |order| history(&repo, order).commits.at(1).id;
    let mut bodies = Vec::new();
    let mut commit = |parent: ObjectId, t: i64, msg: &str| {
        let body = format!(
            "tree {tree}\nparent {parent}\nauthor T <t@x> {t} +0000\ncommitter T <t@x> {t} +0000\n\n{msg}\n"
        );
        bodies.push(body.clone().into_bytes());
        object_id("commit", body.as_bytes())
    };
    let mut planned = Vec::new();
    for (k, order) in [LogOrder::Topo, LogOrder::Date].into_iter().enumerate() {
        let b = newest + 1000 * (k as i64 + 1);
        let new1 = commit(first_row(order), b + 100, "new 1");
        let new2 = commit(new1, b + 200, "new 2");
        let ff = commit(new2, b + 300, "ff");
        let side = commit(ids[30], b + 400, "side");
        let skewed = commit(ff, b + 293, "skewed");
        planned.push((order, [new2, ff, side, skewed]));
    }
    write_objects(&p, "commit", &bodies);
    for (order, [new2, ff, side, skewed]) in planned {
        let h = history(&repo, order);
        assert_eq!(h.load, Full);
        assert!(repo.history_update(&h, order).unwrap().is_none(), "nothing moved");
        // A tag on a loaded commit, HEAD on another branch: the same commits.
        write_ref(&p, "refs/tags/t-on-old", &ids[40].to_string());
        let h = expect_update(&repo, &h, SameCommits);
        fs::write(p.join(".git/HEAD"), format!("ref: refs/heads/{}\n", branches[0].0)).unwrap();
        let h = expect_update(&repo, &h, SameCommits);
        fs::write(p.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let h = expect_update(&repo, &h, SameCommits);
        // Two new commits on the first row (a commit on the newest branch).
        let top = format!("refs/heads/top-{order:?}");
        write_ref(&p, &top, &new2.to_string());
        let h = expect_update(&repo, &h, Prepended(2));
        // A fast-forward of the branch just made (its old tip is walked to).
        write_ref(&p, &top, &ff.to_string());
        let h = expect_update(&repo, &h, Prepended(1));
        // A new commit on an old commit: on top in date order, but in topological order
        // it would sort elsewhere — loaded in full there.
        let side_ref = format!("refs/heads/side-{order:?}");
        write_ref(&p, &side_ref, &side.to_string());
        let how = if order == LogOrder::Date { Prepended(1) } else { Full };
        let h = expect_update(&repo, &h, how);
        // A commit older than the newest loaded one: loaded in full.
        write_ref(&p, &top, &skewed.to_string());
        let h = expect_update(&repo, &h, Full);
        // A branch deleted: its commits may go — loaded in full.
        fs::remove_file(p.join(".git").join(&side_ref)).unwrap();
        let h = expect_update(&repo, &h, Full);
        // A merge started (MERGE_HEAD on a loaded commit) and aborted: the same commits,
        // the working-directory row laid out as the pending merge and back.
        fs::write(p.join(".git/MERGE_HEAD"), format!("{}\n", ids[20])).unwrap();
        let h = expect_update(&repo, &h, SameCommits);
        assert_eq!(h.merge_heads, [ids[20]]);
        fs::remove_file(p.join(".git/MERGE_HEAD")).unwrap();
        let h = expect_update(&repo, &h, SameCommits);
        assert!(h.merge_heads.is_empty());
        // The other order: loaded in full.
        let other = if order == LogOrder::Topo { LogOrder::Date } else { LogOrder::Topo };
        let o = repo.history_update(&h, other).unwrap().expect("reloaded");
        assert_eq!((o.load, o.order()), (Full, other));
        // A quick partial history is never the base of an update.
        let quick = repo.history_newest(order, 10).unwrap();
        assert_eq!(repo.history_update(&quick, order).unwrap().map(|h| h.load), Some(Full));
        fs::remove_file(p.join(".git/refs/tags/t-on-old")).unwrap();
    }
}
