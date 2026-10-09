//! Times the file watcher's setup on a repository:
//! `cargo run -p gitti-core --example watch_bench -- /path/to/repo`.
//!
//! Prints how long `watch()` blocks its caller (the UI thread), how long until all
//! watches are added (Linux: counted in `/proc/self/fdinfo`) and how many there are —
//! and, for comparison, how many directories the former recursive watch would have
//! needed (every directory, ignored trees included; counted only, not watched, so the
//! user's inotify limit isn't used up for other programs).

use std::path::Path;
use std::time::{Duration, Instant};

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let workdir = gitti_core::paths::canonical(Path::new(&path));

    let t = Instant::now();
    let dirs = count_dirs(&workdir);
    println!("a recursive watch would need {dirs} watches; walking them took {:?}", t.elapsed());

    // `WATCH_BENCH_OLD=1`: time the former synchronous recursive watch itself (adds a
    // watch per directory — mind the user's inotify limit on big trees).
    if std::env::var_os("WATCH_BENCH_OLD").is_some() {
        use notify::Watcher as _;
        let t = Instant::now();
        let mut w = notify::recommended_watcher(|_: notify::Result<notify::Event>| {}).unwrap();
        let r = w.watch(&workdir, notify::RecursiveMode::Recursive);
        println!(
            "recursive watch: {:?} after {:?}, {} watches",
            r.map_err(|e| e.to_string()),
            t.elapsed(),
            inotify_watches()
        );
        return;
    }

    let t = Instant::now();
    let w = gitti_core::watch::watch(&workdir, |c| {
        if let Some(n) = c.notice {
            println!("notice: {n}");
        }
    })
    .expect("watch");
    println!("watch() returned after {:?}", t.elapsed());
    // Until the number of watches stays put for a second.
    let (mut last, mut since, mut done_at) = (0, Instant::now(), t.elapsed());
    while since.elapsed() < Duration::from_secs(1) && t.elapsed() < Duration::from_secs(120) {
        std::thread::sleep(Duration::from_millis(5));
        let n = inotify_watches();
        if n != last {
            (last, since, done_at) = (n, Instant::now(), t.elapsed());
        }
    }
    println!("{last} watches added after {done_at:?}");
    drop(w);
}

fn count_dirs(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 1;
    };
    1 + entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| count_dirs(&e.path()))
        .sum::<usize>()
}

/// inotify watches of this process (0 elsewhere than Linux).
fn inotify_watches() -> usize {
    let Ok(fds) = std::fs::read_dir("/proc/self/fdinfo") else {
        return 0;
    };
    fds.flatten()
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .map(|s| s.lines().filter(|l| l.starts_with("inotify wd:")).count())
        .sum()
}
