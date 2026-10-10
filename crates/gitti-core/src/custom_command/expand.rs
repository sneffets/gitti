//! Placeholders of a custom command and how they become a command line: a git command's
//! values become whole arguments (git gets them directly, no shell sees them); a shell
//! command's values are quoted for the shell where they stand — outside quotes, inside
//! `'…'` or inside `"…"` — so no file or branch name can run anything.

use std::path::Path;

/// A value the command line takes from where it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Placeholder {
    Repo,
    Commit,
    Branch,
    Remote,
    File,
    Files,
}

impl Placeholder {
    pub const ALL: [Placeholder; 6] = [
        Placeholder::Repo,
        Placeholder::Commit,
        Placeholder::Branch,
        Placeholder::Remote,
        Placeholder::File,
        Placeholder::Files,
    ];

    /// The word between the braces: `commit` for `{commit}`.
    pub fn name(self) -> &'static str {
        match self {
            Placeholder::Repo => "repo",
            Placeholder::Commit => "commit",
            Placeholder::Branch => "branch",
            Placeholder::Remote => "remote",
            Placeholder::File => "file",
            Placeholder::Files => "files",
        }
    }

    /// What it stands for, in plain words.
    pub fn meaning(self) -> &'static str {
        match self {
            Placeholder::Repo => "the repository's folder",
            Placeholder::Commit => "the selected commit's id",
            Placeholder::Branch => {
                "the branch whose menu it runs from (a remote branch: its name there), else the \
                 current branch"
            }
            Placeholder::Remote => {
                "the remote whose menu it runs from, else the current branch's upstream remote \
                 (or the only remote)"
            }
            Placeholder::File => {
                "the file the menu is for or the diff cursor is in, relative to the repository's \
                 folder"
            }
            Placeholder::Files => {
                "every file the view shows — the selected commit's, or the changed ones of the \
                 working directory — each as its own argument"
            }
        }
    }

    /// Why it has no value now (the entry is greyed with this).
    pub fn missing(self) -> &'static str {
        match self {
            Placeholder::Repo => "no repository",
            Placeholder::Commit => "no commit selected",
            Placeholder::Branch => "no current branch",
            Placeholder::Remote => "no remote",
            Placeholder::File => "no file selected",
            Placeholder::Files => "no files shown",
        }
    }

    fn from_name(name: &str) -> Option<Placeholder> {
        Placeholder::ALL.into_iter().find(|p| p.name() == name)
    }

    /// The private-use character standing for it while a git command line is split.
    fn sentinel(self) -> char {
        let i = Placeholder::ALL.iter().position(|p| *p == self).unwrap_or(0) as u32;
        char::from_u32(0xE000 + i).unwrap_or('\u{E000}')
    }

    fn of_sentinel(c: char) -> Option<Placeholder> {
        let i = (c as u32).checked_sub(0xE000)? as usize;
        Placeholder::ALL.get(i).copied()
    }
}

/// `{repo} {commit} …`: every placeholder, for the dialogs.
pub fn placeholder_list() -> String {
    Placeholder::ALL
        .iter()
        .map(|p| format!("{{{}}}", p.name()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A piece of a command template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    Value(Placeholder),
}

/// Splits `template` into text and placeholders. `{{` is a literal `{`; a `{` after `$`
/// or `@` (`${HOME}`, `@{u}`) and braces around anything but a single word (`{}`,
/// `{a,b}`, `{ ls; }`) stay as typed. A word in braces that is no placeholder is an error
/// (a typo would otherwise run as text).
pub fn parse(template: &str) -> Result<Vec<Piece>, String> {
    let mut pieces = Vec::new();
    let mut text = String::new();
    let mut chars = template.char_indices().peekable();
    let mut prev: Option<char> = None;
    while let Some((i, c)) = chars.next() {
        if c != '{' {
            text.push(c);
            prev = Some(c);
            continue;
        }
        if chars.peek().map(|(_, n)| *n) == Some('{') {
            chars.next();
            text.push('{');
            prev = Some('{');
            continue;
        }
        if matches!(prev, Some('$' | '@')) {
            text.push('{');
            prev = Some('{');
            continue;
        }
        let rest = &template[i + 1..];
        let word: &str = match rest.find('}') {
            Some(end) => &rest[..end],
            None => "",
        };
        let is_word = !word.is_empty()
            && word.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !is_word {
            text.push('{');
            prev = Some('{');
            continue;
        }
        match Placeholder::from_name(word) {
            Some(p) => {
                if !text.is_empty() {
                    pieces.push(Piece::Text(std::mem::take(&mut text)));
                }
                pieces.push(Piece::Value(p));
                for _ in 0..word.len() + 1 {
                    chars.next();
                }
                prev = Some('}');
            }
            None => {
                return Err(format!(
                    "{{{word}}} is no placeholder — they are {} (write {{{{ for a literal {{)",
                    placeholder_list()
                ));
            }
        }
    }
    if !text.is_empty() {
        pieces.push(Piece::Text(text));
    }
    Ok(pieces)
}

/// The placeholders `template` uses (each once, in order); none for a template that
/// doesn't parse.
pub fn used(template: &str) -> Vec<Placeholder> {
    let mut out = Vec::new();
    for p in parse(template).unwrap_or_default() {
        if let Piece::Value(v) = p
            && !out.contains(&v)
        {
            out.push(v);
        }
    }
    out
}

/// The values a command line is filled with; `None` = there is none now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Values {
    pub repo: Option<String>,
    pub commit: Option<String>,
    pub branch: Option<String>,
    pub remote: Option<String>,
    pub file: Option<String>,
    pub files: Vec<String>,
}

impl Values {
    /// The value(s) of `p`: `files` gives every file, the others one value.
    pub fn get(&self, p: Placeholder) -> Option<Vec<String>> {
        let one = |v: &Option<String>| v.clone().map(|v| vec![v]);
        match p {
            Placeholder::Repo => one(&self.repo),
            Placeholder::Commit => one(&self.commit),
            Placeholder::Branch => one(&self.branch),
            Placeholder::Remote => one(&self.remote),
            Placeholder::File => one(&self.file).map(|v| v.into_iter().map(path_arg).collect()),
            Placeholder::Files => {
                (!self.files.is_empty()).then(|| self.files.iter().cloned().map(path_arg).collect())
            }
        }
    }
}

/// A relative path that starts with `-` would read as an option: `./-x` is the same file.
fn path_arg(p: String) -> String {
    if p.starts_with('-') { format!("./{p}") } else { p }
}

/// The arguments of a git command (without the leading `git`) from `template`, its
/// placeholders filled from `values` — `None` values stay as `{name}` (the dialogs'
/// preview; [`super::missing`] keeps such a command from running). A leading `git` word
/// is dropped (a pasted command line). `{files}` must be an argument of its own.
pub fn git_args(template: &str, values: &Values) -> Result<Vec<String>, String> {
    let pieces = parse(template)?;
    let mut marked = String::new();
    for p in &pieces {
        match p {
            Piece::Text(t) => marked.push_str(t),
            Piece::Value(v) => marked.push(v.sentinel()),
        }
    }
    let mut words = crate::ops::split_command_line(&marked);
    if words.first().map(String::as_str) == Some("git") {
        words.remove(0);
    }
    if words.is_empty() {
        return Err("The command is empty — type what follows `git`".into());
    }
    let mut args = Vec::new();
    for w in words {
        let files = Placeholder::Files.sentinel();
        if w.contains(files) {
            if w != files.to_string() {
                return Err("{files} must stand alone (a space before and after it): it becomes \
                            one argument per file"
                    .into());
            }
            match values.get(Placeholder::Files) {
                Some(v) => args.extend(v),
                None => args.push("{files}".into()),
            }
            continue;
        }
        let mut arg = String::new();
        for c in w.chars() {
            match Placeholder::of_sentinel(c) {
                Some(p) => match values.get(p) {
                    Some(v) => arg.push_str(&v.join(" ")),
                    None => arg.push_str(&format!("{{{}}}", p.name())),
                },
                None => arg.push(c),
            }
        }
        args.push(arg);
    }
    Ok(args)
}

/// A command line as a POSIX shell would read it back (for showing a git command's
/// arguments): plain words as they are, others in single quotes.
pub fn display_args(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            let plain = !a.is_empty()
                && a.chars().all(|c| {
                    c.is_alphanumeric() || "-_./=:@%+,^~{}".contains(c)
                });
            if plain {
                a.clone()
            } else {
                quote(Shell::Posix, Quoting::Bare, a)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The shell a shell command runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    /// `/bin/sh -c` (macOS, Linux).
    Posix,
    /// Windows PowerShell (`powershell -EncodedCommand`).
    PowerShell,
}

impl Shell {
    /// The one this platform uses.
    pub fn native() -> Shell {
        if cfg!(windows) { Shell::PowerShell } else { Shell::Posix }
    }

    /// Its name in the dialogs and the Git Log.
    pub fn name(self) -> &'static str {
        match self {
            Shell::Posix => "sh",
            Shell::PowerShell => "PowerShell",
        }
    }

    /// The program and arguments that run `line`.
    pub fn argv(self, line: &str) -> (String, Vec<String>) {
        match self {
            Shell::Posix => ("/bin/sh".into(), vec!["-c".into(), line.into()]),
            Shell::PowerShell => {
                // UTF-16LE base64: no quoting rule of the Windows command line or of
                // PowerShell's own argument parsing touches the text.
                use base64::Engine as _;
                let bytes: Vec<u8> = line.encode_utf16().flat_map(u16::to_le_bytes).collect();
                let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
                (
                    "powershell.exe".into(),
                    vec![
                        "-NoProfile".into(),
                        "-NonInteractive".into(),
                        "-EncodedCommand".into(),
                        encoded,
                    ],
                )
            }
        }
    }
}

/// Where a placeholder stands in a shell command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quoting {
    Bare,
    Single,
    Double,
}

/// PowerShell takes the typographic quotes as quotes too.
fn ps_single(c: char) -> bool {
    matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}')
}

fn ps_double(c: char) -> bool {
    matches!(c, '"' | '\u{201C}' | '\u{201D}' | '\u{201E}')
}

/// `value` written so `shell` reads it back as exactly that text, at a place in the line
/// quoted as `at`.
pub fn quote(shell: Shell, at: Quoting, value: &str) -> String {
    match (shell, at) {
        (Shell::Posix, Quoting::Bare) => format!("'{}'", value.replace('\'', r"'\''")),
        (Shell::Posix, Quoting::Single) => value.replace('\'', r"'\''"),
        (Shell::Posix, Quoting::Double) => {
            let mut out = String::new();
            for c in value.chars() {
                if matches!(c, '\\' | '"' | '$' | '`') {
                    out.push('\\');
                }
                out.push(c);
            }
            out
        }
        (Shell::PowerShell, Quoting::Bare) => format!("'{}'", ps_single_inner(value)),
        (Shell::PowerShell, Quoting::Single) => ps_single_inner(value),
        (Shell::PowerShell, Quoting::Double) => {
            let mut out = String::new();
            for c in value.chars() {
                if c == '`' || c == '$' || ps_double(c) {
                    out.push('`');
                }
                out.push(c);
            }
            out
        }
    }
}

fn ps_single_inner(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars() {
        if ps_single(c) {
            out.push(c);
        }
        out.push(c);
    }
    out
}

/// Follows the quoting of a shell command line over its text pieces.
struct QuoteState {
    shell: Shell,
    at: Quoting,
    /// The previous character escaped the next one.
    escape: bool,
    /// PowerShell: a quote that may be the first of a doubled (escaped) one.
    closing: Option<char>,
}

impl QuoteState {
    fn new(shell: Shell) -> Self {
        Self { shell, at: Quoting::Bare, escape: false, closing: None }
    }

    fn feed(&mut self, text: &str) {
        for c in text.chars() {
            self.step(c);
        }
        // A quote just before a placeholder (or at the end) closed its string: a doubled
        // one would need a second quote character, which isn't there.
        if self.closing.take().is_some() {
            self.at = Quoting::Bare;
        }
    }

    fn step(&mut self, c: char) {
        if self.escape {
            self.escape = false;
            return;
        }
        if let Some(q) = self.closing.take() {
            // `''` / `""` inside a PowerShell string: an escaped quote, the string goes on.
            let doubled = match self.at {
                Quoting::Single => ps_single(c) && ps_single(q),
                Quoting::Double => ps_double(c) && ps_double(q),
                Quoting::Bare => false,
            };
            if doubled {
                return;
            }
            self.at = Quoting::Bare;
        }
        match (self.shell, self.at) {
            (Shell::Posix, Quoting::Bare) => match c {
                '\\' => self.escape = true,
                '\'' => self.at = Quoting::Single,
                '"' => self.at = Quoting::Double,
                _ => {}
            },
            (Shell::Posix, Quoting::Single) => {
                if c == '\'' {
                    self.at = Quoting::Bare;
                }
            }
            (Shell::Posix, Quoting::Double) => match c {
                '\\' => self.escape = true,
                '"' => self.at = Quoting::Bare,
                _ => {}
            },
            (Shell::PowerShell, Quoting::Bare) => {
                if c == '`' {
                    self.escape = true;
                } else if ps_single(c) {
                    self.at = Quoting::Single;
                } else if ps_double(c) {
                    self.at = Quoting::Double;
                }
            }
            (Shell::PowerShell, Quoting::Single) => {
                if ps_single(c) {
                    self.closing = Some(c);
                }
            }
            (Shell::PowerShell, Quoting::Double) => {
                if c == '`' {
                    self.escape = true;
                } else if ps_double(c) {
                    self.closing = Some(c);
                }
            }
        }
    }

    /// Still inside a string at the end of the line.
    fn open(&self) -> bool {
        self.at != Quoting::Bare
    }
}

/// A shell command line from `template` for `shell`, every value quoted where it stands
/// (`{files}`: each file, separated by spaces). `None` values stay as `{name}` (preview).
pub fn shell_line(template: &str, values: &Values, shell: Shell) -> Result<String, String> {
    let pieces = parse(template)?;
    let mut state = QuoteState::new(shell);
    let mut line = String::new();
    for p in &pieces {
        match p {
            Piece::Text(t) => {
                state.feed(t);
                line.push_str(t);
            }
            Piece::Value(v) => match values.get(*v) {
                Some(vals) => {
                    let quoted: Vec<String> = vals.iter().map(|x| quote(shell, state.at, x)).collect();
                    line.push_str(&quoted.join(" "));
                }
                None => line.push_str(&format!("{{{}}}", v.name())),
            },
        }
    }
    if state.open() {
        return Err("A quote is not closed".into());
    }
    if line.trim().is_empty() {
        return Err("The command is empty".into());
    }
    Ok(line)
}

/// The working folder from its template: empty = the repository's folder; `~/…` in the
/// home folder; relative paths inside the repository. Placeholders are filled as plain
/// text (no shell reads it).
pub fn workdir(template: &str, values: &Values, repo: &Path) -> Result<std::path::PathBuf, String> {
    let t = template.trim();
    if t.is_empty() {
        return Ok(repo.to_path_buf());
    }
    let mut text = String::new();
    for p in parse(t)? {
        match p {
            Piece::Text(s) => text.push_str(&s),
            Piece::Value(Placeholder::Files) => {
                return Err("{files} can't be a working folder".into());
            }
            Piece::Value(v) => match values.get(v) {
                Some(vals) => text.push_str(&vals.join(" ")),
                None => text.push_str(&format!("{{{}}}", v.name())),
            },
        }
    }
    let path = match text.strip_prefix("~/").or(text.strip_prefix("~\\")) {
        Some(rest) => match dirs::home_dir() {
            Some(home) => home.join(rest),
            None => return Err("The home folder is unknown".into()),
        },
        None if text == "~" => dirs::home_dir().ok_or("The home folder is unknown")?,
        None => std::path::PathBuf::from(&text),
    };
    Ok(if path.is_absolute() { path } else { repo.join(path) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vals() -> Values {
        Values {
            repo: Some("/r/my repo".into()),
            commit: Some("abc123".into()),
            branch: Some("feature/x".into()),
            remote: Some("origin".into()),
            file: Some("a b.txt".into()),
            files: vec!["a b.txt".into(), "-rf".into(), "it's".into()],
        }
    }

    #[gitti_test::test]
    fn parse_finds_placeholders_and_keeps_shell_braces() {
        let p = parse("echo {commit} ${HOME} @{u} {} {a,b} { ls; } {{commit}").unwrap();
        assert_eq!(
            p,
            vec![
                Piece::Text("echo ".into()),
                Piece::Value(Placeholder::Commit),
                Piece::Text(" ${HOME} @{u} {} {a,b} { ls; } {commit}".into()),
            ]
        );
        let e = parse("git show {comit}").unwrap_err();
        assert!(e.contains("{comit} is no placeholder"), "{e}");
        assert_eq!(used("x {file} {repo} {file}"), vec![Placeholder::File, Placeholder::Repo]);
        assert_eq!(parse("a {").unwrap(), vec![Piece::Text("a {".into())]);
    }

    #[gitti_test::test]
    fn git_args_are_whole_arguments() {
        let v = vals();
        let args = git_args("git log --format=%H {branch} -- {files}", &v).unwrap();
        assert_eq!(
            args,
            vec!["log", "--format=%H", "feature/x", "--", "a b.txt", "./-rf", "it's"]
        );
        // Inside a quoted word the value stays part of it.
        let args = git_args("tag -m \"from {branch}\" t {commit}", &v).unwrap();
        assert_eq!(args, vec!["tag", "-m", "from feature/x", "t", "abc123"]);
        assert_eq!(git_args("show {file}", &v).unwrap(), vec!["show", "a b.txt"]);
        assert!(git_args("add x{files}", &v).unwrap_err().contains("stand alone"));
        assert!(git_args("git", &v).unwrap_err().contains("empty"));
        // No value: the placeholder shows (preview).
        let none = Values::default();
        assert_eq!(git_args("show {commit} {files}", &none).unwrap(), vec!["show", "{commit}", "{files}"]);
    }

    #[gitti_test::test]
    fn posix_quoting_where_the_value_stands() {
        let mut v = vals();
        v.branch = Some("x'; rm -rf ~; echo '".into());
        let line = shell_line("echo {branch} '{branch}' \"{branch}\"", &v, Shell::Posix).unwrap();
        assert_eq!(
            line,
            r#"echo 'x'\''; rm -rf ~; echo '\''' 'x'\''; rm -rf ~; echo '\''' "x'; rm -rf ~; echo '""#
        );
        v.branch = Some("$(touch pwned)`id`\"\\".into());
        let line = shell_line("echo \"{branch}\"", &v, Shell::Posix).unwrap();
        assert_eq!(line, r#"echo "\$(touch pwned)\`id\`\"\\""#);
        let line = shell_line("ls {files}", &v, Shell::Posix).unwrap();
        assert_eq!(line, r#"ls 'a b.txt' './-rf' 'it'\''s'"#);
        // An escaped quote doesn't open a string.
        let line = shell_line(r"echo \'{commit}", &v, Shell::Posix).unwrap();
        assert_eq!(line, r"echo \''abc123'");
        assert!(shell_line("echo 'x", &v, Shell::Posix).unwrap_err().contains("quote"));
    }

    #[gitti_test::test]
    fn powershell_quoting_where_the_value_stands() {
        let mut v = vals();
        v.branch = Some("x'; Remove-Item -Recurse ~; '\u{2019}".into());
        let line = shell_line("echo {branch} '{branch}'", &v, Shell::PowerShell).unwrap();
        assert_eq!(
            line,
            "echo 'x''; Remove-Item -Recurse ~; ''\u{2019}\u{2019}' 'x''; Remove-Item -Recurse ~; ''\u{2019}\u{2019}'"
        );
        v.branch = Some("$(calc)`\"".into());
        let line = shell_line("echo \"{branch}\"", &v, Shell::PowerShell).unwrap();
        assert_eq!(line, "echo \"`$(calc)```\"\"");
        // A doubled quote inside a string doesn't end it.
        let line = shell_line("echo 'it''s {commit}'", &v, Shell::PowerShell).unwrap();
        assert_eq!(line, "echo 'it''s abc123'");
        let (prog, args) = Shell::PowerShell.argv("echo 'ü'");
        assert_eq!(prog, "powershell.exe");
        assert_eq!(args.last().unwrap(), "ZQBjAGgAbwAgACcA/AAnAA==");
    }

    #[gitti_test::test]
    fn display_quotes_what_needs_it() {
        let a: Vec<String> = ["log", "--format=%H", "a b", "it's", "origin/main"].map(String::from).into();
        assert_eq!(display_args(&a), r#"log --format=%H 'a b' 'it'\''s' origin/main"#);
    }

    #[gitti_test::test]
    fn workdir_relative_absolute_home() {
        let repo = std::env::temp_dir().join("my repo");
        let repo = repo.as_path();
        let mut v = vals();
        v.repo = Some(repo.to_string_lossy().into_owned());
        assert_eq!(workdir("", &v, repo).unwrap(), repo);
        assert_eq!(workdir("lib/x", &v, repo).unwrap(), repo.join("lib/x"));
        assert_eq!(
            workdir("{repo}/sub", &v, repo).unwrap(),
            Path::new(&format!("{}/sub", repo.display()))
        );
        if let Some(home) = dirs::home_dir() {
            assert_eq!(workdir("~/src", &v, repo).unwrap(), home.join("src"));
        }
        assert!(workdir("{files}", &v, repo).is_err());
    }
}
