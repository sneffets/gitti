//! `#[gitti_test::test]` — `#[test]` with the suite's time limit: every test at most 2 s.
//!
//! A test that takes longer than [`LIMIT_MS`] fails (when it is done); one still running
//! after [`KILL_MS`] ends the whole test process with its name — libtest itself has no
//! per-test timeout. The gitti UI tests (own harness, `tests/ui/common.rs`) apply the same
//! limits per scenario. Every test of the workspace uses this attribute instead of a plain
//! `#[test]` (`tests/core/main.rs::every_test_has_the_time_limit` checks that).
//! `GITTI_TEST_TIMES=1` prints every test's time to stderr (to find the slow ones).
//!
//! Hand-rolled on `proc_macro` (no syn): the attribute keeps the function's tokens and only
//! puts a guard in front of its body.

use proc_macro::{Delimiter, Group, Span, TokenStream, TokenTree};

/// A test slower than this fails.
const LIMIT_MS: u64 = 2_000;
/// A test still running after this is killed (with the whole test process).
const KILL_MS: u64 = 10_000;

#[proc_macro_attribute]
pub fn test(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return error("`#[gitti_test::test]` takes no arguments", Span::call_site());
    }
    let mut tokens: Vec<TokenTree> = item.into_iter().collect();
    let name = tokens
        .iter()
        .zip(tokens.iter().skip(1))
        .find_map(|(a, b)| match (a, b) {
            (TokenTree::Ident(f), TokenTree::Ident(n)) if f.to_string() == "fn" => Some(n.to_string()),
            _ => None,
        });
    let (Some(name), Some(TokenTree::Group(body))) = (name, tokens.last().cloned()) else {
        return error("`#[gitti_test::test]` goes on a function", Span::call_site());
    };
    if body.delimiter() != Delimiter::Brace {
        return error("`#[gitti_test::test]` goes on a function with a body", body.span());
    }
    let guard: TokenStream = guard_code(&name).parse().expect("guard code parses");
    let mut inner = guard;
    inner.extend(body.stream());
    let mut new_body = Group::new(Delimiter::Brace, inner);
    new_body.set_span(body.span());
    *tokens.last_mut().unwrap() = TokenTree::Group(new_body);
    let mut out: TokenStream = "#[::core::prelude::v1::test]".parse().unwrap();
    out.extend(tokens);
    out
}

/// The statement put in front of the test's body: a guard that fails the test in its
/// `drop` when it took too long, and a watchdog thread that kills the process when the
/// test hangs (it wakes up when the guard drops its sender).
fn guard_code(name: &str) -> String {
    format!(
        r#"let __gitti_test_limit = {{
            const NAME: &str = ::core::concat!(::core::module_path!(), "::", "{name}");
            struct Limit(::std::time::Instant, ::std::option::Option<::std::sync::mpsc::Sender<()>>);
            impl ::core::ops::Drop for Limit {{
                fn drop(&mut self) {{
                    self.1.take();
                    let took = self.0.elapsed();
                    if ::std::env::var_os("GITTI_TEST_TIMES").is_some() {{
                        let _ = ::std::io::Write::write_all(
                            &mut ::std::io::stderr(),
                            ::std::format!("{{:>6.2}} s  {{NAME}}\n", took.as_secs_f64()).as_bytes(),
                        );
                    }}
                    if took > ::std::time::Duration::from_millis({LIMIT_MS}) && !::std::thread::panicking() {{
                        ::core::panic!(
                            "{{NAME}} took {{:.1}} s, over the {limit} s limit of every test — make it cheaper",
                            took.as_secs_f64()
                        );
                    }}
                }}
            }}
            let (tx, rx) = ::std::sync::mpsc::channel::<()>();
            ::std::thread::spawn(move || {{
                if let ::core::result::Result::Err(::std::sync::mpsc::RecvTimeoutError::Timeout) =
                    rx.recv_timeout(::std::time::Duration::from_millis({KILL_MS}))
                {{
                    // Past libtest's output capture (it also takes this thread's prints).
                    let _ = ::std::io::Write::write_all(
                        &mut ::std::io::stderr(),
                        ::std::format!("\ntest {{NAME}}: still running after {kill} s — killed (the whole test process)\n").as_bytes(),
                    );
                    ::std::process::exit(101);
                }}
            }});
            Limit(::std::time::Instant::now(), ::core::option::Option::Some(tx))
        }};"#,
        limit = LIMIT_MS / 1000,
        kill = KILL_MS / 1000,
    )
}

fn error(msg: &str, span: Span) -> TokenStream {
    let mut ts: TokenStream = format!("::core::compile_error!({msg:?});").parse().unwrap();
    ts = ts
        .into_iter()
        .map(|mut t| {
            t.set_span(span);
            t
        })
        .collect();
    ts
}
