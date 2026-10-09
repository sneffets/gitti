//! Secure keyboard entry while a password / passphrase field is shown.
//!
//! macOS: `EnableSecureEventInput` (what Terminal's "Secure Keyboard Entry" and native
//! `NSSecureTextField`s use) keeps other processes from reading keystrokes via event taps.
//! gpui draws its own text fields, so we have to switch it on ourselves. It is a global,
//! counted state: every enable needs exactly one disable, and it must not stay on while
//! another app is in front — [`SecureInput`] tracks our part and releases it on drop.
//!
//! Windows has no per-app equivalent (low-level keyboard hooks cannot be blocked from user
//! mode). On Linux, Wayland already isolates keyboard input between clients; on X11 the
//! equivalent would be an exclusive keyboard grab, which gpui does not expose.

#[derive(Debug, Default)]
pub struct SecureInput {
    on: bool,
}

impl SecureInput {
    /// Turns our secure input request on or off (no-op if unchanged).
    pub fn set(&mut self, want: bool) {
        if want != self.on {
            platform::set(want);
            self.on = want;
        }
    }
}

impl Drop for SecureInput {
    fn drop(&mut self) {
        self.set(false);
    }
}

/// Whether secure input is on system-wide (from any process).
pub fn system_enabled() -> bool {
    platform::system_enabled()
}

#[cfg(target_os = "macos")]
mod platform {
    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn EnableSecureEventInput() -> i32;
        fn DisableSecureEventInput() -> i32;
        fn IsSecureEventInputEnabled() -> u8;
    }

    pub fn set(on: bool) {
        // SAFETY: plain HIToolbox calls without arguments; balanced by `SecureInput`.
        unsafe {
            if on {
                EnableSecureEventInput();
            } else {
                DisableSecureEventInput();
            }
        }
    }

    pub fn system_enabled() -> bool {
        // SAFETY: read-only query.
        unsafe { IsSecureEventInputEnabled() != 0 }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub fn set(_on: bool) {}

    pub fn system_enabled() -> bool {
        false
    }
}
