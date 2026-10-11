//! Headless scripted runs for tests and checks: `GITTY_SCRIPT="key:j;key:s;dump"` renders
//! into an in-memory terminal (`GITTY_SIZE=WxH`, default 160x45) instead of the real one.
//!
//! Steps: `wait` (until no job is running), `sleep:MS`, `closed` (until no dialog is open —
//! one that closes by itself; at most 5 s), `key:SPEC` (e.g. `j`, `S`, `enter`,
//! `esc`, `tab`, `space`, `ctrl-p`, `alt-down`, `shift-up`, `f5`), `press:SPEC` (a key
//! without waiting for the jobs it starts — e.g. Esc on a running operation), `type:TEXT`,
//! `click:X,Y`, `rclick:X,Y`, `pressclick:X,Y` (a left click without waiting for the jobs
//! — e.g. on a running operation's status line), `wheel:X,Y,DY`, `answer:TEXT` (askpass), `dump` (screen as
//! text, after a `=== dump ===` line), `cells:Y` (colour runs of one screen row), `state` (selection, focus, status),
//! `changes` (Search in Changes: query, toggles, scans, hits with why, note), `reflog`
//! (the reflog view: title, note, selection, shown commit, entries),
//! `write:PATH=TEXT` (writes a working-tree file as another program would; `\n` = newline),
//! `clip:TEXT` (the clipboard's text as the Clone dialog reads it — a scripted run never
//! reads the system clipboard, and copies land there too), `paste:TEXT` (a bracketed paste),
//! `focus` (terminal focus gained), `cmds` (the git commands run since the previous `cmds`:
//! a `cmds: N` line, then one `  git …` line each), `items` (the entries of the open picker /
//! menu: one `item: LABEL` line each; after a typed query the matches, best first), `bar`
//! (the status line's context key hints, all pages: `bar: PAGE/PAGES`, then one
//! `hint: KEY | LABEL` line each, `hint (dim): …` for a greyed entry), `resize:WxH` (the terminal changes size, as on
//! SIGWINCH), `check` (screen invariants: `check: ok` or `check: N issues` and one `  …`
//! line per issue — see [`check`]), `quit`.
//!
//! No desktop program starts (`gitti_core::desktop::sandbox`: editors only by absolute
//! path, the tests' fakes). A whole run is limited to 10 s (`GITTY_SCRIPT_LIMIT=SECS`): past it gitty prints the step
//! it hangs in and exits with 124 — a hung test fails fast instead of spinning. A key that quits gitty ends the run with a
//! `(quit)` line. A terminal editor gitty would hand the terminal to (`$VISUAL=vim`) runs
//! without a terminal (stdin / stdout null) after a `(terminal editor: ARGV)` line.

use std::sync::Mutex;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::app::{App, Msg};
use crate::modal::Modal;

/// The step a scripted run is at, for [`watchdog`].
static STEP: Mutex<String> = Mutex::new(String::new());

/// Ends the process when the run takes longer than its limit (see the module docs).
fn watchdog() {
    let limit = std::env::var("GITTY_SCRIPT_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(limit));
        let step = STEP.lock().map(|s| s.clone()).unwrap_or_default();
        eprintln!("gitty script: over the {limit}s limit, hanging in step `{step}`");
        std::process::exit(124);
    });
}

pub fn run(mut app: App, rx: Receiver<Msg>, script: &str) -> Result<()> {
    let result = run_steps(&mut app, rx, script);
    // Like the terminal loop's end: the typed message is kept.
    app.save_draft_now();
    result
}

fn run_steps(app: &mut App, rx: Receiver<Msg>, script: &str) -> Result<()> {
    watchdog();
    // Nothing a script triggers opens windows on the user's desktop (editor, file manager).
    gitti_core::desktop::sandbox();
    *STEP.lock().unwrap() = "start".into();
    let (w, h) = std::env::var("GITTY_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((160u16, 45u16));
    let mut term = Terminal::new(TestBackend::new(w, h))?;
    app.start();
    settle(app, &rx, &mut term, Duration::from_secs(20))?;
    let mut cmds_seen = 0;
    for step in script.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        *STEP.lock().unwrap() = step.chars().take(60).collect();
        let (cmd, arg) = step.split_once(':').unwrap_or((step, ""));
        match cmd {
            "wait" => settle(app, &rx, &mut term, Duration::from_secs(20))?,
            "sleep" => {
                let until = Instant::now() + Duration::from_millis(arg.parse()?);
                while Instant::now() < until {
                    pump(app, &rx, Duration::from_millis(10));
                }
                term.draw(|f| crate::ui::draw(f, app))?;
            }
            "closed" => {
                let until = Instant::now() + Duration::from_secs(5);
                while app.modal.is_some() && Instant::now() < until {
                    pump(app, &rx, Duration::from_millis(10));
                }
                term.draw(|f| crate::ui::draw(f, app))?;
            }
            "key" => {
                app.on_event(Event::Key(parse_key(arg)?));
                settle(app, &rx, &mut term, Duration::from_secs(20))?;
            }
            "press" => {
                app.on_event(Event::Key(parse_key(arg)?));
                pump(app, &rx, Duration::from_millis(1));
                term.draw(|f| crate::ui::draw(f, app))?;
            }
            "pressclick" => {
                let (x, y) = arg
                    .split_once(',')
                    .ok_or_else(|| anyhow!("pressclick:X,Y"))?;
                app.on_event(Event::Mouse(MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: x.trim().parse()?,
                    row: y.trim().parse()?,
                    modifiers: KeyModifiers::NONE,
                }));
                pump(app, &rx, Duration::from_millis(1));
                term.draw(|f| crate::ui::draw(f, app))?;
            }
            "paste" => {
                app.on_event(Event::Paste(arg.to_string()));
                settle(app, &rx, &mut term, Duration::from_secs(20))?;
            }
            "type" => {
                for ch in arg.chars() {
                    app.on_event(Event::Key(KeyEvent::new(
                        KeyCode::Char(ch),
                        KeyModifiers::NONE,
                    )));
                }
                settle(app, &rx, &mut term, Duration::from_secs(20))?;
            }
            "click" | "rclick" | "wheel" => {
                let n: Vec<i32> = arg
                    .split(',')
                    .map(|v| v.trim().parse())
                    .collect::<Result<_, _>>()?;
                let (x, y) = (n[0] as u16, n[1] as u16);
                let kind = match cmd {
                    "click" => MouseEventKind::Down(MouseButton::Left),
                    "rclick" => MouseEventKind::Down(MouseButton::Right),
                    _ if n.get(2).copied().unwrap_or(1) > 0 => MouseEventKind::ScrollDown,
                    _ => MouseEventKind::ScrollUp,
                };
                app.on_event(Event::Mouse(MouseEvent {
                    kind,
                    column: x,
                    row: y,
                    modifiers: KeyModifiers::NONE,
                }));
                settle(app, &rx, &mut term, Duration::from_secs(20))?;
            }
            "drag" => {
                // drag:X1,Y1,X2,Y2 — press at 1, move to 2, release.
                let n: Vec<u16> = arg
                    .split(',')
                    .map(|v| v.trim().parse())
                    .collect::<Result<_, _>>()?;
                let at = |kind, x, y| {
                    Event::Mouse(MouseEvent {
                        kind,
                        column: x,
                        row: y,
                        modifiers: KeyModifiers::NONE,
                    })
                };
                app.on_event(at(MouseEventKind::Down(MouseButton::Left), n[0], n[1]));
                app.on_event(at(MouseEventKind::Drag(MouseButton::Left), n[2], n[3]));
                app.on_event(at(MouseEventKind::Up(MouseButton::Left), n[2], n[3]));
                settle(app, &rx, &mut term, Duration::from_secs(20))?;
            }
            "write" => {
                let (path, text) = arg
                    .split_once('=')
                    .ok_or_else(|| anyhow!("write:PATH=TEXT"))?;
                let full = gitti_core::paths::join_repo_path(app.repo.workdir(), path);
                std::fs::write(full, text.replace("\\n", "\n"))?;
            }
            "clip" => App::set_script_clipboard(arg),
            "focus" => {
                app.on_event(Event::FocusGained);
                settle(app, &rx, &mut term, Duration::from_secs(20))?;
            }
            "answer" => {
                if let Some(Modal::Ask { .. }) = app.modal {
                    app.answer_ask(Some(arg.to_string()));
                } else {
                    return Err(anyhow!("answer: no askpass prompt open"));
                }
                settle(app, &rx, &mut term, Duration::from_secs(20))?;
            }
            "dump" => {
                term.draw(|f| crate::ui::draw(f, app))?;
                // A line of its own to split at: a dialog can cover any of the screen's rows.
                println!("=== dump ===\n{}", screen_text(term.backend().buffer()));
            }
            "cells" => {
                term.draw(|f| crate::ui::draw(f, app))?;
                println!("{}", cell_runs(term.backend().buffer(), arg.parse()?));
            }
            "resize" => {
                let (w, h) = arg
                    .split_once('x')
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                    .ok_or_else(|| anyhow!("resize:WxH"))?;
                term.backend_mut().resize(w, h);
                app.on_event(Event::Resize(w, h));
                settle(app, &rx, &mut term, Duration::from_secs(20))?;
            }
            "check" => {
                // The frame as drawn: the backend's copy keeps stale symbols under wide
                // ones (the diff skips covered cells; a terminal hides them the same way).
                let mut issues = Vec::new();
                term.draw(|f| {
                    crate::ui::draw(f, app);
                    issues = check(f.buffer_mut(), app);
                })?;
                if issues.is_empty() {
                    println!("check: ok");
                } else {
                    println!("check: {} issues", issues.len());
                    for i in issues {
                        println!("  {i}");
                    }
                }
            }
            "state" => println!("{}", app.debug_state()),
            "bar" => {
                println!("bar: {}/{}", app.keybar.page + 1, app.keybar.pages);
                for h in app.keybar_hints() {
                    let dim = if h.dim { " (dim)" } else { "" };
                    println!("hint{dim}: {} | {}", h.key, h.label);
                }
            }
            "reflog" => println!("{}", app.reflog_debug()),
            "changes" => println!(
                "{}",
                app.change_search_debug().unwrap_or_else(|| "changes: closed".into())
            ),
            "items" => match &app.modal {
                // With a typed query: the matches, best first (as the picker lists them).
                Some(Modal::Picker {
                    items,
                    filtered,
                    query,
                    search: false,
                    ..
                }) if !query.text().is_empty() => {
                    let shown: Vec<_> = filtered.iter().filter_map(|&i| items.get(i)).collect();
                    for it in &shown {
                        println!("item: {}", it.label);
                    }
                    print_off(shown.iter().map(|it| (&it.label, &it.disabled)));
                }
                Some(Modal::Picker { items, .. }) => {
                    for it in items {
                        println!("item: {}", it.label);
                    }
                    print_off(items.iter().map(|it| (&it.label, &it.disabled)));
                }
                Some(Modal::Menu { items, .. }) => {
                    for it in items.iter().filter(|i| i.action.is_some()) {
                        println!("item: {}", it.label);
                    }
                    print_off(items.iter().map(|it| (&it.label, &it.disabled)));
                }
                _ => println!("item: (no list open)"),
            },
            "cmds" => {
                let entries: Vec<_> = app
                    .repo
                    .command_log()
                    .snapshot()
                    .into_iter()
                    // git processes only (not work gitti did itself, `in_process`).
                    .filter(|e| !e.previous_session && !e.in_process)
                    .collect();
                let new = &entries[cmds_seen.min(entries.len())..];
                println!("cmds: {}", new.len());
                for e in new {
                    println!("  git {}", e.args.join(" "));
                }
                cmds_seen = entries.len();
            }
            "quit" => return Ok(()),
            other => return Err(anyhow!("unknown script step {other:?}")),
        }
        // Like `App::handle` after every message: view options go to the database.
        app.save_settings();
        // Like the terminal loop, without handing over a terminal.
        if let Some((argv, dir)) = app.run_in_terminal.take() {
            println!("(terminal editor: {})", argv.join(" "));
            let status = std::process::Command::new(&argv[0])
                .args(&argv[1..])
                .current_dir(dir)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .status();
            app.terminal_editor_done(&argv[0], status);
            settle(app, &rx, &mut term, Duration::from_secs(20))?;
        }
        // Like the terminal loop: a quit key / command ends the run.
        if app.quit {
            println!("(quit)");
            return Ok(());
        }
        // The terminal loop would restart gitty in the new repository here.
        if let Some(path) = app.reopen.take() {
            println!("(reopen {})", path.display());
            return Ok(());
        }
    }
    Ok(())
}

/// Handles messages for up to `wait`; whether anything but a [`Msg::Tick`] (or the draft's
/// or a commit load's timer) came. A tick only redraws (the busy label shows after `BUSY_SHOW_DELAY`): counted
/// as activity, it kept every operation's [`settle`] waiting for it (~0.4 s instead of
/// ~0.15 s).
fn pump(app: &mut App, rx: &Receiver<Msg>, wait: Duration) -> bool {
    match rx.recv_timeout(wait) {
        Ok(msg) => {
            let mut active = !matches!(msg, Msg::Tick | Msg::DraftDue | Msg::DetailSlow(_));
            app.handle(msg);
            while let Ok(msg) = rx.try_recv() {
                active |= !matches!(msg, Msg::Tick | Msg::DraftDue | Msg::DetailSlow(_));
                app.handle(msg);
            }
            active
        }
        Err(_) => false,
    }
}

/// How long a step waits for nothing more to arrive: after a git command that changed
/// something, long enough for the file watcher's echo of the write (its debounce is
/// 100 ms); a step that only read (loading, moving, scrolling, opening a menu) has no echo
/// to wait for (most steps, several times faster).
const QUIET_AFTER_WRITE: Duration = Duration::from_millis(150);
const QUIET_READ_ONLY: Duration = Duration::from_millis(25);

/// Runs until no background job is pending and nothing arrived for a moment.
fn settle(
    app: &mut App,
    rx: &Receiver<Msg>,
    term: &mut Terminal<TestBackend>,
    timeout: Duration,
) -> Result<()> {
    let start = Instant::now();
    let started_at = std::time::SystemTime::now();
    let mut quiet_since = Instant::now();
    loop {
        term.draw(|f| crate::ui::draw(f, app))?;
        if pump(app, rx, Duration::from_millis(10)) {
            quiet_since = Instant::now();
        }
        // A credential prompt blocks the running job until the script answers it.
        let asking = matches!(app.modal, Some(Modal::Ask { .. }));
        let wrote = app
            .repo
            .command_log()
            .shared()
            .iter()
            .rev()
            .take_while(|e| e.at >= started_at)
            .any(|e| !e.read_only);
        let quiet = if wrote { QUIET_AFTER_WRITE } else { QUIET_READ_ONLY };
        if (app.is_idle() || asking) && quiet_since.elapsed() > quiet {
            term.draw(|f| crate::ui::draw(f, app))?;
            return Ok(());
        }
        if start.elapsed() > timeout {
            return Err(anyhow!("timeout waiting for gitty to settle"));
        }
    }
}

pub fn screen_text(buf: &Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut line = String::new();
        let mut skip = 0;
        for x in 0..buf.area.width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let sym = buf[(x, y)].symbol();
            line.push_str(sym);
            skip = unicode_width::UnicodeWidthStr::width(sym).saturating_sub(1);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// Screen invariants a text dump can't show: a wide symbol (CJK, emoji) must fit
/// and the cells it covers must be blank — whatever is written there stays invisible on a
/// real terminal; no cell may hold a control character (a tab moves the terminal's cursor)
/// or a zero-width symbol; the separators between panes stay blank (nothing spilled over);
/// no pane drew outside its area (clipped, but a layout bug: `ui::clipped`); every pane
/// lies on the screen and the status bar is its last row.
pub fn check(buf: &Buffer, app: &App) -> Vec<String> {
    use unicode_width::UnicodeWidthStr as _;
    let mut out = Vec::new();
    let area = buf.area;
    for y in 0..area.height {
        let mut x = 0;
        while x < area.width {
            let sym = buf[(x, y)].symbol();
            let w = sym.width() as u16;
            if sym.chars().any(char::is_control) {
                out.push(format!("{x},{y}: control character in {sym:?}"));
            } else if w == 0 && !sym.is_empty() {
                out.push(format!("{x},{y}: zero-width symbol {sym:?}"));
            }
            if w > 1 {
                if x + w > area.width {
                    out.push(format!("{x},{y}: {sym:?} cut at the right edge"));
                }
                for cx in x + 1..(x + w).min(area.width) {
                    let hidden = buf[(cx, y)].symbol();
                    if hidden != " " {
                        out.push(format!("{cx},{y}: {hidden:?} hidden under {sym:?}"));
                    }
                }
            }
            x += w.max(1);
        }
    }
    // Mapped down to 256 / 16 colours: no text may end up in its background's colour.
    if app.pal.depth != crate::theme::ColorDepth::TrueColor {
        for y in 0..area.height {
            for x in 0..area.width {
                let c = &buf[(x, y)];
                if !c.symbol().trim().is_empty() && c.fg == c.bg {
                    out.push(format!("{x},{y}: {:?} invisible ({:?} on {:?})", c.symbol(), c.fg, c.bg));
                }
            }
        }
    }
    let a = &app.areas;
    if app.modal.is_none() {
        for sep in [a.sep_sidebar, a.sep_list].into_iter().flatten() {
            for y in a.body.y..a.body.bottom() {
                let sym = buf[(sep, y)].symbol();
                if sym != " " {
                    out.push(format!("{sep},{y}: {sym:?} on the pane separator"));
                }
            }
        }
    }
    if let Some((pane, x, y, ch)) = a.spill {
        out.push(format!("{x},{y}: the {pane} pane drew {ch:?} outside its area"));
    }
    let panes = [
        ("body", a.body),
        ("sidebar", a.sidebar),
        ("commits", a.commits),
        ("detail", a.detail),
        ("message", a.message),
        ("log", a.log),
        ("status", a.status),
    ];
    for (name, r) in panes {
        if !r.is_empty() && r.intersection(area) != r {
            out.push(format!("{name} {r:?} outside the screen {area:?}"));
        }
    }
    if area.height >= 4 && (a.status.height != 1 || a.status.bottom() != area.bottom()) {
        out.push(format!("status bar {:?} is not the last row", a.status));
    }
    out
}

/// `x..x fg/bg "text"` runs of equal style in row `y`.
fn cell_runs(buf: &Buffer, y: u16) -> String {
    let mut out = String::new();
    let mut x = 0;
    while x < buf.area.width {
        let c = &buf[(x, y)];
        let (fg, bg, m) = (c.fg, c.bg, c.modifier);
        let start = x;
        let mut text = String::new();
        while x < buf.area.width {
            let c = &buf[(x, y)];
            if (c.fg, c.bg, c.modifier) != (fg, bg, m) {
                break;
            }
            text.push_str(c.symbol());
            x += 1;
        }
        out.push_str(&format!(
            "{start:>3}-{:<3} fg={fg:?} bg={bg:?} {m:?} {text:?}\n",
            x - 1
        ));
    }
    out
}

pub fn parse_key(spec: &str) -> Result<KeyEvent> {
    let mut mods = KeyModifiers::NONE;
    let mut rest = spec;
    while let Some((m, r)) = rest.split_once('-') {
        if r.is_empty() {
            break;
        }
        match m {
            "ctrl" => mods |= KeyModifiers::CONTROL,
            "alt" => mods |= KeyModifiers::ALT,
            "shift" => mods |= KeyModifiers::SHIFT,
            // ⌘ through the kitty keyboard protocol.
            "super" => mods |= KeyModifiers::SUPER,
            _ => break,
        }
        rest = r;
    }
    let code = match rest {
        "enter" => KeyCode::Enter,
        "esc" => KeyCode::Esc,
        "tab" if mods.contains(KeyModifiers::SHIFT) => KeyCode::BackTab,
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "space" => KeyCode::Char(' '),
        "backspace" => KeyCode::Backspace,
        "delete" => KeyCode::Delete,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "semicolon" => KeyCode::Char(';'),
        s if s.len() > 1 && s.starts_with('f') && s[1..].parse::<u8>().is_ok() => {
            KeyCode::F(s[1..].parse()?)
        }
        s if s.chars().count() == 1 => KeyCode::Char(s.chars().next().unwrap_or(' ')),
        other => return Err(anyhow!("unknown key {other:?}")),
    };
    Ok(KeyEvent::new(code, mods))
}

/// After an `items` list (so its `item:` lines stay one run): `off: LABEL = REASON` for
/// each greyed entry.
fn print_off<'a>(items: impl Iterator<Item = (&'a String, &'a Option<String>)>) {
    for (label, why) in items {
        if let Some(why) = why {
            println!("off: {label} = {why}");
        }
    }
}
