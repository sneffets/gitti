//! `cargo run -p gitti-core --release --example bench -- <repo>`: timing of the read paths.
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let repo = gitti_core::Repo::open(&path)?;
    let t = std::time::Instant::now();
    let refs = repo.refs()?;
    println!("refs: {} in {:?}", refs.len(), t.elapsed());
    let t = std::time::Instant::now();
    let q = repo.history_newest(gitti_core::LogOrder::Topo, 500)?;
    println!("newest: {} rows, {:?}", q.commits.len(), t.elapsed());
    let t = std::time::Instant::now();
    let h = repo.history(gitti_core::LogOrder::Topo)?;
    let max_w = h.graph.max_lanes();
    println!(
        "history: {} commits, max lanes {max_w}, {:?}",
        h.commits.len(),
        t.elapsed()
    );
    // A partial clone without checkout would fetch trees for `git status`.
    if std::env::var_os("BENCH_HISTORY_ONLY").is_some() {
        return Ok(());
    }
    let t = std::time::Instant::now();
    let ws = repo.working_state()?;
    println!(
        "working state: {} unstaged, {} staged, {:?}",
        ws.unstaged.len(),
        ws.staged.len(),
        t.elapsed()
    );
    Ok(())
}
