//! Command-line flags that print instead of opening a window, as gitty's:
//! `gitti --version` (`about::version_line`), `gitti --licenses` (the third-party list of
//! this binary on this platform), `gitti --help`. On Windows gitti is a GUI program without
//! a console of its own: the text goes to the console it was started from (a redirect to a
//! file or pipe works as well); started from Explorer there is none, and nothing shows.

const USAGE: &str = "\
gitti — git client with hunk and line staging

usage: gitti [PATH]    open the repository containing PATH (default: current directory)
       gitti --version
       gitti --licenses print the third-party licences of this binary

Inside, every command is in the menus and in the command palette (Cmd-P on macOS,
Ctrl-P elsewhere).";

/// The text a flag prints (`None`: no such flag — the argument is a path to open).
pub(crate) fn flag_output(arg: Option<&str>) -> Option<String> {
    match arg? {
        "-h" | "--help" => Some(format!("{USAGE}\n")),
        "-V" | "--version" => {
            let (version, build) = (env!("CARGO_PKG_VERSION"), env!("GITTI_BUILD"));
            Some(format!("{}\n", gitti_core::about::version_line("gitti", version, build)))
        }
        "--licenses" | "--licences" => Some(gitti_core::third_party::text("gitti")),
        _ => None,
    }
}

/// Prints what the first argument asks for and returns `true` (the caller exits), or
/// `false` when it isn't one of the flags.
pub(crate) fn run_flag() -> bool {
    let Some(text) = flag_output(std::env::args().nth(1).as_deref()) else {
        return false;
    };
    attach_parent_console();
    // Into a pager / `head` that quits early: no broken-pipe panic.
    use std::io::Write as _;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
    true
}

/// Windows: a GUI-subsystem program has no console; the one of the shell that started it,
/// if any, takes the output (a redirected stdout keeps its file or pipe).
#[cfg(windows)]
fn attach_parent_console() {
    use ::windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // Fails without a parent console (started from Explorer): then nothing shows.
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

#[cfg(not(windows))]
fn attach_parent_console() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn flags_print_what_gitty_prints() {
        let version = flag_output(Some("--version")).unwrap();
        assert!(version.starts_with(&format!("gitti {}", env!("CARGO_PKG_VERSION"))), "{version}");
        assert_eq!(flag_output(Some("-V")), Some(version));
        let licences = flag_output(Some("--licenses")).unwrap();
        assert_eq!(licences, gitti_core::third_party::text("gitti"));
        assert!(licences.starts_with("Third-party licences of gitti"), "{}", &licences[..80]);
        assert!(flag_output(Some("--help")).unwrap().contains("gitti --licenses"));
        assert_eq!(flag_output(Some("/some/repo")), None);
        assert_eq!(flag_output(None), None);
    }
}
