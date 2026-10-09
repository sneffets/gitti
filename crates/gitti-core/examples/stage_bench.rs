//! `cargo run -p gitti-core --release --example stage_bench -- <repo>`: what a stage click
//! costs — the apply itself and every query of the refresh that follows. The repo needs an
//! unstaged modification; its first hunk is staged and unstaged again (use a scratch clone).
use gitti_core::patch::{PatchMode, Selection};
use std::time::{Duration, Instant};

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn time<T>(runs: usize, mut f: impl FnMut() -> T) -> Duration {
    median(
        (0..runs)
            .map(|_| {
                let t = Instant::now();
                std::hint::black_box(f());
                t.elapsed()
            })
            .collect(),
    )
}

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let repo = gitti_core::Repo::open(&path)?;
    let runs = 7;
    let opts = gitti_core::DiffOptions::default();
    let parts: Vec<(&str, Duration)> = vec![
        ("status", time(runs, || repo.status())),
        ("working_state", time(runs, || repo.working_state_with(&opts))),
        ("head", time(runs, || repo.head())),
        ("refs", time(runs, || repo.refs())),
        ("remotes", time(runs, || repo.remotes())),
        ("stashes", time(runs, || repo.stashes())),
        ("state", time(runs, || repo.state())),
        ("tracking", time(runs, || repo.tracking())),
        ("identity", time(runs, || repo.identity())),
        ("snapshot (parallel)", time(runs, || repo.snapshot(&opts))),
    ];
    for (name, d) in &parts {
        println!("{name:>22}: {:>7.1} ms", d.as_secs_f64() * 1e3);
    }

    let ws = repo.working_state_with(&opts)?;
    let Some(file) = ws.unstaged.iter().find_map(|f| f.diff.clone()) else {
        println!("no unstaged diff — modify a file to time staging");
        return Ok(());
    };
    let sel = Selection::whole_hunk(0);
    let paths = vec![file.path().to_string()];
    let (mut stage, mut unstage, mut after) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..runs {
        let prev = repo.working_state_with(&opts)?;
        let t = Instant::now();
        repo.apply_selection(&file, &sel, PatchMode::Stage)?;
        stage.push(t.elapsed());
        let t = Instant::now();
        let staged = repo.working_state_after(&opts, &prev, &paths)?;
        after.push(t.elapsed());
        let back = staged
            .staged
            .iter()
            .find(|f| f.path() == file.path())
            .and_then(|f| f.diff.clone())
            .expect("staged diff");
        let t = Instant::now();
        repo.apply_selection(&back, &Selection::whole_hunk(0), PatchMode::Unstage)?;
        unstage.push(t.elapsed());
    }
    println!("{:>22}: {:>7.1} ms", "apply stage hunk", median(stage).as_secs_f64() * 1e3);
    println!("{:>22}: {:>7.1} ms", "apply unstage hunk", median(unstage).as_secs_f64() * 1e3);
    println!("{:>22}: {:>7.1} ms", "working_state_after", median(after).as_secs_f64() * 1e3);
    Ok(())
}
