//! Machine-local dev settings. They describe one machine (an external drive, how many
//! agent slots fit), so they live outside the repository:
//! `$GITTI_DEV_CONFIG`, else `~/.config/gitti-dev/config.toml` (also on Windows, under
//! `%USERPROFILE%`). Only `key = value` lines; `#` starts a comment.
//!
//! ```toml
//! worktree_root = "/Volumes/Some SSD/temp"   # where the agent slots live
//! slots = 5                                  # at most this many slots
//! ```
//! `GITTI_WORKTREE_ROOT` overrides `worktree_root` for one call.

use std::path::PathBuf;

pub const DEFAULT_SLOTS: usize = 4;

#[derive(Debug, Default, PartialEq)]
pub struct Config {
    pub worktree_root: Option<PathBuf>,
    pub slots: Option<usize>,
}

pub fn path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("GITTI_DEV_CONFIG").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".config/gitti-dev/config.toml"))
}

pub fn load() -> crate::Result<Config> {
    let mut config = match path() {
        Some(p) if p.exists() => parse(&std::fs::read_to_string(&p)?)
            .map_err(|e| crate::Failure::new(format!("{}: {e}", p.display())))?,
        _ => Config::default(),
    };
    if let Some(root) = std::env::var_os("GITTI_WORKTREE_ROOT").filter(|r| !r.is_empty()) {
        config.worktree_root = Some(PathBuf::from(root));
    }
    Ok(config)
}

/// Where `worktree_root` came from, for messages.
pub fn root_source() -> String {
    if std::env::var_os("GITTI_WORKTREE_ROOT").is_some_and(|r| !r.is_empty()) {
        "GITTI_WORKTREE_ROOT".into()
    } else {
        path().map_or("the dev config".into(), |p| p.display().to_string())
    }
}

pub fn parse(text: &str) -> Result<Config, String> {
    let mut config = Config::default();
    for (n, line) in text.lines().enumerate() {
        let line = strip_comment(line).trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {}: expected `key = value`", n + 1))?;
        let value = value.trim();
        let unquoted = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(value);
        match key.trim() {
            "worktree_root" => config.worktree_root = Some(PathBuf::from(unquoted)),
            "slots" => {
                config.slots = Some(
                    unquoted
                        .parse()
                        .ok()
                        .filter(|&n| n > 0)
                        .ok_or_else(|| format!("line {}: slots must be a number > 0", n + 1))?,
                )
            }
            other => return Err(format!("line {}: unknown key `{other}`", n + 1)),
        }
    }
    Ok(config)
}

/// `#` outside of a quoted value starts a comment.
fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '#' if !quoted => return &line[..i],
            _ => {}
        }
    }
    line
}

pub fn describe() -> String {
    let file = path().map_or("(no home directory)".into(), |p| p.display().to_string());
    let state = match load() {
        Ok(c) => format!(
            "worktree_root = {}\nslots = {}",
            c.worktree_root
                .map_or("(not set: <repo>/.claude/worktrees)".into(), |p| p
                    .display()
                    .to_string()),
            c.slots.unwrap_or(DEFAULT_SLOTS),
        ),
        Err(e) => e.message,
    };
    format!(
        "machine-local dev config: {file}\n(override the file with GITTI_DEV_CONFIG, the root \
         with GITTI_WORKTREE_ROOT)\n\n{state}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn parses_quoted_paths_with_spaces_and_comments() {
        let c = parse(
            "# a laptop\nworktree_root = \"/Volumes/Some SSD/temp\"  # the SSD\nslots = 5\n",
        )
        .unwrap();
        assert_eq!(
            c.worktree_root,
            Some(PathBuf::from("/Volumes/Some SSD/temp"))
        );
        assert_eq!(c.slots, Some(5));
        assert_eq!(
            parse("worktree_root = \"/a#b\"").unwrap().worktree_root,
            Some("/a#b".into())
        );
    }

    #[gitti_test::test]
    fn rejects_unknown_keys_and_bad_numbers() {
        assert!(
            parse("root = x")
                .unwrap_err()
                .contains("unknown key `root`")
        );
        assert!(parse("slots = 0").is_err());
        assert!(parse("slots").is_err());
        assert_eq!(parse("").unwrap(), Config::default());
    }
}
