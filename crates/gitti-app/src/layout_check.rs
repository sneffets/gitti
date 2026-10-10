//! Geometric invariants over the [`probe`](crate::probe)s of one frame — what must hold in
//! every state (scroll offset, message length, font size, window size). Used by the
//! headless UI tests and the `check` script step; each violation is one readable line.

use gpui_kit::*;

use crate::probe::Probe;

/// Rounding slack: borders (probes fill the padding box) and half-pixel layout.
const TOL: f32 = 1.5;

/// Areas laid out next to / below each other that must never overlap on screen.
const REGIONS: &[&str] = &[
    "tab-bar",
    "toolbar",
    "sidebar",
    "commit-list",
    "commit-box",
    "compare-banner",
    "file-tabs",
    "diff-list",
    "status-bar",
];

pub fn check(probes: &[Probe]) -> Vec<String> {
    let mut out = Vec::new();
    let get = |name: &str| probes.iter().find(|p| p.name == name);
    let all = |kind: &'static str| {
        let mut v: Vec<&Probe> = probes.iter().filter(move |p| p.kind() == kind).collect();
        v.sort_by_key(|p| p.index());
        v
    };

    // 1. Top-level regions don't overlap.
    let regions: Vec<(&str, Bounds<Pixels>)> = REGIONS
        .iter()
        .filter_map(|n| Some((*n, get(n)?.visible()?)))
        .collect();
    for (i, (a, ra)) in regions.iter().enumerate() {
        for (b, rb) in &regions[i + 1..] {
            if let Some(o) = overlap(ra, rb) {
                out.push(format!("{a} and {b} overlap ({o})"));
            }
        }
    }

    // 2. Vertical order: tab bar, toolbar, then the panes; commit box / file tabs above
    //    the diff list, the working directory's file tabs above the commit box, the
    //    compare banner above all three.
    for (upper, lower) in [
        ("tab-bar", "toolbar"),
        ("toolbar", "commit-box"),
        ("toolbar", "file-tabs"),
        ("toolbar", "diff-list"),
        ("file-tabs", "commit-box"),
        ("commit-box", "diff-list"),
        ("file-tabs", "diff-list"),
        ("toolbar", "compare-banner"),
        ("compare-banner", "commit-box"),
        ("compare-banner", "file-tabs"),
        ("compare-banner", "diff-list"),
        ("diff-list", "status-bar"),
        // Search pickers: the line saying what was searched, above the results.
        ("picker-note", "picker-list"),
    ] {
        if let (Some(u), Some(l)) = (get(upper), get(lower))
            && let (Some(u), Some(l)) = (u.visible(), l.visible())
            && f32::from(u.bottom()) > f32::from(l.top()) + TOL
        {
            out.push(format!(
                "{upper} (bottom {}) reaches below the top of {lower} ({})",
                f32::from(u.bottom()),
                f32::from(l.top())
            ));
        }
    }

    // 2a. Toolbar: branch, command bar and buttons side by side inside it,
    //     the error mark and Push ▾ among the buttons; the palette lies over the command
    //     bar.
    if let Some(tb) = get("toolbar").and_then(Probe::visible) {
        let parts: Vec<(&str, Bounds<Pixels>)> = ["toolbar-branch", "command-bar", "toolbar-buttons"]
            .iter()
            .filter_map(|n| Some((*n, get(n)?.visible()?)))
            .collect();
        for (i, (a, ra)) in parts.iter().enumerate() {
            if !contains(&tb, ra) {
                out.push(format!("{a} is drawn outside the toolbar"));
            }
            for (b, rb) in &parts[i + 1..] {
                if let Some(o) = overlap(ra, rb) {
                    out.push(format!("{a} and {b} overlap ({o})"));
                }
            }
        }
        if let Some(buttons) = get("toolbar-buttons").and_then(Probe::visible) {
            for n in ["error-mark", "push-split"] {
                if let Some(v) = get(n).and_then(Probe::visible)
                    && !contains(&buttons, &v)
                {
                    out.push(format!("{n} is drawn outside the toolbar buttons"));
                }
            }
        }
    }
    // 2b. A running operation in the status bar: name, message and Cancel side by
    //     side inside it, the message's parts inside the message area; the output window
    //     right above the message area, exactly as wide, whole inside the window.
    if let Some(sb) = get("status-bar").and_then(Probe::visible) {
        let parts: Vec<(&str, Bounds<Pixels>)> = ["busy-name", "busy-message", "busy-cancel"]
            .iter()
            .filter_map(|n| Some((*n, get(n)?.visible()?)))
            .collect();
        for (i, (a, ra)) in parts.iter().enumerate() {
            if !contains(&sb, ra) {
                out.push(format!("{a} is drawn outside the status bar"));
            }
            for (b, rb) in &parts[i + 1..] {
                if overlap(ra, rb).is_some() || f32::from(ra.right()) > f32::from(rb.left()) + TOL {
                    out.push(format!("{a} and {b} overlap or are out of order"));
                }
            }
        }
        if let Some(msg) = get("busy-message").and_then(Probe::visible) {
            for n in ["busy-phase", "busy-track", "busy-percent", "busy-amount", "busy-output"] {
                if let Some(v) = get(n).and_then(Probe::visible)
                    && !contains(&msg, &v)
                {
                    out.push(format!("{n} is drawn outside the busy message area"));
                }
            }
            if let Some(pop) = get("busy-popup") {
                let f = |x: Pixels| f32::from(x);
                let b = &pop.bounds;
                if (f(b.left()) - f(msg.left())).abs() > TOL
                    || (f(b.right()) - f(msg.right())).abs() > TOL
                    || (f(b.bottom()) - f(sb.top())).abs() > 2. * TOL
                {
                    out.push(format!(
                        "the output window ({b:?}) does not sit on the message area ({msg:?})"
                    ));
                }
                if f(b.top()) < 0. || pop.visible().is_none_or(|v| !same(&v, b)) {
                    out.push(format!("the output window ({b:?}) is cut off"));
                }
            }
        }
    }
    if let (Some(bar), Some(picker)) = (
        get("command-bar").and_then(Probe::visible),
        get("palette").and_then(Probe::visible),
    ) {
        // The palette lies over the whole command bar (same or larger bounds); the
        // file list drops down from the file tabs' chevron instead.
        let f = |x: Pixels| f32::from(x);
        if f(picker.top()) > f(bar.top()) + TOL
            || f(picker.left()) > f(bar.left()) + TOL
            || f(picker.right()) + TOL < f(bar.right())
            || f(picker.bottom()) + TOL < f(bar.bottom())
        {
            out.push(format!(
                "the palette ({:?}) does not cover the command bar ({:?})",
                picker, bar
            ));
        }
    }

    // 3. Rows: one uniform pitch (a probe fills the padding box, so a row's border moves
    //    its top by a pixel — hence the pitch, not the heights), drawn only inside their
    //    list, and whole wherever the list shows them whole (nothing else clips them).
    for (kind, list) in [
        ("diff-row", "diff-list"),
        ("commit-row", "commit-list"),
        ("log-row", "git-log"),
        ("picker-item", "picker-list"),
        ("folder-row", "folder-list"),
    ] {
        let rows = all(kind);
        let list_vis = get(list).and_then(Probe::visible);
        let mut pitches: Vec<(f32, &Probe, &Probe)> = rows
            .windows(2)
            .filter(|w| w[0].index().map(|i| i + 1) == w[1].index())
            .map(|w| (f32::from(w[1].bounds.top() - w[0].bounds.top()), w[0], w[1]))
            .collect();
        pitches.sort_by(|a, b| a.0.total_cmp(&b.0));
        if let Some(&(median, _, _)) = pitches.get(pitches.len() / 2) {
            for (pitch, a, b) in &pitches {
                if (pitch - median).abs() > 2. * TOL {
                    out.push(format!(
                        "{} → {}: row pitch {pitch} px, the others {median} px",
                        a.name, b.name
                    ));
                }
            }
        }
        if let Some(lv) = list_vis {
            for r in &rows {
                let Some(v) = r.visible() else { continue };
                if !contains(&lv, &v) {
                    out.push(format!("{} is drawn outside {list}", r.name));
                }
                if contains(&lv, &r.bounds) && !same(&v, &r.bounds) {
                    out.push(format!("{} is clipped inside {list}", r.name));
                }
            }
        }
    }

    // 3c. What a commit row shows of its subject, author and date (one or two
    //     lines) never overlaps and stays inside the row.
    for subject in all("commit-subject") {
        let Some(i) = subject.index() else { continue };
        let row = probes.iter().find(|p| p.name == format!("commit-row:{i}"));
        let parts: Vec<(&str, Bounds<Pixels>)> = ["commit-subject", "commit-author", "commit-date"]
            .iter()
            .filter_map(|k| probes.iter().find(|p| p.name == format!("{k}:{i}")))
            .filter_map(|p| Some((p.name.as_str(), p.visible()?)))
            .collect();
        for (j, (a, ra)) in parts.iter().enumerate() {
            if let Some(row) = row
                && !contains(&row.bounds, ra)
            {
                out.push(format!("{a} ({ra:?}) lies outside its row ({:?})", row.bounds));
            }
            for (b, rb) in &parts[j + 1..] {
                if let Some(o) = overlap(ra, rb) {
                    out.push(format!("{a} and {b} overlap ({o})"));
                }
            }
        }
    }

    // 3b. The dashes of a pending merge's edges are drawn inside their commit row.
    for d in all("graph-dash") {
        let row = d
            .index()
            .and_then(|i| probes.iter().find(|p| p.name == format!("commit-row:{i}")));
        match row {
            Some(row) if contains(&row.bounds, &d.bounds) => {}
            Some(row) => out.push(format!(
                "{} ({:?}) lies outside its row ({:?})",
                d.name, d.bounds, row.bounds
            )),
            None => out.push(format!("{} has no commit row", d.name)),
        }
    }

    // 3a. A picker row's match reason stays inside its row (never pushed out by a long
    //     label or path).
    for tag in all("picker-tag") {
        let row = tag
            .index()
            .and_then(|i| probes.iter().find(|p| p.name == format!("picker-item:{i}")));
        if let (Some(row), Some(tv)) = (row, tag.visible())
            && let Some(rv) = row.visible()
            && !contains(&rv, &tv)
        {
            out.push(format!("{} is drawn outside {}", tag.name, row.name));
        }
    }

    // 3a'. The picker's keys column: inside its row, right-aligned at one x over
    //      all rows, never overlapped by the row's label or description (they truncate).
    let keys = all("picker-keys");
    let right = keys.first().map(|k| f32::from(k.bounds.right()));
    for k in &keys {
        let Some(i) = k.index() else { continue };
        let named = |kind: &str| {
            let name = format!("{kind}:{i}");
            probes.iter().find(move |p| p.name == name)
        };
        if let Some(row) = named("picker-item")
            && !contains(&row.bounds, &k.bounds)
        {
            out.push(format!("{} sticks out of {}", k.name, row.name));
        }
        if let Some(r) = right
            && (f32::from(k.bounds.right()) - r).abs() > TOL
        {
            out.push(format!("{} ends at {:?}, the column at {r}", k.name, k.bounds.right()));
        }
        for kind in ["picker-label", "picker-detail", "picker-tag"] {
            if let Some(o) = named(kind).and_then(|t| overlap(&t.bounds, &k.bounds)) {
                out.push(format!("{kind}:{i} overlaps {} ({o})", k.name));
            }
        }
    }

        // 3b. Sidebar entries (rows of different heights, not a uniform list) are only drawn
    //     inside the sidebar, however far it is scrolled.
    if let Some(sv) = get("sidebar").and_then(Probe::visible) {
        for r in probes
            .iter()
            .filter(|p| matches!(p.kind(), "side" | "side-section"))
        {
            if let Some(v) = r.visible()
                && !contains(&sv, &v)
            {
                out.push(format!("{} is drawn outside the sidebar", r.name));
            }
        }
    }

    // 4. Sticky header rows and the commit header stay inside the diff list (below the
    //    commit box / file tabs), and the sticky file row is above the sticky hunk row.
    if let Some(lv) = get("diff-list").and_then(Probe::visible) {
        for name in ["sticky-file", "sticky-hunk", "commit-header"] {
            if let Some(v) = get(name).and_then(Probe::visible)
                && !contains(&lv, &v)
            {
                out.push(format!("{name} is drawn outside the diff list"));
            }
        }
        for m in all("msg-line") {
            if let Some(v) = m.visible()
                && !contains(&lv, &v)
            {
                out.push(format!("{} is drawn outside the diff list", m.name));
            }
        }
    }
    // (A hunk row pushed out by the next hunk slides under the file row.)
    if let (Some(f), Some(h)) = (get("sticky-file"), get("sticky-hunk"))
        && f32::from(h.bounds.top()) + TOL < f32::from(f.bounds.top())
    {
        out.push("sticky-hunk is above the sticky file row".into());
    }

    // 4b. The commit header lies over its `HeaderSpace` rows only: it covers no diff row.
    if let Some(hv) = get("commit-header").and_then(Probe::visible) {
        for r in all("diff-row") {
            if r.detail() == "header-space" {
                continue;
            }
            if let Some(rv) = r.visible()
                && let Some(o) = overlap(&hv, &rv)
            {
                out.push(format!("commit-header covers {} ({o})", r.name));
            }
        }
    }

    // 5. Message lines (long ones wrap): one below the other, inside the header.
    let lines = all("msg-line");
    for w in lines.windows(2) {
        let (a, b) = (w[0], w[1]);
        let overlap = f32::from(a.bounds.bottom() - b.bounds.top());
        if overlap > TOL {
            out.push(format!("{} and {} overlap by {overlap} px", a.name, b.name));
        }
    }
    if let Some(h) = get("commit-header") {
        for l in &lines {
            if f32::from(l.bounds.left()) + TOL < f32::from(h.bounds.left())
                || f32::from(l.bounds.right()) > f32::from(h.bounds.right()) + TOL
            {
                out.push(format!("{} sticks out of the commit header", l.name));
            }
        }
    }

    // 5a. The commit header's ref badges (all refs, wrapping): inside their row, none
    //     over another. A commit row's badges: what shows of them inside the row.
    let refs = all("header-ref");
    if let Some(row) = get("header-refs") {
        for (i, a) in refs.iter().enumerate() {
            if !contains(&row.bounds, &a.bounds) {
                out.push(format!("{} sticks out of the header's refs", a.name));
            }
            for b in &refs[i + 1..] {
                if let Some(o) = overlap(&a.bounds, &b.bounds) {
                    out.push(format!("{} and {} overlap ({o})", a.name, b.name));
                }
            }
        }
    }
    for b in all("ref-badge").into_iter().chain(all("ref-more")) {
        let row = b
            .index()
            .and_then(|i| probes.iter().find(|p| p.name == format!("commit-row:{i}")));
        if let (Some(row), Some(v)) = (row, b.visible())
            && !contains(&row.bounds, &v)
        {
            out.push(format!("{} is drawn outside {}", b.name, row.name));
        }
    }

    // 6. The commit box's content: the shrunk summary and the editor stay inside it.
    if let Some(b) = get("commit-box").and_then(Probe::visible) {
        for name in ["commit-summary", "commit-editor"] {
            if let Some(v) = get(name).and_then(Probe::visible)
                && !contains(&b, &v)
            {
                out.push(format!("{name} is drawn outside the commit box"));
            }
        }
    }

    // 7. Tabs inside their strip.
    for (kind, strip) in [("file-tab", "file-tabs"), ("repo-tab", "tab-bar")] {
        if let Some(s) = get(strip) {
            for t in all(kind) {
                if f32::from(t.bounds.top()) + TOL < f32::from(s.bounds.top())
                    || f32::from(t.bounds.bottom()) > f32::from(s.bounds.bottom()) + TOL
                {
                    out.push(format!("{} sticks out of {strip} vertically", t.name));
                }
            }
        }
    }

    // 7b. The ▾ at the right edge of a tab strip is inside its bar and no tab is
    //     drawn over it (the tabs scroll under their own clip, left of it).
    for (kind, menu, strip) in [
        ("file-tab", "file-tabs-menu", "file-tabs"),
        ("repo-tab", "repo-tabs-menu", "tab-bar"),
    ] {
        let (Some(m), Some(s)) = (get(menu).and_then(Probe::visible), get(strip)) else {
            continue;
        };
        if !contains(&s.bounds, &m) {
            out.push(format!("{menu} is drawn outside {strip}"));
        }
        for t in all(kind) {
            if let Some(o) = t.visible().and_then(|v| overlap(&v, &m)) {
                out.push(format!("{} and {menu} overlap ({o})", t.name));
            }
        }
    }

    // 8. Merge editor: a side's line text is drawn only inside its cell (a long
    // line used to run on under the other side's text), and the bars' texts stay on one
    // line inside their bar (wrapped, they ran out of it).
    // Paired with the cell / bar recorded last before them: a merge window's probes can
    // pile up over several of its draws within one frame of the main window.
    let mut cells: std::collections::HashMap<&str, &Probe> = Default::default();
    for t in probes {
        match t.kind() {
            "merge-cell" | "merge-toolbar" | "merge-folded" => {
                cells.insert(t.name.as_str(), t);
            }
            "merge-text" => {
                let cell = format!("merge-cell:{}:{}", t.index().unwrap_or(0), t.detail());
                if let (Some(v), Some(c)) = (t.visible(), cells.get(cell.as_str()))
                    && !contains(&c.bounds, &v)
                {
                    out.push(format!("{} is drawn outside {cell}", t.name));
                }
            }
            "merge-bar-text" => {
                // 0–3 in the toolbar, 4–7 in the single view's folded sides bar.
                let bar = if t.index().unwrap_or(0) < 4 {
                    "merge-toolbar"
                } else {
                    "merge-folded"
                };
                if let Some(b) = cells.get(bar)
                    && (f32::from(t.bounds.top()) + TOL < f32::from(b.bounds.top())
                        || f32::from(t.bounds.bottom()) > f32::from(b.bounds.bottom()) + TOL)
                {
                    out.push(format!("{} sticks out of {bar} vertically", t.name));
                }
            }
            _ => {}
        }
    }
    out
}

/// `a` contains `b` (with [`TOL`]).
pub fn contains(a: &Bounds<Pixels>, b: &Bounds<Pixels>) -> bool {
    let f = f32::from;
    f(b.left()) + TOL >= f(a.left())
        && f(b.top()) + TOL >= f(a.top())
        && f(b.right()) <= f(a.right()) + TOL
        && f(b.bottom()) <= f(a.bottom()) + TOL
}

fn same(a: &Bounds<Pixels>, b: &Bounds<Pixels>) -> bool {
    contains(a, b) && contains(b, a)
}

/// The overlap of `a` and `b` beyond [`TOL`] in both directions, as "W×H".
fn overlap(a: &Bounds<Pixels>, b: &Bounds<Pixels>) -> Option<String> {
    let i = a.intersect(b);
    let (w, h) = (f32::from(i.size.width), f32::from(i.size.height));
    (w > TOL && h > TOL).then(|| format!("{w}×{h} px"))
}

#[cfg(test)]
mod tests {
    // Explicit imports: with `test-support` the gpui glob would shadow `#[test]`.
    use super::check;
    use crate::probe::Probe;
    use gpui_kit::{Bounds, point, px, size};

    fn probe(name: &str, x: f32, y: f32, w: f32, h: f32) -> Probe {
        let b = Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
        Probe {
            name: name.into(),
            bounds: b,
            clip: Bounds::new(point(px(0.), px(0.)), size(px(2000.), px(2000.))),
        }
    }

    #[gitti_test::test]
    fn finds_box_over_list_and_row_gaps() {
        let ok = vec![
            probe("commit-box", 0., 40., 500., 100.),
            probe("diff-list", 0., 140., 500., 400.),
            probe("diff-row:0:section", 0., 140., 500., 20.),
            probe("diff-row:1:file", 0., 160., 500., 20.),
        ];
        assert!(check(&ok).is_empty(), "{:?}", check(&ok));
        let mut bad = ok.clone();
        bad[0].bounds.size.height = px(130.);
        bad.push(probe("diff-row:2:line", 0., 180., 500., 20.));
        bad.push(probe("diff-row:3:line", 0., 206., 500., 20.));
        let v = check(&bad);
        assert!(v.iter().any(|s| s.contains("commit-box and diff-list overlap")), "{v:?}");
        assert!(v.iter().any(|s| s.contains("row pitch 26")), "{v:?}");
    }

    #[gitti_test::test]
    fn worktree_file_tabs_sit_above_the_commit_box() {
        let ok = vec![
            probe("file-tabs", 0., 40., 500., 20.),
            probe("file-tab:0", 0., 40., 80., 20.),
            probe("commit-box", 0., 60., 500., 80.),
            probe("diff-list", 0., 140., 500., 400.),
        ];
        assert!(check(&ok).is_empty(), "{:?}", check(&ok));
        // Tabs below the box (or over it) are wrong.
        let mut bad = ok.clone();
        bad[0].bounds.origin.y = px(130.);
        bad[1].bounds.origin.y = px(130.);
        let v = check(&bad);
        assert!(
            v.iter().any(|s| s.contains("file-tabs (bottom 150) reaches below the top of commit-box")),
            "{v:?}"
        );
        assert!(v.iter().any(|s| s.contains("commit-box and file-tabs overlap")), "{v:?}");
    }

    #[gitti_test::test]
    fn finds_rows_clipped_inside_their_list() {
        let mut row = probe("diff-row:0:line", 0., 150., 500., 20.);
        row.clip = Bounds::new(point(px(0.), px(155.)), size(px(500.), px(400.)));
        let probes = vec![probe("diff-list", 0., 140., 500., 400.), row];
        let v = check(&probes);
        assert!(v.iter().any(|s| s.contains("clipped inside")), "{v:?}");
    }
}
