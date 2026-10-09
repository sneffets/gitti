//! `cargo run -p gitti-core --example history_mem -- <dir> [commits] [--commit-graph]
//! [--sha256]`: heap bytes per commit of a loaded [`gitti_core::History`] and its load time,
//! measured with a counting allocator. A synthetic repository (branches, merges, empty
//! trees) is built with one `git fast-import` in `<dir>` unless it already exists.
use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicIsize, Ordering::Relaxed};
use std::time::Instant;

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let live = LIVE.fetch_add(layout.size() as isize, Relaxed) + layout.size() as isize;
        PEAK.fetch_max(live, Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let d = new_size as isize - layout.size() as isize;
        let live = LIVE.fetch_add(d, Relaxed) + d;
        PEAK.fetch_max(live, Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn git(dir: &Path, args: &[&str]) -> Command {
    let mut c = Command::new("git");
    c.current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    c
}

/// A history like a busy project: eight long-lived branches, commits on a random one,
/// one in twenty a merge of another branch, a tag every 500 commits.
fn build(dir: &Path, n: usize, sha256: bool) {
    std::fs::create_dir_all(dir).unwrap();
    let mut init = vec!["init", "-q", "-b", "b0"];
    if sha256 {
        init.push("--object-format=sha256");
    }
    assert!(git(dir, &init).status().unwrap().success());
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut rnd = move |m: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % m as u64) as usize
    };
    const BRANCHES: usize = 8;
    let mut tips: Vec<Option<usize>> = vec![None; BRANCHES];
    let mut s = String::with_capacity(n * 120);
    use std::fmt::Write as _;
    for i in 1..=n {
        let b = if i < BRANCHES * 4 { 0 } else { rnd(BRANCHES) };
        let _ = write!(
            s,
            "commit refs/heads/b{b}\nmark :{i}\ncommitter A <a@x> {} +0000\ndata 8\nc{:06}\n",
            1_000_000 + i * 60,
            i % 1_000_000
        );
        match tips[b] {
            Some(t) => {
                let _ = writeln!(s, "from :{t}");
            }
            None if i > 1 => {
                let _ = writeln!(s, "from :{}", i - 1);
            }
            None => {}
        }
        if i > BRANCHES * 4 && rnd(20) == 0 {
            let o = rnd(BRANCHES);
            if o != b
                && let Some(t) = tips[o]
            {
                let _ = writeln!(s, "merge :{t}");
            }
        }
        s.push('\n');
        tips[b] = Some(i);
        if i % 500 == 0 {
            let _ = write!(s, "reset refs/tags/t{i}\nfrom :{i}\n\n");
        }
    }
    let mut child = git(dir, &["fast-import", "--quiet", "--done"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    s.push_str("done\n");
    child.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
    assert!(child.wait().unwrap().success());
}

fn mb(b: isize) -> f64 {
    b as f64 / (1 << 20) as f64
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = Path::new(args.first().map(String::as_str).unwrap_or("history_mem_repo")).to_owned();
    let n: usize = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(200_000);
    let flag = |f: &str| args.iter().any(|a| a == f);
    if !dir.join(".git").exists() {
        let t = Instant::now();
        build(&dir, n, flag("--sha256"));
        println!("built {n} commits in {:?}", t.elapsed());
    }
    if flag("--commit-graph") {
        assert!(git(&dir, &["commit-graph", "write", "--reachable"]).status().unwrap().success());
    } else {
        let _ = std::fs::remove_file(dir.join(".git/objects/info/commit-graph"));
    }
    let repo = gitti_core::Repo::open(&dir)?;
    for round in 0..2 {
        let before = LIVE.load(Relaxed);
        PEAK.store(before, Relaxed);
        let t = Instant::now();
        let h = repo.history(gitti_core::LogOrder::Topo)?;
        let took = t.elapsed();
        let live = LIVE.load(Relaxed) - before;
        let peak = PEAK.load(Relaxed) - before;
        let rows = h.commits.len();
        println!(
            "round {round}: {rows} rows in {took:?}, commit-graph {}, max lanes {}: \
             {:.1} MB live = {:.1} bytes/commit, peak {:.1} MB = {:.1} bytes/commit",
            h.used_commit_graph,
            h.graph.max_lanes(),
            mb(live),
            live as f64 / rows as f64,
            mb(peak),
            peak as f64 / rows as f64,
        );
        let t = Instant::now();
        let q = repo.history_newest(gitti_core::LogOrder::Topo, 500)?;
        println!("newest 500: {} rows in {:?}", q.commits.len(), t.elapsed());
        // Touch the lazy metadata of a screen of rows.
        for row in 0..100.min(rows) {
            h.meta(row);
        }
        let t = Instant::now();
        let g = h.graph.rows(rows / 2..rows / 2 + 100);
        println!("graph rows 100 in the middle: {} in {:?}", g.len(), t.elapsed());
    }
    Ok(())
}
