//! Web links in git's output — "Create a pull request for 'x' on GitHub by visiting:
//! https://…" after a push, a remote's notices — for the git log panels: gitti opens
//! them, gitty offers to open or copy them.

/// The http(s) URLs in `text`, in order, each once. Punctuation that ends the sentence
/// around a URL (`.`, `,`, `:`, quotes, a `)` without its `(` in the URL, …) is not part
/// of it. Other schemes are ignored: output from a remote must not start other handlers.
pub fn find_links(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = text;
    let mut offset = 0;
    while let Some(i) = rest.find("http") {
        let start = offset + i;
        let tail = &text[start..];
        let scheme_ok = tail.starts_with("https://") || tail.starts_with("http://");
        // A word of its own: "xhttp://" is not a link.
        let boundary = text[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let len = tail
            .find(|c: char| c.is_whitespace() || c.is_control() || "<>\"'`".contains(c))
            .unwrap_or(tail.len());
        if scheme_ok && boundary {
            let url = trim_end(&tail[..len]);
            let host = url.split("://").nth(1).unwrap_or("");
            if !host.is_empty() && !out.iter().any(|u| u == url) {
                out.push(url.to_string());
            }
        }
        let step = i + len.max(4);
        offset += step;
        rest = &text[offset..];
    }
    out
}

/// `url` without the sentence's punctuation after it.
fn trim_end(mut url: &str) -> &str {
    loop {
        let Some(last) = url.chars().last() else {
            return url;
        };
        let strip = match last {
            '.' | ',' | ';' | ':' | '!' | '?' => true,
            ')' => url.matches('(').count() < url.matches(')').count(),
            ']' => url.matches('[').count() < url.matches(']').count(),
            '}' => url.matches('{').count() < url.matches('}').count(),
            _ => false,
        };
        if !strip {
            return url;
        }
        url = &url[..url.len() - last.len_utf8()];
    }
}

/// Whether `url` is a web link [`find_links`] would give (http / https with a host).
pub fn is_web_link(url: &str) -> bool {
    find_links(url).first().is_some_and(|u| u == url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn push_output_links() {
        let github = "remote: \nremote: Create a pull request for 'feature' on GitHub by visiting:\n\
                      remote:      https://github.com/o/r/pull/new/feature\nremote: \n";
        assert_eq!(find_links(github), ["https://github.com/o/r/pull/new/feature"]);
        let gitlab = "remote: To create a merge request for x, visit:\nremote:   \
                      https://gitlab.com/o/r/-/merge_requests/new?merge_request%5Bsource_branch%5D=x\n";
        assert_eq!(
            find_links(gitlab),
            ["https://gitlab.com/o/r/-/merge_requests/new?merge_request%5Bsource_branch%5D=x"]
        );
    }

    #[gitti_test::test]
    fn punctuation_schemes_and_duplicates() {
        let text = "See (https://example.com/a_(b)). Or http://h.io/x, 'https://q.org/y'. \
                    https://example.com/a_(b) again; ftp://no file:///no xhttps://no https:// \
                    javascript:alert(1)";
        assert_eq!(
            find_links(text),
            ["https://example.com/a_(b)", "http://h.io/x", "https://q.org/y"]
        );
        assert!(is_web_link("https://github.com/o/r"));
        assert!(!is_web_link("file:///etc/passwd") && !is_web_link("https://x.org/a."));
    }
}
