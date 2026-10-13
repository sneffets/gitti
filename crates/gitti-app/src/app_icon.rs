//! App identity and icon (sources and derived files in `assets/icon/`, regenerated with
//! `cargo xtask icons`).
//!
//! - Windows: `build.rs` embeds `gitti.ico` as icon resource 1, which gpui uses for its
//!   windows — nothing to do at runtime.
//! - Linux: windows carry [`APP_ID`] (Wayland app_id / X11 WM_CLASS); the desktop shell
//!   finds the icon through `assets/linux/<APP_ID>.desktop` (`scripts/install-linux.sh`).
//!   A debug build has an id of its own and writes its desktop entry itself
//!   ([`register_dev_build`]).
//! - macOS: a bundle (`scripts/bundle-macos.sh`) has `gitti.icns`; a bare binary
//!   (`cargo run`) would show the generic executable icon in the Dock, so [`set_dock_icon`]
//!   sets the same artwork at runtime.

/// Linux app_id / desktop file name / icon name, macOS bundle identifier. A debug build has
/// the `.Devel` id (the GNOME convention for development builds): its windows get a dock
/// entry of their own, "gitti (dev)", beside an installed gitti.
pub const APP_ID: &str = if cfg!(debug_assertions) {
    "io.github.sneffets.gitti.Devel"
} else {
    "io.github.sneffets.gitti"
};

#[cfg(target_os = "macos")]
pub fn set_dock_icon() {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    const PNG: &[u8] = include_bytes!("../../../assets/icon/gitti-dock.png");
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let data = NSData::with_bytes(PNG);
    if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
        unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set_dock_icon() {}

/// Debug builds on Linux: writes the desktop entry "gitti (dev)" for [`APP_ID`] (`Exec` =
/// this binary) and its icons into the user's data dir, so the shell shows our icon and
/// name for a build that `scripts/install-linux.sh` never installed. Only what changed is
/// rewritten; of several checkouts' debug builds the entry starts the one started last.
#[cfg(all(debug_assertions, unix, not(target_os = "macos")))]
pub fn register_dev_build() {
    if let Err(e) = dev_entry::register() {
        eprintln!("gitti: desktop entry of the debug build not written: {e}");
    }
}

#[cfg(not(all(debug_assertions, unix, not(target_os = "macos"))))]
pub fn register_dev_build() {}

#[cfg(all(debug_assertions, unix, not(target_os = "macos")))]
mod dev_entry {
    use std::io;
    use std::path::{Path, PathBuf};

    use super::APP_ID;

    /// One checked-in hicolor size (`cargo xtask icons`): (size, PNG).
    macro_rules! icon {
        ($size:literal) => {
            (
                $size,
                include_bytes!(concat!(
                    "../../../assets/icon/hicolor/",
                    $size,
                    "/apps/io.github.sneffets.gitti.png"
                )) as &[u8],
            )
        };
    }

    /// The icon theme sizes, installed under the `.Devel` name.
    const ICONS: &[(&str, &[u8])] = &[
        icon!("16x16"),
        icon!("24x24"),
        icon!("32x32"),
        icon!("48x48"),
        icon!("64x64"),
        icon!("128x128"),
        icon!("256x256"),
        icon!("512x512"),
    ];

    pub fn register() -> io::Result<()> {
        let Some(data) = data_dir() else {
            return Ok(());
        };
        let exe = std::env::current_exe()?;
        // Desktop entries are UTF-8: a binary under another path gets no entry.
        let Some(exe) = exe.to_str() else {
            return Ok(());
        };
        let hicolor = data.join("icons/hicolor");
        let mut icons_changed = false;
        for (size, png) in ICONS {
            icons_changed |=
                write_if_changed(&hicolor.join(format!("{size}/apps/{APP_ID}.png")), png)?;
        }
        // An icon cache (the install script makes one) hides icons it does not list.
        if icons_changed && hicolor.join("icon-theme.cache").exists() {
            let _ = std::process::Command::new("gtk-update-icon-cache")
                .args(["-q", "-t"])
                .arg(&hicolor)
                .status();
        }
        let entry = data.join(format!("applications/{APP_ID}.desktop"));
        write_if_changed(&entry, desktop_entry(exe).as_bytes())?;
        Ok(())
    }

    /// `$XDG_DATA_HOME` (only an absolute one counts), else `~/.local/share`.
    fn data_dir() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from)
            && dir.is_absolute()
        {
            return Some(dir);
        }
        Some(PathBuf::from(std::env::var_os("HOME")?).join(".local/share"))
    }

    /// Writes `bytes` to `path` unless it holds them already; true if it wrote.
    fn write_if_changed(path: &Path, bytes: &[u8]) -> io::Result<bool> {
        if std::fs::read(path).is_ok_and(|old| old == bytes) {
            return Ok(false);
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, bytes)?;
        Ok(true)
    }

    pub(super) fn desktop_entry(exe: &str) -> String {
        format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=gitti (dev)\n\
             GenericName=Git GUI\n\
             Comment=Debug build {}\n\
             Exec={} %f\n\
             Icon={APP_ID}\n\
             Terminal=false\n\
             Categories=Development;RevisionControl;\n\
             StartupWMClass={APP_ID}\n\
             StartupNotify=true\n",
            escape_value(exe),
            escape_value(&exec_arg(exe)),
        )
    }

    /// One `Exec` argument, quoted as the Desktop Entry spec says: `"` `` ` `` `$` `\` get a
    /// backslash, `%` (a field code otherwise) is doubled.
    fn exec_arg(arg: &str) -> String {
        let mut out = String::from('"');
        for c in arg.chars() {
            match c {
                '"' | '`' | '$' | '\\' => {
                    out.push('\\');
                    out.push(c);
                }
                '%' => out.push_str("%%"),
                _ => out.push(c),
            }
        }
        out.push('"');
        out
    }

    /// A string value of a desktop entry: backslash and line breaks escaped.
    fn escape_value(s: &str) -> String {
        let mut out = String::new();
        for c in s.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                _ => out.push(c),
            }
        }
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[gitti_test::test]
        fn exec_quotes_and_escapes_the_binary_path() {
            let entry = desktop_entry("/home/a b/\"q\"$`x\\100%/gitti");
            // Exec quoting first (\" \$ \` \\ %%), then the string-value escaping (\ → \\).
            assert!(
                entry.contains(
                    "\nExec=\"/home/a b/\\\\\"q\\\\\"\\\\$\\\\`x\\\\\\\\100%%/gitti\" %f\n"
                ),
                "{entry}"
            );
            assert!(
                entry.contains("\nIcon=io.github.sneffets.gitti.Devel\n"),
                "{entry}"
            );
            assert!(
                entry.contains("\nStartupWMClass=io.github.sneffets.gitti.Devel\n"),
                "{entry}"
            );
            assert!(entry.contains("\nName=gitti (dev)\n"), "{entry}");
        }

        #[gitti_test::test]
        fn line_breaks_in_the_path_stay_in_one_line() {
            let entry = desktop_entry("/tmp/a\nb/gitti");
            assert_eq!(entry.lines().count(), 11, "{entry}");
            assert!(
                entry.contains("Comment=Debug build /tmp/a\\nb/gitti\n"),
                "{entry}"
            );
        }
    }
}
