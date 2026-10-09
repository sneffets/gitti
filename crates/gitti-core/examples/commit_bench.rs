//! `cargo run --example commit_bench -- <repo> <full commit id>`: timing of a commit's load —
//! `git show` + parse, intra-line marks, rows — e.g. for a huge merge.
fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| ".".into());
    let hex = args.next().expect("a full commit id");
    let repo = gitti_core::Repo::open(&path)?;
    let id = gitti_core::ObjectId::from_hex(hex.as_bytes()).expect("a full commit id");
    let opts = gitti_core::DiffOptions::default();
    let t = std::time::Instant::now();
    let files = repo.commit_diff_with(id, &opts)?;
    let lines: usize = files.iter().flat_map(|f| &f.hunks).map(|h| h.lines.len()).sum();
    let placeholders = files.iter().filter(|f| f.too_large.is_some()).count();
    println!(
        "diff: {} files ({placeholders} not loaded), {lines} lines, {:?}",
        files.len(),
        t.elapsed()
    );
    let t = std::time::Instant::now();
    gitti_core::intraline::warm(&files, false);
    println!("intra-line marks: {:?}", t.elapsed());
    let t = std::time::Instant::now();
    let exp = gitti_core::rows::Expansion::default();
    let rows = gitti_core::rows::build_commit_rows(&files, None, &exp, None);
    println!("rows: {} in {:?}", rows.len(), t.elapsed());
    let t = std::time::Instant::now();
    let data = repo.load_commit(id, None, &opts)?;
    println!(
        "load_commit: {:?}, cached afterwards: {}",
        t.elapsed(),
        repo.cached_commit(id, &opts).is_some_and(|d| std::sync::Arc::ptr_eq(&d, &data))
    );
    Ok(())
}
