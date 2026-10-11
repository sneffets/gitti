//! Parsing `git status --porcelain=v2 -z --branch`.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchStatus {
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: String,
    pub orig_path: Option<String>,
    /// Index status letter (`M`, `A`, `D`, `R`, `C`, `T`, `U`) or `None` if unchanged.
    pub index: Option<char>,
    /// Worktree status letter or `None` if unchanged.
    pub worktree: Option<char>,
    pub untracked: bool,
    pub conflicted: bool,
    /// Set for a submodule (porcelain v2 `S<c><m><u>`): what changed inside it.
    pub submodule: Option<SubmoduleFlags>,
}

/// How a submodule differs from what the superproject records (`git status` v2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SubmoduleFlags {
    /// Its checked-out commit differs from the recorded one (`C`).
    pub commit_changed: bool,
    /// Tracked files inside it are modified (`M`).
    pub modified: bool,
    /// It has untracked files (`U`).
    pub untracked: bool,
}

impl SubmoduleFlags {
    /// Uncommitted changes inside the submodule (modified or untracked files).
    pub fn dirty(&self) -> bool {
        self.modified || self.untracked
    }

    fn parse(field: &str) -> Option<Self> {
        let b = field.as_bytes();
        (b.len() == 4 && b[0] == b'S').then(|| Self {
            commit_changed: b[1] == b'C',
            modified: b[2] == b'M',
            untracked: b[3] == b'U',
        })
    }
}

impl StatusEntry {
    pub fn is_staged(&self) -> bool {
        self.index.is_some() && !self.conflicted
    }
    pub fn is_unstaged(&self) -> bool {
        self.worktree.is_some() || self.untracked || self.conflicted
    }
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub branch: BranchStatus,
    pub entries: Vec<StatusEntry>,
    /// How untracked files were listed (`status.showUntrackedFiles`).
    pub untracked_files: UntrackedFiles,
}

impl Status {
    /// The user's git config hides untracked files (`status.showUntrackedFiles=no`): both
    /// apps say so instead of looking like there were none.
    pub fn untracked_hidden(&self) -> bool {
        self.untracked_files == UntrackedFiles::No
    }
}

/// `git status --untracked-files=<mode>`, from the user's `status.showUntrackedFiles`
/// Unset, gitti lists every untracked file (`all`): the file list shows files, and an
/// untracked folder as one entry would hide what is in it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum UntrackedFiles {
    /// Every untracked file (gitti's default).
    #[default]
    All,
    /// An untracked folder as one entry (`dir/`, with the trailing slash): staged and
    /// discarded as a whole.
    Normal,
    /// None: the apps show "untracked files hidden by your git config".
    No,
}

impl UntrackedFiles {
    /// From the value of `status.showUntrackedFiles` (`None`: unset). Like git, the
    /// booleans mean `normal` (true) and `no` (false); an unknown value keeps the default.
    pub fn from_config(value: Option<&str>) -> Self {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("no" | "false" | "off" | "0") => UntrackedFiles::No,
            Some("normal" | "true" | "yes" | "on" | "1" | "") => UntrackedFiles::Normal,
            _ => UntrackedFiles::All,
        }
    }

    /// The `git status` argument.
    pub fn arg(self) -> &'static str {
        match self {
            UntrackedFiles::All => "--untracked-files=all",
            UntrackedFiles::Normal => "--untracked-files=normal",
            UntrackedFiles::No => "--untracked-files=no",
        }
    }
}

/// `git status` as both the working state and the worktree marks read it (then the
/// untracked-files argument; the working state adds `--branch` and pathspecs).
pub const STATUS_ARGS: [&str; 3] = ["status", "--porcelain=v2", "-z"];

/// The note both apps show while untracked files are hidden (under "Nothing to commit").
pub const UNTRACKED_HIDDEN_NOTE: &str =
    "Untracked files are hidden by your git config (status.showUntrackedFiles=no)";

/// The same, short, on the working directory's row of the commit list.
pub const UNTRACKED_HIDDEN_SHORT: &str = "untracked files hidden by git config";

fn letter(c: u8) -> Option<char> {
    if c == b'.' { None } else { Some(c as char) }
}

pub fn parse_porcelain_v2(out: &[u8]) -> Status {
    let mut status = Status::default();
    let text = String::from_utf8_lossy(out);
    let mut records = text.split('\0').peekable();
    while let Some(rec) = records.next() {
        if rec.is_empty() {
            continue;
        }
        if let Some(h) = rec.strip_prefix("# ") {
            if let Some(v) = h.strip_prefix("branch.head ") {
                status.branch.head = (v != "(detached)").then(|| v.to_string());
            } else if let Some(v) = h.strip_prefix("branch.upstream ") {
                status.branch.upstream = Some(v.to_string());
            } else if let Some(v) = h.strip_prefix("branch.ab ") {
                for part in v.split(' ') {
                    if let Some(a) = part.strip_prefix('+') {
                        status.branch.ahead = a.parse().unwrap_or(0);
                    } else if let Some(b) = part.strip_prefix('-') {
                        status.branch.behind = b.parse().unwrap_or(0);
                    }
                }
            }
            continue;
        }
        let kind = rec.as_bytes()[0];
        match kind {
            b'1' | b'2' | b'u' => {
                // Fixed number of space separated fields before the path.
                let fields_before_path = match kind {
                    b'1' => 8,
                    b'2' => 9,
                    _ => 10,
                };
                let mut parts = rec.splitn(fields_before_path + 1, ' ');
                let _ = parts.next();
                let xy = parts.next().unwrap_or("..").as_bytes().to_vec();
                let submodule = parts.next().and_then(SubmoduleFlags::parse);
                let path = parts.nth(fields_before_path - 3).unwrap_or("").to_string();
                let orig_path = if kind == b'2' {
                    records.next().map(str::to_string)
                } else {
                    None
                };
                status.entries.push(StatusEntry {
                    path,
                    orig_path,
                    index: letter(xy[0]),
                    worktree: letter(xy[1]),
                    untracked: false,
                    conflicted: kind == b'u',
                    submodule,
                });
            }
            b'?' => status.entries.push(StatusEntry {
                path: rec[2..].to_string(),
                orig_path: None,
                index: None,
                worktree: None,
                untracked: true,
                conflicted: false,
                submodule: None,
            }),
            _ => {}
        }
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn parses_entries() {
        let raw = b"# branch.oid abc\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 aaa aaa src/a b.rs\0\
2 R. N... 100644 100644 100644 aaa bbb R100 new.rs\0old.rs\0\
? untracked.txt\0";
        let s = parse_porcelain_v2(raw);
        assert_eq!(s.branch.head.as_deref(), Some("main"));
        assert_eq!((s.branch.ahead, s.branch.behind), (2, 1));
        assert_eq!(s.entries.len(), 3);
        assert_eq!(s.entries[0].path, "src/a b.rs");
        assert_eq!(s.entries[0].worktree, Some('M'));
        assert_eq!(s.entries[0].index, None);
        assert_eq!(s.entries[1].path, "new.rs");
        assert_eq!(s.entries[1].orig_path.as_deref(), Some("old.rs"));
        assert!(s.entries[2].untracked);
        assert_eq!(s.entries[0].submodule, None);
    }

    #[gitti_test::test]
    fn parses_submodule_flags() {
        let raw = b"1 .M SC.U 160000 160000 160000 aaa aaa lib/sub\0\
1 .M S.M. 160000 160000 160000 aaa aaa other\0";
        let s = parse_porcelain_v2(raw);
        assert_eq!(s.entries[0].path, "lib/sub");
        let f = s.entries[0].submodule.unwrap();
        assert!(f.commit_changed && !f.modified && f.untracked && f.dirty());
        let g = s.entries[1].submodule.unwrap();
        assert!(!g.commit_changed && g.modified);
    }

    #[gitti_test::test]
    fn untracked_files_mode_from_config() {
        use UntrackedFiles as U;
        assert_eq!(U::from_config(None), U::All);
        assert_eq!(U::from_config(Some("no")), U::No);
        assert_eq!(U::from_config(Some("False")), U::No);
        assert_eq!(U::from_config(Some("normal")), U::Normal);
        assert_eq!(U::from_config(Some("true")), U::Normal);
        assert_eq!(U::from_config(Some("all")), U::All);
        assert_eq!(U::from_config(Some("bogus")), U::All);
    }
}
