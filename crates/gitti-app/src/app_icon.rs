//! App identity and icon (sources and derived files in `assets/icon/`, regenerated with
//! `cargo run -p gitti-app --example make_icons`).
//!
//! - Windows: `build.rs` embeds `gitti.ico` as icon resource 1, which gpui uses for its
//!   windows — nothing to do at runtime.
//! - Linux: windows carry [`APP_ID`] (Wayland app_id / X11 WM_CLASS); the desktop shell
//!   finds the icon through `assets/linux/<APP_ID>.desktop` (`scripts/install-linux.sh`).
//! - macOS: a bundle (`scripts/bundle-macos.sh`) has `gitti.icns`; a bare binary
//!   (`cargo run`) would show the generic executable icon in the Dock, so [`set_dock_icon`]
//!   sets the same artwork at runtime.

/// Linux app_id / desktop file name / icon name, macOS bundle identifier.
pub const APP_ID: &str = "io.github.sneffets.gitti";

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
