//! "Another program": what an editor, a formatter, a build, a sync client or a terminal
//! `git` does to the working tree while the app shows an older state. Plain file
//! operations (no git, except [`terminal_git`]); paths are full paths.

use std::fs;
use std::io;
use std::path::Path;
use std::process::Output;
use std::time::{Duration, SystemTime};

fn read(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

fn write(path: &Path, data: &[u8]) {
    fs::write(path, data).unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
}

/// The lines of `data` with their endings (`\n` or `\r\n`; the last one may have none).
fn split_lines(data: &[u8]) -> Vec<&[u8]> {
    data.split_inclusive(|&b| b == b'\n').collect()
}

/// `line` without its ending.
fn body(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn with_ending(line: &[u8], crlf: bool) -> Vec<u8> {
    let mut out = body(line).to_vec();
    if line.ends_with(b"\n") {
        out.extend_from_slice(if crlf { b"\r\n" } else { b"\n" });
    }
    out
}

/// Every line ending of `path` becomes CRLF (an editor set to Windows line endings).
pub fn crlf_all(path: &Path) {
    let data = read(path);
    let out: Vec<u8> = split_lines(&data).into_iter().flat_map(|l| with_ending(l, true)).collect();
    write(path, &out);
}

/// Only the lines `numbers` (1-based) of `path` get CRLF; the others keep theirs (an
/// editor that wrote the lines it touched with its own line ending).
pub fn crlf_lines(path: &Path, numbers: &[usize]) {
    let data = read(path);
    let out: Vec<u8> = split_lines(&data)
        .into_iter()
        .enumerate()
        .flat_map(|(i, l)| {
            if numbers.contains(&(i + 1)) { with_ending(l, true) } else { l.to_vec() }
        })
        .collect();
    write(path, &out);
}

/// Every line ending of `path` becomes LF.
pub fn lf_all(path: &Path) {
    let data = read(path);
    let out: Vec<u8> = split_lines(&data).into_iter().flat_map(|l| with_ending(l, false)).collect();
    write(path, &out);
}

/// The UTF-8 byte order mark.
pub const BOM: &[u8] = b"\xef\xbb\xbf";

/// A UTF-8 byte order mark in front of `path` (if it has none).
pub fn add_bom(path: &Path) {
    let data = read(path);
    if !data.starts_with(BOM) {
        write(path, &[BOM, &data].concat());
    }
}

/// `path` without its UTF-8 byte order mark.
pub fn strip_bom(path: &Path) {
    let data = read(path);
    if let Some(rest) = data.strip_prefix(BOM) {
        write(path, rest);
    }
}

/// A code page a file can be saved in again ([`reencode`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// ISO-8859-1: U+0000..=U+00FF as one byte each.
    Latin1,
    /// Windows-1252: Latin-1 plus € ‚ ƒ „ … † ‡ ˆ ‰ Š ‹ Œ Ž ‘ ’ “ ” • – — ˜ ™ š › œ ž Ÿ
    /// in 0x80..=0x9F.
    Cp1252,
    /// UTF-16 little endian with its byte order mark (as Windows editors write it): git
    /// sees a binary file.
    Utf16Le,
}

/// Windows-1252's 0x80..=0x9F (`None` = unassigned).
const CP1252_HIGH: [Option<char>; 32] = [
    Some('€'), None, Some('‚'), Some('ƒ'), Some('„'), Some('…'), Some('†'), Some('‡'),
    Some('ˆ'), Some('‰'), Some('Š'), Some('‹'), Some('Œ'), None, Some('Ž'), None,
    None, Some('‘'), Some('’'), Some('“'), Some('”'), Some('•'), Some('–'), Some('—'),
    Some('˜'), Some('™'), Some('š'), Some('›'), Some('œ'), None, Some('ž'), Some('Ÿ'),
];

/// `text` in `encoding`; panics on a character the code page doesn't have.
pub fn encode(text: &str, encoding: Encoding) -> Vec<u8> {
    match encoding {
        Encoding::Latin1 => text
            .chars()
            .map(|c| u8::try_from(u32::from(c)).unwrap_or_else(|_| panic!("{c:?} is not in Latin-1")))
            .collect(),
        Encoding::Cp1252 => text
            .chars()
            .map(|c| {
                if let Some(i) = CP1252_HIGH.iter().position(|&h| h == Some(c)) {
                    return 0x80 + i as u8;
                }
                match u32::from(c) {
                    n @ (0..=0x7f | 0xa0..=0xff) => n as u8,
                    _ => panic!("{c:?} is not in Windows-1252"),
                }
            })
            .collect(),
        Encoding::Utf16Le => {
            let mut out = vec![0xff, 0xfe];
            for unit in text.encode_utf16() {
                out.extend_from_slice(&unit.to_le_bytes());
            }
            out
        }
    }
}

/// `path` (UTF-8) saved again in `encoding`, as an editor does when the user picks
/// another code page.
pub fn reencode(path: &Path, encoding: Encoding) {
    let data = read(path);
    let text = String::from_utf8(data)
        .unwrap_or_else(|_| panic!("{} is not UTF-8", path.display()));
    write(path, &encode(&text, encoding));
}

/// `path` executable (`true`) or not (`false`) for everyone who may read it.
#[cfg(unix)]
pub fn chmod_x(path: &Path, executable: bool) {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = fs::metadata(path).unwrap().permissions().mode();
    let mode = if executable { mode | ((mode & 0o444) >> 2) } else { mode & !0o111 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

/// `path` replaced by a symlink to `target`. An error where the platform can't (Windows
/// without the privilege): the caller skips that part.
pub fn replace_with_symlink(path: &Path, target: &str) -> io::Result<()> {
    remove(path);
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, path);
    #[cfg(windows)]
    return std::os::windows::fs::symlink_file(target, path);
}

/// `path` (a file or a link) replaced by a folder holding `files` (`name`, `text`) — a
/// build that turns `out` into `out/`.
pub fn replace_with_dir(path: &Path, files: &[(&str, &str)]) {
    remove(path);
    fs::create_dir_all(path).unwrap();
    for (name, text) in files {
        write(&path.join(name), text.as_bytes());
    }
}

/// `path` deleted (a file, a link or a folder with everything in it); nothing when it
/// doesn't exist.
pub fn remove(path: &Path) {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(path).unwrap(),
        Ok(_) => fs::remove_file(path).unwrap(),
        Err(_) => {}
    }
}

/// `path` written with `data` and its modification time set to `mtime` — `cp -p`,
/// `rsync -t`, unpacking an archive, a sync client.
pub fn write_with_mtime(path: &Path, data: &[u8], mtime: SystemTime) {
    write(path, data);
    set_mtime(path, mtime);
}

/// `path`'s modification time `secs` seconds in the past (a file or a folder) — e.g. past
/// the "racily clean" window, or older than the app's last read.
pub fn age(path: &Path, secs: u64) {
    set_mtime(path, SystemTime::now() - Duration::from_secs(secs));
}

fn set_mtime(path: &Path, mtime: SystemTime) {
    let mut opts = fs::OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        // FILE_WRITE_ATTRIBUTES; FILE_FLAG_BACKUP_SEMANTICS opens folders too.
        opts.access_mode(0x100).custom_flags(0x0200_0000);
    }
    #[cfg(not(windows))]
    {
        if path.is_dir() {
            opts.read(true);
        } else {
            opts.write(true);
        }
    }
    let f = opts
        .open(path)
        .unwrap_or_else(|e| panic!("opening {}: {e}", path.display()));
    f.set_modified(mtime).unwrap();
}

/// A git command the user runs in a terminal next to the app (`git add`, `git stash`,
/// `git commit`, …) in the repository `dir`; its output whatever the exit code. The
/// user's global config is shut out like in the fixtures' git.
pub fn terminal_git(dir: &Path, args: &[&str]) -> Output {
    crate::git::run(dir, args, None)
}
