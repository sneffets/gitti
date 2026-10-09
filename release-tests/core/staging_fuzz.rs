//! Randomized line staging (release suite): the key feature against a model.
//!
//! Every round writes a random file, changes it at random places, selects random change
//! lines (none / some / whole hunks) and stages, unstages or discards that selection. The
//! result must be exactly "the old side plus the selected changes" (stage), "the new side
//! minus the selected changes" (unstage, discard) — computed here from the diff lines alone
//! — and the side that wasn't operated on must stay untouched.
//!
//! - `GITTI_FUZZ_SEED=N` — start seed (default: from the clock, printed);
//! - `GITTI_FUZZ_ROUNDS=N` — rounds (default 300, ~30 s);
//! - `GITTI_FUZZ_SECS=N` — run for N seconds instead (overnight: 28800).
//!
//! A failure prints the seed, the round and both file versions, the selection and the
//! expected / actual text, so `GITTI_FUZZ_SEED` + the round replays it.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use gitti_core::Repo;
use gitti_core::diff::{FileDiff, LineKind};
use gitti_core::patch::{PatchMode, Selection};

/// xorshift64*: small, deterministic per seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

fn sh(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// A random text: few distinct lines, so diffs align in interesting ways; sometimes no
/// newline at the end.
fn random_text(rng: &mut Rng, lines: usize) -> String {
    let mut s: String = (0..lines)
        .map(|_| format!("l{}\n", rng.below(12)))
        .collect();
    if !s.is_empty() && rng.chance(15) {
        s.pop();
    }
    s
}

/// `base` with 1..8 random line edits (delete / insert / replace), the end-of-file newline
/// toggled sometimes.
fn mutate(rng: &mut Rng, base: &str) -> String {
    let mut lines: Vec<String> = base.lines().map(str::to_string).collect();
    let no_eol = !base.is_empty() && !base.ends_with('\n');
    for _ in 0..1 + rng.below(8) {
        let at = rng.below(lines.len() + 1);
        match rng.below(3) {
            0 if at < lines.len() => {
                lines.remove(at);
            }
            1 if at < lines.len() => lines[at] = format!("m{}", rng.below(12)),
            _ => lines.insert(at, format!("n{}", rng.below(12))),
        }
    }
    let mut s: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let no_eol = if rng.chance(10) { !no_eol } else { no_eol };
    if no_eol && !s.is_empty() {
        s.pop();
    }
    s
}

/// A random selection over `d`'s hunks: per hunk none, all, or a random subset of its
/// change lines. `None` when it selects nothing.
fn random_selection(rng: &mut Rng, d: &FileDiff) -> Option<Selection> {
    let mut sel = Selection::default();
    for (h, hunk) in d.hunks.iter().enumerate() {
        match rng.below(3) {
            0 => {}
            1 => {
                sel.hunks.insert(h, None);
            }
            _ => {
                let lines: Vec<usize> = hunk
                    .lines
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| l.is_change() && rng.chance(50))
                    .map(|(i, _)| i)
                    .collect();
                if !lines.is_empty() {
                    sel.add_lines(h, lines);
                }
            }
        }
    }
    (!sel.hunks.is_empty()).then_some(sel)
}

fn selected(sel: &Selection, hunk: usize, line: usize) -> bool {
    match sel.hunks.get(&hunk) {
        None => false,
        Some(None) => true,
        Some(Some(set)) => set.contains(&line),
    }
}

/// The text after applying `sel` of diff `d` (old side `old` → new side) as `mode` does:
/// stage keeps the old side and takes the selected changes; unstage / discard keep the new
/// side and drop the selected changes. Lines outside the hunks come from the side kept.
fn model(old: &str, new: &str, d: &FileDiff, sel: &Selection, mode: PatchMode) -> String {
    let forward = matches!(mode, PatchMode::Stage);
    // The side the result starts from, as (line, has its newline).
    let (side, side_is_old) = if forward { (old, true) } else { (new, false) };
    let raw: Vec<(String, bool)> = side
        .split_inclusive('\n')
        .map(|l| (l.trim_end_matches('\n').to_string(), l.ends_with('\n')))
        .collect();
    let mut out: Vec<(String, bool)> = Vec::new();
    let mut next = 0usize; // next line of `side` (0-based)
    for (h, hunk) in d.hunks.iter().enumerate() {
        let (start, count) = if side_is_old {
            (hunk.old_start, hunk.old_count)
        } else {
            (hunk.new_start, hunk.new_count)
        };
        // `-S,C`: the hunk covers lines S.. (1-based); with C = 0 it inserts after line S.
        let first = if count == 0 { start } else { start.saturating_sub(1) } as usize;
        let first = first.clamp(next, raw.len());
        out.extend(raw[next..first].iter().cloned());
        let mut used = 0usize; // lines of `side` the hunk covers
        for (i, l) in hunk.lines.iter().enumerate() {
            let eol = hunk
                .lines
                .get(i + 1)
                .is_none_or(|n| n.kind != LineKind::NoNewline);
            let line = (l.text.clone(), eol);
            let sel = selected(sel, h, i);
            match l.kind {
                LineKind::NoNewline => {}
                LineKind::Context => {
                    out.push(line);
                    used += 1;
                }
                // On the side the result starts from a change line stays unless selected;
                // from the other side it comes in only when selected.
                LineKind::Removed if side_is_old => {
                    used += 1;
                    if !sel {
                        out.push(line);
                    }
                }
                LineKind::Added if !side_is_old => {
                    used += 1;
                    if !sel {
                        out.push(line);
                    }
                }
                LineKind::Removed | LineKind::Added => {
                    if sel {
                        out.push(line);
                    }
                }
            }
        }
        next = first + used;
    }
    out.extend(raw[next.min(raw.len())..].iter().cloned());
    // Only the last line can lack its newline: one that ends up in the middle gets one.
    let n = out.len();
    out.iter()
        .enumerate()
        .map(|(i, (text, eol))| format!("{text}{}", if *eol || i + 1 < n { "\n" } else { "" }))
        .collect()
}

fn diff_of(repo: &Repo, staged: bool) -> Option<FileDiff> {
    let s = repo.working_state().unwrap();
    let list = if staged { &s.staged } else { &s.unstaged };
    list.iter()
        .find(|f| f.path() == "f.txt")
        .and_then(|f| f.diff.clone())
}

#[test]
fn staging_fuzz() {
    let seed: u64 = std::env::var("GITTI_FUZZ_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64
        })
        | 1;
    let rounds: usize = std::env::var("GITTI_FUZZ_ROUNDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);
    let until = std::env::var("GITTI_FUZZ_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .map(|s| Instant::now() + Duration::from_secs(s));
    eprintln!("staging_fuzz: GITTI_FUZZ_SEED={seed}");
    let mut rng = Rng(seed);

    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    sh(p, &["init", "-q", "-b", "main"]);
    for (k, v) in [
        ("user.name", "Fuzz"),
        ("user.email", "fuzz@example.com"),
        ("commit.gpgsign", "false"),
        ("core.autocrlf", "false"),
    ] {
        sh(p, &["config", k, v]);
    }
    let repo = Repo::open(p).unwrap();
    let file = p.join("f.txt");

    let mut round = 0usize;
    let mut checked = 0usize;
    loop {
        match until {
            Some(t) if Instant::now() >= t => break,
            None if round >= rounds => break,
            _ => {}
        }
        round += 1;
        let len = 1 + rng.below(60);
        let base = random_text(&mut rng, len);
        let new = mutate(&mut rng, &base);
        if base == new {
            continue;
        }
        let mode = match rng.below(3) {
            0 => PatchMode::Stage,
            1 => PatchMode::Unstage,
            _ => PatchMode::Discard,
        };
        // HEAD = index = base, worktree = new; for unstage the index = new as well.
        fs::write(&file, &base).unwrap();
        sh(p, &["add", "f.txt"]);
        sh(p, &["commit", "-q", "--allow-empty", "-m", "base"]);
        fs::write(&file, &new).unwrap();
        if mode == PatchMode::Unstage {
            sh(p, &["add", "f.txt"]);
        }
        let Some(d) = diff_of(&repo, mode == PatchMode::Unstage) else {
            panic!("seed {seed} round {round}: no diff for {base:?} → {new:?}");
        };
        let Some(sel) = random_selection(&mut rng, &d) else {
            continue;
        };
        let expected = model(&base, &new, &d, &sel, mode);
        let result = repo.apply_selection(&d, &sel, mode);
        let (actual, untouched_ok) = match mode {
            PatchMode::Stage | PatchMode::Unstage => (
                sh(p, &["show", ":f.txt"]),
                fs::read_to_string(&file).unwrap() == new,
            ),
            PatchMode::Discard => (
                fs::read_to_string(&file).unwrap(),
                sh(p, &["show", ":f.txt"]) == base,
            ),
        };
        if result.is_err() || actual != expected || !untouched_ok {
            panic!(
                "staging_fuzz failed — GITTI_FUZZ_SEED={seed}, round {round}, {mode:?}\n\
                 result: {result:?}\nselection: {:?}\n--- old\n{base:?}\n--- new\n{new:?}\n\
                 --- expected\n{expected:?}\n--- actual\n{actual:?}\nother side untouched: \
                 {untouched_ok}\n--- diff hunks\n{:#?}",
                sel.hunks, d.hunks.iter().map(|h| &h.lines).collect::<Vec<_>>()
            );
        }
        checked += 1;
    }
    eprintln!("staging_fuzz: {checked} selections checked in {round} rounds, seed {seed}");
}
