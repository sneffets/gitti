// Shared by the build scripts of gitti and gitty (`include!`): the git commit the binary is
// built from, for Help ▸ About and `--version`.

/// Sets `GITTI_BUILD` to e.g. `0123456789 (2026-01-01), release` — with `+ uncommitted
/// changes` when the workspace differs from that commit, `unknown commit` outside a git
/// checkout. Reruns when HEAD moves or a workspace source changes (only then can the
/// answer change; both rebuild the crate anyway, except HEAD moves of unrelated commits).
fn emit_build_info(manifest_dir: &std::path::Path) {
    let workspace = manifest_dir.join("../..");
    let git = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&workspace)
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("LC_ALL", "C")
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let profile = std::env::var("PROFILE").unwrap_or_default();
    // The sources of every workspace crate (not tests: editing one would rebuild the app).
    let crates = std::fs::read_dir(workspace.join("crates"))
        .into_iter()
        .flatten()
        .flatten();
    for krate in crates {
        for part in ["src", "Cargo.toml", "build.rs"] {
            let path = krate.path().join(part);
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
    println!(
        "cargo:rerun-if-changed={}",
        workspace.join("Cargo.lock").display()
    );
    let Some(commit) = git(&["rev-parse", "--short=10", "HEAD"]) else {
        println!("cargo:rustc-env=GITTI_BUILD=unknown commit, {profile}");
        return;
    };
    // HEAD moves: the HEAD file (checkout), the branch ref (commit) or packed-refs.
    let mut watch = Vec::new();
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        watch.push(std::path::PathBuf::from(dir).join("HEAD"));
    }
    if let (Some(common), Some(head_ref)) = (
        git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]),
        git(&["symbolic-ref", "-q", "HEAD"]),
    ) {
        let common = std::path::PathBuf::from(common);
        watch.push(common.join(&head_ref));
        watch.push(common.join("packed-refs"));
    }
    // A missing path would make cargo rerun the script on every build.
    for path in watch.iter().filter(|p| p.exists()) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    let date = git(&["log", "-1", "--format=%cs", "HEAD"]).unwrap_or_default();
    let dirty = git(&["status", "--porcelain", "--", "."]).is_some_and(|s| !s.is_empty());
    println!(
        "cargo:rustc-env=GITTI_BUILD={commit} ({date}){}, {profile}",
        if dirty { " + uncommitted changes" } else { "" }
    );
}
