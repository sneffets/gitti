//! Help ▸ About and `--version`, the same for gitti and gitty. `build` is the app's
//! `GITTI_BUILD` (set by its build script, `crates/gitti-app/build_info.rs`): the git commit
//! the binary was built from, e.g. `3cb5d11d10 (2026-10-04), release`.
//!
//! The About text: the version and build commit first (to tell which state a running
//! process is on), what the app is and where it lives (homepage, bug and security
//! reports), its licence (GPL-3.0-or-later, no warranty, where the source is), the
//! summary of the third-party components (`third_party.rs`; their whole list is a window /
//! `gitty --licenses` of its own), and the facts of this system a bug report needs.

use crate::caps::GitCaps;

/// Homepage and source code.
pub const REPOSITORY: &str = "https://github.com/sneffets/gitti";
/// Where bugs are reported.
pub const ISSUES: &str = "https://github.com/sneffets/gitti/issues";
/// Private vulnerability reporting (GitHub security advisories), see `SECURITY.md`.
pub const SECURITY: &str = "https://github.com/sneffets/gitti/security/advisories/new";
/// The apps' copyright line.
pub const COPYRIGHT: &str = "Copyright (c) 2026 sneffets";

/// What the About window shows besides the app's own facts: where to find the whole
/// third-party list, and the renderer (gitti; `None` in gitty).
pub struct Extras<'a> {
    /// E.g. "the Third-Party Licences… button below".
    pub licences_hint: &'a str,
    /// E.g. "Metal" or the GPU and driver (gitti).
    pub renderer: Option<&'a str>,
}

/// The About window's text of `app` ("gitti" / "gitty"), with the git it runs.
pub fn text(app: &str, version: &str, build: &str, extras: &Extras) -> String {
    let path = std::env::var_os("PATH");
    let git_path = crate::terminal::find_on_path("git", path.as_deref());
    let facts = Facts {
        git: GitCaps::get(),
        git_path: git_path.map(|p| p.display().to_string()),
        os: os_description(),
        config: crate::paths::config_dir().map(|p| p.display().to_string()),
        third_party: crate::third_party::summary(app),
    };
    text_with(app, version, build, extras, &facts)
}

/// The facts of this system and build that [`text`] looks up.
pub struct Facts<'a> {
    pub git: &'a GitCaps,
    pub git_path: Option<String>,
    pub os: String,
    pub config: Option<String>,
    /// The third-party summary (`third_party::summary`).
    pub third_party: String,
}

/// [`text`] for given facts.
pub fn text_with(app: &str, version: &str, build: &str, extras: &Extras, f: &Facts) -> String {
    let (what, other) = if app == "gitty" {
        (
            "A terminal git client with hunk and line staging; gitti is its GUI sibling.",
            "gitti",
        )
    } else {
        (
            "A GPU-rendered git GUI with hunk and line staging; gitty is its terminal sibling.",
            "gitty",
        )
    };
    let commit = build.split_whitespace().next().unwrap_or(build);
    let git = match &f.git_path {
        Some(path) if !f.git.missing => format!("{} at {path}", f.git.describe()),
        _ => f.git.describe(),
    };
    let mut system = format!("Runs {git}\nOn {}", f.os);
    if let Some(r) = extras.renderer {
        system.push_str(&format!("\nRenderer: {r}"));
    }
    if let Some(c) = &f.config {
        system.push_str(&format!("\nSettings and logs: {c}"));
    }
    format!(
        "{app} {version}\nBuilt from git commit {build}\n\n{what}\n\nHomepage and source \
         code: {REPOSITORY}\nReport a bug: {ISSUES}\nReport a security issue (privately): \
         {SECURITY}\n\nLicence\n{COPYRIGHT}\n{app} is free software: you can redistribute it \
         and/or modify it under the terms of the GNU General Public License as published by \
         the Free Software Foundation, either version 3 of the License, or (at your option) \
         any later version (GPL-3.0-or-later, as is {other}).\n{app} is distributed in the \
         hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied \
         warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU \
         General Public License for more details: https://www.gnu.org/licenses/gpl-3.0.html\n\
         Source code: {REPOSITORY} (this build: commit {commit}).\nThe shared core, \
         gitti-core, is licensed under MIT OR Apache-2.0.\n\nThird-party components\n{} \
         Their licences: {}.\n\nThis system\n{system}",
        f.third_party, extras.licences_hint,
    )
}

/// The `--version` line (also what About's Copy copies).
pub fn version_line(app: &str, version: &str, build: &str) -> String {
    format!("{app} {version} (git commit {build})")
}

/// "macOS 15.1 (aarch64)", "Ubuntu 24.04.1 LTS (x86_64)", "Windows (x86_64)": the OS
/// from a file read, no process start.
pub fn os_description() -> String {
    let arch = std::env::consts::ARCH;
    let name = if cfg!(target_os = "macos") {
        std::fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist")
            .ok()
            .and_then(|plist| plist_value(&plist, "ProductVersion"))
            .map_or_else(|| "macOS".to_string(), |v| format!("macOS {v}"))
    } else if cfg!(target_os = "linux") {
        std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|s| os_release_name(&s))
            .unwrap_or_else(|| "Linux".into())
    } else {
        crate::third_party::os_name(std::env::consts::OS).to_string()
    };
    format!("{name} ({arch})")
}

/// `<key>KEY</key><string>VALUE</string>` of a plist.
fn plist_value(plist: &str, key: &str) -> Option<String> {
    let rest = &plist[plist.find(&format!("<key>{key}</key>"))?..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest[start..].find("</string>")?;
    Some(rest[start..start + end].trim().to_string())
}

/// `PRETTY_NAME` (else `NAME`) of `/etc/os-release`.
fn os_release_name(s: &str) -> Option<String> {
    let value = |key: &str| {
        s.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
            .map(|v| v.trim().trim_matches('"').to_string())
            .filter(|v| !v.is_empty())
    };
    value("PRETTY_NAME").or_else(|| value("NAME"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(git: &GitCaps) -> Facts<'_> {
        Facts {
            git,
            git_path: Some("/usr/bin/git".into()),
            os: "macOS 15.1 (aarch64)".into(),
            config: Some("/home/u/.config/gitti".into()),
            third_party: "gitty on macOS contains 2 open-source components under MIT (2).".into(),
        }
    }

    const GITTY: Extras = Extras {
        licences_hint: "press l here, or run `gitty --licenses`",
        renderer: None,
    };

    #[gitti_test::test]
    fn names_the_commit_first() {
        let git = GitCaps::from_probe(Ok("git version 2.39.5 (Apple Git-154)\n".into()));
        let build = "3cb5d11d10 (2026-10-04), release";
        let t = text_with("gitty", "0.1.0", build, &GITTY, &facts(&git));
        assert!(
            t.starts_with("gitty 0.1.0\nBuilt from git commit 3cb5d11d10 (2026-10-04), release\n"),
            "{t}"
        );
        assert!(t.contains("terminal git client"));
        assert!(
            t.contains(
                "\nRuns git 2.39.5 (Apple Git-154) (supported: 2.36 or newer) at /usr/bin/git\n"
            ),
            "{t}"
        );
        assert!(
            t.contains("\nOn macOS 15.1 (aarch64)\nSettings and logs: /home/u/.config/gitti"),
            "{t}"
        );
        assert!(!t.contains("Renderer"));
        let old = GitCaps::of(crate::caps::GitVersion::new(2, 34, 1));
        let t = text_with("gitty", "0.1.0", "x", &GITTY, &facts(&old));
        assert!(t.contains("Runs git 2.34.1 — older than 2.36"), "{t}");
        let gitti = Extras {
            licences_hint: "the button below",
            renderer: Some("Metal"),
        };
        let t = text_with("gitti", "0.1.0", "x", &gitti, &facts(&git));
        assert!(
            t.contains("GPU-rendered") && t.contains("\nRenderer: Metal\n"),
            "{t}"
        );
        assert_eq!(
            version_line("gitti", "0.1.0", "abc (d), debug"),
            "gitti 0.1.0 (git commit abc (d), debug)"
        );
    }

    #[gitti_test::test]
    fn says_licence_warranty_source_and_where_to_report() {
        let git = GitCaps::of(crate::caps::GitVersion::new(2, 45, 0));
        let build = "3cb5d11d10 (2026-10-04), release";
        let t = text_with("gitty", "0.1.0", build, &GITTY, &facts(&git));
        for part in [
            "\nHomepage and source code: https://github.com/sneffets/gitti\n",
            "\nReport a bug: https://github.com/sneffets/gitti/issues\n",
            "\nReport a security issue (privately): \
             https://github.com/sneffets/gitti/security/advisories/new\n",
            "\nLicence\nCopyright (c) 2026 sneffets\ngitty is free software",
            "either version 3 of the License, or (at your option) any later version",
            "WITHOUT ANY WARRANTY",
            "\nSource code: https://github.com/sneffets/gitti (this build: commit 3cb5d11d10).\n",
            "gitti-core, is licensed under MIT OR Apache-2.0.",
            "\nThird-party components\ngitty on macOS contains 2 open-source components under \
             MIT (2). Their licences: press l here, or run `gitty --licenses`.\n",
        ] {
            assert!(t.contains(part), "{part:?} missing in:\n{t}");
        }
    }

    #[gitti_test::test]
    fn os_names() {
        let plist = "<dict>\n<key>ProductName</key>\n<string>macOS</string>\n\
                     <key>ProductVersion</key>\n<string>15.1</string>\n</dict>";
        assert_eq!(
            plist_value(plist, "ProductVersion").as_deref(),
            Some("15.1")
        );
        assert_eq!(
            os_release_name("NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\n").as_deref(),
            Some("Ubuntu 24.04.1 LTS")
        );
        assert_eq!(
            os_release_name("NAME=Arch Linux\n").as_deref(),
            Some("Arch Linux")
        );
        assert!(os_description().ends_with(&format!("({})", std::env::consts::ARCH)));
    }
}
