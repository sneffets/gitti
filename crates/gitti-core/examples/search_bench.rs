//! `cargo run -p gitti-core --example search_bench -- <repo> <query>...`: timing of the
//! "Go to Commit…" smart search — HEAD path index, first early result, final result.
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use gitti_core::commit_search::CommitQuery;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| ".".into());
    let repo = gitti_core::Repo::open(&path)?;
    let t = Instant::now();
    let h = repo.history(gitti_core::LogOrder::Topo)?;
    println!("history: {} commits, {:?}", h.commits.len(), t.elapsed());
    let t = Instant::now();
    let ix = h.head_paths();
    println!("HEAD paths: {} in {:?}", ix.len(), t.elapsed());
    for q in args {
        let no = AtomicBool::new(false);
        let t = Instant::now();
        let mut first = None;
        let r = h
            .search_smart(repo.git(), &CommitQuery::parse(&q), 200, &no, &mut |r| {
                if first.is_none() && !r.hits.is_empty() {
                    first = Some((t.elapsed(), r.hits.len()));
                }
            })
            .expect("not cancelled");
        println!(
            "{q:?}: {} hits in {:?} (first early {first:?}), note: {:?}",
            r.hits.len(),
            t.elapsed(),
            r.note()
        );
        for hit in r.hits.iter().take(3) {
            println!("    {} {} [{}]", hit.id.to_hex_with_len(8), hit.summary, hit.reason_label());
        }
    }
    Ok(())
}
