//! `cargo run -p gitti-core --example change_search_bench -- <repo> <query>...`: timing of
//! "Search in Changes" — history load, first hit, the scan up to the cap (or
//! `BENCH_MAX=N` commits), commits per second; `BENCH_STRICT=1` strict mode,
//! `BENCH_CAP=N` hits per scan, `BENCH_WORKERS=N` parallel diff-trees.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use gitti_core::change_search::ChangeSearch;

fn env_num(name: &str) -> Option<usize> {
    std::env::var(name).ok()?.parse().ok()
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| ".".into());
    let repo = gitti_core::Repo::open(&path)?;
    let t = Instant::now();
    let h = Arc::new(repo.history(gitti_core::LogOrder::Topo)?);
    println!("history: {} commits, {:?}", h.commits.len(), t.elapsed());
    let max = env_num("BENCH_MAX").unwrap_or(usize::MAX);
    for q in args {
        let mut s = ChangeSearch::new();
        s.set_strict(std::env::var_os("BENCH_STRICT").is_some());
        if let Some(cap) = env_num("BENCH_CAP") {
            s.set_cap(cap);
        }
        s.set_text(&q);
        let t = Instant::now();
        let Some((generation, mut job)) = s.start(&h) else {
            println!("{q:?}: nothing to search");
            continue;
        };
        if let Some(w) = env_num("BENCH_WORKERS") {
            job.workers = w;
        }
        let workers = job.workers;
        let cancel = AtomicBool::new(false);
        let mut first = None;
        job.run(repo.git(), &cancel, &mut |ev| {
            s.apply(generation, ev);
            if first.is_none() && !s.hits().is_empty() {
                first = Some(t.elapsed());
            }
            if s.status().searched >= max {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        s.finished(generation);
        let el = t.elapsed();
        let st = s.status();
        println!(
            "{q:?} ({workers} workers): {} hits, {} of {} commits in {el:?} ({:.0} commits/s), first hit {first:?}",
            st.hits,
            st.searched,
            st.total,
            st.searched as f64 / el.as_secs_f64()
        );
        println!("    {}", s.note());
        for hit in s.hits().iter().take(3) {
            let meta = h.meta(h.row_of(&hit.id).unwrap_or(0));
            println!(
                "    {} {} [{}]",
                hit.id.to_hex_with_len(8),
                meta.summary,
                hit.proximity.label()
            );
        }
    }
    Ok(())
}
