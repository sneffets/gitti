//! `i18n.commitEncoding` (`gitti_core::commit_encoding`): every path that hands git a
//! commit message writes it in the configured encoding — characters it lacks replaced by
//! their closest equivalent and said in the result —, refuses a name gitti can't write
//! before git runs, and reads messages back by their `encoding` header. Commits are
//! checked as raw objects (gix, no git process).

use std::path::Path;

use gitti_core::commit_encoding::{self, CommitEncoding, SetIn};
use gitti_core::commit_message::CommitOptions;
use gitti_core::ops::{CommitKind, FastForward, MergeOptions, StashFlags};
use gitti_core::Repo;
use gitti_workflow::{Fixture, put};

/// The raw object of `rev` (header and message as stored).
fn raw(dir: &Path, rev: &str) -> Vec<u8> {
    let repo = gix::open(dir).unwrap();
    let id = repo.rev_parse_single(rev).unwrap().detach();
    repo.find_object(id).unwrap().data.clone()
}

/// The `encoding` header of a raw commit and its message bytes.
fn split(raw: &[u8]) -> (Option<String>, Vec<u8>) {
    let at = raw.windows(2).position(|w| w == b"\n\n").expect("a message");
    let header = String::from_utf8_lossy(&raw[..at]).into_owned();
    let enc = header.lines().find_map(|l| l.strip_prefix("encoding ").map(String::from));
    (enc, raw[at + 2..].to_vec())
}

fn latin1(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| u8::try_from(u32::from(c)).expect("a Latin-1 character"))
        .collect()
}

fn encoding_config(name: &str) -> String {
    format!("[i18n]\n\tcommitEncoding = {name}\n")
}

/// Commit, amend and reword in ISO-8859-1: the header and Latin-1 bytes, what the
/// encoding lacks replaced and said first in the result, read back decoded.
#[gitti_test::test]
fn commit_amend_and_reword_write_the_configured_encoding() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut fx = Fixture::new();
    fx.commit("main", "base", &[put("a.txt", "a\n")]);
    fx.commit("main", "second", &[put("a.txt", "a\nb\n")]);
    fx.config(&encoding_config("ISO-8859-1"));
    fx.write("a.txt", "a\nb\nc\n");
    fx.build(dir);
    let repo = Repo::open(dir).unwrap();
    repo.stage_paths(&["a.txt"]).unwrap();
    let enc = repo.commit_encoding();
    assert_eq!((enc.configured.as_deref(), enc.origin.as_deref()), (Some("ISO-8859-1"), Some(".git/config")));

    let typed = "Grüße “Größe” – 5 €";
    let written = "Grüße \"Größe\" - 5 EUR";
    let note = enc.note(typed).unwrap();
    assert_eq!(
        note,
        "Not in ISO-8859-1: “ → \", ” → \", – → -, € → EUR (i18n.commitEncoding in .git/config)"
    );
    let out = repo.commit_with(typed, CommitKind::Staged, CommitOptions::default()).unwrap();
    assert_eq!(
        out.lines().next(),
        Some(note.replacen("Not in ISO-8859-1", "Message written in ISO-8859-1, replaced", 1).as_str()),
        "{out}"
    );
    let (header, message) = split(&raw(dir, "HEAD"));
    assert_eq!(header.as_deref(), Some("ISO-8859-1"));
    assert_eq!(message, [latin1(written), b"\n".to_vec()].concat());
    assert_eq!(repo.head_message().unwrap(), format!("{written}\n"));
    let head = gix::open(dir).unwrap().head_id().unwrap().detach();
    assert_eq!(repo.commit_details(head).unwrap().message, format!("{written}\n"));

    // Amend: everything fits — no note.
    let out = repo.commit_with("Ärger", CommitKind::Amend, CommitOptions::default()).unwrap();
    assert!(!out.contains("written in"), "{out}");
    assert_eq!(split(&raw(dir, "HEAD")).1, b"\xc4rger\n");

    // Reword an older commit (a rebase whose editor copies the message in).
    let second = gix::open(dir).unwrap().rev_parse_single("HEAD~1").unwrap().detach();
    let out = repo.reword(second, "Zweite … ü").unwrap();
    assert!(out.starts_with("Message written in ISO-8859-1"), "{out}");
    let (header, message) = split(&raw(dir, "HEAD~1"));
    assert_eq!((header.as_deref(), message), (Some("ISO-8859-1"), b"Zweite ... \xfc\n".to_vec()));
    // The commit after it is picked unchanged.
    assert_eq!(split(&raw(dir, "HEAD")).1, b"\xc4rger\n");
}

/// A merge with a typed message in Shift_JIS: through a file (a command line carries no
/// legacy bytes on Windows), removed afterwards.
#[gitti_test::test]
fn merge_message_in_shift_jis() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut fx = Fixture::new();
    let base = fx.commit("main", "base", &[put("a.txt", "a\n")]);
    fx.commit("main", "main", &[put("m.txt", "m\n")]);
    fx.commit_from("side", base, "side", &[put("s.txt", "s\n")]);
    fx.config(&encoding_config("Shift_JIS"));
    fx.build(dir);
    let repo = Repo::open(dir).unwrap();
    let opts = MergeOptions {
        ff: FastForward::Never,
        message: Some("マージ – 完了 ü".into()),
        ..MergeOptions::default()
    };
    let out = repo.merge_with("side", &opts).unwrap();
    assert!(out.starts_with("Message written in Shift_JIS"), "{out}");
    let (header, message) = split(&raw(dir, "HEAD"));
    assert_eq!(header.as_deref(), Some("Shift_JIS"));
    assert_eq!(commit_encoding::decode(&message, Some(b"Shift_JIS")), "マージ - 完了 ue\n");
    assert_ne!(message, "マージ - 完了 ue\n".as_bytes());
    assert!(!dir.join(".git/GITTI_MERGE_MSG").exists());
}

/// A rebase stopped on a conflict, in EUC-JP: the box starts with the stopped commit's
/// message decoded (`MERGE_MSG` is in the commit encoding); Continue with an edited
/// message writes it in EUC-JP.
#[gitti_test::test]
fn rebase_continue_with_the_box_message_in_euc_jp() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut fx = Fixture::new();
    let base = fx.commit("main", "base", &[put("a.txt", "base\n")]);
    fx.commit("main", "main", &[put("a.txt", "main\n")]);
    // The side commit's message in EUC-JP, as git writes it with the setting on.
    let side_msg = [b"\xa5\xb5\xa5\xa4\xa5\xc9".to_vec(), b"\n".to_vec()].concat();
    fx.commit_raw("side", Some(base.into()), &side_msg, &[put("a.txt", "side\n")]);
    fx.checkout("side");
    fx.config(&encoding_config("EUC-JP"));
    fx.git_after_stops(&["rebase", "main"]);
    fx.build(dir);
    let repo = Repo::open(dir).unwrap();
    assert_eq!(repo.prepared_message().as_deref(), Some("サイド"));
    std::fs::write(dir.join("a.txt"), "resolved\n").unwrap();
    repo.stage_paths(&["a.txt"]).unwrap();
    let out = repo.continue_with(Some("解決 ok ü")).unwrap();
    assert!(out.starts_with("Rebased side onto main") && out.contains(" Message written in EUC-JP"), "{out}");
    let (header, message) = split(&raw(dir, "HEAD"));
    assert_eq!(header.as_deref(), Some("EUC-JP"));
    assert_eq!(commit_encoding::decode(&message, Some(b"EUC-JP")), "解決 ok ue\n");
}

/// A name gitti can't write: commit, amend, reword and a merge with a message are
/// refused before git runs, naming the setting and where it is set; a stash still works
/// (stashes are UTF-8).
#[gitti_test::test]
fn an_unknown_encoding_is_refused_before_git_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut fx = Fixture::new();
    fx.commit("main", "base", &[put("a.txt", "a\n")]);
    fx.config(&encoding_config("latin-x"));
    fx.write("a.txt", "a\nb\n");
    fx.build(dir);
    let repo = Repo::open(dir).unwrap();
    let before = raw(dir, "HEAD");
    let why = format!("{:#}", repo.commit_with("plain", CommitKind::Empty, CommitOptions::default()).unwrap_err());
    assert!(why.starts_with("Not committed: i18n.commitEncoding in .git/config — gitti can't write commit messages in “latin-x”"), "{why}");
    assert!(why.ends_with("git config --unset i18n.commitEncoding"), "{why}");
    let head = gix::open(dir).unwrap().head_id().unwrap().detach();
    assert!(repo.reword(head, "x").is_err());
    let opts = MergeOptions { message: Some("m".into()), ..MergeOptions::default() };
    assert!(repo.merge_with("main", &opts).is_err());
    assert_eq!(raw(dir, "HEAD"), before, "nothing committed");
    assert!(repo.commit_encoding().note("").is_some(), "the box says it before");
    repo.stash_push("Stäsh", StashFlags::default(), &[]).unwrap();
    let (header, message) = split(&raw(dir, "refs/stash"));
    assert_eq!((header, String::from_utf8(message).unwrap()), (None, "On main: Stäsh".to_string()));
}

/// Stashes in a legacy-encoded repository: UTF-8 without a header (git stash takes the
/// message on the command line only) — the typed one and git's own.
#[gitti_test::test]
fn stashes_are_written_in_utf8() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut fx = Fixture::new();
    fx.commit("main", "base", &[put("a.txt", "a\n")]);
    fx.config(&encoding_config("ISO-8859-1"));
    fx.write("a.txt", "a\nb\n");
    fx.build(dir);
    let repo = Repo::open(dir).unwrap();
    repo.stash_push("Stäsh – 漢", StashFlags::default(), &[]).unwrap();
    let (header, message) = split(&raw(dir, "refs/stash"));
    assert_eq!(header, None);
    assert_eq!(String::from_utf8(message).unwrap(), "On main: Stäsh – 漢");
    let listed = repo.stashes().unwrap();
    assert!(listed[0].message.contains("Stäsh – 漢"), "{listed:?}");
}

/// Where the setting comes from: the repository, the worktree's config, an included
/// file, the user's config (`GIT_CONFIG_GLOBAL` of the runner: one `git config`).
#[gitti_test::test]
fn the_setting_is_read_as_git_resolves_it() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut fx = Fixture::new();
    fx.commit("main", "base", &[put("a.txt", "a\n")]);
    fx.build(dir);
    let mut repo = Repo::open(dir).unwrap();
    assert_eq!(repo.commit_encoding(), CommitEncoding::utf8());
    let config = dir.join(".git/config");
    let base = std::fs::read_to_string(&config).unwrap();
    // An included file wins over an earlier value.
    std::fs::write(dir.join(".git/enc.inc"), encoding_config("KOI8-R")).unwrap();
    std::fs::write(&config, format!("{base}{}[include]\n\tpath = enc.inc\n", encoding_config("ISO-8859-1"))).unwrap();
    let e = repo.commit_encoding();
    assert_eq!((e.name(), e.origin.as_deref()), ("KOI8-R", Some(".git/enc.inc")));
    assert_eq!(e.where_text(), "set in .git/enc.inc (this repository)");
    // The worktree's own config (extensions.worktreeConfig) wins over the repository's.
    std::fs::write(
        &config,
        format!("{base}[extensions]\n\tworktreeConfig = true\n{}", encoding_config("ISO-8859-1")),
    )
    .unwrap();
    std::fs::write(dir.join(".git/config.worktree"), encoding_config("EUC-JP")).unwrap();
    let e = repo.commit_encoding();
    assert_eq!((e.name(), e.origin.as_deref()), ("EUC-JP", Some(".git/config.worktree")));
    assert_eq!(e.scope, Some(SetIn::Worktree));
    std::fs::remove_file(dir.join(".git/config.worktree")).unwrap();
    std::fs::write(&config, &base).unwrap();
    // The runner's own user config: asked from git.
    let global = tmp.path().join("global.gitconfig");
    std::fs::write(&global, encoding_config("windows-1251")).unwrap();
    repo.set_env(vec![("GIT_CONFIG_GLOBAL".into(), global.display().to_string())]);
    let e = repo.commit_encoding();
    assert_eq!(e.name(), "windows-1251");
    assert!(e.is_legacy());
    assert_eq!(e.scope, Some(SetIn::Global), "git's --show-scope");
    let shown = e.origin.unwrap();
    assert!(shown.ends_with("global.gitconfig"), "{shown}");
}

/// Changing the setting (Commit Encoding…): the dialog's texts name the file and what
/// follows, refuse a name gitti can't write and a change that changes nothing, say when
/// a stronger level keeps another value; the write is for new commits only.
#[gitti_test::test]
fn changing_the_setting_says_what_follows() {
    use gitti_core::commit_encoding::change::{self, describe};
    use gitti_core::ops::ConfigScope::{Global, Local};
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut fx = Fixture::new();
    fx.commit("main", "base", &[put("a.txt", "a\n")]);
    fx.build(dir);
    let repo = Repo::open(dir).unwrap();
    let unset = repo.commit_encoding_shown();
    assert!(change::intro(&unset).starts_with("New commit messages are written in UTF-8 — i18n.commitEncoding not set."));
    assert!(describe("", Local, &unset).unwrap_err().contains("not set"));
    assert!(describe("latin-x", Local, &unset).unwrap_err().starts_with("gitti can't write commit messages in “latin-x”"));
    let latin = describe(" ISO-8859-1 ", Local, &unset).unwrap();
    for part in [
        "Writes i18n.commitEncoding = ISO-8859-1 to this repository's .git/config: new commit messages are \
         written in ISO-8859-1 and git records “encoding ISO-8859-1” in them",
        "Existing commits keep their encoding and stay readable.",
    ] {
        assert!(latin.contains(part), "lacks `{part}`: {latin}");
    }
    assert!(describe("UTF-8", Global, &unset).unwrap().contains("every repository of yours"));

    assert!(repo.set_commit_encoding(Local, "latin-x").is_err(), "refused before git");
    let done = repo.set_commit_encoding(Local, "ISO-8859-15").unwrap();
    assert_eq!(done, "Set i18n.commitEncoding = ISO-8859-15 in .git/config: new commits are written in it, existing ones keep theirs");
    let set = repo.commit_encoding_shown();
    assert_eq!((set.name(), set.scope), ("ISO-8859-15", Some(SetIn::Repository)));
    assert_eq!(change::initial_scope(&set), Local);
    assert!(describe("ISO-8859-15", Local, &set).unwrap_err().contains("already"));
    let global = describe("windows-1252", Global, &set).unwrap();
    assert!(global.contains("But i18n.commitEncoding is ISO-8859-15 set in .git/config (this repository) — that wins here"), "{global}");
    assert!(describe("", Local, &set).unwrap().starts_with("Removes i18n.commitEncoding from this repository's .git/config"));
    let removed = repo.set_commit_encoding(Local, "").unwrap();
    assert!(removed.starts_with("Removed i18n.commitEncoding from .git/config"), "{removed}");
    assert_eq!(repo.commit_encoding_shown(), CommitEncoding::utf8());
}

/// Messages read back by their header in history, details and the amend prefill, for
/// the families gitti writes (the commits are git's own, made with the setting on).
#[gitti_test::test]
fn messages_are_decoded_by_their_header() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mut fx = Fixture::new();
    fx.commit("main", "base", &[put("a.txt", "a\n")]);
    let cases: [(&str, &[u8], &str); 3] = [
        ("KOI8-R", b"\xf0\xd2\xc9\xd7\xc5\xd4", "Привет"),
        ("Shift_JIS", b"\x93\xfa\x96\x7b", "日本"),
        ("ISO-2022-JP", b"\x1b$BF|K\\\x1b(B", "日本"),
    ];
    for (i, (name, bytes, _)) in cases.iter().enumerate() {
        let file = format!(".git/m{i}");
        fx.write_bytes(&file, bytes);
        let config = format!("i18n.commitEncoding={name}");
        fx.git_after(&["-c", &config, "commit", "-q", "--allow-empty", "-F", &file]);
    }
    fx.build(dir);
    let repo = Repo::open(dir).unwrap();
    let g = gix::open(dir).unwrap();
    for (i, (name, _, text)) in cases.iter().enumerate() {
        let rev = format!("HEAD~{}", cases.len() - 1 - i);
        let id = g.rev_parse_single(rev.as_str()).unwrap().detach();
        assert_eq!(split(&raw(dir, &rev)).0.as_deref(), Some(*name));
        assert_eq!(repo.commit_details(id).unwrap().message, format!("{text}\n"), "{name}");
    }
    assert_eq!(repo.head_message().unwrap(), "日本\n");
}
