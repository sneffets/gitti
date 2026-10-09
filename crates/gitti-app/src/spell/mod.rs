//! Spell checking of the commit message with the OS's checker: `NSSpellChecker` on macOS,
//! `ISpellChecker` on Windows, none on Linux (yet). Which words to check is
//! `gitti_core::spell::words`; this module answers "misspelled?" and "suggestions?" per
//! word.
//!
//! The checker runs on one background thread of its own (the platform objects stay on it;
//! COM is initialised there), so typing never waits for it. Verdicts are cached per word
//! for the whole process: the UI looks words up synchronously ([`Speller::verdict`]) and
//! sends the unknown ones to the thread ([`Speller::check`]), which answers once they are
//! in the cache. A word counts as correct if any of the languages accepts it — English
//! plus the user's preferred languages ([`pick_languages`]); `GITTI_SPELL_LANGS=en,de`
//! overrides the list.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod win;

/// Most suggestions offered for a word.
pub const MAX_SUGGESTIONS: usize = 8;

/// A platform spell checker, living on the checker thread.
pub trait Backend {
    /// The languages it checks against, in the checker's spelling.
    fn languages(&self) -> Vec<String>;
    /// Whether any of the checker's languages accepts `word`.
    fn is_correct(&mut self, word: &str) -> bool;
    /// Replacements for a misspelled `word`, best first, at most `max`.
    fn suggestions(&mut self, word: &str, max: usize) -> Vec<String>;
    /// Adds `word` to the user's dictionary (shared with other apps of the OS).
    fn learn(&mut self, word: &str);
}

/// What the checker said about a word.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    Correct,
    Misspelled(Arc<Vec<String>>),
}

enum Job {
    Check(Vec<String>, async_channel::Sender<()>),
    Learn(String),
}

/// The process-wide checker: verdict cache, session ignores, the checker thread.
pub struct Speller {
    verdicts: Mutex<HashMap<String, Verdict>>,
    /// "Ignore Spelling": correct for the rest of the session (all tabs).
    ignored: Mutex<HashSet<String>>,
    jobs: Mutex<Option<mpsc::Sender<Job>>>,
}

type Factory = Box<dyn FnOnce() -> Option<Box<dyn Backend>> + Send>;

/// A backend to use instead of the platform's (tests); must be set before the first use.
static OVERRIDE: Mutex<Option<Factory>> = Mutex::new(None);

impl Speller {
    pub fn global() -> &'static Speller {
        static SPELLER: OnceLock<Speller> = OnceLock::new();
        SPELLER.get_or_init(|| Speller {
            verdicts: Mutex::default(),
            ignored: Mutex::default(),
            jobs: Mutex::new(None),
        })
    }

    /// The cached verdict for `word` (`None` = not checked yet).
    pub fn verdict(&self, word: &str) -> Option<Verdict> {
        if self.ignored.lock().unwrap().contains(word) {
            return Some(Verdict::Correct);
        }
        self.verdicts.lock().unwrap().get(word).cloned()
    }

    /// Checks `words` on the checker thread; the receiver gets `()` (or closes) once their
    /// verdicts are cached. Without a checker on this platform the words stay unknown.
    pub fn check(&self, words: Vec<String>) -> async_channel::Receiver<()> {
        let (tx, rx) = async_channel::bounded(1);
        self.send(Job::Check(words, tx));
        rx
    }

    /// "Learn Spelling": into the OS user dictionary, correct from now on.
    pub fn learn(&self, word: &str) {
        self.verdicts
            .lock()
            .unwrap()
            .insert(word.to_string(), Verdict::Correct);
        self.send(Job::Learn(word.to_string()));
    }

    /// "Ignore Spelling": correct for the rest of this session.
    pub fn ignore(&self, word: &str) {
        self.ignored.lock().unwrap().insert(word.to_string());
    }

    fn send(&self, job: Job) {
        let mut jobs = self.jobs.lock().unwrap();
        let tx = jobs.get_or_insert_with(|| {
            let (tx, rx) = mpsc::channel();
            let factory = OVERRIDE.lock().unwrap().take();
            std::thread::Builder::new()
                .name("gitti-spell".into())
                .spawn(move || worker(factory, rx))
                .expect("spawning the spell checker thread");
            tx
        });
        let _ = tx.send(job);
    }
}

fn worker(factory: Option<Factory>, rx: mpsc::Receiver<Job>) {
    let mut backend = match factory {
        Some(f) => f(),
        None => platform_backend(),
    };
    let speller = Speller::global();
    for job in rx {
        let Some(b) = backend.as_mut() else {
            // No checker: answer (the words stay unknown, nothing is marked).
            continue;
        };
        match job {
            Job::Check(words, done) => {
                for word in words {
                    if speller.verdicts.lock().unwrap().contains_key(&word) {
                        continue;
                    }
                    let verdict = if b.is_correct(&word) {
                        Verdict::Correct
                    } else {
                        Verdict::Misspelled(Arc::new(b.suggestions(&word, MAX_SUGGESTIONS)))
                    };
                    speller.verdicts.lock().unwrap().insert(word, verdict);
                }
                let _ = done.try_send(());
            }
            Job::Learn(word) => b.learn(&word),
        }
    }
}

fn platform_backend() -> Option<Box<dyn Backend>> {
    if std::env::var("GITTI_SPELL").is_ok_and(|v| v == "off") {
        return None;
    }
    #[cfg(target_os = "macos")]
    {
        macos::MacSpeller::new().map(|b| Box::new(b) as Box<dyn Backend>)
    }
    #[cfg(target_os = "windows")]
    {
        win::WinSpeller::new().map(|b| Box::new(b) as Box<dyn Backend>)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

/// Uses a fixed word list instead of the OS checker: `misspelled` maps the wrong words to
/// their suggestions, every other word is correct. For tests and scripted runs; only
/// effective before the checker's first use.
pub fn use_word_list(misspelled: &[(&str, &[&str])]) {
    let list: HashMap<String, Vec<String>> = misspelled
        .iter()
        .map(|(w, s)| (w.to_string(), s.iter().map(|s| s.to_string()).collect()))
        .collect();
    *OVERRIDE.lock().unwrap() = Some(Box::new(move || {
        Some(Box::new(WordList(list)) as Box<dyn Backend>)
    }));
}

struct WordList(HashMap<String, Vec<String>>);

impl Backend for WordList {
    fn languages(&self) -> Vec<String> {
        vec!["word list".into()]
    }
    fn is_correct(&mut self, word: &str) -> bool {
        !self.0.contains_key(word)
    }
    fn suggestions(&mut self, word: &str, max: usize) -> Vec<String> {
        let mut s = self.0.get(word).cloned().unwrap_or_default();
        s.truncate(max);
        s
    }
    fn learn(&mut self, word: &str) {
        self.0.remove(word);
    }
}

/// The languages to check against: English, then the user's `preferred` ones (`de-DE`,
/// `en_GB`, …) — or `GITTI_SPELL_LANGS` (comma separated) when set.
pub fn wanted_languages(preferred: &[String]) -> Vec<String> {
    match std::env::var("GITTI_SPELL_LANGS") {
        Ok(list) => list.split(',').map(|s| s.trim().to_string()).collect(),
        Err(_) => std::iter::once("en".to_string())
            .chain(preferred.iter().cloned())
            .collect(),
    }
}

/// `wanted` languages mapped onto what the checker has (`available`): the exact tag, else
/// its base language (`de`), else the first variant of the base (`de_DE` / `de-CH`);
/// unavailable ones dropped. Comparison ignores case and `-` vs `_`; the result uses the
/// checker's spelling.
pub fn pick_languages(available: &[String], wanted: &[String]) -> Vec<String> {
    let norm = |s: &str| s.replace('_', "-").to_ascii_lowercase();
    let avail: Vec<(String, &String)> = available.iter().map(|a| (norm(a), a)).collect();
    let mut out: Vec<String> = Vec::new();
    for want in wanted {
        let w = norm(want);
        if w.is_empty() {
            continue;
        }
        let base = w.split('-').next().unwrap_or_default().to_string();
        let found = avail
            .iter()
            .find(|(a, _)| *a == w)
            .or_else(|| avail.iter().find(|(a, _)| *a == base))
            .or_else(|| avail.iter().find(|(a, _)| a.starts_with(&format!("{base}-"))));
        if let Some((_, orig)) = found
            && !out.contains(orig)
        {
            out.push((*orig).clone());
        }
    }
    out
}

/// Suggestion lists of several languages merged round-robin (every language's best guess
/// comes early), duplicates dropped, at most `max`.
pub fn interleave(lists: Vec<Vec<String>>, max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let longest = lists.iter().map(Vec::len).max().unwrap_or(0);
    for i in 0..longest {
        for list in &lists {
            if let Some(s) = list.get(i)
                && !out.contains(s)
            {
                out.push(s.clone());
            }
        }
    }
    out.truncate(max);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn suggestions_interleaved() {
        let l = |v: &[&str]| s(v);
        assert_eq!(
            interleave(vec![l(&["the", "tea", "ten"]), l(&["Tee", "the"])], 4),
            l(&["the", "Tee", "tea", "ten"])
        );
        assert!(interleave(vec![], 3).is_empty());
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[gitti_test::test]
    fn languages_english_first_then_preferred() {
        let mac = s(&["en", "en_GB", "de", "fr", "pt_BR"]);
        let pick = |avail: &[String], pref: &[&str]| {
            let mut wanted = s(&["en"]);
            wanted.extend(s(pref));
            pick_languages(avail, &wanted)
        };
        assert_eq!(pick(&mac, &["de-DE", "en-US"]), s(&["en", "de"]));
        assert_eq!(pick(&mac, &["en-GB"]), s(&["en", "en_GB"]));
        assert_eq!(pick(&mac, &["pt-PT", "xx"]), s(&["en", "pt_BR"]));
        let win = s(&["en-US", "de-DE", "fr-FR"]);
        assert_eq!(pick(&win, &["de-DE"]), s(&["en-US", "de-DE"]));
        assert_eq!(pick(&win, &["de-AT"]), s(&["en-US", "de-DE"]));
    }

    /// The OS checker (macOS / Windows): flags "teh", accepts "the"; with English + German
    /// (when the German dictionary is there) a German word too. Skips quietly where there
    /// is no checker.
    #[gitti_test::test]
    fn os_checker_flags_typos() {
        let wanted = ["en".to_string(), "de".to_string()];
        #[cfg(target_os = "macos")]
        let b = macos::MacSpeller::with_wanted(&wanted);
        #[cfg(target_os = "windows")]
        let b = win::WinSpeller::with_wanted(&wanted);
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let b: Option<WordList> = {
            let _ = wanted;
            None
        };
        let Some(mut b) = b else {
            eprintln!("no OS spell checker here — skipped");
            return;
        };
        // Windows has only the dictionaries of the languages added in its settings (e.g.
        // just `de-AT`): no English then.
        if b.languages().iter().any(|l| l.starts_with("en")) {
            assert!(b.is_correct("the"), "{:?}", b.languages());
            assert!(b.is_correct("commit"));
            assert!(!b.is_correct("teh"));
            assert!(!b.is_correct("wrnog"));
            let sugg = b.suggestions("teh", MAX_SUGGESTIONS);
            assert!(sugg.iter().any(|s| s == "the"), "suggestions for teh: {sugg:?}");
        } else {
            eprintln!("no English dictionary ({:?}) — English words not checked", b.languages());
        }
        if b.languages().iter().any(|l| l.starts_with("de")) {
            assert!(b.is_correct("Größe"), "{:?}", b.languages());
            assert!(b.is_correct("prüfen"));
            assert!(!b.is_correct("Grösze"));
        } else {
            eprintln!("no German dictionary ({:?}) — German words not checked", b.languages());
        }
    }
}
