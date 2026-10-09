//! The error mark of both apps' top bar: a failed git operation leaves "⚠ Pull
//! failed" there until the user opens its details, the same operation succeeds, or the
//! next git operation the user starts (any one — background refreshes don't count);
//! a later info message in the status bar no longer hides that something went wrong. The
//! details stay reachable after the mark went (View ▸ Last Error Details).
//!
//! Operations are told apart by the first word of their label ("Push main to origin" and
//! "Push" are the same operation), so any successful push clears a failed one.

/// One failed operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// The operation's label as the app named it ("Pull", "Push main to origin").
    pub label: String,
    /// git's error output.
    pub details: String,
}

/// The failures not yet looked at, oldest first (one per operation).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ErrorMarks {
    failures: Vec<Failure>,
    /// The details shown last or dismissed last (title, text), for Last Error Details.
    last: Option<(String, String)>,
}

/// The operation a label stands for (its first word, lower case).
pub fn op_key(label: &str) -> String {
    label
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches(['…', '.', ':'])
        .to_lowercase()
}

/// Whether an operation the user starts while another one runs waits for it (queued)
/// instead of being refused ("Busy"): the remote ones, which take seconds and are what
/// one clicks while a pull or push still runs — refused, the earlier warning seemed to
/// survive the fetch. Others (commit, merge, …) are refused: re-running them
/// later with what they captured now could repeat them.
pub fn waits_while_busy(label: &str) -> bool {
    matches!(op_key(label).as_str(), "fetch" | "pull" | "push" | "force")
}

impl ErrorMarks {
    /// Records a failure of `label` (replaces an older one of the same operation).
    pub fn failed(&mut self, label: &str, details: impl Into<String>) {
        let key = op_key(label);
        self.failures.retain(|f| op_key(&f.label) != key);
        self.failures.push(Failure {
            label: label.to_string(),
            details: details.into(),
        });
    }

    /// `label` succeeded: its operation's failure is over.
    pub fn succeeded(&mut self, label: &str) {
        if self.failures.is_empty() {
            return;
        }
        let key = op_key(label);
        self.failures.retain(|f| op_key(&f.label) != key);
    }

    pub fn is_empty(&self) -> bool {
        self.failures.is_empty()
    }

    pub fn failures(&self) -> &[Failure] {
        &self.failures
    }

    /// The mark's text: "⚠ Pull failed", with "+N" when more operations failed.
    pub fn text(&self) -> Option<String> {
        let last = self.failures.last()?;
        let more = self.failures.len() - 1;
        Some(if more > 0 {
            format!("⚠ {} failed +{more}", last.label)
        } else {
            format!("⚠ {} failed", last.label)
        })
    }

    /// The details of all failures (newest first) for the details dialog; clears the mark
    /// (the details stay for [`Self::last_details`]).
    pub fn take_details(&mut self) -> Option<(String, String)> {
        let details = self.details()?;
        self.failures.clear();
        self.last = Some(details.clone());
        Some(details)
    }

    /// The user starts a git operation: the mark goes. Its details — else
    /// `status_error`, the error the status bar showed — stay for [`Self::last_details`].
    pub fn dismiss(&mut self, status_error: Option<&str>) {
        if let Some(details) = self.details() {
            self.last = Some(details);
            self.failures.clear();
        } else if let Some(text) = status_error {
            self.last = Some(("Error".to_string(), text.to_string()));
        }
    }

    /// The details shown or dismissed last (after the mark went).
    pub fn last_details(&self) -> Option<(String, String)> {
        self.last.clone()
    }

    fn details(&self) -> Option<(String, String)> {
        let title = match self.failures.len() {
            0 => return None,
            1 => format!("{} failed", self.failures[0].label),
            n => format!("{n} operations failed"),
        };
        let body = self
            .failures
            .iter()
            .rev()
            .map(|f| {
                if self.failures.len() == 1 {
                    f.details.clone()
                } else {
                    format!("── {} ──\n{}", f.label, f.details)
                }
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        Some((title, body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn stays_until_the_same_op_succeeds_or_is_looked_at() {
        let mut m = ErrorMarks::default();
        assert_eq!(m.text(), None);
        m.failed("Pull", "fatal: no upstream");
        assert_eq!(m.text().as_deref(), Some("⚠ Pull failed"));
        // Other operations succeeding don't clear it.
        m.succeeded("Fetch");
        m.succeeded("Stage");
        assert_eq!(m.text().as_deref(), Some("⚠ Pull failed"));
        m.failed("Push main to origin", "rejected");
        assert_eq!(m.text().as_deref(), Some("⚠ Push main to origin failed +1"));
        // A plain "Push" is the same operation.
        m.succeeded("Push");
        assert_eq!(m.text().as_deref(), Some("⚠ Pull failed"));
        m.failed("Pull", "again");
        assert_eq!(m.failures().len(), 1, "one entry per operation");
        let (title, body) = m.take_details().unwrap();
        assert_eq!((title.as_str(), body.as_str()), ("Pull failed", "again"));
        assert!(m.is_empty());
        assert_eq!(m.take_details(), None);
        assert_eq!(m.last_details(), Some((title, body)), "still reachable");
    }

    #[gitti_test::test]
    fn the_next_user_operation_dismisses_the_mark_but_keeps_the_details() {
        let mut m = ErrorMarks::default();
        m.failed("Merge", "CONFLICT");
        m.dismiss(Some("an older status error"));
        assert_eq!(m.text(), None);
        assert!(m.is_empty());
        let last = Some(("Merge failed".to_string(), "CONFLICT".to_string()));
        assert_eq!(m.last_details(), last);
        // Nothing failed since: dismissing again keeps them.
        m.dismiss(None);
        assert_eq!(m.last_details(), last);
        // Only a status bar error (no failed operation): that one is kept.
        m.dismiss(Some("Busy (Pull) — try again"));
        assert_eq!(
            m.last_details(),
            Some(("Error".to_string(), "Busy (Pull) — try again".to_string()))
        );
    }

    #[gitti_test::test]
    fn remote_ops_wait() {
        for l in ["Fetch", "Pull", "Push main to origin", "Force push main"] {
            assert!(waits_while_busy(l), "{l}");
        }
        for l in ["Commit", "Merge", "Staging", "Prune"] {
            assert!(!waits_while_busy(l), "{l}");
        }
    }

    #[gitti_test::test]
    fn details_of_several() {
        let mut m = ErrorMarks::default();
        m.failed("Fetch", "a");
        m.failed("Refresh", "b");
        let (title, body) = m.take_details().unwrap();
        assert_eq!(title, "2 operations failed");
        assert!(body.starts_with("── Refresh ──\nb"), "{body}");
        assert_eq!(op_key("Fetch…"), "fetch");
    }
}
