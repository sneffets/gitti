//! `i18n.commitEncoding`: the encoding a repository's commit messages are written in.
//!
//! git doesn't convert a message it is given (`git commit -F`, `-m`): it takes the bytes
//! as being in the configured encoding and records `encoding <name>` in the commit
//! (nothing for UTF-8). The apps hold messages as Unicode text, so writing one in a legacy
//! encoding is theirs to do — [`CommitEncoding::encode`]: every character the encoding
//! has as itself, the others as their closest equivalent ([`translit`]: `ä` → `ae`,
//! `“` → `"`, `€` → `EUR`, `é` → `e`), the rest as [`translit::PLACEHOLDER`]. What
//! gets replaced is said before committing ([`CommitEncoding::note`]) and in the result
//! ([`CommitEncoding::written_note`]). A name gitti can't write is refused in plain words
//! before git runs ([`CommitEncoding::refusal`]).
//!
//! What git does where (checked with git 2.54): `commit` / `commit-tree` / `merge` /
//! `stash` record the header; the sequencer's own commits (`cherry-pick`, `revert`,
//! `rebase` picks) re-encode the picked message; tags and notes never record one (a tag
//! message stays UTF-8 — as git, gitti writes it as typed). `git stash push -m` and
//! `git subtree --message` take the message on the command line only, which can't carry
//! legacy bytes on Windows: stashes, and a subtree merge with a message that isn't ASCII,
//! are written in UTF-8 with no header ([`CommitEncoding::stash_config`],
//! [`CommitEncoding::command_line_config`]), which every reader shows right.
//!
//! Reading: [`decode`] — a message (and the names) of a commit by its `encoding` header,
//! for every encoding gitti can write; without a header UTF-8 (git's rule).

pub mod change;
pub mod translit;

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use encoding_rs::Encoding;

/// The setting's name, as messages show it.
pub const KEY: &str = "i18n.commitEncoding";

/// How the result line about replaced characters starts ([`CommitEncoding::written_note`]).
pub const WRITTEN_LEAD: &str = "Message written in";

/// What makes git write a commit in UTF-8 whatever the repository says.
const UTF8_CONFIG: [&str; 2] = ["-c", "i18n.commitEncoding=UTF-8"];

/// How the bytes an encoder writes are checked beyond the encoder itself: the WHATWG
/// encoders `encoding_rs` has are supersets of some names git users set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Strict {
    /// What the encoder writes.
    No,
    /// US-ASCII: bytes below 0x80 only.
    Ascii,
    /// An ISO-8859 code page written by its Windows superset (ISO-8859-1 by
    /// windows-1252, …): nothing in 0x80..=0x9F, which the ISO page leaves to controls.
    Iso,
    /// EUC-KR / GB2312 written by their supersets (UHC, GBK): both bytes of a
    /// two-byte character in 0xA1..=0xFE.
    Euc,
}

/// A legacy encoding gitti writes and reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Codec {
    encoding: &'static Encoding,
    strict: Strict,
}

/// What a configured name means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// UTF-8 (unset, `UTF-8`, `utf8`, …): messages go as they are.
    Utf8,
    /// A legacy encoding gitti knows.
    Legacy(Codec),
    /// A name gitti can't write (unknown, or one that can't hold a message at all).
    Unsupported,
}

/// The names gitti knows (compared without case, `-`, `_`, `.`, `:` and spaces) and what
/// they stand for: iconv's and git's usual spellings plus the WHATWG labels. ISO-8859-N
/// names are added in [`lookup`].
const NAMES: &[(&[&str], &str, Strict)] = &[
    (&["ascii", "usascii", "ansix341968", "iso646us", "us", "646", "csascii"], "windows-1252", Strict::Ascii),
    (
        &["latin1", "l1", "isoir100", "ibm819", "cp819", "csisolatin1", "iso885911987"],
        "windows-1252",
        Strict::Iso,
    ),
    (&["latin2", "l2"], "iso-8859-2", Strict::No),
    (&["latin3", "l3"], "iso-8859-3", Strict::No),
    (&["latin4", "l4"], "iso-8859-4", Strict::No),
    (&["cyrillic"], "iso-8859-5", Strict::No),
    (&["arabic"], "iso-8859-6", Strict::No),
    (&["greek", "greek8"], "iso-8859-7", Strict::No),
    (&["hebrew"], "iso-8859-8", Strict::No),
    (&["latin5", "l5"], "windows-1254", Strict::Iso),
    (&["latin6", "l6"], "iso-8859-10", Strict::No),
    (&["tis620", "thai"], "windows-874", Strict::Iso),
    (&["latin7", "l7"], "iso-8859-13", Strict::No),
    (&["latin8", "l8"], "iso-8859-14", Strict::No),
    (&["latin9", "l9"], "iso-8859-15", Strict::No),
    (&["latin10", "l10"], "iso-8859-16", Strict::No),
    (&["windows874", "cp874", "dos874"], "windows-874", Strict::No),
    (&["windows1250", "cp1250", "xcp1250"], "windows-1250", Strict::No),
    (&["windows1251", "cp1251", "xcp1251"], "windows-1251", Strict::No),
    (&["windows1252", "cp1252", "xcp1252"], "windows-1252", Strict::No),
    (&["windows1253", "cp1253", "xcp1253"], "windows-1253", Strict::No),
    (&["windows1254", "cp1254", "xcp1254"], "windows-1254", Strict::No),
    (&["windows1255", "cp1255", "xcp1255"], "windows-1255", Strict::No),
    (&["windows1256", "cp1256", "xcp1256"], "windows-1256", Strict::No),
    (&["windows1257", "cp1257", "xcp1257"], "windows-1257", Strict::No),
    (&["windows1258", "cp1258", "xcp1258"], "windows-1258", Strict::No),
    (&["koi8r", "koi8", "koi", "cskoi8r"], "koi8-r", Strict::No),
    (&["koi8u", "koi8ru"], "koi8-u", Strict::No),
    (&["ibm866", "cp866", "866", "csibm866"], "ibm866", Strict::No),
    (&["macintosh", "mac", "macroman", "xmacroman", "csmacintosh"], "macintosh", Strict::No),
    (&["xmaccyrillic", "maccyrillic", "xmacukrainian"], "x-mac-cyrillic", Strict::No),
    (
        &["shiftjis", "sjis", "cp932", "ms932", "windows31j", "mskanji", "xsjis", "csshiftjis"],
        "shift_jis",
        Strict::No,
    ),
    (&["eucjp", "ujis", "xeucjp", "cseucpkdfmtjapanese"], "euc-jp", Strict::No),
    (&["iso2022jp", "csiso2022jp"], "iso-2022-jp", Strict::No),
    (&["gbk", "cp936", "ms936", "windows936", "xgbk"], "gbk", Strict::No),
    (
        &["gb2312", "euccn", "gb231280", "csgb2312", "chinese", "isoir58", "csiso58gb231280"],
        "gbk",
        Strict::Euc,
    ),
    (&["gb18030"], "gb18030", Strict::No),
    (&["big5", "big5hkscs", "cp950", "ms950", "csbig5", "xxbig5", "bigfive", "cnbig5"], "big5", Strict::No),
    (&["cp949", "uhc", "ms949", "windows949"], "euc-kr", Strict::No),
    (&["euckr", "cseuckr", "ksc5601", "ksc56011987", "ksc56011989", "isoir149", "korean"], "euc-kr", Strict::Euc),
];

/// Names that can't hold a commit message: git cuts a message at its first zero byte.
const WIDE: &[&str] = &[
    "utf16", "utf16le", "utf16be", "utf32", "utf32le", "utf32be", "ucs2", "ucs2le", "ucs2be", "ucs4",
    "ucs4le", "ucs4be", "utf7", "unicode",
];

fn compact(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '-' | '_' | '.' | ':' | ' '))
        .flat_map(char::to_lowercase)
        .collect()
}

/// What `name` (as configured) stands for.
pub fn lookup(name: &str) -> Target {
    let c = compact(name);
    if c == "utf8" {
        return Target::Utf8;
    }
    let codec = |label: &str, strict| {
        Encoding::for_label(label.as_bytes()).map_or(Target::Unsupported, |encoding| {
            Target::Legacy(Codec { encoding, strict })
        })
    };
    if let Some((_, label, strict)) = NAMES.iter().find(|(names, _, _)| names.contains(&c.as_str())) {
        return codec(label, *strict);
    }
    // ISO-8859-N in its spellings: `ISO-8859-1`, `iso8859-15`, `ISO_8859-2:1987`, …
    if let Some(rest) = c.strip_prefix("iso8859") {
        let n: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let year = &rest[n.len()..];
        if year.is_empty() || year.len() == 4 && year.chars().all(|d| d.is_ascii_digit()) {
            return match n.as_str() {
                "1" => codec("windows-1252", Strict::Iso),
                "9" => codec("windows-1254", Strict::Iso),
                "11" => codec("windows-874", Strict::Iso),
                "2" | "3" | "4" | "5" | "6" | "7" | "8" | "10" | "13" | "14" | "15" | "16" => {
                    codec(&format!("iso-8859-{n}"), Strict::No)
                }
                _ => Target::Unsupported,
            };
        }
    }
    Target::Unsupported
}

impl Codec {
    /// Appends `c` to `out` through `enc` when the encoding has it; `false` (nothing
    /// written) when not.
    fn push(&self, enc: &mut encoding_rs::Encoder, c: char, out: &mut Vec<u8>) -> bool {
        let mut buf = [0u8; 4];
        let s = c.encode_utf8(&mut buf);
        let need = enc.max_buffer_length_from_utf8_without_replacement(s.len()).unwrap_or(16);
        out.reserve(need);
        let start = out.len();
        let (result, _) = enc.encode_from_utf8_to_vec_without_replacement(s, out, false);
        if !matches!(result, encoding_rs::EncoderResult::InputEmpty) {
            out.truncate(start);
            return false;
        }
        let written = &out[start..];
        let ok = match self.strict {
            Strict::No => true,
            Strict::Ascii => written.iter().all(u8::is_ascii),
            Strict::Iso => !written.iter().any(|b| (0x80..=0x9f).contains(b)),
            Strict::Euc => match written {
                [b] => b.is_ascii(),
                _ => written.iter().all(|b| *b >= 0xa1),
            },
        };
        if !ok {
            out.truncate(start);
        }
        ok
    }

    /// Whether the encoding has `c` (a fresh encoder: for the stateless check).
    fn has(&self, c: char) -> bool {
        let mut out = Vec::new();
        self.push(&mut self.encoding.new_encoder(), c, &mut out)
    }

    /// `text` in this encoding; characters it lacks as their closest equivalent, the rest
    /// as [`translit::PLACEHOLDER`] — each listed once in `replaced`, in order.
    pub fn encode(&self, text: &str) -> Encoded {
        use unicode_normalization::UnicodeNormalization as _;
        let mut enc = self.encoding.new_encoder();
        let mut out = Vec::with_capacity(text.len());
        let mut replaced: Vec<Replacement> = Vec::new();
        // Composed first: `e` + combining acute is the `é` a Latin page has.
        let composed: Cow<str> = if text.is_ascii() { Cow::Borrowed(text) } else { Cow::Owned(text.nfc().collect()) };
        for c in composed.chars() {
            if self.push(&mut enc, c, &mut out) {
                continue;
            }
            let to = translit::equivalent(c, &|x| self.has(x))
                .unwrap_or_else(|| translit::PLACEHOLDER.to_string());
            for r in to.chars() {
                if !self.push(&mut enc, r, &mut out) {
                    self.push(&mut enc, translit::PLACEHOLDER, &mut out);
                }
            }
            match replaced.iter_mut().find(|r| r.from == c) {
                Some(r) => r.count += 1,
                None => replaced.push(Replacement { from: c, to, count: 1 }),
            }
        }
        // A stateful encoding (ISO-2022-JP) ends in ASCII.
        out.reserve(8);
        let _ = enc.encode_from_utf8_to_vec_without_replacement("", &mut out, true);
        Encoded { bytes: out, replaced }
    }

    /// `bytes` written in this encoding, as text.
    pub fn decode(&self, bytes: &[u8]) -> String {
        match self.strict {
            // The ISO pages' 0x80..=0x9F are the C1 controls, not the Windows letters.
            Strict::Iso | Strict::Ascii => bytes
                .iter()
                .map(|&b| match b {
                    0x80..=0x9f => char::from(b).to_string(),
                    _ => self.encoding.decode_without_bom_handling(&[b]).0.into_owned(),
                })
                .collect(),
            _ => self.encoding.decode_without_bom_handling(bytes).0.into_owned(),
        }
    }
}

/// A message written in the commit encoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Encoded {
    pub bytes: Vec<u8>,
    /// The characters the encoding lacks and what was written instead, in order.
    pub replaced: Vec<Replacement>,
}

/// One character the commit encoding lacks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replacement {
    pub from: char,
    /// What was written instead: the closest equivalent, empty when it was dropped (an
    /// invisible character), [`translit::PLACEHOLDER`] when nothing is close.
    pub to: String,
    /// How often it occurs.
    pub count: usize,
}

impl Replacement {
    /// `ä → ae`; an invisible character by its code point, a dropped one as "removed".
    pub fn text(&self) -> String {
        let from = shown(self.from);
        if self.to.is_empty() {
            format!("{from} removed")
        } else if self.to == " " {
            format!("{from} → space")
        } else {
            format!("{from} → {}", self.to)
        }
    }
}

/// A character as a message names it: itself, or `U+200B` when it doesn't show.
fn shown(c: char) -> String {
    let invisible = c.is_control()
        || c.is_whitespace()
        || unicode_normalization::char::is_combining_mark(c)
        || translit::table(c) == Some("");
    if invisible { format!("U+{:04X}", c as u32) } else { c.to_string() }
}

/// The effective `i18n.commitEncoding` of a repository and where it is set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitEncoding {
    /// The value as configured; `None` while it is unset (UTF-8).
    pub configured: Option<String>,
    /// Where it is set, for messages: a config file (relative to the work tree when in
    /// it, `~/…` in the home directory), or the command line.
    pub origin: Option<String>,
    /// The config file it is set in (to unset it there), if a file.
    file: Option<PathBuf>,
    /// Set in the repository's own config (`.git/config`).
    local: bool,
    /// Which level of git's config sets it (`None` while unset).
    pub scope: Option<SetIn>,
}

/// The level of git's config a setting comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetIn {
    /// The repository's config (`.git/config`, or a file it includes).
    Repository,
    /// This worktree's own config (`config.worktree`, `extensions.worktreeConfig`).
    Worktree,
    /// The user's config (`~/.gitconfig`, `~/.config/git/config`): every repository.
    Global,
    /// The system's (or the git installation's) config: every user.
    System,
    /// `git -c` or the environment (`GIT_CONFIG_COUNT`, …).
    CommandLine,
}

impl SetIn {
    /// `this repository`, `global`, … — what the level means, for messages.
    pub fn text(self) -> &'static str {
        match self {
            SetIn::Repository => "this repository",
            SetIn::Worktree => "this worktree",
            SetIn::Global => "global: all your repositories",
            SetIn::System => "system: all users",
            SetIn::CommandLine => "the command line",
        }
    }

    /// `git config --show-scope`'s word.
    fn from_git(word: &str) -> Option<SetIn> {
        Some(match word {
            "local" => SetIn::Repository,
            "worktree" => SetIn::Worktree,
            "global" => SetIn::Global,
            "system" => SetIn::System,
            "command" => SetIn::CommandLine,
            _ => return None,
        })
    }
}

impl CommitEncoding {
    /// Unset: UTF-8.
    pub fn utf8() -> Self {
        Self::default()
    }

    /// `name` set in `file` (`None`: given on the command line); `local`: the
    /// repository's own config. `workdir` shortens the shown path.
    pub fn new(name: &str, file: Option<&Path>, local: bool, workdir: &Path) -> Self {
        let origin = match file {
            Some(f) => shown_path(f, workdir),
            None => "the command line (-c)".into(),
        };
        Self {
            configured: Some(name.to_string()),
            origin: Some(origin),
            file: file.map(Path::to_path_buf),
            local,
            scope: None,
        }
    }

    /// The same, set at config level `scope`.
    pub fn set_in(mut self, scope: SetIn) -> Self {
        self.scope = Some(scope);
        self
    }

    /// Where the setting comes from, in words: `not set`, `set in .git/config (this
    /// repository)`, `set on the command line (-c)`.
    pub fn where_text(&self) -> String {
        match (&self.origin, self.scope) {
            (None, _) => "not set".into(),
            (Some(_), _) if self.file.is_none() => "set on the command line (-c)".into(),
            (Some(o), Some(scope)) => format!("set in {o} ({})", scope.text()),
            (Some(o), None) => format!("set in {o}"),
        }
    }

    /// What the configured name stands for.
    pub fn target(&self) -> Target {
        self.configured.as_deref().map_or(Target::Utf8, lookup)
    }

    /// The name as configured (`UTF-8` while unset).
    pub fn name(&self) -> &str {
        self.configured.as_deref().unwrap_or("UTF-8")
    }

    /// A legacy encoding gitti writes.
    pub fn is_legacy(&self) -> bool {
        matches!(self.target(), Target::Legacy(_))
    }

    fn setting(&self) -> String {
        match &self.origin {
            Some(o) => format!("{KEY} in {o}"),
            None => KEY.to_string(),
        }
    }

    /// Why no commit message can be written: the configured name is one gitti doesn't
    /// know, or one that can't hold a message.
    pub fn refusal(&self) -> Option<String> {
        if self.target() != Target::Unsupported {
            return None;
        }
        let name = self.name();
        let why = if name.trim().is_empty() {
            "it is empty, which names no encoding".to_string()
        } else if WIDE.contains(&compact(name).as_str()) {
            format!("“{name}” can't hold a commit message (git cuts a message at its first zero byte)")
        } else {
            format!("gitti can't write commit messages in “{name}”")
        };
        let unset = match (&self.file, self.local) {
            (_, true) => format!("git config --unset {KEY}"),
            (Some(f), false) => format!("git config --file \"{}\" --unset {KEY}", f.display()),
            (None, false) => "leave out the -c".into(),
        };
        Some(format!(
            "Not committed: {} — {why}. Set it to an encoding gitti knows (UTF-8, ISO-8859-1 … 16, \
             windows-1250 … 1258, KOI8-R, Shift_JIS, EUC-JP, ISO-2022-JP, GBK, GB18030, Big5, EUC-KR, …) \
             or remove it: {unset}",
            self.setting()
        ))
    }

    /// `message` as git must get it: UTF-8 as it is, a legacy encoding through
    /// [`Codec::encode`]; refused ([`Self::refusal`]) for a name gitti can't write.
    pub fn encode(&self, message: &str) -> Result<Encoded> {
        match self.target() {
            Target::Utf8 => Ok(Encoded { bytes: message.as_bytes().to_vec(), replaced: Vec::new() }),
            Target::Legacy(codec) => Ok(codec.encode(message)),
            Target::Unsupported => bail!(self.refusal().unwrap_or_default()),
        }
    }

    /// What the box shows under a message before it is committed: the refusal, or which
    /// characters the encoding lacks and what is written instead — `None` when the
    /// message goes as typed.
    pub fn note(&self, message: &str) -> Option<String> {
        if let Some(r) = self.refusal() {
            return Some(r);
        }
        if message.is_ascii() {
            return None;
        }
        let Target::Legacy(codec) = self.target() else {
            return None;
        };
        let encoded = codec.encode(message);
        (!encoded.replaced.is_empty())
            .then(|| self.replaced_text(&format!("Not in {}", self.name()), &encoded.replaced))
    }

    /// The line a dialog that takes a commit message shows (reword, merge): the encoding
    /// messages are written in and what happens to characters it lacks, or the refusal;
    /// `None` for UTF-8.
    pub fn dialog_line(&self) -> Option<String> {
        match self.target() {
            Target::Utf8 => None,
            Target::Unsupported => self.refusal(),
            Target::Legacy(_) => Some(format!(
                "The message is written in {} ({}): characters it lacks become their closest \
                 equivalent (ä → ae, “ → \", € → EUR), others {}.",
                self.name(),
                self.setting(),
                translit::PLACEHOLDER
            )),
        }
    }

    /// The line a commit's result gets when characters were replaced (empty when none).
    pub fn written_note(&self, encoded: &Encoded) -> String {
        if encoded.replaced.is_empty() {
            return String::new();
        }
        self.replaced_text(&format!("{WRITTEN_LEAD} {}, replaced", self.name()), &encoded.replaced)
    }

    /// `{lead}: – → -, € → EUR (i18n.commitEncoding in .git/config)` — the replacements
    /// early, where a narrow note row still shows them.
    fn replaced_text(&self, lead: &str, replaced: &[Replacement]) -> String {
        const SHOWN: usize = 8;
        let mut list: Vec<String> = replaced.iter().take(SHOWN).map(Replacement::text).collect();
        if replaced.len() > SHOWN {
            list.push(format!("and {} more", replaced.len() - SHOWN));
        }
        format!("{lead}: {} ({})", list.join(", "), self.setting())
    }

    /// The config for a stash in this repository: `-c i18n.commitEncoding=UTF-8` unless
    /// it is UTF-8 already. `git stash push -m` takes its message on the command line
    /// only, which carries no legacy bytes on Windows, and git puts the branch name (UTF-8
    /// in a ref) into every stash message: stash commits are written in UTF-8 with no
    /// header, which every reader shows right (git's own stash subject of HEAD is then
    /// re-encoded to UTF-8 too). A stash is local and short-lived; what is shared stays
    /// in the configured encoding.
    pub fn stash_config(&self) -> &'static [&'static str] {
        if self.target() == Target::Utf8 { &[] } else { &UTF8_CONFIG }
    }

    /// The config for a command that takes a commit message on the command line only
    /// (`subtree --message`): `-c i18n.commitEncoding=UTF-8` when `message` isn't ASCII
    /// in a repository whose encoding isn't UTF-8 (see [`Self::stash_config`]); said in
    /// the result with [`Self::utf8_note`].
    pub fn command_line_config(&self, message: &str) -> &'static [&'static str] {
        if message.is_ascii() { &[] } else { self.stash_config() }
    }

    /// The line a result gets when [`Self::command_line_config`] applied: `what` (the
    /// merge message) was stored in UTF-8, not the configured encoding.
    pub fn utf8_note(&self, what: &str) -> String {
        format!(
            "{what} is stored in UTF-8, not {} ({}): git subtree takes it on the command line only",
            self.name(),
            self.setting()
        )
    }

    /// A file git wrote in this encoding (`MERGE_MSG`, `SQUASH_MSG`, …) as text: UTF-8
    /// when it is valid UTF-8, else decoded by this encoding.
    pub fn decode_file(&self, bytes: &[u8]) -> String {
        match std::str::from_utf8(bytes) {
            Ok(s) => s.to_string(),
            Err(_) => match self.target() {
                Target::Legacy(codec) => codec.decode(bytes),
                _ => crate::git_cli::decode_native(bytes),
            },
        }
    }
}

/// `path` relative to `workdir` when inside it, `~/…` in the home directory.
pub(crate) fn shown_path(path: &Path, workdir: &Path) -> String {
    if let Ok(rel) = path.strip_prefix(workdir) {
        return rel.display().to_string();
    }
    if let Some(home) = dirs::home_dir()
        && let Ok(rel) = path.strip_prefix(&home)
    {
        return format!("~/{}", rel.display());
    }
    path.display().to_string()
}

/// Commit text (message, names) of a commit with `encoding` header `header` as UTF-8.
/// Text that is valid UTF-8 is taken as it is (also under a legacy header: messages
/// that were passed to git as UTF-8 with the setting on, as gitti did before); else it
/// is decoded by the header for every encoding gitti writes ([`lookup`]); no header or an
/// unknown one: UTF-8 with invalid bytes replaced.
pub fn decode(bytes: &[u8], header: Option<&[u8]>) -> String {
    let codec = header
        .and_then(|h| std::str::from_utf8(h).ok())
        .and_then(|name| match lookup(name.trim()) {
            Target::Legacy(codec) => Some(codec),
            _ => None,
        });
    match (std::str::from_utf8(bytes), codec) {
        // ISO-2022-JP is 7-bit — valid UTF-8 whatever it says.
        (_, Some(codec)) if codec.encoding == encoding_rs::ISO_2022_JP => codec.decode(bytes),
        (Ok(s), _) => s.to_string(),
        (Err(_), Some(codec)) => codec.decode(bytes),
        (Err(_), None) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// The full message of `commit`, decoded by its `encoding` header.
pub(crate) fn message_of(commit: &gix::Commit<'_>) -> String {
    match commit.decode() {
        Ok(d) => decode(d.message, d.encoding.map(|e| e.as_ref())),
        Err(_) => crate::git_cli::decode_native(commit.message_raw_sloppy()),
    }
}

impl crate::Repo {
    /// The effective `i18n.commitEncoding` as git's commands see it — the repository's,
    /// its worktree's, the user's and the system's config with their includes, the
    /// environment's `GIT_CONFIG_GLOBAL` / `GIT_CONFIG_SYSTEM` / `GIT_CONFIG_NOSYSTEM`.
    /// In-process; one `git config` only when this repository's runner sets config
    /// variables of its own (tests).
    pub fn commit_encoding(&self) -> CommitEncoding {
        let git = self.git();
        let runner_sets = [
            "GIT_CONFIG_GLOBAL",
            "GIT_CONFIG_SYSTEM",
            "GIT_CONFIG_NOSYSTEM",
            "GIT_CONFIG_PARAMETERS",
            "GIT_CONFIG_COUNT",
        ]
        .iter()
        .any(|v| git.env_var(v) != std::env::var(v).ok());
        let read = if runner_sets { None } else { self.commit_encoding_in_process() };
        read.unwrap_or_else(|| self.commit_encoding_by_git())
    }

    /// [`Self::commit_encoding`] for what the apps show before committing: in-process
    /// always (a refresh runs no extra git) — the commit itself reads it exactly.
    pub fn commit_encoding_shown(&self) -> CommitEncoding {
        self.commit_encoding_in_process().unwrap_or_default()
    }

    fn commit_encoding_in_process(&self) -> Option<CommitEncoding> {
        // As `Repo::gix`, but with the `GIT_CONFIG_*` file variables git honours (the
        // runner passes them on); `GIT_CONFIG_COUNT` / `-PARAMETERS` the runner strips.
        let mut perms = crate::repo::gix_permissions();
        perms.env.git_prefix = gix::sec::Permission::Allow;
        perms.config.env = false;
        let repo = gix::open_opts(self.workdir(), gix::open::Options::default().permissions(perms)).ok()?;
        let config = repo.config_snapshot();
        let file = config.plumbing();
        let mut found: Option<(String, gix::config::file::Metadata)> = None;
        for section in file.sections_by_name("i18n").into_iter().flatten() {
            if section.header().subsection_name().is_some() {
                continue;
            }
            // The last value wins, as with git; a bare key is `true` to git — no name.
            if let Some(v) = section.value_implicit("commitEncoding") {
                let v = v.map(|v| v.to_string()).unwrap_or_default();
                found = Some((v, section.meta().clone()));
            }
        }
        let Some((value, meta)) = found else {
            return Some(CommitEncoding::utf8());
        };
        use gix::config::Source;
        let local = matches!(meta.source, Source::Local) && meta.level == 0;
        let file = match meta.source {
            Source::Cli | Source::Api | Source::Env | Source::EnvOverride => None,
            _ => meta.path.clone(),
        };
        let scope = match meta.source {
            Source::Local => SetIn::Repository,
            Source::Worktree => SetIn::Worktree,
            Source::User | Source::Git => SetIn::Global,
            Source::System | Source::GitInstallation => SetIn::System,
            Source::Cli | Source::Api | Source::Env | Source::EnvOverride => SetIn::CommandLine,
        };
        Some(CommitEncoding::new(&value, file.as_deref(), local, self.workdir()).set_in(scope))
    }

    /// `git config --show-scope --show-origin --get i18n.commitEncoding`.
    fn commit_encoding_by_git(&self) -> CommitEncoding {
        let args = ["config", "--show-scope", "--show-origin", "--get", KEY];
        let Ok(out) = self.git().run_raw(&args, None) else {
            return CommitEncoding::utf8();
        };
        let text = out.stdout_str();
        let line = text.lines().next().unwrap_or("");
        if out.code != 0 || line.is_empty() {
            return CommitEncoding::utf8();
        }
        let (scope, line) = line.split_once('\t').unwrap_or(("", line));
        let (origin, value) = line.split_once('\t').unwrap_or(("", line));
        let file = origin.strip_prefix("file:").map(PathBuf::from).map(|p| {
            if p.is_relative() { self.workdir().join(p) } else { p }
        });
        let local = file.as_ref().is_some_and(|f| {
            let common = self.common_dir();
            f == &common.join("config") || f.canonicalize().ok() == common.join("config").canonicalize().ok()
        });
        let encoding = CommitEncoding::new(value, file.as_deref(), local, self.workdir());
        match SetIn::from_git(scope) {
            Some(scope) => encoding.set_in(scope),
            None => encoding,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codec(name: &str) -> Codec {
        match lookup(name) {
            Target::Legacy(c) => c,
            t => panic!("{name}: {t:?}"),
        }
    }

    #[gitti_test::test]
    fn names_as_git_users_write_them() {
        for n in ["UTF-8", "utf8", "UTF_8", "Utf-8"] {
            assert_eq!(lookup(n), Target::Utf8, "{n}");
        }
        for n in [
            "ISO-8859-1", "iso8859-1", "ISO_8859-1:1987", "latin1", "Latin-1", "L1", "CP1252", "windows-1252",
            "ISO-8859-15", "latin9", "ISO-8859-2", "ISO8859-9", "KOI8-R", "koi8r", "KOI8-U", "CP1251",
            "SHIFT-JIS", "Shift_JIS", "SJIS", "CP932", "EUC-JP", "eucJP", "ISO-2022-JP", "GBK", "CP936",
            "GB2312", "EUC-CN", "GB18030", "BIG5", "Big5-HKSCS", "EUC-KR", "CP949", "UHC", "US-ASCII",
            "MacRoman", "CP866", "TIS-620", "ISO-8859-16",
        ] {
            assert!(matches!(lookup(n), Target::Legacy(_)), "{n}");
        }
        for n in ["UTF-16", "UTF-16LE", "UCS-2", "UTF-32", "latin-x", "", "ISO-8859-12", "EBCDIC-US", "CP437"] {
            assert_eq!(lookup(n), Target::Unsupported, "{n}");
        }
    }

    /// Round trip per encoding family: what an encoding has comes back unchanged.
    #[gitti_test::test]
    fn round_trip_per_family() {
        let cases: &[(&str, &str)] = &[
            ("ISO-8859-1", "Grüße, café, Señor"),
            ("ISO-8859-15", "Preis: 5 €, œuvre"),
            ("windows-1252", "„Anführung“ – … €"),
            ("ISO-8859-2", "Zażółć gęślą jaźń"),
            ("KOI8-R", "Съешь же ещё этих мягких французских булок"),
            ("windows-1251", "Привет, мир"),
            ("ISO-8859-7", "Καλημέρα"),
            ("Shift_JIS", "日本語のコミットメッセージ"),
            ("EUC-JP", "日本語のコミットメッセージ"),
            ("ISO-2022-JP", "日本語 and ASCII 混在"),
            ("GBK", "中文提交信息"),
            ("GB2312", "中文提交信息"),
            ("GB18030", "中文提交信息 😀"),
            ("Big5", "中文提交訊息"),
            ("EUC-KR", "한국어 커밋 메시지"),
            ("US-ASCII", "plain ascii"),
        ];
        for (name, text) in cases {
            let c = codec(name);
            let e = c.encode(text);
            assert!(e.replaced.is_empty(), "{name}: {:?}", e.replaced);
            if text.is_ascii() {
                assert_eq!(e.bytes, text.as_bytes(), "{name}");
            } else {
                assert_ne!(e.bytes, text.as_bytes(), "{name}: written in the encoding");
            }
            assert_eq!(c.decode(&e.bytes), *text, "{name}");
            assert_eq!(decode(&e.bytes, Some(name.as_bytes())), *text, "{name} by the header");
        }
        // Latin-1 bytes are the code points.
        assert_eq!(codec("ISO-8859-1").encode("Grüße").bytes, b"Gr\xfc\xdfe");
    }

    #[gitti_test::test]
    fn characters_the_encoding_lacks() {
        let e = codec("ISO-8859-1").encode("“Grüße” – 5 € … 漢字 😀 ok");
        assert_eq!(e.bytes, b"\"Gr\xfc\xdfe\" - 5 EUR ... ?? ? ok");
        let r: Vec<String> = e.replaced.iter().map(Replacement::text).collect();
        assert_eq!(r, ["“ → \"", "” → \"", "– → -", "€ → EUR", "… → ...", "漢 → ?", "字 → ?", "😀 → ?"]);
        // ä missing (KOI8-R): the German way.
        let e = codec("KOI8-R").encode("Größe: ä");
        assert_eq!(e.bytes, b"Groesse: ae");
        // ISO-8859-1 has no 0x80..=0x9F letters (windows-1252's €, Œ, …).
        assert_eq!(codec("ISO-8859-1").encode("Œ").bytes, b"OE");
        assert_eq!(codec("windows-1252").encode("Œ€").bytes, b"\x8c\x80");
        // GB2312 is not GBK: € (GBK 0x80) and 㐀 (GBK only) aren't in it.
        assert_eq!(codec("GB2312").encode("€").bytes, b"EUR");
        assert_eq!(codec("GBK").encode("€").bytes, b"\x80");
        // A decomposed accent composes first; an invisible joiner goes.
        let e = codec("ISO-8859-1").encode("e\u{301} a\u{200b}b");
        assert_eq!(e.bytes, b"\xe9 ab");
        assert_eq!(e.replaced.iter().map(Replacement::text).collect::<Vec<_>>(), ["U+200B removed"]);
        // Counted once each.
        let e = codec("US-ASCII").encode("ä ä ä");
        assert_eq!((e.replaced.len(), e.replaced[0].count), (1, 3));
        // ISO-2022-JP ends in ASCII after a replacement too.
        let e = codec("ISO-2022-JP").encode("日本 €");
        assert_eq!(e.bytes, b"\x1b$BF|K\\\x1b(B EUR");
    }

    #[gitti_test::test]
    fn notes_and_refusals() {
        let wd = Path::new("/r");
        let latin1 = CommitEncoding::new("ISO-8859-1", Some(Path::new("/r/.git/config")), true, wd);
        assert_eq!(latin1.note("plain"), None);
        assert_eq!(latin1.note("Grüße"), None, "Latin-1 has them");
        assert_eq!(
            latin1.note("Grüße “x”").as_deref(),
            Some("Not in ISO-8859-1: “ → \", ” → \" (i18n.commitEncoding in .git/config)")
        );
        assert_eq!(CommitEncoding::utf8().note("漢字"), None);
        let bad = CommitEncoding::new("latin-x", Some(Path::new("/elsewhere/gitconfig")), false, wd);
        let why = bad.refusal().unwrap();
        assert!(why.starts_with("Not committed: i18n.commitEncoding in /elsewhere/gitconfig — gitti can't write commit messages in “latin-x”"), "{why}");
        assert!(why.ends_with("git config --file \"/elsewhere/gitconfig\" --unset i18n.commitEncoding"), "{why}");
        assert_eq!(bad.note("plain ascii").as_deref(), Some(why.as_str()));
        assert!(bad.encode("x").is_err());
        let wide = CommitEncoding::new("UTF-16", None, false, wd);
        assert!(wide.refusal().unwrap().contains("can't hold a commit message"));
        assert_eq!(latin1.stash_config(), UTF8_CONFIG);
        assert_eq!(bad.stash_config(), UTF8_CONFIG);
        assert!(CommitEncoding::utf8().stash_config().is_empty());
        assert_eq!(latin1.command_line_config("Grüße"), UTF8_CONFIG);
        assert!(latin1.command_line_config("plain").is_empty());
        assert!(CommitEncoding::utf8().command_line_config("Grüße").is_empty());
    }

    #[gitti_test::test]
    fn decode_by_header() {
        assert_eq!(decode(b"Caf\xe9", Some(b"ISO-8859-1")), "Café");
        assert_eq!(decode(b"Caf\xe9", Some(b"latin1")), "Café");
        // Valid UTF-8 wins over a wrong header.
        assert_eq!(decode("Café".as_bytes(), Some(b"ISO-8859-1")), "Café");
        assert_eq!(decode(b"\xa4 \xbd", Some(b"ISO-8859-15")), "€ œ");
        assert_eq!(decode(b"\x80 \x93x\x94 \xe9", Some(b"windows-1252")), "€ “x” é");
        assert_eq!(decode(b"\x80\x81", Some(b"CP1252")), "€\u{81}");
        // ISO-8859-1's 0x80 is a control, not windows-1252's €.
        assert_eq!(decode(b"\x80\xe9", Some(b"ISO-8859-1")), "\u{80}é");
        assert_eq!(decode(b"\xf0\xd2\xc9\xd7\xc5\xd4", Some(b"KOI8-R")), "Привет");
        assert_eq!(decode(b"\x93\xfa\x96\x7b", Some(b"SHIFT_JIS")), "日本");
        assert_eq!(decode(b"\xc6\xfc\xcb\xdc", Some(b"EUC-JP")), "日本");
        // No or an unknown header: UTF-8 with replacement characters.
        assert_eq!(decode(b"Caf\xe9", None), "Caf\u{fffd}");
        assert_eq!(decode(b"Caf\xe9", Some(b"latin-x")), "Caf\u{fffd}");
    }
}
