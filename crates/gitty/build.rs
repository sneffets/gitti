//! Embeds the app icon (`assets/icon/gitti.ico`) as icon resource 1 into the Windows
//! executable: Explorer and the taskbar show it, and gpui loads resource 1 for its windows.
//! On every platform it warns when the icon SVGs changed without rerunning `cargo xtask icons`,
//! and records the git commit the binary is built from (`build_info.rs`).

use std::path::PathBuf;

include!("../gitti-app/icon_hash.rs");
include!("../gitti-app/build_info.rs");

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    emit_build_info(&manifest_dir);
    let ico = manifest_dir.join("../../assets/icon/gitti.ico");
    println!("cargo:rerun-if-changed={}", ico.display());
    warn_if_icons_stale(&manifest_dir.join("../../assets/icon"));
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let rc = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("icon.rc");
    let ico = ico.display().to_string().replace('\\', "/");
    std::fs::write(&rc, format!("1 ICON \"{ico}\"\n")).unwrap();
    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
