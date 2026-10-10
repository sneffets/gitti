//! gitty — terminal UI for git on top of `gitti-core`, with gitti's look and keys.

mod app;
mod clipboard;
mod edit;
mod graph;
mod input;
mod kb;
mod merge;
mod message_view;
mod modal;
mod script;
mod sidebar;
mod theme;
mod ui;

use std::io::{BufWriter, Stdout, Write as _, stdout};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::sync::mpsc::{self, Receiver};

use anyhow::{Context as _, Result};
use gitti_core::Repo;
use gitti_core::askpass::AskpassServer;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::terminal::{
    BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate, EnterAlternateScreen,
};
use ratatui::crossterm::{execute, queue};

use crate::app::{App, Msg};

const USAGE: &str = "\
gitty — terminal git client (gitti's TUI sibling)

usage: gitty [PATH]    open the repository containing PATH (default: current directory)

Press ? inside for the key bindings, ctrl-p for all commands.

Colours: 24-bit when COLORTERM=truecolor, 16 under TERM=linux / *-16color, else 256;
GITTY_COLORS=truecolor|256|16 overrides, NO_COLOR turns colours off.";

fn main() {
    // The binary doubles as git's askpass helper; must come first.
    if let Some(code) = gitti_core::askpass::run_client_if_requested() {
        std::process::exit(code);
    }
    // Panics also go to <config dir>/logs/panic.log (the terminal is restored first, see
    // `run_terminal`, whose hook runs this one afterwards).
    gitti_core::panic_log::install("gitty", env!("CARGO_PKG_VERSION"));
    // Many git processes at once (refresh); a low open-file limit (`fd_limit`).
    gitti_core::fd_limit::raise();
    // Command logs from before their redaction go, once per machine.
    gitti_core::redact::purge_unredacted_logs_in_config_dir();
    let result = run();
    // Settings writes are queued on a background thread: let the last ones land.
    gitti_core::store::flush();
    // Quit before the start was complete: keep what was measured.
    gitti_core::startup::abandon();
    if let Err(e) = result {
        eprintln!("gitty: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        Some("-h" | "--help") => {
            println!("{USAGE}");
            return Ok(());
        }
        Some("-V" | "--version") => {
            let (version, build) = (env!("CARGO_PKG_VERSION"), env!("GITTI_BUILD"));
            println!("{}", gitti_core::about::version_line("gitty", version, build));
            return Ok(());
        }
        _ => {}
    }
    let script = std::env::var("GITTY_SCRIPT").ok();
    // Start-up timings; scripted runs would only bury the real starts.
    if script.is_none() {
        gitti_core::startup::begin("gitty", env!("CARGO_PKG_VERSION"));
    }
    let path = arg.unwrap_or_else(|| ".".into());
    let mut repo = match Repo::open(&path) {
        Ok(repo) => repo,
        // A bare repository / no git: the message says what to do instead.
        Err(e) => match e.downcast_ref::<gitti_core::OpenError>() {
            Some(kind) => anyhow::bail!("{kind}"),
            None => return Err(e.context(format!("opening repository at {path}"))),
        },
    };

    let (tx, rx) = mpsc::channel::<Msg>();
    // Credential prompts (ssh passphrases, https passwords, host keys) come back to us
    // through the askpass helper and are answered in a dialog.
    let ask_tx = tx.clone();
    let askpass = AskpassServer::start(Arc::new(move |prompt| {
        let (reply_tx, reply_rx) = mpsc::channel();
        let asker = prompt.asker.clone();
        ask_tx.send(Msg::Ask(prompt, reply_tx)).ok()?;
        // A helper that went away (operation cancelled or over): its dialog closes.
        asker.wait(&reply_rx, || {
            let _ = ask_tx.send(Msg::AskGone);
        })
    }))?;
    repo.set_env(askpass.env());
    gitti_core::startup::mark("repository opened, askpass ready");
    let _ = gitti_core::startup::expect_tabs(1);
    let repo = Arc::new(repo);
    // Same log file as gitti, so both show each other's commands.
    repo.command_log()
        .set_file(gitti_core::paths::log_file_for(repo.workdir()));

    let fs_tx = tx.clone();
    let _watcher = gitti_core::watch::watch(repo.workdir(), move |c| {
        let _ = fs_tx.send(Msg::Fs(c));
    })
    .ok();

    let mut app = App::new(repo, tx.clone());
    app.pal.depth = theme::ColorDepth::detect(script.is_some());
    if let Some(script) = script {
        return script::run(app, rx, &script);
    }
    run_terminal(app, tx, rx)
}

fn run_terminal(mut app: App, tx: mpsc::Sender<Msg>, rx: Receiver<Msg>) -> Result<()> {
    let enhanced = matches!(
        ratatui::crossterm::terminal::supports_keyboard_enhancement(),
        Ok(true)
    );
    // The query waits for the terminal's answer (long in terminals that never answer).
    gitti_core::startup::mark("terminal keyboard query");
    enter_tui(enhanced)?;
    let mut terminal = new_terminal()?;
    gitti_core::startup::mark("terminal ready");
    let restore = move || leave_tui(enhanced);
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        hook(info);
    }));

    // Paused while a terminal editor owns the terminal (it must get the keys, not us).
    let paused = Arc::new(AtomicBool::new(false));
    let input_paused = paused.clone();
    std::thread::Builder::new()
        .name("gitty-input".into())
        .spawn(move || {
            loop {
                if input_paused.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
                match event::poll(Duration::from_millis(50)) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(_) => break,
                }
                let Ok(ev) = event::read() else { break };
                if tx.send(Msg::Term(ev)).is_err() {
                    break;
                }
            }
        })?;

    app.start();
    let result = event_loop(&mut terminal, &mut app, &rx, &paused, enhanced);
    restore();
    // The typed commit message is kept for the next start.
    app.save_draft_now();
    // A clone / init asked to switch: start over in the new repository.
    if result.is_ok()
        && let Some(path) = app.reopen.take()
    {
        app::reopen(&path).with_context(|| format!("restarting gitty in {}", path.display()))?;
    }
    result
}

/// Raw mode, alternate screen, mouse / focus / paste reporting (+ kitty keyboard flags).
fn enter_tui(enhanced: bool) -> Result<()> {
    ratatui::crossterm::terminal::enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    let _ = execute!(
        stdout(),
        EnableMouseCapture,
        EnableFocusChange,
        EnableBracketedPaste
    );
    if enhanced {
        // Distinguishes ctrl-enter / alt-enter etc. where the terminal supports it.
        let _ = execute!(
            stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
    }
    Ok(())
}

fn leave_tui(enhanced: bool) {
    if enhanced {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        stdout(),
        DisableMouseCapture,
        DisableFocusChange,
        DisableBracketedPaste
    );
    ratatui::restore();
}

/// Runs a terminal editor (vim, nvim, …) with the terminal handed over, then takes it back.
fn run_terminal_editor(
    terminal: &mut Tui,
    app: &mut App,
    paused: &AtomicBool,
    enhanced: bool,
    argv: &[String],
    dir: &std::path::Path,
) -> Result<()> {
    paused.store(true, Ordering::SeqCst);
    // Let the input thread leave its poll before the editor reads the keyboard.
    std::thread::sleep(Duration::from_millis(80));
    leave_tui(enhanced);
    let status = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(dir)
        .status();
    enter_tui(enhanced)?;
    // A fresh terminal redraws everything; `Terminal::clear` would query the cursor position.
    execute!(stdout(), Clear(ClearType::All))?;
    *terminal = new_terminal()?;
    paused.store(false, Ordering::SeqCst);
    app.terminal_editor_done(&argv[0], status);
    Ok(())
}

type Tui = Terminal<CrosstermBackend<BufWriter<Stdout>>>;

/// One buffered write per frame: the terminal gets the whole frame at once instead of ~1 KB
/// chunks from the line-buffered stdout (matters most on terminals without synchronized
/// output, e.g. cmd / conhost).
fn new_terminal() -> Result<Tui> {
    Ok(Terminal::new(CrosstermBackend::new(BufWriter::with_capacity(1 << 20, stdout())))?)
}

fn event_loop(
    terminal: &mut Tui,
    app: &mut App,
    rx: &Receiver<Msg>,
    paused: &AtomicBool,
    enhanced: bool,
) -> Result<()> {
    let mut first_frame = true;
    loop {
        // Synchronized output: the terminal shows the frame at once instead of painting it
        // top to bottom while the bytes arrive (terminals without support ignore it). Begin,
        // frame and end leave in one buffered write.
        queue!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
        terminal.draw(|f| ui::draw(f, app))?;
        queue!(terminal.backend_mut(), EndSynchronizedUpdate)?;
        terminal.backend_mut().flush()?;
        if std::mem::take(&mut first_frame)
            && let Some(record) = gitti_core::startup::first_frame()
        {
            // Completed the start (the repository was read before the first frame).
            app.startup_finished(&record);
            continue;
        }
        let msg = rx.recv()?;
        app.handle(msg);
        // Coalesce bursts (key repeat, fs events) into one frame.
        while let Ok(msg) = rx.try_recv() {
            app.handle(msg);
        }
        if let Some((argv, dir)) = app.run_in_terminal.take() {
            run_terminal_editor(terminal, app, paused, enhanced, &argv, &dir)?;
        }
        if app.quit || app.reopen.is_some() {
            return Ok(());
        }
    }
}
