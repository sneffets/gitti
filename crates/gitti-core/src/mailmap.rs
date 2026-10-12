//! `.mailmap` as `git log` applies it by default (`log.mailmap`, on unless set to false):
//! the commit list, the commit header and the author search show the mapped names and
//! e-mails; the header keeps the identity the commit records for its tooltip, and the
//! search matches both.
//!
//! Sources as git reads them (gix's `open_mailmap`): `.mailmap` in the working tree (not
//! followed when it is a symlink), `mailmap.blob` (default `HEAD:.mailmap` in a bare
//! repository only) and `mailmap.file`. Read in-process once per refresh
//! ([`crate::Repo::reload_mailmap`]); a change bumps a generation that empties the
//! metadata cache of the history and the commit cache.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// A parsed mailmap (empty: nothing is mapped).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mailmap(Arc<gix::mailmap::Snapshot>);

/// The identity a commit records and what the mailmap makes of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapped {
    pub name: String,
    pub email: String,
    /// `(name, email)` as the commit records them, when the mailmap changed them.
    pub raw: Option<(String, String)>,
}

impl Mailmap {
    pub fn is_empty(&self) -> bool {
        self.0.iter().next().is_none()
    }

    /// `name <email>` as `git log` shows it (`%aN <%aE>`).
    pub fn map(&self, name: &str, email: &str) -> Mapped {
        let unchanged = || Mapped {
            name: name.to_string(),
            email: email.to_string(),
            raw: None,
        };
        if self.is_empty() {
            return unchanged();
        }
        let sig = gix::actor::SignatureRef {
            name: name.into(),
            email: email.into(),
            time: "",
        };
        let Some(r) = self.0.try_resolve_ref(sig) else {
            return unchanged();
        };
        let to_s = |b: &gix::bstr::BStr| String::from_utf8_lossy(b).into_owned();
        let new_name = r.name.map(to_s).unwrap_or_else(|| name.to_string());
        let new_email = r.email.map(to_s).unwrap_or_else(|| email.to_string());
        let raw = (new_name != name || new_email != email).then(|| (name.to_string(), email.to_string()));
        Mapped {
            name: new_name,
            email: new_email,
            raw,
        }
    }
}

/// The tooltip of an identity the mailmap changed: what the commit records.
pub fn recorded_tip(raw: Option<&(String, String)>) -> Option<String> {
    let (name, email) = raw?;
    Some(format!("Recorded in the commit as {name} <{email}> — shown as .mailmap maps it"))
}

/// The mailmap `git log` would apply in `repo` (empty with `log.mailmap=false`).
pub(crate) fn read(repo: &gix::Repository) -> Mailmap {
    let on = repo.config_snapshot().boolean("log.mailmap").unwrap_or(true);
    if !on {
        return Mailmap::default();
    }
    Mailmap(Arc::new(repo.open_mailmap()))
}

/// A repository's current mailmap, shared with its histories' metadata stores.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    /// 0: not read yet; bumped on every change.
    generation: AtomicU64,
    current: Mutex<Mailmap>,
}

impl Shared {
    /// `(generation, mailmap)`; 0 while never read.
    pub(crate) fn get(&self) -> (u64, Mailmap) {
        let current = self.current.lock().map(|m| m.clone()).unwrap_or_default();
        (self.generation.load(Ordering::Acquire), current)
    }

    /// Stores `new`; whether it differs from what was there (a first read counts when it
    /// maps anything).
    pub(crate) fn set(&self, new: Mailmap) -> bool {
        let Ok(mut current) = self.current.lock() else {
            return false;
        };
        let first = self.generation.load(Ordering::Acquire) == 0;
        let changed = *current != new;
        if changed || first {
            *current = new;
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
        changed
    }
}

impl crate::Repo {
    /// The mailmap of the last refresh (read now when none was yet).
    pub fn mailmap(&self) -> Mailmap {
        let (generation, current) = self.mailmap_shared().get();
        if generation == 0 {
            self.reload_mailmap();
            return self.mailmap_shared().get().1;
        }
        current
    }

    /// Reads the mailmap again (in-process: a small file or blob and the config);
    /// `true` when it changed — the commit cache is emptied then (its headers carry the
    /// old names) and the history's metadata re-read as rows are drawn.
    pub fn reload_mailmap(&self) -> bool {
        let Ok(repo) = self.gix() else {
            return false;
        };
        self.set_mailmap(read(&repo))
    }

    /// Stores `mailmap` as the current one ([`Self::reload_mailmap`]).
    pub(crate) fn set_mailmap(&self, mailmap: Mailmap) -> bool {
        let changed = self.mailmap_shared().set(mailmap);
        if changed {
            self.clear_commit_cache();
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mm(text: &str) -> Mailmap {
        Mailmap(Arc::new(gix::mailmap::Snapshot::from_bytes(text.as_bytes())))
    }

    #[gitti_test::test]
    fn maps_name_and_email_like_git() {
        let m = mm("Proper Name <proper@example.com> <old@example.com>\nOnly Name <x@example.com>\n");
        let a = m.map("old name", "OLD@example.com");
        assert_eq!((a.name.as_str(), a.email.as_str()), ("Proper Name", "proper@example.com"));
        assert_eq!(a.raw, Some(("old name".into(), "OLD@example.com".into())));
        let b = m.map("x", "x@example.com");
        assert_eq!((b.name.as_str(), b.email.as_str()), ("Only Name", "x@example.com"));
        let c = m.map("Nobody", "n@example.com");
        assert_eq!(c.raw, None);
        assert!(Mailmap::default().map("a", "b").raw.is_none());
    }

    #[gitti_test::test]
    fn shared_bumps_only_on_change() {
        let s = Shared::default();
        assert_eq!(s.get().0, 0);
        assert!(!s.set(Mailmap::default()), "first read of an empty one: no change");
        assert_eq!(s.get().0, 1);
        assert!(s.set(mm("A <a@x> <b@x>\n")));
        assert!(!s.set(mm("A <a@x> <b@x>\n")));
        assert_eq!(s.get().0, 2);
    }
}
