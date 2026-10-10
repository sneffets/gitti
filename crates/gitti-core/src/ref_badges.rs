//! Which refs a commit's badges show and in what order, shared by gitti and gitty: the
//! commit header shows every ref (wrapping onto more lines), a commit-list row at most
//! [`LIST_MAX`] — and never hides a tag behind its "+N" while there is one.

use crate::{RefInfo, RefKind};

/// At most this many ref badges in a commit-list row; the others are counted in a "+N"
/// (gitti: their names in its tooltip) and shown in the commit header.
pub const LIST_MAX: usize = 3;

/// Whether `r` is the checked-out branch `current`.
pub fn is_current(r: &RefInfo, current: Option<&str>) -> bool {
    r.kind == RefKind::LocalBranch && current == Some(r.short_name.as_str())
}

/// Badge order: the current branch, then local branches, remote branches, tags, stashes —
/// by name within a kind (the order [`crate::Repo::refs`] keeps is not relied on).
pub fn ordered<'a>(
    refs: impl IntoIterator<Item = &'a RefInfo>,
    current: Option<&str>,
) -> Vec<&'a RefInfo> {
    let mut v: Vec<&RefInfo> = refs.into_iter().collect();
    v.sort_by(|a, b| {
        (!is_current(a, current), a.kind, &a.short_name).cmp(&(
            !is_current(b, current),
            b.kind,
            &b.short_name,
        ))
    });
    v
}

/// A commit-list row's badges: what is drawn (in badge order) and what only the "+N"
/// counts.
#[derive(Debug, Default)]
pub struct ListBadges<'a> {
    pub shown: Vec<&'a RefInfo>,
    pub hidden: Vec<&'a RefInfo>,
}

/// The list's choice among `ordered` refs ([`ordered`]): all of them when they fit in
/// `max`; otherwise the first `max - 1` plus the first tag, which keeps its place after the
/// branches — a commit's (first) tag is always visible, the other tags join the "+N".
pub fn for_list<'a>(ordered: &[&'a RefInfo], max: usize) -> ListBadges<'a> {
    if ordered.len() <= max {
        return ListBadges {
            shown: ordered.to_vec(),
            hidden: Vec::new(),
        };
    }
    let tag = ordered.iter().position(|r| r.kind == RefKind::Tag);
    let keep = |i: usize| match tag {
        Some(t) if t >= max.saturating_sub(1) => i < max.saturating_sub(1) || i == t,
        _ => i < max,
    };
    let (mut shown, mut hidden) = (Vec::new(), Vec::new());
    for (i, r) in ordered.iter().enumerate() {
        if keep(i) { &mut shown } else { &mut hidden }.push(*r);
    }
    ListBadges { shown, hidden }
}

/// [`for_list`] for a row with `room` for its badges: the most badges (at most `max`, at
/// least one — the tag when the commit has one) whose widths fit — each ref's `width`, the
/// "+N" `more(n)`, `gap` after each but the last. Units are the caller's (px, columns).
/// So a narrow row drops branches into the "+N" before its tag, and the subject keeps
/// the room the caller left out of `room`.
pub fn fit_list<'a>(
    ordered: &[&'a RefInfo],
    max: usize,
    room: f32,
    gap: f32,
    width: impl Fn(&RefInfo) -> f32,
    more: impl Fn(usize) -> f32,
) -> ListBadges<'a> {
    let need = |b: &ListBadges| {
        let items = b.shown.len() + usize::from(!b.hidden.is_empty());
        b.shown.iter().map(|r| width(r)).sum::<f32>()
            + if b.hidden.is_empty() { 0. } else { more(b.hidden.len()) }
            + gap * items.saturating_sub(1) as f32
    };
    let mut k = max.max(1);
    loop {
        let b = for_list(ordered, k);
        if k == 1 || need(&b) <= room {
            return b;
        }
        k -= 1;
    }
}

impl ListBadges<'_> {
    /// What the "+N" says about the refs it hides (gitti's tooltip): one line per ref with
    /// its kind; `None` when nothing is hidden.
    pub fn hidden_summary(&self) -> Option<String> {
        if self.hidden.is_empty() {
            return None;
        }
        let mut s = format!(
            "{} more {} on this commit (all in its details):",
            self.hidden.len(),
            if self.hidden.len() == 1 { "ref" } else { "refs" }
        );
        for r in &self.hidden {
            s.push_str(&format!("\n{} — {}", r.short_name, kind_label(r.kind)));
        }
        Some(s)
    }
}

/// "branch", "remote branch", "tag", … for texts about a ref.
pub fn kind_label(kind: RefKind) -> &'static str {
    match kind {
        RefKind::Head => "HEAD",
        RefKind::LocalBranch => "branch",
        RefKind::RemoteBranch => "remote branch",
        RefKind::Tag => "tag",
        RefKind::Stash => "stash",
        RefKind::Other => "ref",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(kind: RefKind, name: &str) -> RefInfo {
        RefInfo {
            full_name: name.into(),
            short_name: name.into(),
            kind,
            target: gix::ObjectId::null(gix::hash::Kind::Sha1),
        }
    }

    fn names(v: &[&RefInfo]) -> Vec<String> {
        v.iter().map(|r| r.short_name.clone()).collect()
    }

    #[gitti_test::test]
    fn order_puts_the_current_branch_first_then_by_kind_and_name() {
        let refs = [
            r(RefKind::Tag, "v1"),
            r(RefKind::RemoteBranch, "origin/main"),
            r(RefKind::LocalBranch, "zeta"),
            r(RefKind::LocalBranch, "alpha"),
            r(RefKind::LocalBranch, "main"),
            r(RefKind::Stash, "stash@{0}"),
        ];
        let o = ordered(&refs, Some("main"));
        assert_eq!(names(&o), ["main", "alpha", "zeta", "origin/main", "v1", "stash@{0}"]);
        // A remote branch named like the current branch is not "current".
        let o = ordered(&refs, Some("origin/main"));
        assert_eq!(names(&o)[..3], ["alpha", "main", "zeta"]);
    }

    #[gitti_test::test]
    fn list_shows_all_that_fit() {
        let refs = [r(RefKind::LocalBranch, "a"), r(RefKind::Tag, "v1")];
        let o = ordered(&refs, None);
        let b = for_list(&o, LIST_MAX);
        assert_eq!(names(&b.shown), ["a", "v1"]);
        assert!(b.hidden.is_empty() && b.hidden_summary().is_none());
    }

    #[gitti_test::test]
    fn list_keeps_the_single_tag_behind_many_branches() {
        let refs = [
            r(RefKind::LocalBranch, "a"),
            r(RefKind::LocalBranch, "b"),
            r(RefKind::LocalBranch, "c"),
            r(RefKind::RemoteBranch, "origin/a"),
            r(RefKind::Tag, "v1.0"),
        ];
        let o = ordered(&refs, Some("b"));
        let b = for_list(&o, LIST_MAX);
        assert_eq!(names(&b.shown), ["b", "a", "v1.0"]);
        assert_eq!(names(&b.hidden), ["c", "origin/a"]);
        let tip = b.hidden_summary().unwrap();
        assert!(tip.starts_with("2 more refs"), "{tip}");
        assert!(tip.contains("c — branch") && tip.contains("origin/a — remote branch"), "{tip}");
    }

    #[gitti_test::test]
    fn list_shows_the_first_of_several_tags_and_counts_the_rest() {
        let refs = [
            r(RefKind::LocalBranch, "main"),
            r(RefKind::LocalBranch, "next"),
            r(RefKind::RemoteBranch, "origin/main"),
            r(RefKind::Tag, "v2.0"),
            r(RefKind::Tag, "latest"),
        ];
        let o = ordered(&refs, Some("main"));
        let b = for_list(&o, LIST_MAX);
        assert_eq!(names(&b.shown), ["main", "next", "latest"]);
        assert_eq!(names(&b.hidden), ["origin/main", "v2.0"]);
        // Only tags: the first `max` of them.
        let tags = [r(RefKind::Tag, "a"), r(RefKind::Tag, "b"), r(RefKind::Tag, "c"), r(RefKind::Tag, "d")];
        let o = ordered(&tags, None);
        let b = for_list(&o, LIST_MAX);
        assert_eq!((names(&b.shown), names(&b.hidden)), (vec!["a".into(), "b".into(), "c".into()], vec!["d".to_string()]));
        assert!(b.hidden_summary().unwrap().starts_with("1 more ref "));
    }

    #[gitti_test::test]
    fn narrow_rows_keep_the_tag_and_drop_branches() {
        let refs = [
            r(RefKind::LocalBranch, "main"),
            r(RefKind::LocalBranch, "feature"),
            r(RefKind::RemoteBranch, "origin/main"),
            r(RefKind::Tag, "v1.0"),
        ];
        let o = ordered(&refs, Some("main"));
        // Widths: the name's length + 2, "+N" 3, gap 1.
        let fit = |room| {
            let b = fit_list(&o, LIST_MAX, room, 1., |r| r.short_name.len() as f32 + 2., |_| 3.);
            (names(&b.shown), b.hidden.len())
        };
        // main 6 + feature 9 + v1.0 6 + "+1" 3 + 3 gaps = 27.
        assert_eq!(fit(27.), (vec!["main".into(), "feature".into(), "v1.0".into()], 1));
        // main 6 + v1.0 6 + "+2" 3 + 2 gaps = 17.
        assert_eq!(fit(26.), (vec!["main".into(), "v1.0".into()], 2));
        assert_eq!(fit(17.), (vec!["main".into(), "v1.0".into()], 2));
        // Only the tag, however narrow.
        assert_eq!(fit(16.), (vec!["v1.0".to_string()], 3));
        assert_eq!(fit(0.), (vec!["v1.0".to_string()], 3));
        // Everything fits: no "+N" to pay for.
        let all = fit_list(&o, 4, 100., 1., |_| 5., |_| 3.);
        assert_eq!((all.shown.len(), all.hidden.len()), (4, 0));
        // No tag: the first ref.
        let branches = &o[..3];
        let b = fit_list(branches, LIST_MAX, 0., 1., |_| 5., |_| 3.);
        assert_eq!((names(&b.shown), b.hidden.len()), (vec!["main".to_string()], 2));
    }

    #[gitti_test::test]
    fn list_without_tags_cuts_after_max() {
        let refs: Vec<RefInfo> = (0..5).map(|i| r(RefKind::LocalBranch, &format!("b{i}"))).collect();
        let o = ordered(&refs, None);
        let b = for_list(&o, LIST_MAX);
        assert_eq!(names(&b.shown), ["b0", "b1", "b2"]);
        assert_eq!(b.hidden.len(), 2);
        // A tag already among the first `max` takes no extra slot.
        let refs = [
            r(RefKind::LocalBranch, "a"),
            r(RefKind::Tag, "t1"),
            r(RefKind::Tag, "t2"),
            r(RefKind::Tag, "t3"),
        ];
        let o = ordered(&refs, None);
        assert_eq!(names(&for_list(&o, LIST_MAX).shown), ["a", "t1", "t2"]);
    }
}
