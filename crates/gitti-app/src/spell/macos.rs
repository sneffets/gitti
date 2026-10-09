//! `NSSpellChecker` (AppKit). Used from the spell thread only; the shared checker may be
//! used off the main thread for checking. Each word is checked per language explicitly —
//! automatic language detection on a single word is a guess.

use objc2::rc::{Retained, autoreleasepool};
use objc2_app_kit::NSSpellChecker;
use objc2_foundation::{NSLocale, NSRange, NSString};

use super::Backend;

pub struct MacSpeller {
    checker: Retained<NSSpellChecker>,
    tag: isize,
    langs: Vec<Retained<NSString>>,
}

impl MacSpeller {
    /// English + the user's preferred languages (System Settings ▸ Language & Region).
    pub fn new() -> Option<Self> {
        let preferred: Vec<String> = autoreleasepool(|_| {
            NSLocale::preferredLanguages()
                .to_vec()
                .iter()
                .map(|s| s.to_string())
                .collect()
        });
        Self::with_wanted(&super::wanted_languages(&preferred))
    }

    /// Checks against `wanted` (tags like `en`, `de-DE`), as far as the system has them;
    /// `None` without any.
    pub fn with_wanted(wanted: &[String]) -> Option<Self> {
        autoreleasepool(|_| {
            let checker = NSSpellChecker::sharedSpellChecker();
            let available: Vec<String> = checker
                .availableLanguages()
                .to_vec()
                .iter()
                .map(|s| s.to_string())
                .collect();
            let langs: Vec<Retained<NSString>> = super::pick_languages(&available, wanted)
                .iter()
                .map(|l| NSString::from_str(l))
                .collect();
            if langs.is_empty() {
                return None;
            }
            let tag = NSSpellChecker::uniqueSpellDocumentTag();
            Some(Self {
                checker,
                tag,
                langs,
            })
        })
    }
}

impl Backend for MacSpeller {
    fn languages(&self) -> Vec<String> {
        self.langs.iter().map(|l| l.to_string()).collect()
    }

    fn is_correct(&mut self, word: &str) -> bool {
        autoreleasepool(|_| {
            let s = NSString::from_str(word);
            self.langs.iter().any(|lang| {
                // SAFETY: `wordCount` may be null.
                let r = unsafe {
                    self.checker
                        .checkSpellingOfString_startingAt_language_wrap_inSpellDocumentWithTag_wordCount(
                            &s,
                            0,
                            Some(lang),
                            false,
                            self.tag,
                            std::ptr::null_mut(),
                        )
                };
                r.length == 0
            })
        })
    }

    /// The guesses of every language, interleaved (each language's best first), without
    /// duplicates.
    fn suggestions(&mut self, word: &str, max: usize) -> Vec<String> {
        autoreleasepool(|_| {
            let s = NSString::from_str(word);
            let range = NSRange::new(0, s.length());
            let per_lang: Vec<Vec<String>> = self
                .langs
                .iter()
                .map(|lang| {
                    self.checker
                        .guessesForWordRange_inString_language_inSpellDocumentWithTag(
                            range,
                            &s,
                            Some(lang),
                            self.tag,
                        )
                        .map(|a| a.to_vec().iter().map(|g| g.to_string()).collect())
                        .unwrap_or_default()
                })
                .collect();
            super::interleave(per_lang, max)
        })
    }

    fn learn(&mut self, word: &str) {
        autoreleasepool(|_| self.checker.learnWord(&NSString::from_str(word)));
    }
}
