// Shared by `examples/make_icons.rs` and the build scripts of gitti-app and gitty (via
// `include!`): a hash over the icon SVG sources. `make_icons` stores it in
// `assets/icon/sources.hash`; the build scripts warn when the SVGs no longer match, i.e. the
// checked-in icns/ico/png files are stale. Content-based, because git checkouts make mtimes
// meaningless; `\r` is dropped so a CRLF checkout on Windows hashes the same.

/// The icon sources in `assets/icon`, in hash order.
const ICON_SOURCES: [&str; 3] = ["gitti.svg", "gitti-medium.svg", "gitti-small.svg"];

/// File name of the stored hash in `assets/icon`.
const ICON_HASH_FILE: &str = "sources.hash";

/// FNV-1a (64 bit) over all sources in `dir`, as 16 hex digits.
fn icon_sources_hash(dir: &std::path::Path) -> std::io::Result<String> {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for name in ICON_SOURCES {
        let data = std::fs::read(dir.join(name))?;
        for &byte in name.as_bytes().iter().chain([&0u8]).chain(&data) {
            if byte != b'\r' {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    Ok(format!("{hash:016x}"))
}

/// For build scripts: rebuild when the SVGs or the stored hash change, and warn while the
/// derived icon files are older than the SVGs.
#[allow(dead_code)]
fn warn_if_icons_stale(dir: &std::path::Path) {
    for name in ICON_SOURCES.iter().chain([&ICON_HASH_FILE]) {
        println!("cargo:rerun-if-changed={}", dir.join(name).display());
    }
    let stored = std::fs::read_to_string(dir.join(ICON_HASH_FILE)).unwrap_or_default();
    if icon_sources_hash(dir).ok().as_deref() != Some(stored.trim()) {
        println!(
            "cargo:warning=assets/icon/*.svg changed, the icon files are stale: \
             cargo run -p gitti-app --example make_icons"
        );
    }
}
