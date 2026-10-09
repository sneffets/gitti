//! Help ▸ About and `--version`, the same for gitti and gitty. `build` is the app's
//! `GITTI_BUILD` (set by its build script, `crates/gitti-app/build_info.rs`): the git commit
//! the binary was built from, e.g. `3cb5d11d10 (2026-10-04), release`.

use crate::caps::GitCaps;

/// The About window's text of `app` ("gitti" / "gitty"), with the git it runs.
pub fn text(app: &str, version: &str, build: &str) -> String {
    text_with(app, version, build, GitCaps::get())
}

/// [`text`] for a given git.
pub fn text_with(app: &str, version: &str, build: &str, git: &GitCaps) -> String {
    let what = if app == "gitty" {
        "A terminal git client with hunk and line staging; gitti is its GUI sibling."
    } else {
        "A GPU-rendered git GUI with hunk and line staging; gitty is its terminal sibling."
    };
    let config = crate::paths::config_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let git = git.describe();
    format!(
        "{app} {version}\nBuilt from git commit {build}\n\n{what}\n\nRuns {git}\nSettings and \
         logs: {config}"
    )
}

/// The `--version` line.
pub fn version_line(app: &str, version: &str, build: &str) -> String {
    format!("{app} {version} (git commit {build})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn names_the_commit() {
        let t = text("gitty", "0.1.0", "3cb5d11d10 (2026-10-04), release");
        assert!(
            t.starts_with("gitty 0.1.0\nBuilt from git commit 3cb5d11d10 (2026-10-04), release\n")
        );
        assert!(t.contains("terminal git client"));
        assert!(text("gitti", "0.1.0", "x").contains("GPU-rendered"));
        let git = GitCaps::from_probe(Ok("git version 2.39.5 (Apple Git-154)\n".into()));
        let t = text_with("gitti", "0.1.0", "x", &git);
        assert!(t.contains("\nRuns git 2.39.5 (Apple Git-154) (supported: 2.36 or newer)\n"), "{t}");
        let old = GitCaps::of(crate::caps::GitVersion::new(2, 34, 1));
        assert!(text_with("gitty", "0.1.0", "x", &old).contains("Runs git 2.34.1 — older than 2.36"));
        assert_eq!(
            version_line("gitti", "0.1.0", "abc (d), debug"),
            "gitti 0.1.0 (git commit abc (d), debug)"
        );
    }
}
