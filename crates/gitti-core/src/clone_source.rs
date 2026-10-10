//! Whether a piece of text names a repository `git clone` can take — the Clone dialogs
//! (gitti and gitty) put the clipboard's text into their URL field when it does.
//!
//! Recognised: `https` / `http` / `ssh` / `git` / `git+ssh` / `ftp(s)` / `file` URLs,
//! scp-like `[user@]host:path`, absolute local paths (to an existing repository, checked by
//! [`from_clipboard`]), and a whole `git clone … URL` command line (its URL). Not: plain
//! words, several lines, URLs with an unknown scheme, remote-helper addresses
//! (`helper::address`).
//!
//! Web pages: the page URLs of the big forges (`…/blob/main/README.md`, `…/tree/dev`, an
//! issue, `?tab=readme`, `#readme`) are cut back to the repository (GitHub, Codeberg,
//! Bitbucket, Gitea, sourcehut: owner / repository; GitLab and any other host: up to the
//! `/-/` GitLab puts before its pages). Elsewhere a URL with a query or fragment, without a
//! path, or ending in a document (`.html`, `.md`, `.pdf`, an image, an archive, …) is no
//! repository; any other http(s) URL is taken as one (self-hosted forges look like plain
//! web pages).

use std::path::Path;

use crate::create::expand_home;

/// A repository location found in a text ([`parse`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloneSource {
    /// A URL or scp-like address, as `git clone` takes it (web page URLs cut back).
    Url(String),
    /// An absolute local path (`~` not expanded yet; not checked on disk).
    Path(String),
}

/// Longer texts are no address (a pasted log, a file …).
const MAX_LEN: usize = 2048;

/// The repository location `text` names (trimmed, one line), if it names one. Pure: a
/// local path is not checked on disk ([`from_clipboard`] does that).
pub fn parse(text: &str) -> Option<CloneSource> {
    let t = text.trim();
    if t.is_empty() || t.len() > MAX_LEN || t.contains(['\n', '\r']) {
        return None;
    }
    if let Some(args) = git_clone_args(t) {
        // `git clone [options] URL [folder]`: the first argument that is a location (an
        // option's value such as a branch name is none).
        return args
            .split_whitespace()
            .filter(|a| !a.starts_with('-'))
            .find_map(|a| parse_one(unquote(a)));
    }
    parse_one(unquote(t))
}

/// The note under the URL field when the Clone dialog took the clipboard's text.
pub const CLIPBOARD_NOTE: &str = "From the clipboard — type or paste to replace it";

/// What the Clone dialog takes from the clipboard's `text`: a URL as [`parse`] finds it,
/// or a local path when it is a repository (`~` expanded — git doesn't expand it). A
/// network path (`\\server\share…`) is not looked at: an unreachable server would keep
/// the dialog from opening for a long time.
pub fn from_clipboard(text: &str) -> Option<String> {
    match parse(text)? {
        CloneSource::Url(u) => Some(u),
        CloneSource::Path(p) if p.starts_with("\\\\") || p.starts_with("//") => None,
        CloneSource::Path(p) => {
            let path = expand_home(&p);
            is_repository(&path).then(|| path.display().to_string())
        }
    }
}

/// `dir` is a repository: a work tree with `.git` (folder or file), or a git directory
/// itself (bare, or a `.git` folder): `HEAD`, `objects/` and `refs/`. No git call.
pub fn is_repository(dir: &Path) -> bool {
    dir.join(".git").exists()
        || (dir.join("HEAD").is_file() && dir.join("objects").is_dir() && dir.join("refs").is_dir())
}

/// The arguments after `git clone` (a shell prompt `$ ` in front allowed).
fn git_clone_args(t: &str) -> Option<&str> {
    let t = t.strip_prefix("$ ").unwrap_or(t).trim_start();
    let rest = t.strip_prefix("git")?;
    let rest = rest.strip_prefix(char::is_whitespace)?.trim_start();
    let rest = rest.strip_prefix("clone")?;
    if rest.is_empty() {
        return Some(rest);
    }
    rest.strip_prefix(char::is_whitespace)
}

/// `s` without one pair of enclosing quotes / angle brackets / backticks.
fn unquote(s: &str) -> &str {
    for (open, close) in [('"', '"'), ('\'', '\''), ('<', '>'), ('`', '`')] {
        if let Some(inner) = s.strip_prefix(open).and_then(|r| r.strip_suffix(close)) {
            return inner.trim();
        }
    }
    s
}

fn parse_one(s: &str) -> Option<CloneSource> {
    if s.is_empty() {
        return None;
    }
    if let Some((scheme, rest)) = s.split_once("://") {
        return url(s, scheme, rest).map(CloneSource::Url);
    }
    if is_absolute_path(s) {
        return Some(CloneSource::Path(s.to_string()));
    }
    scp_like(s).then(|| CloneSource::Url(s.to_string()))
}

/// `/…`, `~`, `~/…`, `C:\…`, `C:/…`, `\\server\share\…`.
fn is_absolute_path(s: &str) -> bool {
    let b = s.as_bytes();
    s.starts_with('/')
        || s == "~"
        || s.starts_with("~/")
        || s.starts_with("~\\")
        || s.starts_with("\\\\")
        || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/'))
}

/// git's scp-like syntax `[user@]host:path` (a `:` with no `/` before it), and enough of
/// an address not to be any word with a colon: a user, a dotted host, `localhost` or a
/// path ending in `.git`.
fn scp_like(s: &str) -> bool {
    if s.contains(char::is_whitespace) {
        return false;
    }
    // The colon after the host: behind the brackets of an IPv6 address (`git@[::1]:x`).
    let colon = match (s.find('['), s.find(']'), s.find(':')) {
        (Some(open), Some(close), Some(first)) if open < first && open < close => {
            s[close..].find(':').map(|i| close + i)
        }
        (_, _, first) => first,
    };
    let Some((before, path)) = colon.map(|i| (&s[..i], &s[i + 1..])) else {
        return false;
    };
    if before.contains('/') || path.is_empty() || path.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let (user, host) = match before.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, before),
    };
    if user.is_some_and(str::is_empty) || !valid_host(host) {
        return false;
    }
    // A single letter is a Windows drive (`C:foo`), as for git.
    if host.len() == 1 {
        return false;
    }
    user.is_some()
        || host.contains('.')
        || host.starts_with('[')
        || host.eq_ignore_ascii_case("localhost")
        || path.trim_end_matches('/').ends_with(".git")
}

/// A host name or IP address (`[…]` for IPv6).
fn valid_host(h: &str) -> bool {
    if let Some(inner) = h.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        return !inner.is_empty() && inner.bytes().all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.');
    }
    !h.is_empty()
        && !h.starts_with(['.', '-'])
        && h.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// The URL git clones for `s` = `scheme://rest`, if it is one.
fn url(s: &str, scheme: &str, rest: &str) -> Option<String> {
    if s.contains(char::is_whitespace) || rest.is_empty() {
        return None;
    }
    let scheme_l = scheme.to_ascii_lowercase();
    match scheme_l.as_str() {
        "file" => return Some(s.to_string()),
        "http" | "https" | "ssh" | "git" | "git+ssh" | "ssh+git" | "ftp" | "ftps" => {}
        _ => return None,
    }
    let (authority, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    // Without the port (an IPv6 address keeps its colons inside the brackets).
    let host = match host.rfind(':') {
        Some(i) if !host[i..].contains(']') => &host[..i],
        _ => host,
    };
    if !valid_host(host) {
        return None;
    }
    if !scheme_l.starts_with("http") {
        let has_path = path.split('/').any(|seg| !seg.is_empty());
        return has_path.then(|| s.to_string());
    }
    web_url(s, scheme, authority, &host.to_ascii_lowercase(), path)
}

/// Hosts whose repositories are `owner/name` and whose pages continue below that.
const OWNER_REPO_HOSTS: &[&str] = &[
    "github.com",
    "codeberg.org",
    "bitbucket.org",
    "gitea.com",
    "git.sr.ht",
];

/// GitHub pages with an owner-like first segment that are no owner.
const GITHUB_PAGES: &[&str] = &[
    "about", "account", "apps", "codespaces", "collections", "contact", "customer-stories",
    "dashboard", "enterprise", "events", "explore", "features", "issues", "join", "login",
    "logout", "marketplace", "new", "notifications", "organizations", "orgs", "pricing",
    "pulls", "readme", "search", "security", "sessions", "settings", "signup", "site",
    "sponsors", "team", "topics", "trending", "users",
];

/// Hosts that serve files, never repositories.
const FILE_HOSTS: &[&str] = &["raw.githubusercontent.com", "gist.githubusercontent.com"];

/// The last path segment ends like a document, not a repository.
const DOCUMENT_ENDINGS: &[&str] = &[
    "html", "htm", "xhtml", "php", "asp", "aspx", "jsp", "md", "markdown", "txt", "rst",
    "pdf", "png", "jpg", "jpeg", "gif", "svg", "webp", "ico", "zip", "tar", "gz", "tgz",
    "bz2", "xz", "7z", "rar", "exe", "dmg", "msi", "deb", "rpm", "json", "xml", "yaml",
    "yml", "csv", "mp3", "mp4", "mov",
];

/// An http(s) URL: a forge's page URL cut back to its repository; other pages refused.
fn web_url(s: &str, scheme: &str, authority: &str, host: &str, path: &str) -> Option<String> {
    let host = host.strip_prefix("www.").unwrap_or(host);
    if FILE_HOSTS.contains(&host) {
        return None;
    }
    let cut_at = path.find(['?', '#']);
    let bare_path = cut_at.map_or(path, |i| &path[..i]);
    let mut segs: Vec<&str> = bare_path.split('/').filter(|seg| !seg.is_empty()).collect();
    // GitLab (gitlab.com or self-hosted): its pages follow a `-` segment.
    let gitlab_cut = segs.iter().position(|seg| *seg == "-");
    if let Some(i) = gitlab_cut {
        segs.truncate(i);
    }
    let known = OWNER_REPO_HOSTS.contains(&host);
    let rebuilt = |segs: &[&str]| format!("{scheme}://{authority}/{}", segs.join("/"));
    if known {
        if segs.len() < 2 || (host == "github.com" && GITHUB_PAGES.contains(&segs[0])) {
            return None;
        }
        return Some(rebuilt(&segs[..2]));
    }
    if host == "gitlab.com" || host.starts_with("gitlab.") {
        // Groups nest (`group/sub/project`): only the `-` and the query are cut.
        return (segs.len() >= 2).then(|| rebuilt(&segs));
    }
    if segs.is_empty() {
        return None;
    }
    if gitlab_cut.is_some() {
        return Some(rebuilt(&segs));
    }
    if cut_at.is_some() {
        return None;
    }
    let last = segs[segs.len() - 1];
    let document = last
        .rsplit_once('.')
        .is_some_and(|(_, ext)| DOCUMENT_ENDINGS.contains(&ext.to_ascii_lowercase().as_str()));
    (!document).then(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Option<CloneSource> {
        Some(CloneSource::Url(s.to_string()))
    }

    fn path(s: &str) -> Option<CloneSource> {
        Some(CloneSource::Path(s.to_string()))
    }

    #[gitti_test::test]
    fn repository_urls_are_taken_as_they_are() {
        for s in [
            "https://github.com/user/repo.git",
            "https://github.com/user/repo",
            "http://git.example.com/team/project",
            "https://git.example.com:8443/scm/team/project.git",
            "https://user:token@git.example.com/team/project.git",
            "https://dev.azure.com/org/project/_git/repo",
            "https://git.kernel.org/pub/scm/git/git.git",
            "https://gitlab.example.org/group/sub/project",
            "https://[::1]:3000/alice/lfs-test.git",
            "ssh://git@host.example.com:22/~user/repo.git",
            "ssh://host/srv/repo",
            "git://git.example.com/project.git",
            "git+ssh://git@host.example.com/repo.git",
            "ftps://ftp.example.com/pub/repo.git",
            "file:///srv/git/repo.git",
            "FILE:///srv/git/repo.git",
            "HTTPS://GitHub.com/user/repo",
            "git@github.com:user/repo.git",
            "git@gitlab.com:group/sub/project.git",
            "alice@192.168.1.5:repos/x",
            "host.example.com:repo",
            "localhost:repo",
            "myalias:projects/thing.git",
            "git@[::1]:repo.git",
        ] {
            let want = if s.starts_with("HTTPS") {
                url("HTTPS://GitHub.com/user/repo")
            } else {
                url(s)
            };
            assert_eq!(parse(s), want, "{s}");
        }
    }

    #[gitti_test::test]
    fn surroundings_are_trimmed() {
        let want = url("git@github.com:user/repo.git");
        for s in [
            "  git@github.com:user/repo.git\n",
            "\t\"git@github.com:user/repo.git\"",
            "'git@github.com:user/repo.git'",
            "<git@github.com:user/repo.git>",
            "`git@github.com:user/repo.git`",
            "git clone git@github.com:user/repo.git",
            "$ git clone git@github.com:user/repo.git",
            "git clone --recurse-submodules -b main git@github.com:user/repo.git my-folder",
            "git  clone\t'git@github.com:user/repo.git'",
        ] {
            assert_eq!(parse(s), want, "{s:?}");
        }
        // The command line's options and folder name are not taken over.
        assert_eq!(
            parse("git clone --depth 1 https://github.com/a/b.git target"),
            url("https://github.com/a/b.git")
        );
        assert_eq!(parse("git clone /srv/repo"), path("/srv/repo"));
        assert_eq!(parse("git clone"), None);
        assert_eq!(parse("git clone --bare main"), None);
        assert_eq!(parse("git cloned https://github.com/a/b"), None);
    }

    #[gitti_test::test]
    fn forge_pages_are_cut_back_to_the_repository() {
        for (page, repo) in [
            ("https://github.com/user/repo/", "https://github.com/user/repo"),
            ("https://github.com/user/repo/tree/main/src", "https://github.com/user/repo"),
            ("https://github.com/user/repo/blob/main/README.md", "https://github.com/user/repo"),
            ("https://github.com/user/repo?tab=readme-ov-file", "https://github.com/user/repo"),
            ("https://github.com/user/repo#readme", "https://github.com/user/repo"),
            ("https://github.com/user/repo/issues/12", "https://github.com/user/repo"),
            ("https://www.github.com/user/repo/pulls", "https://www.github.com/user/repo"),
            ("https://codeberg.org/owner/repo/src/branch/main", "https://codeberg.org/owner/repo"),
            ("https://bitbucket.org/ws/repo/src/main/", "https://bitbucket.org/ws/repo"),
            ("https://git.sr.ht/~user/repo/tree", "https://git.sr.ht/~user/repo"),
            (
                "https://gitlab.com/group/sub/project/-/tree/main?ref_type=heads",
                "https://gitlab.com/group/sub/project",
            ),
            ("https://gitlab.com/group/project#top", "https://gitlab.com/group/project"),
            (
                "https://git.example.com/team/project/-/merge_requests/3",
                "https://git.example.com/team/project",
            ),
        ] {
            assert_eq!(parse(page), url(repo), "{page}");
        }
    }

    #[gitti_test::test]
    fn web_pages_and_text_are_refused() {
        for s in [
            "",
            "   ",
            "hello",
            "two words",
            "repo",
            "main",
            "Note: see the docs",
            "Note:see",
            "TODO:fix",
            "12:30",
            "10.0.0.1:22",
            "C:foo",
            "alice@example.com",
            "mailto:alice@example.com",
            "https://github.com/user/repo\nhttps://github.com/user/other",
            "line one\r\nline two",
            "https://github.com",
            "https://github.com/",
            "https://github.com/user",
            "https://github.com/settings/keys",
            "https://github.com/orgs/rust-lang/repositories",
            "https://github.com/notifications",
            "https://gitlab.com/group",
            "https://www.rust-lang.org/",
            "https://example.com/search?q=git",
            "https://example.com/page#section",
            "https://example.com/docs/index.html",
            "https://example.com/README.md",
            "https://example.com/files/release.tar.gz",
            "https://example.com/logo.PNG",
            "https://raw.githubusercontent.com/user/repo/main/install",
            "https://exa mple.com/repo",
            "https://",
            "ssh://host",
            "ssh://host/",
            "ftp:/x",
            "mailto://alice@example.com",
            "javascript://alert(1)",
            "transport::address",
            "ext::sh -c touch% /tmp/pwned",
            "./relative/repo",
            "relative/repo",
            "../repo",
            "~user/repo",
        ] {
            assert_eq!(parse(s), None, "{s:?}");
        }
        assert_eq!(parse(&format!("https://example.com/{}", "a".repeat(3000))), None);
    }

    #[gitti_test::test]
    fn local_paths_are_absolute() {
        for s in [
            "/srv/git/repo.git",
            "/Volumes/External Drive/repo",
            "~",
            "~/src/gitti",
            r"C:\src\gitti",
            "C:/src/gitti",
            r"\\server\share\repo",
        ] {
            assert_eq!(parse(s), path(s), "{s}");
        }
        assert_eq!(parse("\"/path with/spaces\"\n"), path("/path with/spaces"));
    }

    #[gitti_test::test]
    fn clipboard_paths_must_be_repositories() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir_all(work.join(".git")).unwrap();
        let bare = dir.path().join("bare.git");
        for d in ["objects", "refs"] {
            std::fs::create_dir_all(bare.join(d)).unwrap();
        }
        std::fs::write(bare.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let linked = dir.path().join("linked");
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::write(linked.join(".git"), "gitdir: /elsewhere\n").unwrap();
        let plain = dir.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        for repo in [&work, &bare, &linked, &work.join(".git").join("..")] {
            let s = repo.display().to_string();
            assert_eq!(from_clipboard(&s), Some(s.clone()), "{s}");
        }
        assert_eq!(from_clipboard(&plain.display().to_string()), None);
        assert_eq!(from_clipboard(&dir.path().join("missing").display().to_string()), None);
        assert_eq!(from_clipboard("hello"), None);
        assert_eq!(from_clipboard(r"\\server\share\repo"), None, "network paths aren't looked at");
        assert_eq!(
            from_clipboard("https://github.com/user/repo/tree/main"),
            Some("https://github.com/user/repo".to_string())
        );
        // `~` is expanded: git doesn't.
        let home = dirs::home_dir().unwrap();
        if is_repository(&home) {
            assert_eq!(from_clipboard("~"), Some(home.display().to_string()));
        } else {
            assert_eq!(from_clipboard("~"), None);
        }
    }
}
