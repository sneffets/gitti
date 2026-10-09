//! Live output of long git operations.
//!
//! An operation that runs inside [`crate::process::with_progress`] gets every line its
//! writing git commands print on stderr while they run: git's progress meters (`fetch`,
//! `pull` and `push` get `--progress` there, `Git::run_unlogged`), the `Rebasing (1/2)`
//! counter and the output of hooks (git runs them with stdout on stderr: a slow
//! pre-commit hook shows what it does). [`OpProgress`] keeps what a frontend shows: the
//! latest meter ([`Meter`]: phase, percent, objects / bytes), the latest other line and
//! the whole output so far (the live output window), plus fixed-width texts for
//! the status bar's columns ([`Meter::percent_text`], [`Meter::amount`], [`bar_cells`])
//! so nothing there moves while the numbers change.
//!
//! git overwrites a meter in place with `\r`; [`ProgressLines`] splits the stream into
//! those updates, [`without_progress`] keeps only each line's final state (the command
//! log, the pull status) and [`strip_meters`] drops the meters altogether (what callers
//! parse is what git prints without `--progress`).

/// A progress meter line of git: `Receiving objects:  45% (450/1000), 1.20 MiB | 2.00
/// MiB/s`, `remote: Counting objects: 1234, done.`, `Rebasing (1/2)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meter {
    /// `Receiving objects`.
    pub phase: String,
    /// Printed by the remote side (`remote: …`).
    pub remote: bool,
    pub done: u64,
    pub total: Option<u64>,
    /// Given by git, or computed from `done / total`.
    pub percent: Option<u8>,
    /// What follows the counts: `1.20 MiB | 2.00 MiB/s`.
    pub detail: Option<String>,
    /// Ends with `, done.`.
    pub finished: bool,
}

impl Meter {
    /// Short text for a status line: `Receiving objects 45% · 1.20 MiB`,
    /// `remote: Counting objects 1234`.
    pub fn text(&self) -> String {
        let mut s = String::new();
        if self.remote {
            s.push_str("remote: ");
        }
        s.push_str(&self.phase);
        match self.percent {
            Some(p) => s.push_str(&format!(" {p}%")),
            None => s.push_str(&format!(" {}", self.done)),
        }
        // The amount, not the rate: `1.20 MiB | 2.00 MiB/s` → `1.20 MiB`.
        if let Some(amount) = self.detail.as_deref().and_then(|d| d.split('|').next()) {
            let amount = amount.trim();
            if amount.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                s.push_str(" · ");
                s.push_str(amount);
            }
        }
        s
    }

    /// 0.0 ..= 1.0 when the meter has a total.
    pub fn fraction(&self) -> Option<f32> {
        self.percent.map(|p| f32::from(p.min(100)) / 100.)
    }

    /// The percentage, always four characters wide (`"  5%"`, `" 45%"`, `"100%"`; four
    /// blanks without a total): a column of it doesn't move while it counts up.
    pub fn percent_text(&self) -> String {
        match self.percent {
            Some(p) => format!("{:>3}%", p.min(100)),
            None => " ".repeat(4),
        }
    }

    /// The amount beside the percentage: the bytes so far (`1.20 MiB`, the rate dropped),
    /// else the counts (`450/1000`, `1234` without a total). Shown right-aligned in a
    /// column of its own.
    pub fn amount(&self) -> String {
        if let Some(amount) = self.detail.as_deref().and_then(|d| d.split('|').next()) {
            let amount = amount.trim();
            if amount.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                return amount.to_string();
            }
        }
        match self.total {
            Some(t) => format!("{}/{t}", self.done),
            None => self.done.to_string(),
        }
    }

    /// Whether `other` is an update of the same meter (git rewrites it in place).
    fn same_phase(&self, other: &Meter) -> bool {
        self.phase == other.phase && self.remote == other.remote
    }
}

/// A text progress bar exactly `cells` characters wide: full blocks, one partial block in
/// eighths, blanks for the rest (all blanks without a fraction: an even, calm track).
pub fn bar_cells(fraction: Option<f32>, cells: usize) -> String {
    const PARTIAL: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let eighths = fraction.map_or(0, |f| (f.clamp(0., 1.) * (cells * 8) as f32).round() as usize);
    let (full, part) = (eighths / 8, eighths % 8);
    let mut s = "█".repeat(full.min(cells));
    if full < cells {
        s.push(PARTIAL[part]);
        s.push_str(&" ".repeat(cells - full - 1));
    }
    s
}

/// Parses one line of git's stderr as a progress meter (`None`: any other line).
pub fn parse_meter(line: &str) -> Option<Meter> {
    let line = line.trim();
    let (remote, line) = match line.strip_prefix("remote:") {
        Some(rest) => (true, rest.trim()),
        None => (false, line),
    };
    // `Rebasing (1/2)`.
    if let Some((phase, rest)) = line.split_once(" (")
        && let Some(counts) = rest.strip_suffix(')')
        && is_phase(phase)
        && let Some((done, total)) = fraction_counts(counts)
    {
        return Some(meter(phase, remote, done, Some(total), None, None, false));
    }
    let (phase, rest) = line.split_once(": ")?;
    if !is_phase(phase) {
        return None;
    }
    let rest = rest.trim_start();
    let digits = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    if digits == 0 {
        return None;
    }
    let number: u64 = rest[..digits].parse().ok()?;
    let mut rest = &rest[digits..];
    let (percent, done, total);
    if let Some(after) = rest.strip_prefix('%') {
        // `45% (450/1000)`.
        let after = after.trim_start();
        let close = after.find(')')?;
        let (d, t) = fraction_counts(after.strip_prefix('(')?.get(..close - 1)?)?;
        percent = Some(u8::try_from(number.min(100)).ok()?);
        (done, total) = (d, Some(t));
        rest = &after[close + 1..];
    } else {
        percent = None;
        (done, total) = (number, None);
    }
    // `, 1.20 MiB | 2.00 MiB/s, done.` (or nothing).
    let mut finished = false;
    let mut detail = Vec::new();
    if !rest.is_empty() {
        let tail = rest.strip_prefix(", ")?;
        for part in tail.split(", ") {
            if part == "done." || part == "done" {
                finished = true;
            } else {
                detail.push(part);
            }
        }
    }
    let detail = (!detail.is_empty()).then(|| detail.join(", "));
    Some(meter(phase, remote, done, total, percent, detail, finished))
}

fn meter(
    phase: &str,
    remote: bool,
    done: u64,
    total: Option<u64>,
    percent: Option<u8>,
    detail: Option<String>,
    finished: bool,
) -> Meter {
    let percent = percent.or_else(|| {
        total
            .filter(|&t| t > 0)
            .map(|t| u8::try_from((done.min(t) * 100) / t).unwrap_or(100))
    });
    Meter {
        phase: phase.to_string(),
        remote,
        done,
        total,
        percent,
        detail,
        finished,
    }
}

/// A meter's title: words of letters (`Receiving objects`, `Checking out files`).
fn is_phase(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && s.chars().all(|c| c.is_ascii_alphabetic() || c == ' ')
}

/// `450/1000`.
fn fraction_counts(s: &str) -> Option<(u64, u64)> {
    let (a, b) = s.split_once('/')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// Most output lines an [`OpProgress`] keeps; older ones are dropped (counted).
pub const MAX_OUTPUT_LINES: usize = 5000;
/// Longest output line kept (characters); the rest is cut.
const MAX_LINE_CHARS: usize = 2000;
/// How many lines back a meter's update looks for its line (remote and local meters may
/// interleave).
const METER_LOOKBACK: usize = 4;

/// What the frontends show of a running operation: the latest progress meter, the latest
/// other line (hook output, `To origin`, …) and every line so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpProgress {
    pub meter: Option<Meter>,
    pub line: Option<String>,
    /// The output as a terminal would show it: a meter's updates (git rewrites them with
    /// `\r`) replace its line until it is done; other lines in order.
    output: std::collections::VecDeque<OutputLine>,
    /// Lines dropped from the front (more than [`MAX_OUTPUT_LINES`]).
    dropped: usize,
    /// Changes with every line taken (a view follows the output's end with it).
    generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OutputLine {
    text: String,
    /// `(phase, remote)` of a meter line that is still being updated.
    open: Option<(String, bool)>,
}

impl OpProgress {
    /// Takes one live line (ANSI colours dropped).
    pub fn feed(&mut self, raw: &str) {
        let text = strip_ansi(raw);
        let text = text.trim_end();
        if text.trim().is_empty() {
            return;
        }
        let meter = parse_meter(text);
        self.push_output(text, meter.as_ref());
        match meter {
            Some(mut m) => {
                // Steady: an update of the running meter never goes back.
                if let Some(cur) = &self.meter
                    && cur.same_phase(&m)
                    && !cur.finished
                    && let (Some(was), Some(now)) = (cur.percent, m.percent)
                    && now < was
                {
                    m.percent = Some(was);
                }
                self.meter = Some(m)
            }
            None => self.line = Some(text.trim().to_string()),
        }
    }

    fn push_output(&mut self, text: &str, meter: Option<&Meter>) {
        self.generation += 1;
        let text = match text.char_indices().nth(MAX_LINE_CHARS) {
            Some((cut, _)) => format!("{}…", &text[..cut]),
            None => text.to_string(),
        };
        let key = meter.map(|m| (m.phase.clone(), m.remote));
        if let Some(key) = &key
            && let Some(l) = (self.output.iter_mut().rev().take(METER_LOOKBACK))
                .find(|l| l.open.as_ref() == Some(key))
        {
            l.text = text;
            if meter.is_some_and(|m| m.finished) {
                l.open = None;
            }
            return;
        }
        let open = key.filter(|_| meter.is_some_and(|m| !m.finished));
        self.output.push_back(OutputLine { text, open });
        while self.output.len() > MAX_OUTPUT_LINES {
            self.output.pop_front();
            self.dropped += 1;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.meter.is_none() && self.line.is_none()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// The meter as text (`Receiving objects 45% · 1.20 MiB`).
    pub fn meter_text(&self) -> Option<String> {
        self.meter.as_ref().map(Meter::text)
    }

    pub fn fraction(&self) -> Option<f32> {
        self.meter.as_ref().and_then(Meter::fraction)
    }

    /// One line for a status bar: `meter — last line`, either alone, or `None`.
    pub fn summary(&self) -> Option<String> {
        match (self.meter_text(), self.line.as_deref()) {
            (Some(m), Some(l)) => Some(format!("{m} — {l}")),
            (Some(m), None) => Some(m),
            (None, Some(l)) => Some(l.to_string()),
            (None, None) => None,
        }
    }

    /// The output lines kept, oldest first.
    pub fn output(&self) -> impl ExactSizeIterator<Item = &str> + DoubleEndedIterator {
        self.output.iter().map(|l| l.text.as_str())
    }

    /// Output line `ix` of [`Self::output`].
    pub fn output_line(&self, ix: usize) -> Option<&str> {
        self.output.get(ix).map(|l| l.text.as_str())
    }

    pub fn output_len(&self) -> usize {
        self.output.len()
    }

    /// Lines dropped from the front of the output (it keeps [`MAX_OUTPUT_LINES`]).
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// The note shown above the kept lines when older ones were dropped.
    pub fn dropped_note(&self) -> Option<String> {
        (self.dropped > 0).then(|| {
            format!(
                "… {} earlier lines not kept here (the Git Log gets the whole output when the \
                 operation ends)",
                self.dropped
            )
        })
    }

    /// Changes whenever a line was taken.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The whole output as one text (to copy), with the dropped-lines note first.
    pub fn output_text(&self) -> String {
        let mut s = String::new();
        if let Some(n) = self.dropped_note() {
            s.push_str(&n);
            s.push('\n');
        }
        for l in self.output() {
            s.push_str(l);
            s.push('\n');
        }
        s
    }
}

/// `text` without ANSI escape sequences (colours of hook output).
pub fn strip_ansi(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('\x1b') {
        return text.into();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: parameters, then a final byte in `@`..=`~`.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: up to BEL or ST (`ESC \`).
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' {
                        chars.next_if_eq(&'\\');
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out.into()
}

/// `text` without the progress git overwrites in place (`Rebasing (1/2)\r…`): of each line
/// only what follows its last carriage return.
pub fn without_progress(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    text.split_inclusive('\n')
        .map(|line| {
            let (body, end) = match line.strip_suffix('\n') {
                // A CRLF line ending is not progress.
                Some(b) => (b.strip_suffix('\r').unwrap_or(b), "\n"),
                None => (line, ""),
            };
            let last = body.rsplit('\r').next().unwrap_or(body);
            format!("{last}{end}")
        })
        .collect()
}

/// `text` (without `\r` updates) minus its progress meter lines: what git prints without
/// `--progress`.
pub fn strip_meters(text: &str) -> String {
    text.split_inclusive('\n')
        .filter(|l| parse_meter(l).is_none())
        .collect()
}

/// Splits git's stderr into lines as it arrives: `\r` rewrites the current line (progress
/// counters), `\n` ends it. Every non-empty line goes to the callback; the text kept is
/// each line's last state.
#[derive(Default)]
pub(crate) struct ProgressLines {
    /// Bytes of the line being received.
    part: Vec<u8>,
    /// Last complete state of the current line.
    current: String,
    text: String,
}

impl ProgressLines {
    pub(crate) fn feed(&mut self, bytes: &[u8], progress: &mut dyn FnMut(&str)) {
        for &b in bytes {
            match b {
                b'\r' | b'\n' => {
                    if !self.part.is_empty() {
                        self.current =
                            crate::git_cli::decode_native(&std::mem::take(&mut self.part));
                        // Leading blanks stay (`   1234567..89abcde  main -> main`).
                        let line = self.current.trim_end();
                        if !line.trim_start().is_empty() {
                            progress(line);
                        }
                    }
                    if b == b'\n' {
                        self.text.push_str(&std::mem::take(&mut self.current));
                        self.text.push('\n');
                    }
                }
                _ => self.part.push(b),
            }
        }
    }

    pub(crate) fn finish(mut self) -> String {
        if !self.part.is_empty() {
            self.current = crate::git_cli::decode_native(&self.part);
        }
        self.text.push_str(&self.current);
        self.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn progress_lines_keep_the_last_state() {
        let mut seen = Vec::new();
        let mut lines = ProgressLines::default();
        // Split across reads in awkward places, as a pipe delivers it.
        for chunk in [
            &b"Cloning into 'x'...\nRecei"[..],
            b"ving objects:  50% (1/2)\rReceiving objects: 100% (2/2), done.\r\n",
            b"warning: tail",
        ] {
            lines.feed(chunk, &mut |l| seen.push(l.to_string()));
        }
        assert_eq!(
            seen,
            [
                "Cloning into 'x'...",
                "Receiving objects:  50% (1/2)",
                "Receiving objects: 100% (2/2), done.",
            ]
        );
        assert_eq!(
            lines.finish(),
            "Cloning into 'x'...\nReceiving objects: 100% (2/2), done.\nwarning: tail"
        );
    }

    #[gitti_test::test]
    fn meters_of_fetch_push_and_rebase() {
        let m = parse_meter("Receiving objects:  45% (450/1000), 1.20 MiB | 2.00 MiB/s").unwrap();
        assert_eq!(
            (m.phase.as_str(), m.done, m.total, m.percent, m.finished),
            ("Receiving objects", 450, Some(1000), Some(45), false)
        );
        assert_eq!(m.detail.as_deref(), Some("1.20 MiB | 2.00 MiB/s"));
        assert_eq!(m.text(), "Receiving objects 45% · 1.20 MiB");
        assert_eq!(m.fraction(), Some(0.45));

        let m = parse_meter("remote: Counting objects: 1234, done.").unwrap();
        assert!(m.remote && m.finished && m.total.is_none() && m.percent.is_none());
        assert_eq!(m.text(), "remote: Counting objects 1234");

        let m =
            parse_meter("Writing objects: 100% (3/3), 250 bytes | 250.00 KiB/s, done.").unwrap();
        assert!(m.finished);
        assert_eq!(m.text(), "Writing objects 100% · 250 bytes");

        let m =
            parse_meter("Resolving deltas: 100% (1/1), completed with 1 local object.").unwrap();
        assert_eq!(m.detail.as_deref(), Some("completed with 1 local object."));
        assert_eq!(m.text(), "Resolving deltas 100%");

        let m = parse_meter("Rebasing (1/4)").unwrap();
        assert_eq!((m.phase.as_str(), m.percent), ("Rebasing", Some(25)));
        assert_eq!(m.text(), "Rebasing 25%");

        let m = parse_meter("Enumerating objects: 5, done.").unwrap();
        assert_eq!((m.done, m.finished), (5, true));
    }

    #[gitti_test::test]
    fn other_lines_are_not_meters() {
        for line in [
            "From /tmp/remote",
            " * branch            main       -> FETCH_HEAD",
            "To github.com:a/b.git",
            "   1234567..89abcde  main -> main",
            "remote: Total 3 (delta 0), reused 0 (delta 0), pack-reused 0",
            "error: 3 files failed",
            "running tests: 12 passed",
            "hint: Updates were rejected (1/2)",
            "Successfully rebased and updated refs/heads/main.",
            "lint: 50% (1/2) and more",
            "",
        ] {
            assert_eq!(parse_meter(line), None, "{line:?}");
        }
    }

    #[gitti_test::test]
    fn op_progress_keeps_meter_and_last_line() {
        let mut p = OpProgress::default();
        assert!(p.is_empty() && p.summary().is_none());
        p.feed("\x1b[32mlint ok\x1b[0m\n");
        assert_eq!(p.summary().as_deref(), Some("lint ok"));
        p.feed("Receiving objects:  50% (1/2)");
        p.feed("   ");
        assert_eq!(
            p.summary().as_deref(),
            Some("Receiving objects 50% — lint ok")
        );
        assert_eq!(p.fraction(), Some(0.5));
        p.clear();
        assert!(p.is_empty());
    }

    /// The live output window's lines — a meter's `\r` updates are one line in its
    /// final state (also when remote and local meters interleave), other lines in order,
    /// colours gone, leading blanks kept.
    #[gitti_test::test]
    fn output_keeps_every_line_with_meters_collapsed() {
        let mut p = OpProgress::default();
        for l in [
            "From ../remote",
            "remote: Counting objects:  50% (2/4)",
            "remote: Counting objects: 100% (4/4), done.",
            "Receiving objects:  10% (1/10)",
            "remote: Total 4 (delta 0)",
            "Receiving objects:  60% (6/10), 1.00 MiB | 1.00 MiB/s",
            "\x1b[32mhook says hi\x1b[0m",
            "Receiving objects: 100% (10/10), 2.00 MiB | 1.00 MiB/s, done.",
            "Rebasing (1/2)",
            "Rebasing (2/2)",
            "   1234567..89abcde  main -> origin/main",
        ] {
            p.feed(l);
        }
        let out: Vec<&str> = p.output().collect();
        assert_eq!(
            out,
            [
                "From ../remote",
                "remote: Counting objects: 100% (4/4), done.",
                "Receiving objects: 100% (10/10), 2.00 MiB | 1.00 MiB/s, done.",
                "remote: Total 4 (delta 0)",
                "hook says hi",
                "Rebasing (2/2)",
                "   1234567..89abcde  main -> origin/main",
            ]
        );
        assert_eq!(p.output_line(4), Some("hook says hi"));
        assert_eq!(p.output_len(), 7);
        assert_eq!(p.line.as_deref(), Some("1234567..89abcde  main -> origin/main"));
        // A finished meter is closed: the same phase again (the next remote of
        // `fetch --all`) is a line of its own.
        p.feed("Receiving objects:  50% (1/2)");
        assert_eq!(p.output_len(), 8);
        let g = p.generation();
        p.feed("  ");
        assert_eq!(p.generation(), g, "blank lines are not output");
        assert!(p.output_text().ends_with("Receiving objects:  50% (1/2)\n"));
        p.clear();
        assert_eq!((p.output_len(), p.dropped()), (0, 0));
    }

    #[gitti_test::test]
    fn output_is_bounded() {
        let mut p = OpProgress::default();
        for i in 0..MAX_OUTPUT_LINES + 3 {
            p.feed(&format!("line {i}"));
        }
        assert_eq!((p.output_len(), p.dropped()), (MAX_OUTPUT_LINES, 3));
        assert_eq!(p.output().next(), Some("line 3"));
        assert!(p.output_text().starts_with("… 3 earlier lines"), "{}", p.output_text());
        p.feed(&"x".repeat(MAX_LINE_CHARS + 10));
        assert_eq!(p.output().last().map(|l| l.chars().count()), Some(MAX_LINE_CHARS + 1));
    }

    /// The status bar's columns keep their width while the numbers change, and the
    /// bar never goes back within a phase.
    #[gitti_test::test]
    fn status_columns_are_steady() {
        let mut p = OpProgress::default();
        let mut widths = Vec::new();
        let mut fills = Vec::new();
        for l in [
            "Receiving objects:   0% (0/1000)",
            "Receiving objects:   5% (50/1000), 12.00 KiB | 10.00 KiB/s",
            "Receiving objects:  45% (450/1000), 1.20 MiB | 2.00 MiB/s",
            // Out of order (never seen from git, but the bar must not jump back).
            "Receiving objects:  40% (400/1000), 1.10 MiB | 2.00 MiB/s",
            "Receiving objects: 100% (1000/1000), 12.34 MiB | 2.00 MiB/s, done.",
        ] {
            p.feed(l);
            let m = p.meter.as_ref().unwrap();
            widths.push(m.percent_text().chars().count());
            let bar = bar_cells(p.fraction(), 10);
            assert_eq!(bar.chars().count(), 10, "{bar:?}");
            fills.push(bar.chars().filter(|&c| c != ' ').count());
        }
        assert!(widths.iter().all(|&w| w == 4), "{widths:?}");
        assert!(fills.windows(2).all(|w| w[0] <= w[1]), "{fills:?}");
        assert_eq!(p.meter.as_ref().unwrap().percent, Some(100));
        assert_eq!(p.meter.as_ref().unwrap().amount(), "12.34 MiB");
        p.feed("Resolving deltas:  20% (1/5)");
        assert_eq!(p.meter.as_ref().unwrap().percent_text(), " 20%");
        assert_eq!(p.meter.as_ref().unwrap().amount(), "1/5");
        p.feed("remote: Counting objects: 1234");
        let m = p.meter.as_ref().unwrap();
        assert_eq!((m.percent_text().as_str(), m.amount().as_str()), ("    ", "1234"));
        assert_eq!(bar_cells(None, 6), "      ");
        assert_eq!(bar_cells(Some(1.), 4), "████");
        assert_eq!(bar_cells(Some(0.5), 3), "█▌ ");
    }

    #[gitti_test::test]
    fn ansi_and_carriage_returns_are_dropped() {
        assert_eq!(strip_ansi("\x1b[1;31mred\x1b[0m plain"), "red plain");
        assert_eq!(strip_ansi("\x1b]8;;http://x\x07link\x1b]8;;\x1b\\"), "link");
        assert_eq!(
            without_progress("Rebasing (1/2)\rRebasing (2/2)\rSuccessfully rebased.\nnext\n"),
            "Successfully rebased.\nnext\n"
        );
        assert_eq!(
            without_progress("Already up to date.\n"),
            "Already up to date.\n"
        );
        assert_eq!(without_progress("a\rb"), "b");
        assert_eq!(without_progress("one\r\ntwo\r\n"), "one\ntwo\n");
        // A captured `git fetch --progress` stderr.
        let raw = "From ../remote\nremote: Enumerating objects: 4, done.\nremote: Counting \
                   objects:  50% (2/4)\rremote: Counting objects: 100% (4/4), done.\n\
                   Unpacking objects:  33% (1/3)\rUnpacking objects: 100% (3/3), 264 bytes \
                   | 132.00 KiB/s, done.\n   1234567..89abcde  main       -> origin/main\n";
        let clean = without_progress(raw);
        assert!(
            clean.contains("Unpacking objects: 100% (3/3), 264 bytes"),
            "{clean}"
        );
        assert!(!clean.contains("33%") && !clean.contains('\r'), "{clean}");
        assert_eq!(
            strip_meters(&clean),
            "From ../remote\n   1234567..89abcde  main       -> origin/main\n"
        );
    }
}
