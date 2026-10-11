//! Fault injection for the git runner ([`crate::process::execute`]), for tests: what a
//! busy machine or a broken environment does to a git command — a pipe reader thread
//! scheduled late, a killed child, output that ends early with exit code 0, git that
//! can't be started, a failure without a message. Compiled into debug builds only (tests,
//! `cargo run` without `--release`); a release build has no hook at all.
//!
//! Three ways in:
//! - [`inject`]: a rule for the commands of one repository (tests run side by side in one
//!   process, each in its own temporary directory), removed when its [`Injection`] drops;
//! - `GITTI_TEST_FAULT` ([`ENV_FAULT`]): rules for every command of the process, for a
//!   scripted gitty / gitti run started by a test — `show=kill*1,cat-file=cut`, i.e.
//!   `<git subcommand or program>=<fault>[*<times>]`, faults `slow:<ms>`, `cut`, `kill`,
//!   `truncate:<bytes>`, `spawn`, `exit:<code>` (see [`Fault`]);
//! - `GITTI_TEST_STRESS=1` ([`ENV_STRESS`]): every command's readers start late now and
//!   then (up to [`STRESS_MAX_DELAY`], well inside the runner's grace) and two threads keep
//!   the CPU half busy — the normal suites run under it now and then, never per merge.

use std::path::Path;
use std::time::Duration;

/// Rules for every git command of the process (see the module docs).
pub const ENV_FAULT: &str = "GITTI_TEST_FAULT";
/// Stress mode (see the module docs).
pub const ENV_STRESS: &str = "GITTI_TEST_STRESS";
/// The longest reader delay the stress mode adds.
pub const STRESS_MAX_DELAY: Duration = Duration::from_millis(250);

/// What happens to a command a rule matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// The pipe reader threads start this late (not scheduled on a busy machine). The
    /// output must arrive all the same.
    SlowReader(Duration),
    /// The readers come only after the grace that follows git's exit: the output counts
    /// as cut (a read fails with [`crate::git_cli::GitError::OutputCut`]).
    PipesCut,
    /// The process is killed (SIGKILL) right after it started: it ends by the signal,
    /// with part or none of its output.
    Kill,
    /// stdout ends after this many bytes, the exit code is git's: a short read reported
    /// as a success (what a too short grace did).
    Truncate(usize),
    /// The process can't be started (the OS refuses: no git, no file descriptors).
    SpawnFails,
    /// The command ends with this exit code and no output at all, no message on stderr
    /// (git isn't started).
    SilentExit(i32),
}

/// What [`crate::process::execute`] does differently for one command.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Plan {
    pub reader_delay: Duration,
    /// Instead of the usual grace after the process exited.
    pub grace: Option<Duration>,
    pub kill: bool,
    pub truncate: Option<usize>,
    pub spawn_fails: bool,
    pub exit: Option<i32>,
}

impl Plan {
    fn add(&mut self, fault: Fault) {
        match fault {
            Fault::SlowReader(d) => self.reader_delay = self.reader_delay.max(d),
            Fault::PipesCut => {
                self.reader_delay = self.reader_delay.max(Duration::from_millis(300));
                self.grace = Some(Duration::from_millis(30));
            }
            Fault::Kill => self.kill = true,
            Fault::Truncate(n) => self.truncate = Some(n),
            Fault::SpawnFails => self.spawn_fails = true,
            Fault::SilentExit(code) => self.exit = Some(code),
        }
    }
}

/// An injected rule; removed when dropped.
#[must_use = "the rule is removed when this is dropped"]
pub struct Injection {
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    id: u64,
}

impl Injection {
    /// How many commands the rule hit so far.
    pub fn hits(&self) -> usize {
        imp::hits(self.id)
    }
}

impl Drop for Injection {
    fn drop(&mut self) {
        imp::remove(self.id);
    }
}

/// Injects `fault` into the next `times` commands (`usize::MAX`: all) run in `dir` or
/// below it whose git subcommand (or, for another program, its name) is `command` (`*`:
/// any). Debug builds only: a release build panics here.
pub fn inject(dir: &Path, command: &str, fault: Fault, times: usize) -> Injection {
    Injection {
        id: imp::add(Some(dir), command, fault, times),
    }
}

/// The plan for `cmd` (debug builds: the matching rules and the stress mode).
#[cfg(debug_assertions)]
pub(crate) fn plan(cmd: &std::process::Command) -> Plan {
    imp::plan(cmd)
}

#[cfg(not(debug_assertions))]
#[inline(always)]
pub(crate) fn plan(_cmd: &std::process::Command) -> Plan {
    Plan::default()
}

/// Parses a `GITTI_TEST_FAULT` value: `(command, fault, times)` per rule.
pub fn parse_spec(spec: &str) -> Result<Vec<(String, Fault, usize)>, String> {
    let mut out = Vec::new();
    for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (command, rest) = part
            .split_once('=')
            .ok_or_else(|| format!("{part:?}: expected <command>=<fault>"))?;
        let (fault, times) = match rest.split_once('*') {
            Some((f, n)) => (f, n.parse().map_err(|_| format!("{part:?}: bad count {n:?}"))?),
            None => (rest, usize::MAX),
        };
        let (name, arg) = fault.split_once(':').unwrap_or((fault, ""));
        let num = |what: &str| -> Result<u64, String> {
            arg.parse().map_err(|_| format!("{part:?}: {name} needs a number ({what})"))
        };
        let fault = match name {
            "slow" => Fault::SlowReader(Duration::from_millis(num("milliseconds")?)),
            "cut" => Fault::PipesCut,
            "kill" => Fault::Kill,
            "truncate" => Fault::Truncate(num("bytes")? as usize),
            "spawn" => Fault::SpawnFails,
            "exit" => Fault::SilentExit(num("exit code")? as i32),
            _ => return Err(format!("{part:?}: unknown fault {name:?}")),
        };
        out.push((command.trim().to_string(), fault, times));
    }
    Ok(out)
}

/// The git subcommand of `args` (after the `-c key=value` pairs), else `program`'s name.
fn command_name(program: &str, args: &[&str]) -> String {
    let name = Path::new(program)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name != "git" {
        return name;
    }
    let mut a = args;
    while a.first() == Some(&"-c") && a.len() >= 2 {
        a = &a[2..];
    }
    a.first().map(|s| s.to_string()).unwrap_or_default()
}

#[cfg(debug_assertions)]
mod imp {
    use super::{Fault, Plan, STRESS_MAX_DELAY};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    struct Rule {
        id: u64,
        /// `None`: every directory (`GITTI_TEST_FAULT`). The path as given and resolved.
        dir: Option<(PathBuf, PathBuf)>,
        command: String,
        fault: Fault,
        left: usize,
        hits: usize,
    }

    static RULES: Mutex<Vec<Rule>> = Mutex::new(Vec::new());
    /// Whether [`RULES`] may hold a rule (the common case asks no lock).
    static ANY: AtomicBool = AtomicBool::new(false);
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);

    fn rules() -> std::sync::MutexGuard<'static, Vec<Rule>> {
        RULES.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn add(dir: Option<&Path>, command: &str, fault: Fault, times: usize) -> u64 {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let dir = dir.map(|d| (d.to_path_buf(), crate::paths::canonical(d)));
        let mut rules = rules();
        rules.push(Rule {
            id,
            dir,
            command: command.to_string(),
            fault,
            left: times,
            hits: 0,
        });
        ANY.store(true, Ordering::SeqCst);
        id
    }

    pub(super) fn remove(id: u64) {
        let mut rules = rules();
        rules.retain(|r| r.id != id);
        if rules.is_empty() {
            ANY.store(false, Ordering::SeqCst);
        }
    }

    pub(super) fn hits(id: u64) -> usize {
        rules().iter().find(|r| r.id == id).map_or(0, |r| r.hits)
    }

    /// The `GITTI_TEST_FAULT` rules, added once.
    fn env_rules() {
        static ONCE: OnceLock<()> = OnceLock::new();
        ONCE.get_or_init(|| {
            let Some(spec) = std::env::var_os(super::ENV_FAULT) else {
                return;
            };
            match super::parse_spec(&spec.to_string_lossy()) {
                Ok(list) => {
                    for (command, fault, times) in list {
                        add(None, &command, fault, times);
                    }
                }
                Err(e) => panic!("{}: {e}", super::ENV_FAULT),
            }
        });
    }

    pub(super) fn plan(cmd: &std::process::Command) -> Plan {
        env_rules();
        let mut plan = Plan::default();
        if let Some(stress) = stress() {
            plan.reader_delay = stress.delay();
        }
        if !ANY.load(Ordering::SeqCst) {
            return plan;
        }
        let program = cmd.get_program().to_string_lossy();
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let name = super::command_name(&program, &args);
        let dir = cmd.get_current_dir().map(Path::to_path_buf);
        let canonical = dir.as_deref().map(crate::paths::canonical);
        let mut rules = rules();
        for r in rules.iter_mut() {
            if r.left == 0 || (r.command != "*" && r.command != name) {
                continue;
            }
            if let Some((raw, resolved)) = &r.dir {
                let inside = |d: &Option<PathBuf>| d.as_deref().is_some_and(|d| d.starts_with(raw) || d.starts_with(resolved));
                if !inside(&dir) && !inside(&canonical) {
                    continue;
                }
            }
            r.left = r.left.saturating_sub(1);
            r.hits += 1;
            plan.add(r.fault);
        }
        plan
    }

    struct Stress {
        state: AtomicU64,
    }

    impl Stress {
        /// xorshift64*: a reader delay for every third command or so.
        fn delay(&self) -> Duration {
            let mut x = self.state.load(Ordering::Relaxed);
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.state.store(x, Ordering::Relaxed);
            let r = x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32;
            if !r.is_multiple_of(3) {
                return Duration::ZERO;
            }
            Duration::from_millis((r >> 2) % (STRESS_MAX_DELAY.as_millis() as u64 + 1))
        }
    }

    /// The stress mode, started on first use when `GITTI_TEST_STRESS=1`.
    fn stress() -> Option<&'static Stress> {
        static STRESS: OnceLock<Option<Stress>> = OnceLock::new();
        STRESS
            .get_or_init(|| {
                if std::env::var_os(super::ENV_STRESS).is_none_or(|v| v.is_empty() || v == "0") {
                    return None;
                }
                // Two CPU hogs: 30 ms busy, 30 ms asleep — the reader threads compete.
                for _ in 0..2 {
                    let _ = std::thread::Builder::new()
                        .name("stress-hog".into())
                        .stack_size(64 * 1024)
                        .spawn(|| {
                            loop {
                                let until = std::time::Instant::now() + Duration::from_millis(30);
                                let mut n = 0u64;
                                while std::time::Instant::now() < until {
                                    n = std::hint::black_box(n.wrapping_add(1));
                                }
                                std::thread::sleep(Duration::from_millis(30));
                            }
                        });
                }
                let seed = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(1, |d| d.as_nanos() as u64)
                    ^ u64::from(std::process::id()).rotate_left(32);
                Some(Stress {
                    state: AtomicU64::new(seed | 1),
                })
            })
            .as_ref()
    }
}

#[cfg(not(debug_assertions))]
mod imp {
    use super::Fault;
    use std::path::Path;

    pub(super) fn add(_dir: Option<&Path>, _command: &str, _fault: Fault, _times: usize) -> u64 {
        panic!("fault injection is compiled into debug builds only");
    }

    pub(super) fn remove(_id: u64) {}

    pub(super) fn hits(_id: u64) -> usize {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn spec_parses_every_fault() {
        let rules = parse_spec("show=kill*1, cat-file=cut,log=truncate:0*2,*=slow:400,x=spawn,y=exit:128").unwrap();
        assert_eq!(
            rules,
            vec![
                ("show".to_string(), Fault::Kill, 1),
                ("cat-file".to_string(), Fault::PipesCut, usize::MAX),
                ("log".to_string(), Fault::Truncate(0), 2),
                ("*".to_string(), Fault::SlowReader(Duration::from_millis(400)), usize::MAX),
                ("x".to_string(), Fault::SpawnFails, usize::MAX),
                ("y".to_string(), Fault::SilentExit(128), usize::MAX),
            ]
        );
        assert!(parse_spec("show").is_err());
        assert!(parse_spec("show=boom").is_err());
        assert!(parse_spec("show=slow").is_err());
    }

    #[gitti_test::test]
    fn the_subcommand_comes_after_the_config_pairs() {
        assert_eq!(command_name("git", &["-c", "a=b", "-c", "c=d", "show", "-p"]), "show");
        assert_eq!(command_name("/usr/bin/git", &["status"]), "status");
        assert_eq!(command_name("sh", &["-c", "x"]), "sh");
    }
}
