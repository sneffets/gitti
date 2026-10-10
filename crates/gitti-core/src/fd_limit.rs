//! The limit of open files (file descriptors) of the process.
//!
//! On macOS a program started from Finder / the Dock gets a soft limit of 256. gitti runs
//! many git processes at once (every refresh of every tab: three pipes each) and keeps
//! pack files of every open repository open, so a burst — a branch deleted in a repository
//! whose worktrees are open as tabs refreshes all of them together — could hit it: "failed
//! to run git: Too many open files". [`raise`] lifts the soft limit at start-up; the git
//! processes inherit it.

/// What the soft limit is raised to at most (macOS refuses more than `OPEN_MAX` = 10240
/// per process; far above what a burst of refreshes needs).
pub const TARGET: u64 = 10240;

/// Raises the soft limit of open files to the hard limit, at most [`TARGET`]; never lowers
/// it. Returns the soft limit in effect afterwards (`None` where there is no such limit, or
/// it can't be read).
pub fn raise() -> Option<u64> {
    imp::raise()
}

/// The soft limit of open files now (`None` where there is no such limit).
pub fn current() -> Option<u64> {
    imp::current()
}

/// Whether `e` says the process (or the system) has too many files open.
pub fn is_too_many_open_files(e: &std::io::Error) -> bool {
    imp::is_too_many(e)
}

#[cfg(unix)]
mod imp {
    fn get() -> Option<libc::rlimit> {
        let mut lim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: `getrlimit` only writes the struct it is given.
        (unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } == 0).then_some(lim)
    }

    pub fn current() -> Option<u64> {
        get().map(|l| l.rlim_cur)
    }

    pub fn raise() -> Option<u64> {
        let mut lim = get()?;
        let want = super::TARGET.min(lim.rlim_max);
        if lim.rlim_cur >= want {
            return Some(lim.rlim_cur);
        }
        lim.rlim_cur = want;
        // SAFETY: `setrlimit` only reads the struct it is given.
        unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim) };
        current()
    }

    pub fn is_too_many(e: &std::io::Error) -> bool {
        matches!(e.raw_os_error(), Some(libc::EMFILE | libc::ENFILE))
    }
}

#[cfg(not(unix))]
mod imp {
    pub fn current() -> Option<u64> {
        None
    }

    pub fn raise() -> Option<u64> {
        None
    }

    pub fn is_too_many(e: &std::io::Error) -> bool {
        // ERROR_TOO_MANY_OPEN_FILES
        e.raw_os_error() == Some(4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[gitti_test::test]
    fn raise_lifts_the_soft_limit_to_the_hard_one_at_most_the_target() {
        let before = current().expect("an open-file limit");
        let after = raise().expect("an open-file limit");
        let mut lim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: `getrlimit` only writes the struct it is given.
        assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) }, 0);
        let want = TARGET.min(lim.rlim_max);
        assert_eq!(after, before.max(want), "before {before}, hard {}", lim.rlim_max);
        assert_eq!(current(), Some(after));
        // Again: nothing changes (never lowered).
        assert_eq!(raise(), Some(after));
    }

    #[gitti_test::test]
    fn too_many_open_files_is_recognised() {
        #[cfg(unix)]
        {
            assert!(is_too_many_open_files(&std::io::Error::from_raw_os_error(libc::EMFILE)));
            assert!(is_too_many_open_files(&std::io::Error::from_raw_os_error(libc::ENFILE)));
            assert!(!is_too_many_open_files(&std::io::Error::from_raw_os_error(libc::ENOENT)));
        }
        let plain = std::io::Error::other("x");
        assert!(!is_too_many_open_files(&plain));
    }
}
