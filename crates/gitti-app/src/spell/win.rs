//! Windows spell checking API (`ISpellCheckerFactory` / `ISpellChecker`, Windows 8+). COM
//! is initialised (multi-threaded) on the spell thread, the checkers live there. One
//! checker per language; a word is correct if any of them accepts it.

use ::windows::Win32::Globalization::{
    GetUserPreferredUILanguages, ISpellChecker, ISpellCheckerFactory, MUI_LANGUAGE_NAME,
    SpellCheckerFactory,
};
use ::windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    IEnumString,
};
use ::windows::core::{HSTRING, PWSTR};

use super::Backend;

pub struct WinSpeller {
    checkers: Vec<(String, ISpellChecker)>,
}

impl WinSpeller {
    /// English + the user's preferred UI languages (Settings ▸ Time & language).
    pub fn new() -> Option<Self> {
        Self::with_wanted(&super::wanted_languages(&preferred_ui_languages()))
    }

    /// Checks against `wanted` (tags like `en`, `de-DE`), as far as the system has them;
    /// `None` without any.
    pub fn with_wanted(wanted: &[String]) -> Option<Self> {
        // SAFETY: plain COM calls on this thread; strings are owned HSTRINGs.
        unsafe {
            // S_FALSE (already initialised) is fine; a different apartment model fails
            // here but COM still works for in-process objects then.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let factory: ISpellCheckerFactory =
                CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER).ok()?;
            let available = enum_strings(&factory.SupportedLanguages().ok()?);
            let checkers: Vec<(String, ISpellChecker)> =
                super::pick_languages(&available, wanted)
                    .into_iter()
                    .filter_map(|lang| {
                        let c = factory
                            .CreateSpellChecker(&HSTRING::from(lang.as_str()))
                            .ok()?;
                        Some((lang, c))
                    })
                    .collect();
            (!checkers.is_empty()).then_some(Self { checkers })
        }
    }
}

impl Backend for WinSpeller {
    fn languages(&self) -> Vec<String> {
        self.checkers.iter().map(|(l, _)| l.clone()).collect()
    }

    fn is_correct(&mut self, word: &str) -> bool {
        let w = HSTRING::from(word);
        self.checkers.iter().any(|(_, c)| {
            // SAFETY: COM calls on the thread that created the checker.
            unsafe {
                let Ok(errors) = c.Check(&w) else {
                    return true;
                };
                let mut first = None;
                let hr = errors.Next(&mut first);
                // S_OK with an error = misspelled; S_FALSE = none.
                !(hr.0 == 0 && first.is_some())
            }
        })
    }

    fn suggestions(&mut self, word: &str, max: usize) -> Vec<String> {
        let w = HSTRING::from(word);
        let lists = self
            .checkers
            .iter()
            .map(|(_, c)| {
                // SAFETY: as above.
                unsafe { c.Suggest(&w).map(|e| enum_strings(&e)).unwrap_or_default() }
            })
            .collect();
        super::interleave(lists, max)
    }

    fn learn(&mut self, word: &str) {
        let w = HSTRING::from(word);
        // The user dictionary is per language; add it to all.
        for (_, c) in &self.checkers {
            // SAFETY: as above.
            let _ = unsafe { c.Add(&w) };
        }
    }
}

/// All strings of a COM string enumeration (each freed after copying).
fn enum_strings(e: &IEnumString) -> Vec<String> {
    let mut out = Vec::new();
    loop {
        let mut item = [PWSTR::null()];
        let mut fetched = 0u32;
        // SAFETY: one slot, `fetched` reports whether it was filled; the callee allocates
        // the string with the COM allocator, freed here.
        unsafe {
            let hr = e.Next(&mut item, Some(&mut fetched));
            if hr.is_err() || fetched == 0 || item[0].is_null() {
                break;
            }
            out.push(item[0].to_string().unwrap_or_default());
            CoTaskMemFree(Some(item[0].0 as *const _));
        }
    }
    out
}

/// The user's preferred UI languages (`de-DE`, `en-US`, …).
fn preferred_ui_languages() -> Vec<String> {
    let mut num = 0u32;
    let mut len = 0u32;
    // SAFETY: the first call sizes the buffer, the second fills it (a double-NUL
    // terminated list of NUL terminated names).
    unsafe {
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut num, None, &mut len).is_err()
            || len == 0
        {
            return Vec::new();
        }
        let mut buf = vec![0u16; len as usize];
        if GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut num,
            Some(PWSTR(buf.as_mut_ptr())),
            &mut len,
        )
        .is_err()
        {
            return Vec::new();
        }
        buf.split(|&c| c == 0)
            .filter(|s| !s.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    }
}
