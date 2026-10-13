//! Renders the app icon sources in `assets/icon/*.svg` into the platform formats that are
//! checked in next to them (so normal builds need no SVG renderer):
//!
//! - `gitti.icns` — macOS bundle icon (padded macOS artwork, 16–1024 px)
//! - `gitti-dock.png` — 512 px macOS artwork, set as the Dock icon at runtime
//! - `gitti.ico` — Windows, embedded into `gitti.exe` / `gitty.exe` by their build scripts
//! - `hicolor/<n>x<n>/apps/<APP_ID>.png` — Linux icon theme sizes
//! - `sources.hash` — hash of the SVGs; the build scripts warn when it no longer matches
//!
//! When `sources.hash` already matches the SVGs and every file is there, nothing is
//! rendered (`--force` renders anyway). Unchanged files are not rewritten, so running it
//! again triggers no rebuild.
//!
//! Small sizes get their own artwork, chosen by the edge length of the drawn square:
//! up to 24 px `gitti-small.svg`, up to 64 px `gitti-medium.svg` (pixel-aligned 3 px
//! lines — the scaled-down big artwork has ~2 px lines between pixels, whose straight and
//! curved parts blur to visibly different widths), above that `gitti.svg`.
//!
//! Lives in xtask (a small debug build) rather than as an example of gitti-app: an example
//! links the whole app library, which under the release profile meant a second fat-LTO link
//! of the gpui stack before every bundle / install build.
//!
//!   cargo xtask icons [--force]

use std::path::{Path, PathBuf};

use resvg::{tiny_skia, usvg};

use crate::{Failure, Result};

include!("../../gitti-app/icon_hash.rs");

/// Same id as `main::APP_ID` (Linux app_id, desktop file, icon name, macOS bundle id).
const APP_ID: &str = "io.github.sneffets.gitti";

/// The rounded square of `gitti.svg` without the macOS padding (x, y, size).
const BLEED: (f32, f32, f32) = (100.0, 100.0, 824.0);
/// Up to these edge lengths (px) the small / medium artwork is used.
const SMALL_MAX: f32 = 24.0;
const MEDIUM_MAX: f32 = 64.0;

/// Linux hicolor theme sizes.
const HICOLOR_SIZES: [u32; 8] = [16, 24, 32, 48, 64, 128, 256, 512];

pub fn run(args: &[String]) -> Result {
    let force = match args {
        [] => false,
        [f] if f == "--force" => true,
        _ => return Err(Failure::usage("usage: cargo xtask icons [--force]")),
    };
    let dir = crate::workspace_root().join("assets/icon");
    let hash = icon_sources_hash(&dir)?;
    let stored = std::fs::read_to_string(dir.join(ICON_HASH_FILE)).unwrap_or_default();
    if !force && stored.trim() == hash && outputs(&dir).iter().all(|p| p.is_file()) {
        println!("icons: up to date (assets/icon/*.svg unchanged, hash {hash})");
        return Ok(());
    }
    println!("icons: rendering assets/icon/*.svg into icns / ico / png …");
    render_all(&dir)?;
    println!("icons: done");
    Ok(())
}

/// Every file `render_all` writes.
fn outputs(dir: &Path) -> Vec<PathBuf> {
    let mut v = vec![dir.join("gitti.icns"), dir.join("gitti-dock.png"), dir.join("gitti.ico")];
    v.extend(HICOLOR_SIZES.iter().map(|s| hicolor_png(dir, *s)));
    v
}

fn hicolor_png(dir: &Path, size: u32) -> PathBuf {
    dir.join("hicolor").join(format!("{size}x{size}/apps/{APP_ID}.png"))
}

fn render_all(dir: &Path) -> Result {
    let big = load(&dir.join("gitti.svg"))?;
    let medium = load(&dir.join("gitti-medium.svg"))?;
    let small = load(&dir.join("gitti-small.svg"))?;
    let art = Art { big, medium, small };

    // macOS: the artwork keeps its padding inside the canvas.
    let icns_entries: [(&[u8; 4], u32); 10] = [
        (b"icp4", 16),
        (b"icp5", 32),
        (b"ic07", 128),
        (b"ic08", 256),
        (b"ic09", 512),
        (b"ic10", 1024),
        (b"ic11", 32),
        (b"ic12", 64),
        (b"ic13", 256),
        (b"ic14", 512),
    ];
    let mut icns = Vec::new();
    for (kind, size) in icns_entries {
        let png = art.macos(size)?;
        icns.extend_from_slice(kind);
        icns.extend_from_slice(&(png.len() as u32 + 8).to_be_bytes());
        icns.extend_from_slice(&png);
    }
    let mut file = b"icns".to_vec();
    file.extend_from_slice(&(icns.len() as u32 + 8).to_be_bytes());
    file.extend_from_slice(&icns);
    write(&dir.join("gitti.icns"), &file)?;
    write(&dir.join("gitti-dock.png"), &art.macos(512)?)?;

    // Windows: full-bleed, PNG-compressed entries (supported since Vista).
    let ico_sizes = [16u32, 20, 24, 32, 40, 48, 64, 128, 256];
    let pngs = ico_sizes
        .iter()
        .map(|&s| art.full_bleed(s))
        .collect::<Result<Vec<_>>>()?;
    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(ico_sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * ico_sizes.len() as u32;
    for (&size, png) in ico_sizes.iter().zip(&pngs) {
        let dim = if size >= 256 { 0 } else { size as u8 };
        ico.extend_from_slice(&[dim, dim, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes()); // planes
        ico.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for png in &pngs {
        ico.extend_from_slice(png);
    }
    write(&dir.join("gitti.ico"), &ico)?;

    // Linux: hicolor theme sizes, full-bleed like most desktop icons.
    for size in HICOLOR_SIZES {
        let path = hicolor_png(dir, size);
        std::fs::create_dir_all(path.parent().unwrap())?;
        write(&path, &art.full_bleed(size)?)?;
    }
    // Compared trimmed: a CRLF checkout on Windows must not count as a change.
    let hash = icon_sources_hash(dir)?;
    let hash_file = dir.join(ICON_HASH_FILE);
    if std::fs::read_to_string(&hash_file).unwrap_or_default().trim() != hash {
        write(&hash_file, format!("{hash}\n").as_bytes())?;
    }
    Ok(())
}

struct Art {
    big: usvg::Tree,
    medium: usvg::Tree,
    small: usvg::Tree,
}

impl Art {
    /// The artwork for a square of `edge` px and its source square (x, y, size).
    fn pick(&self, edge: f32) -> (&usvg::Tree, (f32, f32, f32)) {
        if edge <= SMALL_MAX {
            (&self.small, (0.0, 0.0, 32.0))
        } else if edge <= MEDIUM_MAX {
            (&self.medium, (0.0, 0.0, 48.0))
        } else {
            (&self.big, BLEED)
        }
    }

    /// Full-bleed square (Windows, Linux).
    fn full_bleed(&self, size: u32) -> Result<Vec<u8>> {
        let (tree, src) = self.pick(size as f32);
        render(tree, src, (0.0, size as f32), size)
    }

    /// macOS: the rounded square at 100/1024 inset, transparent around it.
    fn macos(&self, size: u32) -> Result<Vec<u8>> {
        let s = size as f32;
        let dst = (s * BLEED.0 / 1024.0, s * BLEED.2 / 1024.0);
        let (tree, src) = self.pick(dst.1);
        render(tree, src, dst, size)
    }
}

/// Renders the square `src` (x, y, size in SVG units) of `tree` into the square at
/// (`dst.0`, `dst.0`) with edge `dst.1` of a `size`² canvas, as PNG.
fn render(
    tree: &usvg::Tree,
    src: (f32, f32, f32),
    dst: (f32, f32),
    size: u32,
) -> Result<Vec<u8>> {
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("non-zero size");
    let scale = dst.1 / src.2;
    let transform = tiny_skia::Transform::from_row(
        scale,
        0.0,
        0.0,
        scale,
        dst.0 - src.0 * scale,
        dst.0 - src.1 * scale,
    );
    resvg::render(tree, transform, &mut pixmap.as_mut());
    pixmap.encode_png().map_err(|e| Failure::new(format!("PNG: {e}")))
}

fn load(path: &Path) -> Result<usvg::Tree> {
    let data = std::fs::read(path)?;
    usvg::Tree::from_data(&data, &usvg::Options::default())
        .map_err(|e| Failure::new(format!("{}: {e}", path.display())))
}

fn write(path: &Path, data: &[u8]) -> Result {
    if std::fs::read(path).is_ok_and(|old| old == data) {
        return Ok(());
    }
    std::fs::write(path, data)?;
    println!("  {} ({} bytes)", path.display(), data.len());
    Ok(())
}
