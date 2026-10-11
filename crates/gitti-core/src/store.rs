//! Persistent app state in SQLite (`<config dir>/gitti.db`): settings, open tabs, window
//! placements and commit message drafts. gitti and gitty share the file (same [`db_path`]).
//!
//! - Settings are stored one row per field (`key` = field name, `value` = JSON), and a
//!   change writes only the fields that changed. Several windows / processes (e.g. an old
//!   build still running next to a new one) therefore never overwrite each other's
//!   changes with a stale full copy.
//! - Loading is per field: a value that no longer fits (renamed type, removed field) falls
//!   back to its default without resetting the others.
//! - The schema is versioned; [`MIGRATIONS`] upgrades older databases step by step. Never
//!   edit a released migration, append a new one.
//! - The JSON files of earlier versions (`settings.json`, `workspace.json`) are imported
//!   once and renamed to `*.imported`.
//!
//! # Compatibility rule: one file for a whole range of builds
//!
//! Worktree builds, the installed build, a build from months ago and today's run side by
//! side on the same file, in any order. So:
//!
//! - **Migrations are additive only**: `CREATE TABLE`, `CREATE [UNIQUE] INDEX`,
//!   `ALTER TABLE … ADD COLUMN` (a `NOT NULL` column needs a `DEFAULT`). Never drop,
//!   rename or retype anything, never rewrite rows. A test checks every migration.
//! - **A newer schema is used as is**: a build that knows fewer migrations opens a db
//!   another build migrated further, reads the tables / keys it knows and ignores the
//!   rest; it never lowers the schema level. Its writes name their columns (no
//!   `SELECT *`, no `INSERT` without a column list — a test checks this file) and touch only
//!   the rows it owns, so a column added later keeps its default (or is reset to it when an
//!   older build rewrites the row, e.g. the open tabs).
//! - **A truly breaking change needs a new file name** (`gitti-2.db`, importing from
//!   `gitti.db` once), never a destructive migration: older binaries keep using the old
//!   file untouched.
//! - `PRAGMA user_version` stays at most `LEGACY_MAX_USER_VERSION` (4): builds before this
//!   rule refused a db whose `user_version` was above the migrations they knew and ran
//!   without settings. Levels above 4 are recorded in the `schema_level` table instead.
//!
//! # Never on the UI thread: writes are queued
//!
//! Every `save_*` / `note_*` / `forget_*` only queues the write; one background thread
//! ([`writer`]) runs them in order. A commit can stall for seconds under disk load (e.g.
//! in `jbd2_log_wait_commit`); on the UI thread that froze the window — new tabs, pane
//! drags, zooming — and every other write meanwhile failed with "database is locked" after
//! the busy timeout. The db also runs in WAL mode with `synchronous = NORMAL` (no fsync
//! per commit; settings are not worth one). Loads first wait (bounded) for this process's
//! queued writes, so a load right after a save sees it; [`flush`] at exit keeps the last
//! writes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context as _, Result};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::paths::config_dir;

/// Schema steps; `MIGRATIONS[i]` brings level `i` to `i + 1`. Additive only (see the module
/// docs): `CREATE TABLE`, `CREATE INDEX`, `ALTER TABLE … ADD COLUMN`.
const MIGRATIONS: &[&str] = &[
    // 1: settings (one JSON value per field) and the open repository tabs.
    "CREATE TABLE settings (
         key   TEXT PRIMARY KEY,
         value TEXT NOT NULL
     );
     CREATE TABLE open_tabs (
         position INTEGER PRIMARY KEY,
         path     TEXT NOT NULL
     );
     CREATE TABLE workspace (
         key   TEXT PRIMARY KEY,
         value TEXT NOT NULL
     );",
    // 2: last window placement per window kind and display setup.
    "CREATE TABLE window_placement (
         window  TEXT NOT NULL,
         setup   TEXT NOT NULL,
         display TEXT NOT NULL,
         x       REAL NOT NULL,
         y       REAL NOT NULL,
         width   REAL NOT NULL,
         height  REAL NOT NULL,
         state   INTEGER NOT NULL,
         PRIMARY KEY (window, setup)
     );",
    // 3: recently opened repositories (File ▸ Open Recent); `opened` = unix ms.
    "CREATE TABLE recent_repos (
         path   TEXT PRIMARY KEY,
         opened INTEGER NOT NULL
     );",
    // 4: per-repository choices (e.g. the Push… dialog's), one JSON value per key.
    "CREATE TABLE repo_prefs (
         repo  TEXT NOT NULL,
         key   TEXT NOT NULL,
         value TEXT NOT NULL,
         PRIMARY KEY (repo, key)
     );",
    // 5: the commit message being typed, per worktree; `saved` = unix ms.
    "CREATE TABLE commit_drafts (
         worktree TEXT PRIMARY KEY,
         message  TEXT NOT NULL,
         saved    INTEGER NOT NULL
     );",
    // 6: the message git prepared (MERGE_MSG / SQUASH_MSG) the user chose to keep their
    // draft over ("Keep Mine"), so its note stays hidden after a restart.
    "ALTER TABLE commit_drafts ADD COLUMN kept_prepared TEXT;",
    // 7: the message being edited for an amend, per worktree, for the commit it amends
    // (`head`, hex): it comes back only while that commit is still HEAD.
    "CREATE TABLE amend_drafts (
         worktree TEXT PRIMARY KEY,
         head     TEXT NOT NULL,
         message  TEXT NOT NULL,
         saved    INTEGER NOT NULL
     );",
];

/// The settings both apps use (gitty reads and writes only these; gitti's `Settings` is a
/// superset with the same field names — a test in gitti-app checks that). Stored per field,
/// so neither app overwrites the other's fields.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SharedSettings {
    pub topo_order: bool,
    pub wrap_lines: bool,
    pub ignore_whitespace: bool,
    pub diff_context: u32,
    pub show_git_log: bool,
    pub log_show_reads: bool,
    /// "Open in Editor" command, see `crate::editor`.
    pub editor_command: String,
    /// Tree-sitter highlighting in diffs.
    pub syntax_highlight: bool,
    /// Merge editor: only the result (ours / theirs folded to a bar on top).
    pub merge_single_view: bool,
    /// "Open in Terminal" command line, `{dir}` = the repository folder (see
    /// `crate::desktop::terminal_candidates`); empty = the platform's terminal.
    /// `GITTI_TERMINAL` overrides it.
    pub terminal_command: String,
    /// The sidebar (branches, remotes, tags, stashes) is shown (View ▸ Toggle Sidebar).
    pub show_sidebar: bool,
    /// Spell checking of the commit message with the OS checker (gitti only so far;
    /// gitty keeps the value as loaded).
    pub spell_check: bool,
    /// Intra-line diff marks: how much they lighten the line (and its text), percent of
    /// `intraline::MAX_LIFT`; 0 = off.
    pub intraline_strength: u32,
    /// Commit list rows: by width, one line or two lines (View menu / palette).
    pub commit_list_layout: crate::list_layout::CommitListLayout,
    /// The sidebar's WORKTREES section also without linked worktrees: the main worktree
    /// (the repository's folder) alone, for its menu (`worktree::section_shown`).
    pub show_main_worktree_alone: bool,
    /// The sidebar's local branches: by name, last commit date or recently checked out
    /// (`branch_order`).
    pub branch_sort: crate::branch_order::BranchSort,
    /// The sidebar's remote branches: by name or last commit date.
    pub remote_branch_sort: crate::branch_order::BranchSort,
    /// The working directory's changes as a tree of folders instead of a flat list of
    /// paths (`rows::build_worktree_view`).
    pub changes_as_tree: bool,
}

impl Default for SharedSettings {
    fn default() -> Self {
        Self {
            topo_order: true,
            wrap_lines: false,
            ignore_whitespace: false,
            diff_context: 3,
            show_git_log: false,
            log_show_reads: false,
            editor_command: String::new(),
            syntax_highlight: true,
            merge_single_view: false,
            terminal_command: String::new(),
            show_sidebar: true,
            spell_check: true,
            intraline_strength: crate::intraline::DEFAULT_STRENGTH,
            commit_list_layout: crate::list_layout::CommitListLayout::Auto,
            show_main_worktree_alone: true,
            branch_sort: crate::branch_order::BranchSort::Name,
            remote_branch_sort: crate::branch_order::BranchSort::Name,
            changes_as_tree: false,
        }
    }
}

/// Pane sizes as fractions, shared by gitti and gitty (same keys in gitti's `Settings`):
/// the last arrangement the user dragged applies to every repository and, being relative,
/// looks alike on another screen or terminal size. 0 = the app's default.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PaneLayout {
    /// Sidebar width / width of the panes row (sidebar + commit list + detail).
    pub pane_sidebar: f32,
    /// Commit list width / width of the panes row.
    pub pane_list: f32,
    /// Git log panel height / height of panes row + log panel.
    pub pane_log: f32,
}

impl PaneLayout {
    /// `fraction` of `total`, or `default` when unset; kept sane if the stored value isn't.
    pub fn size(fraction: f32, total: f32, default: f32) -> f32 {
        if fraction > 0. && fraction < 1. {
            fraction * total
        } else {
            default
        }
    }

    /// `part / total` for storing; 0 (unset) for an unmeasured total.
    pub fn fraction(part: f32, total: f32) -> f32 {
        if total > 0. {
            (part / total).clamp(0.01, 0.99)
        } else {
            0.
        }
    }
}

pub fn db_path() -> Option<PathBuf> {
    Some(config_dir()?.join("gitti.db"))
}

/// Opens (and creates / migrates) the database for a read; `None` if that fails (logged).
/// Waits (bounded) until this process's queued writes are done, so reads see them.
fn open() -> Option<Connection> {
    let path = db_path()?;
    wait_for_writes(Duration::from_secs(2));
    match open_at(&path) {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("gitti: settings database {}: {e:#}", path.display());
            None
        }
    }
}

fn open_at(path: &Path) -> Result<Connection> {
    open_at_with(path, MIGRATIONS)
}

/// [`open_at`] for a build that knows `migrations` (tests simulate older / newer builds).
fn open_at_with(path: &Path, migrations: &[&str]) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut conn = Connection::open(path)?;
    // Other gitti processes may write at the same time.
    conn.busy_timeout(std::time::Duration::from_secs(3))?;
    // WAL: readers never wait for a writer, and a commit doesn't fsync (`NORMAL`); the
    // mode is stored in the file, so setting it is a no-op after the first time. A file
    // system without WAL support keeps the rollback journal.
    let _ = conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()));
    let _ = conn.execute_batch("PRAGMA synchronous = NORMAL");
    let fresh = migrate(&mut conn, migrations)?;
    if fresh && let Some(dir) = path.parent() {
        import_json_files(&conn, dir);
    }
    Ok(conn)
}

/// The highest `PRAGMA user_version` ever written. Builds before the compatibility rule
/// (2026-10) bail on a db whose `user_version` is above the 4 migrations they know, so
/// levels above live in `schema_level`.
const LEGACY_MAX_USER_VERSION: usize = 4;

/// The number of migrations applied to the db.
fn schema_level(conn: &Connection) -> Result<usize> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let version = usize::try_from(version).unwrap_or(0);
    if version < LEGACY_MAX_USER_VERSION {
        return Ok(version);
    }
    // The table exists only once a level above 4 was reached.
    let level: Option<i64> = conn
        .query_row("SELECT level FROM schema_level WHERE id = 1", [], |r| r.get(0))
        .optional()
        .unwrap_or(None);
    Ok(level.map_or(version, |l| version.max(usize::try_from(l).unwrap_or(0))))
}

fn set_schema_level(conn: &Connection, level: usize) -> Result<()> {
    conn.pragma_update(None, "user_version", level.min(LEGACY_MAX_USER_VERSION) as i64)?;
    if level > LEGACY_MAX_USER_VERSION {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_level (
                 id    INTEGER PRIMARY KEY CHECK (id = 1),
                 level INTEGER NOT NULL
             );",
        )?;
        conn.execute(
            "INSERT INTO schema_level (id, level) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET level = excluded.level",
            params![level as i64],
        )?;
    }
    Ok(())
}

/// Applies the missing `migrations`; returns whether this call created the schema (the db
/// was new). A db at a higher level than `migrations` knows (written by a newer build) is
/// used as is and keeps its level.
fn migrate(conn: &mut Connection, migrations: &[&str]) -> Result<bool> {
    if schema_level(conn)? >= migrations.len() {
        return Ok(false);
    }
    let mut fresh = false;
    loop {
        // Immediate: another process opening the new db at the same moment waits here
        // and then sees the level this one reached, instead of applying a step twice.
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let level = schema_level(&tx)?;
        let Some(sql) = migrations.get(level) else {
            return Ok(fresh);
        };
        tx.execute_batch(sql)
            .with_context(|| format!("migration {}", level + 1))?;
        set_schema_level(&tx, level + 1)?;
        tx.commit()?;
        fresh |= level == 0;
    }
}

/// One-time import of the pre-SQLite JSON files.
fn import_json_files(conn: &Connection, dir: &Path) {
    let read = |name: &str| -> Option<Map<String, Value>> {
        let text = std::fs::read_to_string(dir.join(name)).ok()?;
        match serde_json::from_str::<Value>(&text).ok()? {
            Value::Object(m) => Some(m),
            _ => None,
        }
    };
    let done = |name: &str| {
        let from = dir.join(name);
        let _ = std::fs::rename(&from, from.with_extension("json.imported"));
    };
    if let Some(m) = read("settings.json") {
        for (k, v) in &m {
            let _ = put_value(conn, k, v);
        }
        done("settings.json");
    }
    if let Some(m) = read("workspace.json") {
        let repos: Vec<PathBuf> = m
            .get("repos")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let active = m.get("active").and_then(Value::as_u64).unwrap_or(0) as usize;
        let _ = write_tabs(conn, &repos, active);
        done("workspace.json");
    }
}

fn put_value(conn: &Connection, key: &str, value: &Value) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value.to_string()],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------- settings

/// Loads settings field by field over `T::default()`.
pub fn load_settings<T: Serialize + DeserializeOwned + Default>() -> T {
    let Some(conn) = open() else {
        return T::default();
    };
    load_settings_from(&conn)
}

fn load_settings_from<T: Serialize + DeserializeOwned + Default>(conn: &Connection) -> T {
    let Ok(Value::Object(mut obj)) = serde_json::to_value(T::default()) else {
        return T::default();
    };
    let stored: Vec<(String, String)> = conn
        .prepare("SELECT key, value FROM settings")
        .and_then(|mut st| {
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()
        })
        .unwrap_or_default();
    for (key, raw) in stored {
        // Unknown keys (removed fields) stay in the table but are ignored.
        let Some(old) = obj.get(&key).cloned() else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        obj.insert(key.clone(), value);
        // Keep the value only if the struct still accepts it.
        if serde_json::from_value::<T>(Value::Object(obj.clone())).is_err() {
            obj.insert(key, old);
        }
    }
    serde_json::from_value(Value::Object(obj)).unwrap_or_default()
}

/// Writes the fields that differ between `before` and `after`.
pub fn save_changed_settings<T: Serialize>(before: &T, after: &T) {
    let (Ok(Value::Object(b)), Ok(Value::Object(a))) =
        (serde_json::to_value(before), serde_json::to_value(after))
    else {
        return;
    };
    let changed: Vec<(String, Value)> = a
        .into_iter()
        .filter(|(k, v)| b.get(k) != Some(v))
        .collect();
    if changed.is_empty() {
        return;
    }
    write_later("saving settings", move |conn| write_changed(conn, &changed));
}

fn write_changed(conn: &mut Connection, changed: &[(String, Value)]) -> Result<()> {
    let tx = conn.transaction()?;
    for (k, v) in changed {
        put_value(&tx, k, v)?;
    }
    tx.commit()?;
    Ok(())
}

// ---------------------------------------------------------------------- writer

/// A queued write, run on the writer thread with an open connection.
type Job = Box<dyn FnOnce(&mut Connection) + Send>;

enum WriterMsg {
    Write(PathBuf, Job),
    Flush(mpsc::Sender<()>),
}

/// The background thread that runs every write of this process, in queue order (see the
/// module docs: a commit may wait seconds for the disk, never on the UI thread).
struct Writer {
    tx: Mutex<mpsc::Sender<WriterMsg>>,
    /// Writes queued and not yet done.
    pending: Arc<AtomicUsize>,
}

fn writer() -> &'static Writer {
    static WRITER: OnceLock<Writer> = OnceLock::new();
    WRITER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<WriterMsg>();
        let pending = Arc::new(AtomicUsize::new(0));
        let done = pending.clone();
        std::thread::Builder::new()
            .name("gitti-store".into())
            .spawn(move || {
                // Batches: everything queued while the previous batch ran is written with
                // one connection per db path, opened for the batch and dropped after it —
                // a connection kept open would go on writing to a db file that was deleted
                // or replaced in the meantime (tests reset it; another process may too).
                while let Ok(first) = rx.recv() {
                    let mut batch = vec![first];
                    batch.extend(rx.try_iter());
                    let mut conn: Option<(PathBuf, Connection)> = None;
                    for msg in batch {
                        match msg {
                            WriterMsg::Write(path, job) => {
                                if conn.as_ref().is_none_or(|(p, _)| *p != path) {
                                    conn = match open_at(&path) {
                                        // Off the UI thread waiting longer is fine (another
                                        // process committing on a stalled disk).
                                        Ok(c) => {
                                            let _ = c.busy_timeout(Duration::from_secs(15));
                                            Some((path.clone(), c))
                                        }
                                        Err(e) => {
                                            eprintln!(
                                                "gitti: settings database {}: {e:#}",
                                                path.display()
                                            );
                                            None
                                        }
                                    };
                                }
                                if let Some((_, c)) = conn.as_mut() {
                                    job(c);
                                }
                                done.fetch_sub(1, Ordering::SeqCst);
                            }
                            WriterMsg::Flush(reply) => {
                                let _ = reply.send(());
                            }
                        }
                    }
                }
            })
            .expect("spawning the settings writer thread");
        Writer { tx: Mutex::new(tx), pending }
    })
}

/// Queues `write` for the db at `path`; errors are logged with `what`.
fn write_at(
    path: PathBuf,
    what: &'static str,
    write: impl FnOnce(&mut Connection) -> Result<()> + Send + 'static,
) {
    let w = writer();
    w.pending.fetch_add(1, Ordering::SeqCst);
    let job: Job = Box::new(move |conn| {
        if let Err(e) = write(conn) {
            eprintln!("gitti: {what} failed: {e:#}");
        }
    });
    let sent = w
        .tx
        .lock()
        .map(|tx| tx.send(WriterMsg::Write(path.clone(), job)).is_ok())
        .unwrap_or(false);
    if !sent {
        // No writer thread (can't happen short of a panic there): nothing is written.
        w.pending.fetch_sub(1, Ordering::SeqCst);
        eprintln!("gitti: {what} failed: the settings writer is gone");
    }
}

/// Queues a write for [`db_path`].
fn write_later(
    what: &'static str,
    write: impl FnOnce(&mut Connection) -> Result<()> + Send + 'static,
) {
    if let Some(path) = db_path() {
        write_at(path, what, write);
    }
}

/// Waits until every write queued so far is done, at most `limit`; `true` when done.
fn wait_for_writes(limit: Duration) -> bool {
    let w = writer();
    if w.pending.load(Ordering::SeqCst) == 0 {
        return true;
    }
    let (reply, done) = mpsc::channel();
    let sent = w
        .tx
        .lock()
        .map(|tx| tx.send(WriterMsg::Flush(reply)).is_ok())
        .unwrap_or(false);
    sent && done.recv_timeout(limit).is_ok()
}

/// Writes everything queued before the process ends (call on quit; waits up to 5 s).
pub fn flush() {
    wait_for_writes(Duration::from_secs(5));
}

// ---------------------------------------------------------------------- tabs

/// Open repository tabs and the active one.
pub fn load_tabs() -> (Vec<PathBuf>, usize) {
    let Some(conn) = open() else {
        return (Vec::new(), 0);
    };
    let repos = conn
        .prepare("SELECT path FROM open_tabs ORDER BY position")
        .and_then(|mut st| {
            st.query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect();
    let active = conn
        .query_row(
            "SELECT value FROM workspace WHERE key = 'active_tab'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    (repos, active)
}

pub fn save_tabs(repos: &[PathBuf], active: usize) {
    let repos = repos.to_vec();
    write_later("saving tabs", move |conn| write_tabs(conn, &repos, active));
}

fn write_tabs(conn: &Connection, repos: &[PathBuf], active: usize) -> Result<()> {
    conn.execute_batch("BEGIN; DELETE FROM open_tabs;")?;
    let result = (|| -> Result<()> {
        for (i, p) in repos.iter().enumerate() {
            conn.execute(
                "INSERT INTO open_tabs (position, path) VALUES (?1, ?2)",
                params![i as i64, p.to_string_lossy()],
            )?;
        }
        conn.execute(
            "INSERT INTO workspace (key, value) VALUES ('active_tab', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![active.to_string()],
        )?;
        Ok(())
    })();
    match result {
        Ok(()) => conn.execute_batch("COMMIT")?,
        Err(_) => conn.execute_batch("ROLLBACK")?,
    }
    result
}

// ---------------------------------------------------------- recent repositories

/// Entries kept in the recent repositories list.
pub const MAX_RECENT: usize = 20;

/// Records that `path` (a repository work dir) was opened now: it moves to the top of the
/// recent list (File ▸ Open Recent), which keeps the newest [`MAX_RECENT`].
pub fn note_recent_repo(path: &Path) {
    let (path, at) = (path.to_path_buf(), now_millis());
    write_later("saving the recent repositories", move |conn| {
        write_recent(conn, &path, at)
    });
}

/// The recent repositories, most recently opened first.
pub fn load_recent_repos() -> Vec<PathBuf> {
    open().map(|c| read_recent(&c)).unwrap_or_default()
}

/// Drops `path` from the recent list (it is gone).
pub fn forget_recent_repo(path: &Path) {
    let path = path.to_string_lossy().into_owned();
    write_later("forgetting a recent repository", move |conn| {
        conn.execute("DELETE FROM recent_repos WHERE path = ?1", params![path])?;
        Ok(())
    });
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn write_recent(conn: &Connection, path: &Path, at: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO recent_repos (path, opened) VALUES (?1, ?2)
         ON CONFLICT(path) DO UPDATE SET opened = excluded.opened",
        params![path.to_string_lossy(), at],
    )?;
    conn.execute(
        "DELETE FROM recent_repos WHERE path NOT IN
             (SELECT path FROM recent_repos ORDER BY opened DESC, path LIMIT ?1)",
        params![MAX_RECENT as i64],
    )?;
    Ok(())
}

fn read_recent(conn: &Connection) -> Vec<PathBuf> {
    conn.prepare("SELECT path FROM recent_repos ORDER BY opened DESC, path")
        .and_then(|mut st| {
            st.query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

// ---------------------------------------------------------- per-repository choices

/// A remembered per-repository value (`repo` = its work dir), `None` when there is none
/// or it no longer fits `T`.
pub fn load_repo_pref<T: DeserializeOwned>(repo: &Path, key: &str) -> Option<T> {
    read_repo_pref(&open()?, repo, key)
}

/// Remembers `value` for `repo` under `key`.
pub fn save_repo_pref<T: Serialize>(repo: &Path, key: &str, value: &T) {
    let Ok(value) = serde_json::to_value(value) else { return };
    let (repo, key) = (repo.to_path_buf(), key.to_string());
    write_later("saving a repository choice", move |conn| {
        write_repo_pref(conn, &repo, &key, &value)
    });
}

/// Forgets what was remembered for `repo` under `key`.
pub fn forget_repo_pref(repo: &Path, key: &str) {
    let (repo, key) = (repo.to_string_lossy().into_owned(), key.to_string());
    write_later("forgetting a repository choice", move |conn| {
        conn.execute(
            "DELETE FROM repo_prefs WHERE repo = ?1 AND key = ?2",
            params![repo, key],
        )?;
        Ok(())
    });
}

fn read_repo_pref<T: DeserializeOwned>(conn: &Connection, repo: &Path, key: &str) -> Option<T> {
    let text: String = conn
        .query_row(
            "SELECT value FROM repo_prefs WHERE repo = ?1 AND key = ?2",
            params![repo.to_string_lossy(), key],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten()?;
    serde_json::from_str(&text).ok()
}

fn write_repo_pref<T: Serialize>(conn: &Connection, repo: &Path, key: &str, value: &T) -> Result<()> {
    conn.execute(
        "INSERT INTO repo_prefs (repo, key, value) VALUES (?1, ?2, ?3)
         ON CONFLICT(repo, key) DO UPDATE SET value = excluded.value",
        params![repo.to_string_lossy(), key, serde_json::to_string(value)?],
    )?;
    Ok(())
}

// ---------------------------------------------------------- app-wide values

/// A value kept in the settings table under `key` beside the settings structs (e.g. the
/// custom commands for every repository, `crate::custom_command`); `None` when there is
/// none or it doesn't fit `T`. Neither app's settings struct has a field of that name.
pub fn load_setting<T: DeserializeOwned>(key: &str) -> Option<T> {
    read_setting(&open()?, key)
}

/// Stores `value` under `key` (see [`load_setting`]); queued like every write.
pub fn save_setting<T: Serialize>(key: &str, value: &T) {
    let Ok(value) = serde_json::to_value(value) else { return };
    let key = key.to_string();
    write_later("saving a setting", move |conn| put_value(conn, &key, &value));
}

fn read_setting<T: DeserializeOwned>(conn: &Connection, key: &str) -> Option<T> {
    let text: String = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get(0))
        .optional()
        .ok()
        .flatten()?;
    serde_json::from_str(&text).ok()
}

// ---------------------------------------------------------- commit message drafts

/// A commit message typed in a worktree and not committed yet ([`load_draft`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub message: String,
    /// The message git prepared for the next commit that the user chose to keep this
    /// draft over ("Keep Mine", [`save_kept_prepared`]): its note stays hidden while git's
    /// message is the same.
    pub kept_prepared: Option<String>,
}

/// The commit message typed in `worktree` (its work dir) and not committed yet, if any:
/// it survives a crash, a quit, a failed hook and a restart. Shared by gitti and
/// gitty. A read: call it off the UI thread.
pub fn load_draft(worktree: &Path) -> Option<Draft> {
    read_draft(&open()?, worktree)
}

/// [`load_draft`] from the db at `db` (tests of a gitty / gitti process with a config dir
/// of its own).
pub fn load_draft_in(db: &Path, worktree: &Path) -> Option<Draft> {
    read_draft(&open_at(db).ok()?, worktree)
}

/// Remembers `message` as `worktree`'s draft; an empty (blank) one removes it (and its
/// [`Draft::kept_prepared`]). Queued like every write. The apps call it debounced while
/// typing and once more on quit.
pub fn save_draft(worktree: &Path, message: &str) {
    let (worktree, message, at) = (worktree.to_path_buf(), message.to_string(), now_millis());
    write_later("saving the commit message draft", move |conn| {
        write_draft(conn, &worktree, &message, at)
    });
}

/// Drops `worktree`'s draft: a commit / amend with it succeeded.
pub fn clear_draft(worktree: &Path) {
    save_draft(worktree, "");
}

/// Records that `worktree`'s draft is kept over git's prepared message `kept` ("Keep
/// Mine"), or forgets it (`None`: git's message went). Without a draft there is nothing
/// to keep: no row, no write. Queued.
pub fn save_kept_prepared(worktree: &Path, kept: Option<&str>) {
    let (worktree, kept) = (worktree.to_path_buf(), kept.map(str::to_string));
    write_later("saving the kept commit message", move |conn| {
        write_kept_prepared(conn, &worktree, kept.as_deref())
    });
}

fn read_draft(conn: &Connection, worktree: &Path) -> Option<Draft> {
    conn.query_row(
        "SELECT message, kept_prepared FROM commit_drafts WHERE worktree = ?1",
        params![worktree.to_string_lossy()],
        |r| {
            Ok(Draft {
                message: r.get(0)?,
                kept_prepared: r.get(1)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
    .filter(|d| !d.message.trim().is_empty())
}

fn write_draft(conn: &Connection, worktree: &Path, message: &str, at: i64) -> Result<()> {
    let key = worktree.to_string_lossy();
    if message.trim().is_empty() {
        conn.execute("DELETE FROM commit_drafts WHERE worktree = ?1", params![key])?;
    } else {
        conn.execute(
            "INSERT INTO commit_drafts (worktree, message, saved) VALUES (?1, ?2, ?3)
             ON CONFLICT(worktree) DO UPDATE SET message = excluded.message,
                 saved = excluded.saved",
            params![key, message, at],
        )?;
    }
    Ok(())
}

fn write_kept_prepared(conn: &Connection, worktree: &Path, kept: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE commit_drafts SET kept_prepared = ?2 WHERE worktree = ?1",
        params![worktree.to_string_lossy(), kept],
    )?;
    Ok(())
}

/// The message edited for an amend of commit `head` (hex) in a worktree and not used yet
/// ([`load_amend_draft`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmendDraft {
    pub head: String,
    pub message: String,
}

impl AmendDraft {
    /// Its message while `head` (the commit HEAD is now) is still the one it amends.
    pub fn for_head(&self, head: Option<&str>) -> Option<&str> {
        (head == Some(self.head.as_str())).then_some(self.message.as_str())
    }
}

/// The message edited for an amend in `worktree` (its work dir), whatever commit it was
/// for — the apps use it only while that commit is HEAD ([`AmendDraft::for_head`]). Shared
/// by gitti and gitty. A read: call it off the UI thread.
pub fn load_amend_draft(worktree: &Path) -> Option<AmendDraft> {
    read_amend_draft(&open()?, worktree)
}

/// [`load_amend_draft`] from the db at `db` (tests of a gitty / gitti process with a
/// config dir of its own).
pub fn load_amend_draft_in(db: &Path, worktree: &Path) -> Option<AmendDraft> {
    read_amend_draft(&open_at(db).ok()?, worktree)
}

/// Remembers `message` as the amend message of commit `head` (hex) in `worktree` — one
/// per worktree, the newest wins; an empty (blank) one removes it. Queued like every
/// write. The apps call it debounced while the box is edited in amend mode, and drop it
/// (blank) when the box holds HEAD's own message again or the amend succeeded.
pub fn save_amend_draft(worktree: &Path, head: &str, message: &str) {
    let (worktree, head, message, at) =
        (worktree.to_path_buf(), head.to_string(), message.to_string(), now_millis());
    write_later("saving the amend message draft", move |conn| {
        write_amend_draft(conn, &worktree, &head, &message, at)
    });
}

/// Drops `worktree`'s amend message: the amend with it succeeded.
pub fn clear_amend_draft(worktree: &Path) {
    save_amend_draft(worktree, "", "");
}

fn read_amend_draft(conn: &Connection, worktree: &Path) -> Option<AmendDraft> {
    conn.query_row(
        "SELECT head, message FROM amend_drafts WHERE worktree = ?1",
        params![worktree.to_string_lossy()],
        |r| {
            Ok(AmendDraft {
                head: r.get(0)?,
                message: r.get(1)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
    .filter(|d| !d.message.trim().is_empty())
}

fn write_amend_draft(conn: &Connection, worktree: &Path, head: &str, message: &str, at: i64) -> Result<()> {
    let key = worktree.to_string_lossy();
    if message.trim().is_empty() {
        conn.execute("DELETE FROM amend_drafts WHERE worktree = ?1", params![key])?;
    } else {
        conn.execute(
            "INSERT INTO amend_drafts (worktree, head, message, saved) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(worktree) DO UPDATE SET head = excluded.head,
                 message = excluded.message, saved = excluded.saved",
            params![key, head, message, at],
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------- window placement

/// Where a window was, so it reopens there. Stored per window kind (`"main"`, `"merge"`)
/// and display *setup* (the app's fingerprint of all connected displays): undocking a
/// laptop must not move a window onto a display that is gone, and docking again brings
/// back the placement for that setup.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowPlacement {
    /// The display the window is on (the app's display id string, e.g. its UUID).
    pub display: String,
    /// Position and size in logical pixels as the UI toolkit reports them for `display`.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub state: WindowState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    Windowed,
    Maximized,
    Fullscreen,
}

impl WindowState {
    fn to_db(self) -> i64 {
        match self {
            WindowState::Windowed => 0,
            WindowState::Maximized => 1,
            WindowState::Fullscreen => 2,
        }
    }

    fn from_db(v: i64) -> Self {
        match v {
            1 => WindowState::Maximized,
            2 => WindowState::Fullscreen,
            _ => WindowState::Windowed,
        }
    }
}

pub fn load_window_placement(window: &str, setup: &str) -> Option<WindowPlacement> {
    read_placement(&open()?, window, setup)
}

pub fn save_window_placement(window: &str, setup: &str, p: &WindowPlacement) {
    let (window, setup, p) = (window.to_string(), setup.to_string(), p.clone());
    write_later("saving the window placement", move |conn| {
        write_placement(conn, &window, &setup, &p)
    });
}

fn read_placement(conn: &Connection, window: &str, setup: &str) -> Option<WindowPlacement> {
    conn.query_row(
        "SELECT display, x, y, width, height, state FROM window_placement
         WHERE window = ?1 AND setup = ?2",
        params![window, setup],
        |r| {
            Ok(WindowPlacement {
                display: r.get(0)?,
                x: r.get::<_, f64>(1)? as f32,
                y: r.get::<_, f64>(2)? as f32,
                width: r.get::<_, f64>(3)? as f32,
                height: r.get::<_, f64>(4)? as f32,
                state: WindowState::from_db(r.get(5)?),
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

fn write_placement(conn: &Connection, window: &str, setup: &str, p: &WindowPlacement) -> Result<()> {
    conn.execute(
        "INSERT INTO window_placement (window, setup, display, x, y, width, height, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(window, setup) DO UPDATE SET display = excluded.display,
             x = excluded.x, y = excluded.y, width = excluded.width,
             height = excluded.height, state = excluded.state",
        params![
            window,
            setup,
            p.display,
            p.x as f64,
            p.y as f64,
            p.width as f64,
            p.height as f64,
            p.state.to_db()
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(default)]
    struct S {
        font: f32,
        wrap: bool,
        name: String,
    }

    /// A write while another connection holds the db (or the disk stalls a commit) must
    /// not block the caller — it is queued and lands later.
    #[gitti_test::test]
    fn writes_never_block_the_caller_on_a_locked_db() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gitti.db");
        drop(open_at(&path).unwrap());
        let (locked, release) = (mpsc::channel(), mpsc::channel::<()>());
        let holder = {
            let path = path.clone();
            std::thread::spawn(move || {
                let conn = Connection::open(&path).unwrap();
                conn.execute_batch("BEGIN EXCLUSIVE").unwrap();
                locked.0.send(()).unwrap();
                release.1.recv().unwrap();
                conn.execute_batch("COMMIT").unwrap();
            })
        };
        locked.1.recv().unwrap();
        let start = std::time::Instant::now();
        let changed = vec![("font".to_string(), serde_json::json!(21.0))];
        write_at(path.clone(), "test write", move |c| write_changed(c, &changed));
        assert!(start.elapsed() < Duration::from_millis(100), "{:?}", start.elapsed());
        assert!(!wait_for_writes(Duration::from_millis(300)), "can't land while locked");
        release.0.send(()).unwrap();
        holder.join().unwrap();
        assert!(wait_for_writes(Duration::from_secs(10)));
        let s: S = load_settings_from(&open_at(&path).unwrap());
        assert_eq!(s.font, 21.0);
    }

    #[gitti_test::test]
    fn queued_writes_run_in_order_and_use_wal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gitti.db");
        for i in 0..20 {
            let changed = vec![("name".to_string(), serde_json::json!(format!("v{i}")))];
            write_at(path.clone(), "test write", move |c| write_changed(c, &changed));
        }
        assert!(wait_for_writes(Duration::from_secs(10)));
        let conn = open_at(&path).unwrap();
        let s: S = load_settings_from(&conn);
        assert_eq!(s.name, "v19");
        let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }

    impl Default for S {
        fn default() -> Self {
            Self {
                font: 13.0,
                wrap: false,
                name: "x".into(),
            }
        }
    }

    fn db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_at(&dir.path().join("gitti.db")).unwrap();
        (dir, conn)
    }

    #[gitti_test::test]
    fn migrates_once_and_keeps_version() {
        let (dir, conn) = db();
        assert_eq!(level(&conn), MIGRATIONS.len());
        assert_eq!(user_version(&conn), MIGRATIONS.len().min(LEGACY_MAX_USER_VERSION));
        drop(conn);
        // Reopening applies nothing again.
        let conn = open_at(&dir.path().join("gitti.db")).unwrap();
        assert_eq!(level(&conn), MIGRATIONS.len());
        assert_eq!(user_version(&conn), MIGRATIONS.len().min(LEGACY_MAX_USER_VERSION));
    }

    /// One draft per worktree, replaced while typing, gone when blanked (a
    /// commit / amend succeeded); a db of the builds before drafts (level 4) gets the
    /// table on open.
    #[gitti_test::test]
    fn drafts_per_worktree_roundtrip_and_clear() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gitti.db");
        drop(open_at_with(&path, &MIGRATIONS[..4]).unwrap());
        let conn = open_at(&path).unwrap();
        assert_eq!(level(&conn), MIGRATIONS.len());
        let (main, linked) = (Path::new("/repo"), Path::new("/repo-wt"));
        let text = |conn: &Connection, w| read_draft(conn, w).map(|d| d.message);
        assert_eq!(read_draft(&conn, main), None);
        write_draft(&conn, main, "Fix the parser", 1).unwrap();
        write_draft(&conn, linked, "WIP linked\n\nbody", 2).unwrap();
        write_draft(&conn, main, "Fix the parser\n\n#123 details", 3).unwrap();
        assert_eq!(
            text(&conn, main).as_deref(),
            Some("Fix the parser\n\n#123 details")
        );
        assert_eq!(text(&conn, linked).as_deref(), Some("WIP linked\n\nbody"));
        write_draft(&conn, main, "  \n", 4).unwrap();
        assert_eq!(read_draft(&conn, main), None, "blank = no draft");
        assert_eq!(text(&conn, linked).as_deref(), Some("WIP linked\n\nbody"));
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM commit_drafts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "a cleared draft leaves no row");
    }

    /// The amend message: one per worktree for the commit it amends (a newer one for
    /// another commit replaces it), gone when blanked; a db of the level before (6) gets
    /// the table.
    /// Workflow: CM-05
    #[gitti_test::test]
    fn amend_drafts_are_kept_per_worktree_for_their_commit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        drop(open_at_with(&path, &MIGRATIONS[..6]).unwrap());
        let conn = open_at(&path).unwrap();
        assert_eq!(level(&conn), MIGRATIONS.len());
        let (w, other) = (Path::new("/r/main"), Path::new("/r/linked"));
        assert_eq!(read_amend_draft(&conn, w), None);
        write_amend_draft(&conn, w, "aaa", "Fix it\n\n#12 body", 1).unwrap();
        write_amend_draft(&conn, other, "ccc", "Other", 1).unwrap();
        let d = read_amend_draft(&conn, w).unwrap();
        assert_eq!(d.for_head(Some("aaa")), Some("Fix it\n\n#12 body"));
        assert_eq!(d.for_head(Some("bbb")), None, "HEAD moved: not this commit's");
        assert_eq!(d.for_head(None), None);
        write_amend_draft(&conn, w, "bbb", "Newer", 2).unwrap();
        assert_eq!(read_amend_draft(&conn, w).unwrap(), AmendDraft { head: "bbb".into(), message: "Newer".into() });
        write_amend_draft(&conn, w, "bbb", " \n", 3).unwrap();
        assert_eq!(read_amend_draft(&conn, w), None, "blank = none");
        assert_eq!(read_amend_draft(&conn, other).unwrap().message, "Other");
        // The commit draft is a row of its own.
        assert_eq!(read_draft(&conn, other), None);
    }

    /// "Keep Mine" is stored with the draft: it survives typing on, goes with
    /// `None` (git's message went) and with the draft; without a draft nothing is written.
    #[gitti_test::test]
    fn kept_prepared_message_lives_with_its_draft() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_at(&dir.path().join("gitti.db")).unwrap();
        let w = Path::new("/repo");
        write_kept_prepared(&conn, w, Some("Merge x")).unwrap();
        assert_eq!(read_draft(&conn, w), None, "no draft, nothing kept");
        write_draft(&conn, w, "Mine", 1).unwrap();
        write_kept_prepared(&conn, w, Some("Merge x")).unwrap();
        write_draft(&conn, w, "Mine, more", 2).unwrap();
        let d = read_draft(&conn, w).unwrap();
        assert_eq!((d.message.as_str(), d.kept_prepared.as_deref()), ("Mine, more", Some("Merge x")));
        write_kept_prepared(&conn, w, None).unwrap();
        assert_eq!(read_draft(&conn, w).unwrap().kept_prepared, None);
        write_kept_prepared(&conn, w, Some("Merge y")).unwrap();
        write_draft(&conn, w, "", 3).unwrap();
        write_draft(&conn, w, "Again", 4).unwrap();
        assert_eq!(read_draft(&conn, w).unwrap().kept_prepared, None, "went with the draft");
    }

    /// Saving goes through the writer thread like every write; a load right after sees it.
    #[gitti_test::test]
    fn draft_writes_are_queued() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gitti.db");
        for text in ["a", "ab", "abc"] {
            let text = text.to_string();
            write_at(path.clone(), "test draft", move |c| {
                write_draft(c, Path::new("/w"), &text, 1)
            });
        }
        assert!(wait_for_writes(Duration::from_secs(10)));
        let d = read_draft(&open_at(&path).unwrap(), Path::new("/w")).map(|d| d.message);
        assert_eq!(d.as_deref(), Some("abc"));
    }

    fn level(conn: &Connection) -> usize {
        schema_level(conn).unwrap()
    }

    fn user_version(conn: &Connection) -> usize {
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap() as usize
    }

    /// Hypothetical later migrations of the allowed kinds, on every table today's code
    /// reads or writes: a newer build's db as an older build finds it.
    const FUTURE: &[&str] = &[
        "CREATE TABLE bookmarks (
             worktree TEXT PRIMARY KEY,
             note     TEXT NOT NULL
         );
         CREATE INDEX recent_repos_opened ON recent_repos (opened);",
        "ALTER TABLE settings ADD COLUMN scope TEXT NOT NULL DEFAULT '';
         ALTER TABLE commit_drafts ADD COLUMN amend INTEGER NOT NULL DEFAULT 0;
         ALTER TABLE open_tabs ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
         ALTER TABLE window_placement ADD COLUMN scale REAL;
         ALTER TABLE recent_repos ADD COLUMN name TEXT;
         ALTER TABLE repo_prefs ADD COLUMN changed INTEGER NOT NULL DEFAULT 0;
         ALTER TABLE workspace ADD COLUMN note TEXT;",
    ];

    fn with_future() -> Vec<&'static str> {
        MIGRATIONS.iter().chain(FUTURE).copied().collect()
    }

    /// Tables → their columns (`PRAGMA table_info` rows) and the indexes, of `conn`.
    fn schema(conn: &Connection) -> Vec<(String, Vec<String>)> {
        let names: Vec<(String, String)> = conn
            .prepare("SELECT type, name FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        names
            .into_iter()
            .map(|(kind, name)| {
                let pragma = if kind == "table" { "table_info" } else { "index_info" };
                let cols: Vec<String> = conn
                    .prepare(&format!("SELECT * FROM pragma_{pragma}('{name}')"))
                    .unwrap()
                    .query_map([], |r| {
                        let n = r.as_ref().column_count();
                        let mut cell = Vec::new();
                        for i in 0..n {
                            cell.push(format!("{:?}", r.get_ref(i)?));
                        }
                        Ok(cell.join("|"))
                    })
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap();
                (format!("{kind} {name}"), cols)
            })
            .collect()
    }

    /// Statement kinds a migration may contain (module docs: additive only).
    fn check_additive_sql(i: usize, sql: &str) {
        for stmt in sql.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            let words: Vec<String> = stmt
                .split_whitespace()
                .map(str::to_ascii_uppercase)
                .collect();
            let w = |i: usize| words.get(i).map(String::as_str);
            let ok = match (w(0), w(1)) {
                (Some("CREATE"), Some("TABLE" | "INDEX")) => true,
                (Some("CREATE"), Some("UNIQUE")) => w(2) == Some("INDEX"),
                (Some("ALTER"), Some("TABLE")) => {
                    w(3) == Some("ADD")
                        && (!stmt.to_ascii_uppercase().contains("NOT NULL")
                            || stmt.to_ascii_uppercase().contains("DEFAULT"))
                }
                _ => false,
            };
            assert!(ok, "migration {}: not additive: {stmt}", i + 1);
        }
    }

    #[gitti_test::test]
    fn every_migration_is_additive() {
        for (i, sql) in MIGRATIONS.iter().chain(FUTURE).enumerate() {
            check_additive_sql(i, sql);
        }
        // And by effect: each step keeps every table, column and index as it was.
        let mut conn = Connection::open_in_memory().unwrap();
        let all = with_future();
        let mut before = schema(&conn);
        for n in 1..=all.len() {
            migrate(&mut conn, &all[..n]).unwrap();
            let after = schema(&conn);
            for (name, cols) in &before {
                let now = after.iter().find(|(n, _)| n == name);
                let now = now.unwrap_or_else(|| panic!("migration {n} removed {name}"));
                assert!(
                    now.1.starts_with(cols),
                    "migration {n} changed {name}: {cols:?} -> {:?}",
                    now.1
                );
            }
            before = after;
        }
    }

    #[gitti_test::test]
    fn checker_rejects_destructive_migrations() {
        for bad in [
            "DROP TABLE settings",
            "ALTER TABLE settings RENAME TO s2",
            "ALTER TABLE settings DROP COLUMN value",
            "ALTER TABLE settings ADD COLUMN x TEXT NOT NULL",
            "UPDATE settings SET value = '1'",
            "DELETE FROM open_tabs",
        ] {
            let r = std::panic::catch_unwind(|| check_additive_sql(0, bad));
            assert!(r.is_err(), "accepted: {bad}");
        }
    }

    /// This file's SQL names its columns, so a column added by a later migration never
    /// shifts what an older build reads or writes.
    #[gitti_test::test]
    fn queries_name_their_columns() {
        let src = include_str!("store.rs");
        let code: String = src[..src.find("#[cfg(test)]").unwrap()]
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .map(|l| format!("{l}\n"))
            .collect();
        assert!(!code.contains(concat!("SELECT", " *")), "SELECT * in store.rs");
        for (at, _) in code.match_indices("INSERT INTO ") {
            let rest = code[at + "INSERT INTO ".len()..].trim_start();
            let table_end = rest
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            assert!(
                rest[table_end..].trim_start().starts_with('('),
                "INSERT without a column list: {}",
                &rest[..rest.len().min(60)]
            );
        }
    }

    /// Downgrade case: a newer build migrated the db, then an older build (here:
    /// today's code knowing only [`MIGRATIONS`]) starts — settings, tabs, window placement,
    /// recent repos and repo prefs all work, and the newer build finds its data again.
    #[gitti_test::test]
    fn older_build_works_on_a_newer_db() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gitti.db");
        let newer = with_future();
        {
            let conn = open_at_with(&path, &newer).unwrap();
            assert_eq!(level(&conn), newer.len());
            put_value(&conn, "font", &Value::from(17.0)).unwrap();
            put_value(&conn, "field_of_the_future", &Value::from("x")).unwrap();
            conn.execute(
                "INSERT INTO open_tabs (position, path, pinned) VALUES (0, '/old', 1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO bookmarks (worktree, note) VALUES ('/w', 'wip')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO commit_drafts (worktree, message, saved, amend)
                 VALUES ('/d', 'from the future', 1, 1)",
                [],
            )
            .unwrap();
        }
        // The older build.
        let mut conn = open_at(&path).unwrap();
        let s: S = load_settings_from(&conn);
        assert_eq!(s.font, 17.0, "known keys are read, unknown ones ignored");
        let (k, v) = ("wrap".to_string(), Value::from(true));
        write_changed(&mut conn, &[(k, v)]).unwrap();
        write_tabs(&conn, &[PathBuf::from("/a"), PathBuf::from("/b")], 1).unwrap();
        let p = WindowPlacement {
            display: "d".into(),
            x: 1.0,
            y: 2.0,
            width: 800.0,
            height: 600.0,
            state: WindowState::Maximized,
        };
        write_placement(&conn, "main", "setup", &p).unwrap();
        assert_eq!(read_placement(&conn, "main", "setup"), Some(p));
        write_recent(&conn, Path::new("/a"), 5).unwrap();
        assert_eq!(read_recent(&conn), [PathBuf::from("/a")]);
        write_repo_pref(&conn, Path::new("/a"), "push", &1u32).unwrap();
        assert_eq!(read_repo_pref::<u32>(&conn, Path::new("/a"), "push"), Some(1));
        let d = read_draft(&conn, Path::new("/d")).map(|d| d.message);
        assert_eq!(d.as_deref(), Some("from the future"));
        write_draft(&conn, Path::new("/d"), "rewritten", 2).unwrap();
        write_draft(&conn, Path::new("/e"), "new", 3).unwrap();
        assert_eq!(level(&conn), newer.len(), "an older build never lowers the level");
        assert!(user_version(&conn) <= LEGACY_MAX_USER_VERSION);
        drop(conn);
        // The newer build again: nothing re-applied, its data still there.
        let conn = open_at_with(&path, &newer).unwrap();
        let s: S = load_settings_from(&conn);
        assert_eq!((s.font, s.wrap), (17.0, true));
        let note: String = conn
            .query_row("SELECT note FROM bookmarks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(note, "wip");
        let drafts: Vec<(String, String, i64)> = conn
            .prepare("SELECT worktree, message, amend FROM commit_drafts ORDER BY worktree")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        // The upsert keeps the newer build's column; a new row gets its default.
        assert_eq!(
            drafts,
            [("/d".into(), "rewritten".into(), 1), ("/e".into(), "new".into(), 0)]
        );
        let tabs: Vec<(String, i64)> = conn
            .prepare("SELECT path, pinned FROM open_tabs ORDER BY position")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(tabs, [("/a".into(), 0), ("/b".into(), 0)]);
    }

    /// Every older level is brought up to date, and every build in between can use the
    /// result.
    #[gitti_test::test]
    fn any_level_upgrades_and_any_build_reads() {
        let all = with_future();
        for from in 0..=all.len() {
            for reader in MIGRATIONS.len()..=all.len() {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("gitti.db");
                drop(open_at_with(&path, &all[..from]).unwrap());
                let conn = open_at_with(&path, &all[..reader]).unwrap();
                assert_eq!(level(&conn), from.max(reader), "from {from}, reader {reader}");
                write_tabs(&conn, &[PathBuf::from("/x")], 0).unwrap();
                put_value(&conn, "font", &Value::from(15.0)).unwrap();
                let s: S = load_settings_from(&conn);
                assert_eq!(s.font, 15.0);
            }
        }
    }

    /// Builds from before the compatibility rule bail when `user_version` is above the 4
    /// migrations they know; it never gets there.
    #[gitti_test::test]
    fn user_version_stays_readable_for_builds_before_the_rule() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_at_with(&dir.path().join("gitti.db"), &with_future()).unwrap();
        assert_eq!(user_version(&conn), LEGACY_MAX_USER_VERSION);
        assert_eq!(level(&conn), MIGRATIONS.len() + FUTURE.len());
    }

    #[gitti_test::test]
    fn simultaneous_first_opens_migrate_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gitti.db");
        let all = with_future();
        std::thread::scope(|s| {
            let opens: Vec<_> = (0..4)
                .map(|_| s.spawn(|| open_at_with(&path, &all).map(|c| level(&c))))
                .collect();
            for o in opens {
                assert_eq!(o.join().unwrap().unwrap(), all.len());
            }
        });
    }

    #[gitti_test::test]
    fn per_field_load_survives_bad_and_unknown_values() {
        let (_dir, conn) = db();
        put_value(&conn, "font", &Value::from(18.0)).unwrap();
        put_value(&conn, "wrap", &Value::from("not a bool")).unwrap();
        put_value(&conn, "removed_field", &Value::from(1)).unwrap();
        let s: S = load_settings_from(&conn);
        assert_eq!(
            s,
            S {
                font: 18.0,
                ..S::default()
            }
        );
    }

    /// The commit list layout is stored per field like the others; a value this
    /// build doesn't know (a later build's choice) falls back to Auto, the rest stays.
    #[gitti_test::test]
    fn commit_list_layout_roundtrips() {
        use crate::list_layout::CommitListLayout;
        let (_dir, mut conn) = db();
        let after = SharedSettings {
            commit_list_layout: CommitListLayout::TwoLines,
            wrap_lines: true,
            ..SharedSettings::default()
        };
        let changed = [
            ("commit_list_layout".to_string(), Value::from("two_lines")),
            ("wrap_lines".to_string(), Value::from(true)),
        ];
        write_changed(&mut conn, &changed).unwrap();
        let s: SharedSettings = load_settings_from(&conn);
        assert_eq!(s, after);
        put_value(&conn, "commit_list_layout", &Value::from("three_lines")).unwrap();
        let s: SharedSettings = load_settings_from(&conn);
        assert_eq!((s.commit_list_layout, s.wrap_lines), (CommitListLayout::Auto, true));
    }

    #[gitti_test::test]
    fn only_changed_fields_are_written() {
        let (_dir, mut conn) = db();
        // Another process set the font; our stale copy changes only `wrap`.
        put_value(&conn, "font", &Value::from(20.0)).unwrap();
        let (k, v) = ("wrap".to_string(), Value::from(true));
        write_changed(&mut conn, &[(k, v)]).unwrap();
        let s: S = load_settings_from(&conn);
        assert_eq!((s.font, s.wrap), (20.0, true));
    }

    #[gitti_test::test]
    fn imports_json_files_once() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"font": 16.0, "wrap": true}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("workspace.json"),
            r#"{"repos": ["/a", "/b"], "active": 1}"#,
        )
        .unwrap();
        let conn = open_at(&dir.path().join("gitti.db")).unwrap();
        let s: S = load_settings_from(&conn);
        assert_eq!((s.font, s.wrap), (16.0, true));
        let n: i64 = conn
            .query_row("SELECT count(*) FROM open_tabs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
        assert!(!dir.path().join("settings.json").exists());
        assert!(dir.path().join("settings.json.imported").exists());
    }

    #[gitti_test::test]
    fn app_wide_values_roundtrip_beside_the_settings() {
        let (_dir, conn) = db();
        assert_eq!(read_setting::<Vec<String>>(&conn, "custom_commands"), None);
        put_value(&conn, "custom_commands", &serde_json::json!(["a", "b"])).unwrap();
        assert_eq!(
            read_setting::<Vec<String>>(&conn, "custom_commands"),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        // A value that no longer fits reads as none; the settings struct ignores the key.
        assert_eq!(read_setting::<u32>(&conn, "custom_commands"), None);
        let s: S = load_settings_from(&conn);
        assert_eq!(s, S::default());
    }

    #[gitti_test::test]
    fn tabs_roundtrip() {
        let (_dir, conn) = db();
        write_tabs(&conn, &[PathBuf::from("/x"), PathBuf::from("/y")], 1).unwrap();
        write_tabs(&conn, &[PathBuf::from("/z")], 0).unwrap();
        let paths: Vec<String> = conn
            .prepare("SELECT path FROM open_tabs ORDER BY position")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(paths, vec!["/z"]);
    }

    #[gitti_test::test]
    fn window_placement_per_window_and_setup() {
        let (_dir, conn) = db();
        let p = |x: f32, state| WindowPlacement {
            display: "d1".into(),
            x,
            y: -20.5,
            width: 1400.0,
            height: 900.0,
            state,
        };
        write_placement(&conn, "main", "laptop", &p(10.0, WindowState::Windowed)).unwrap();
        write_placement(&conn, "main", "docked", &p(2000.0, WindowState::Maximized)).unwrap();
        write_placement(&conn, "main", "laptop", &p(30.0, WindowState::Fullscreen)).unwrap();
        assert_eq!(
            read_placement(&conn, "main", "laptop"),
            Some(p(30.0, WindowState::Fullscreen))
        );
        assert_eq!(
            read_placement(&conn, "main", "docked"),
            Some(p(2000.0, WindowState::Maximized))
        );
        assert_eq!(read_placement(&conn, "merge", "laptop"), None);
    }

    #[gitti_test::test]
    fn recent_repos_newest_first_unique_and_bounded() {
        let (_dir, conn) = db();
        assert!(read_recent(&conn).is_empty());
        write_recent(&conn, Path::new("/a"), 1).unwrap();
        write_recent(&conn, Path::new("/b"), 2).unwrap();
        // Opening /a again moves it up instead of adding a second entry.
        write_recent(&conn, Path::new("/a"), 3).unwrap();
        assert_eq!(read_recent(&conn), [PathBuf::from("/a"), PathBuf::from("/b")]);
        for i in 0..MAX_RECENT as i64 + 5 {
            write_recent(&conn, &PathBuf::from(format!("/r{i}")), 10 + i).unwrap();
        }
        let list = read_recent(&conn);
        assert_eq!(list.len(), MAX_RECENT);
        assert_eq!(list[0], PathBuf::from(format!("/r{}", MAX_RECENT + 4)));
        assert!(!list.contains(&PathBuf::from("/a")), "oldest entries are dropped");
    }

    #[gitti_test::test]
    fn repo_prefs_per_repo_and_key() {
        let (_dir, conn) = db();
        let (a, b) = (Path::new("/a"), Path::new("/b"));
        assert_eq!(read_repo_pref::<Vec<String>>(&conn, a, "push"), None);
        write_repo_pref(&conn, a, "push", &vec!["origin"]).unwrap();
        write_repo_pref(&conn, b, "push", &vec!["backup"]).unwrap();
        write_repo_pref(&conn, a, "push", &vec!["origin", "backup"]).unwrap();
        assert_eq!(
            read_repo_pref::<Vec<String>>(&conn, a, "push"),
            Some(vec!["origin".to_string(), "backup".to_string()])
        );
        assert_eq!(
            read_repo_pref::<Vec<String>>(&conn, b, "push"),
            Some(vec!["backup".to_string()])
        );
        // A value that no longer fits the type is no value.
        assert_eq!(read_repo_pref::<u32>(&conn, a, "push"), None);
    }

    #[gitti_test::test]
    fn pane_fractions_roundtrip_and_fall_back() {
        let f = PaneLayout::fraction(400., 1500.);
        assert!((PaneLayout::size(f, 1500., 230.) - 400.).abs() < 0.01);
        // Relative: twice the width, twice the pane.
        assert!((PaneLayout::size(f, 3000., 230.) - 800.).abs() < 0.01);
        // Unset or broken values use the app's default.
        assert_eq!(PaneLayout::size(0., 1500., 230.), 230.);
        assert_eq!(PaneLayout::size(1.5, 1500., 230.), 230.);
        assert_eq!(PaneLayout::fraction(10., 0.), 0.);
    }
}
