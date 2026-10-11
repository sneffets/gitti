//! `cargo xtask licenses`: the third-party licence list both apps embed (About ▸
//! Third-Party Licences…, `gitty --licenses`) and release packages ship:
//! `licenses/THIRD-PARTY-LICENSES.txt`, generated and checked in.
//!
//! What is listed: the crates compiled into the `gitti` and `gitty` binaries — normal
//! dependencies only (no build / dev dependencies, no proc macros: their code doesn't end
//! up in the binary), resolved by `cargo tree` per target platform, so features and
//! `cfg(target_os)` dependencies are exactly those of a release build. Each crate line
//! says which binaries and platforms contain it; the apps show only their own. Licence
//! texts come from the crates' own licence files (`LICENSE*`, `COPYING*`, `NOTICE*`, …,
//! and `license-file`), each distinct text once. A crate that ships no licence file gets
//! the standard text of its licence with its authors as the copyright holders.
//!
//! Only cargo is needed (offline, from the local registry cache) — no extra tool. Every
//! crate's licence must allow a GPL-3.0-compatible choice, otherwise this fails.
//! The file carries a fingerprint of `Cargo.lock`; a test fails when the lock changed
//! without regenerating.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{Failure, Result};

/// The generated list, relative to the workspace root.
pub const OUT: &str = "licenses/THIRD-PARTY-LICENSES.txt";

/// The header line carrying the lock's fingerprint.
const FINGERPRINT: &str = "Cargo.lock fingerprint: ";

/// (package, binary name).
const BINARIES: &[(&str, &str)] = &[("gitti-app", "gitti"), ("gitty", "gitty")];

/// Release platforms: the name the apps match against `std::env::consts::OS`, and its
/// target triples (a crate counts for the platform when one of them compiles it in).
const PLATFORMS: &[(&str, &[&str])] = &[
    ("macos", &["aarch64-apple-darwin", "x86_64-apple-darwin"]),
    (
        "linux",
        &["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"],
    ),
    (
        "windows",
        &["x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"],
    ),
];

/// Material that isn't a crate: (name, version, licence, binaries, text file).
const EXTRAS: &[(&str, &str, &str, &[&str], &str)] = &[(
    "Lucide icons",
    "-",
    "ISC AND MIT",
    &["gitti"],
    "licenses/LUCIDE.txt",
)];

/// Standard licence texts for crates that ship no licence file, in order of preference
/// when the crate offers a choice: (SPDX id, file relative to the workspace root, drop
/// the lines starting with "Copyright" — our own copyright in `LICENSE-MIT`).
const STANDARD: &[(&str, &str, bool)] = &[
    ("MIT", "LICENSE-MIT", true),
    ("Apache-2.0", "LICENSE-APACHE", false),
    ("CC0-1.0", "licenses/standard/CC0-1.0.txt", false),
];

/// Licences a GPL-3.0-or-later program may contain (CLAUDE.md: dependencies stay
/// GPL-3.0-compatible). A crate passes when its expression allows a choice of only these.
const GPL3_COMPATIBLE: &[&str] = &[
    "0BSD",
    "Apache-2.0",
    "BSD-1-Clause",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "BSL-1.0",
    "bzip2-1.0.6",
    "CC0-1.0",
    "GPL-2.0-or-later",
    "GPL-3.0",
    "GPL-3.0-only",
    "GPL-3.0-or-later",
    "ISC",
    "LGPL-2.1-or-later",
    "LGPL-3.0",
    "LGPL-3.0-only",
    "LGPL-3.0-or-later",
    "MIT",
    "MIT-0",
    "MPL-2.0",
    "NCSA",
    "Unicode-3.0",
    "Unicode-DFS-2016",
    "Unlicense",
    "Zlib",
];

pub fn run(args: &[String]) -> Result {
    let root = crate::workspace_root();
    let check = match args {
        [] => false,
        [a] if a == "--check" => true,
        _ => return Err(Failure::usage("cargo xtask licenses [--check]")),
    };
    let text = generate(&root)?;
    let out = root.join(OUT);
    if check {
        let old = std::fs::read_to_string(&out).unwrap_or_default();
        if old != text {
            return Err(Failure::new(format!(
                "{OUT} is out of date: run `cargo xtask licenses` and commit it"
            )));
        }
        println!("{OUT} is up to date");
        return Ok(());
    }
    std::fs::write(&out, &text)?;
    let crates = text
        .lines()
        .filter(|l| l.contains(" | "))
        .count()
        .saturating_sub(1);
    println!(
        "wrote {OUT} ({crates} components, {} KB)",
        text.len() / 1024
    );
    Ok(())
}

/// One crate of the list.
#[derive(Default)]
struct Component {
    name: String,
    version: String,
    licence: String,
    /// Binary → the platforms it is compiled in for.
    used: BTreeMap<String, BTreeSet<&'static str>>,
    /// Licence texts (normalised).
    texts: Vec<String>,
    note: String,
}

fn generate(root: &Path) -> Result<String> {
    let meta = cargo_metadata(root)?;
    let lock = std::fs::read_to_string(root.join("Cargo.lock"))?;
    let mut comps: BTreeMap<(String, String), Component> = BTreeMap::new();
    for (package, bin) in BINARIES {
        for (os, triples) in PLATFORMS {
            for triple in *triples {
                for (name, version) in cargo_tree(root, package, triple)? {
                    let Some(pkg) = meta.get(&(name.clone(), version.clone())) else {
                        return Err(Failure::new(format!(
                            "{name} {version}: not in `cargo metadata`"
                        )));
                    };
                    // Our own crates (path dependencies) are not third-party.
                    if pkg.source.is_none() {
                        continue;
                    }
                    let c = comps.entry((name.clone(), version.clone())).or_default();
                    c.used.entry(bin.to_string()).or_default().insert(os);
                }
            }
        }
    }
    if comps.is_empty() {
        return Err(Failure::new("`cargo tree` listed no dependencies"));
    }
    let apache = standard_text(root, "Apache-2.0")?;
    let mut problems = Vec::new();
    for ((name, version), c) in comps.iter_mut() {
        let pkg = &meta[&(name.clone(), version.clone())];
        c.name = name.clone();
        c.version = version.clone();
        c.licence = pkg
            .license
            .as_deref()
            .map(normalize_expression)
            .unwrap_or_else(|| "see its licence text".into());
        if pkg.license.is_some() && !gpl3_compatible(&c.licence) {
            problems.push(format!(
                "{name} {version}: {} is not GPL-3.0-compatible",
                c.licence
            ));
        }
        c.texts = licence_files(pkg, &apache)?;
        if c.texts.is_empty() {
            let Some(ids) = pkg
                .license
                .as_deref()
                .and_then(|l| standard_choice(&normalize_expression(l)))
            else {
                problems.push(format!(
                    "{name} {version}: ships no licence file and has no standard text for {:?} \
                     (add one to STANDARD in crates/xtask/src/licenses.rs)",
                    pkg.license
                ));
                continue;
            };
            let holders = if pkg.authors.is_empty() {
                format!("the {name} authors")
            } else {
                pkg.authors.join(", ")
            };
            for id in &ids {
                c.texts.push(standard_text(root, id)?);
            }
            c.note = format!(
                "no licence file in the package: standard {} text; copyright: {holders}",
                ids.join(" and ")
            );
        }
    }
    if !problems.is_empty() {
        return Err(Failure::new(problems.join("\n")));
    }
    for (name, version, licence, bins, file) in EXTRAS {
        let text = normalize_text(&std::fs::read_to_string(root.join(file))?);
        let all: BTreeSet<&'static str> = PLATFORMS.iter().map(|(os, _)| *os).collect();
        comps.insert(
            (name.to_lowercase(), version.to_string()),
            Component {
                name: name.to_string(),
                version: version.to_string(),
                licence: licence.to_string(),
                used: bins.iter().map(|b| (b.to_string(), all.clone())).collect(),
                texts: vec![text],
                note: format!("the licence file {file}"),
            },
        );
    }
    Ok(render(&comps, &lock_fingerprint(&lock)))
}

fn render(comps: &BTreeMap<(String, String), Component>, fingerprint: &str) -> String {
    // Each distinct text once, numbered in the order of first use.
    let mut ids: BTreeMap<&str, usize> = BTreeMap::new();
    let mut texts: Vec<&str> = Vec::new();
    let mut rows = Vec::new();
    for c in comps.values() {
        let mut nums = Vec::new();
        for t in &c.texts {
            let n = *ids.entry(t.as_str()).or_insert_with(|| {
                texts.push(t);
                texts.len()
            });
            if !nums.contains(&n) {
                nums.push(n);
            }
        }
        let used = c
            .used
            .iter()
            .map(|(bin, oses)| {
                if oses.len() == PLATFORMS.len() {
                    bin.clone()
                } else {
                    format!(
                        "{bin}[{}]",
                        oses.iter().copied().collect::<Vec<_>>().join(",")
                    )
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        let nums = nums
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        let mut row = format!(
            "{:<32} | {:<14} | {} | {used} | {nums}",
            c.name, c.version, c.licence
        );
        if !c.note.is_empty() {
            let _ = write!(row, " | {}", c.note);
        }
        rows.push(row);
    }
    let mut out = String::new();
    out.push_str("Third-party licences of gitti and gitty\n");
    out.push_str("Generated by `cargo xtask licenses` from Cargo.lock; do not edit.\n");
    let _ = writeln!(out, "{FINGERPRINT}{fingerprint}");
    out.push_str(
        "\ngitti and gitty contain the open-source components listed below, each used under \
         the licence named\nwith it; their licence texts follow the list. \"used in\" names \
         the binaries (and, when not all,\nthe platforms) that contain a component. The apps \
         show their own part of this list in\nHelp > Third-Party Licences (gitti) and \
         `gitty --licenses`.\n\n",
    );
    out.push_str("==== components ====\n");
    let _ = writeln!(
        out,
        "{:<32} | {:<14} | licence | used in | licence texts | note",
        "# name", "version"
    );
    for r in rows {
        out.push_str(&r);
        out.push('\n');
    }
    for (i, t) in texts.iter().enumerate() {
        let _ = write!(out, "\n==== licence text {} ====\n{t}\n", i + 1);
    }
    out
}

/// What the list needs of a package from `cargo metadata`.
struct Package {
    source: Option<String>,
    license: Option<String>,
    license_file: Option<String>,
    authors: Vec<String>,
    dir: PathBuf,
}

fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

fn cargo_metadata(root: &Path) -> Result<BTreeMap<(String, String), Package>> {
    let out = cargo()
        .args(["metadata", "--format-version", "1", "--locked", "--offline"])
        .current_dir(root)
        .output()?;
    if !out.status.success() {
        return Err(Failure::new(format!(
            "cargo metadata failed (are all crates fetched? `cargo fetch`):\n{}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| Failure::new(format!("cargo metadata: {e}")))?;
    let mut map = BTreeMap::new();
    for p in v["packages"].as_array().into_iter().flatten() {
        let s = |k: &str| p[k].as_str().map(str::to_string);
        let manifest = PathBuf::from(s("manifest_path").unwrap_or_default());
        map.insert(
            (
                s("name").unwrap_or_default(),
                s("version").unwrap_or_default(),
            ),
            Package {
                source: s("source"),
                license: s("license"),
                license_file: s("license_file"),
                authors: p["authors"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|a| a.as_str().map(str::to_string))
                    .collect(),
                dir: manifest.parent().map(Path::to_path_buf).unwrap_or_default(),
            },
        );
    }
    Ok(map)
}

/// The (name, version) of every crate compiled into `package` for `triple`.
fn cargo_tree(root: &Path, package: &str, triple: &str) -> Result<BTreeSet<(String, String)>> {
    let out = cargo()
        .args([
            "tree",
            "--locked",
            "--offline",
            "-p",
            package,
            "--target",
            triple,
        ])
        .args([
            "-e",
            "normal,no-proc-macro",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .current_dir(root)
        .output()?;
    if !out.status.success() {
        return Err(Failure::new(format!(
            "cargo tree -p {package} --target {triple} failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            // "name v1.2.3", then " (path)" / " (*)" for repeats.
            let mut parts = l.split_whitespace();
            let name = parts.next()?;
            let version = parts.next()?.strip_prefix('v')?;
            Some((name.to_string(), version.to_string()))
        })
        .collect())
}

/// The texts of a package's licence files: in its folder, names with LICENSE / LICENCE /
/// COPYING / COPYRIGHT / NOTICE / UNLICENSE (also a folder of them, one level), plus
/// `license-file`. A copy of the Apache License 2.0 (re-formatted, `http` links, …) is
/// replaced by `apache`, our standard copy — the licence asks for a copy of itself — and a
/// copyright line its appendix was filled in with is kept as a text of its own.
fn licence_files(pkg: &Package, apache: &str) -> Result<Vec<String>> {
    let is_licence = |name: &str| {
        let up = name.to_ascii_uppercase();
        let ext = Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        ["LICENSE", "LICENCE", "COPYING", "COPYRIGHT", "NOTICE"]
            .iter()
            .any(|k| up.contains(k))
            && matches!(
                ext.to_ascii_lowercase().as_str(),
                "" | "txt" | "md" | "markdown" | "rst"
            )
            || ["mit", "apache", "apache-2.0", "bsd", "zlib", "isc"]
                .iter()
                .any(|k| up == k.to_ascii_uppercase())
    };
    let mut files: BTreeSet<PathBuf> = BTreeSet::new();
    let mut entries: Vec<_> = std::fs::read_dir(&pkg.dir)?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        if !is_licence(&name) {
            continue;
        }
        let path = e.path();
        if path.is_dir() {
            let mut inner: Vec<_> = std::fs::read_dir(&path)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.is_file())
                .collect();
            inner.sort();
            files.extend(inner);
        } else if path.is_file() {
            files.insert(path);
        }
    }
    if let Some(f) = &pkg.license_file {
        let p = pkg.dir.join(f);
        if p.is_file() {
            files.insert(p);
        }
    }
    let mut texts = Vec::new();
    for f in files {
        let bytes = std::fs::read(&f)?;
        let t = normalize_text(&String::from_utf8_lossy(&bytes));
        let parts = match apache_appendix(&t, apache) {
            Some(copyright) => [Some(apache.to_string()), copyright]
                .into_iter()
                .flatten()
                .collect(),
            None => vec![t],
        };
        for t in parts {
            if !t.is_empty() && !texts.contains(&t) {
                texts.push(t);
            }
        }
    }
    Ok(texts)
}

/// `Some` when `t` is the Apache License 2.0 (same terms as `apache`, compared by words
/// only), with the copyright lines its appendix was filled in with, if any.
fn apache_appendix(t: &str, apache: &str) -> Option<Option<String>> {
    const END: &str = "END OF TERMS AND CONDITIONS";
    let terms = |t: &str| -> Option<String> {
        let start = t.find("TERMS AND CONDITIONS FOR USE")?;
        let end = t.find(END).unwrap_or(t.len());
        let words = t
            .get(start..end)?
            .to_lowercase()
            .replace("https://", "http://");
        Some(
            words
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .collect(),
        )
    };
    if !t.contains("Apache License") || terms(t)? != terms(apache)? {
        return None;
    }
    let tail = &t[t.find(END).unwrap_or(t.len())..];
    let own: Vec<&str> = tail
        .lines()
        .map(str::trim)
        .filter(|l| {
            l.starts_with("Copyright")
                && !l.contains("yyyy")
                && !l.contains("name of copyright owner")
        })
        .collect();
    Some((!own.is_empty()).then(|| own.join("\n")))
}

/// Line ends LF, no trailing blanks, no leading / trailing empty lines, no BOM; a line
/// that would read as one of the list's markers gets a space in front.
fn normalize_text(t: &str) -> String {
    let t = t
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let lines: Vec<String> = t
        .lines()
        .map(|l| {
            let l = l.trim_end().replace('\t', "    ");
            if l.starts_with("====") {
                format!(" {l}")
            } else {
                l
            }
        })
        .collect();
    let first = lines
        .iter()
        .position(|l| !l.is_empty())
        .unwrap_or(lines.len());
    let last = lines
        .iter()
        .rposition(|l| !l.is_empty())
        .map_or(first, |i| i + 1);
    lines[first..last].join("\n")
}

fn standard_text(root: &Path, id: &str) -> Result<String> {
    let (_, file, drop_copyright) = STANDARD
        .iter()
        .find(|(i, _, _)| *i == id)
        .ok_or_else(|| Failure::new(format!("no standard text for {id}")))?;
    let t = std::fs::read_to_string(root.join(file))?;
    let t: String = t
        .lines()
        .filter(|l| !(*drop_copyright && l.starts_with("Copyright")))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(normalize_text(&t))
}

/// Cargo's old `MIT/Apache-2.0` → `MIT OR Apache-2.0`, single spaces.
pub fn normalize_expression(e: &str) -> String {
    e.replace('/', " OR ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A licence expression: an id (with an exception), or all / one of several.
#[derive(Debug, PartialEq)]
enum Expr {
    Id(String),
    And(Vec<Expr>),
    Or(Vec<Expr>),
}

/// Parses an SPDX expression (`OR` binds weaker than `AND`; `WITH` stays with its id).
fn parse_expression(e: &str) -> Option<Expr> {
    let spaced = e.replace('(', " ( ").replace(')', " ) ");
    let tokens: Vec<&str> = spaced.split_whitespace().collect();
    let mut pos = 0;
    let expr = parse_or(&tokens, &mut pos)?;
    (pos == tokens.len()).then_some(expr)
}

fn parse_or(t: &[&str], pos: &mut usize) -> Option<Expr> {
    let mut alts = vec![parse_and(t, pos)?];
    while t.get(*pos).is_some_and(|w| w.eq_ignore_ascii_case("OR")) {
        *pos += 1;
        alts.push(parse_and(t, pos)?);
    }
    Some(if alts.len() == 1 {
        alts.pop()?
    } else {
        Expr::Or(alts)
    })
}

fn parse_and(t: &[&str], pos: &mut usize) -> Option<Expr> {
    let mut all = vec![parse_atom(t, pos)?];
    while t.get(*pos).is_some_and(|w| w.eq_ignore_ascii_case("AND")) {
        *pos += 1;
        all.push(parse_atom(t, pos)?);
    }
    Some(if all.len() == 1 {
        all.pop()?
    } else {
        Expr::And(all)
    })
}

fn parse_atom(t: &[&str], pos: &mut usize) -> Option<Expr> {
    let w = *t.get(*pos)?;
    *pos += 1;
    if w == "(" {
        let e = parse_or(t, pos)?;
        (t.get(*pos) == Some(&")")).then(|| *pos += 1)?;
        return Some(e);
    }
    if matches!(w, ")")
        || ["OR", "AND", "WITH"]
            .iter()
            .any(|k| w.eq_ignore_ascii_case(k))
    {
        return None;
    }
    // `Apache-2.0 WITH LLVM-exception`: the exception only adds permissions.
    if t.get(*pos).is_some_and(|w| w.eq_ignore_ascii_case("WITH")) {
        t.get(*pos + 1)?;
        *pos += 2;
    }
    Some(Expr::Id(w.trim_end_matches('+').to_string()))
}

/// Some choice under `e` uses only GPL-3.0-compatible licences.
fn gpl3_compatible(e: &str) -> bool {
    fn ok(e: &Expr) -> bool {
        match e {
            Expr::Id(id) => GPL3_COMPATIBLE.contains(&id.as_str()),
            Expr::And(all) => all.iter().all(ok),
            Expr::Or(alts) => alts.iter().any(ok),
        }
    }
    parse_expression(e).is_some_and(|e| ok(&e))
}

/// The licences whose standard texts cover a package without licence files: the first
/// choice (in [`STANDARD`]'s order of preference) we have texts for.
fn standard_choice(e: &str) -> Option<Vec<String>> {
    fn choose(e: &Expr) -> Option<Vec<String>> {
        match e {
            Expr::Id(id) => STANDARD
                .iter()
                .any(|(i, _, _)| i == id)
                .then(|| vec![id.clone()]),
            Expr::And(all) => {
                let mut v = Vec::new();
                for a in all {
                    v.extend(choose(a)?);
                }
                Some(v)
            }
            Expr::Or(alts) => {
                let rank = |ids: &Vec<String>| {
                    ids.iter()
                        .map(|id| STANDARD.iter().position(|(i, _, _)| i == id).unwrap_or(99))
                        .max()
                        .unwrap_or(99)
                };
                alts.iter().filter_map(choose).min_by_key(rank)
            }
        }
    }
    choose(&parse_expression(e)?)
}

/// Our crates compiled into the binaries.
const SHIPPED: &[&str] = &["gitti-app", "gitty", "gitti-core"];

/// A fingerprint of what the list depends on in `Cargo.lock`: every third-party package
/// entry, and the dependency lists of our shipped crates (not their versions — a version
/// bump changes no third-party component). 64-bit FNV-1a over the normalised entries.
pub fn lock_fingerprint(lock: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |s: &str| {
        for b in s.bytes().chain(std::iter::once(b'\n')) {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for entry in lock.replace("\r\n", "\n").split("[[package]]").skip(1) {
        let lines: Vec<&str> = entry
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        let own = !lines.iter().any(|l| l.starts_with("source ="));
        // Our crates outside the binaries (xtask, gitti-test) don't count.
        if own && !SHIPPED.iter().any(|n| lines.contains(&format!("name = \"{n}\"").as_str())) {
            continue;
        }
        for l in lines {
            if own && l.starts_with("version =") {
                continue;
            }
            feed(l);
        }
    }
    format!("{hash:016x}")
}

/// The fingerprint a generated list was made from.
#[cfg(test)]
fn fingerprint_of(list: &str) -> Option<&str> {
    list.lines()
        .find_map(|l| l.strip_prefix(FINGERPRINT))
        .map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The checked-in list belongs to this `Cargo.lock`.
    #[gitti_test::test]
    fn generated_list_matches_cargo_lock() {
        let root = crate::workspace_root();
        let lock = std::fs::read_to_string(root.join("Cargo.lock")).unwrap();
        let list = std::fs::read_to_string(root.join(OUT)).unwrap();
        assert_eq!(
            fingerprint_of(&list),
            Some(lock_fingerprint(&lock).as_str()),
            "Cargo.lock changed since {OUT} was generated: run `cargo xtask licenses` and \
             commit the result"
        );
    }

    #[gitti_test::test]
    fn fingerprint_ignores_own_versions_and_dev_crates() {
        let lock = "version = 4\n\n[[package]]\nname = \"gitty\"\nversion = \"0.1.0\"\n\
                    dependencies = [\n \"anyhow\",\n]\n\n[[package]]\nname = \"anyhow\"\n\
                    version = \"1.0.1\"\nsource = \"registry+https://x\"\n";
        let f = lock_fingerprint(lock);
        assert_eq!(f, lock_fingerprint(&lock.replace("0.1.0", "0.2.0")));
        assert_ne!(f, lock_fingerprint(&lock.replace("1.0.1", "1.0.2")));
        assert_ne!(
            f,
            lock_fingerprint(&lock.replace(" \"anyhow\",\n]", " \"anyhow\",\n \"x\",\n]"))
        );
        assert_eq!(f, lock_fingerprint(&lock.replace('\n', "\r\n")));
        // A dev crate's dependencies don't count.
        let xtask = format!("{lock}\n[[package]]\nname = \"xtask\"\nversion = \"0.1.0\"\n");
        assert_eq!(f, lock_fingerprint(&xtask));
    }

    #[gitti_test::test]
    fn expressions() {
        assert_eq!(normalize_expression("MIT/Apache-2.0"), "MIT OR Apache-2.0");
        assert_eq!(
            normalize_expression("MIT /  Apache-2.0"),
            "MIT OR Apache-2.0"
        );
        assert!(gpl3_compatible("MIT OR Apache-2.0"));
        assert!(gpl3_compatible("Apache-2.0 OR GPL-2.0-only"));
        assert!(gpl3_compatible("(MIT OR Apache-2.0) AND Unicode-3.0"));
        assert!(gpl3_compatible("Apache-2.0 WITH LLVM-exception OR MIT"));
        assert!(!gpl3_compatible("GPL-2.0-only"));
        assert!(!gpl3_compatible("MIT AND GPL-2.0-only"));
        assert!(!gpl3_compatible("MIT OR"));
        assert_eq!(
            standard_choice("Zlib OR Apache-2.0 OR MIT"),
            Some(vec!["MIT".into()])
        );
        assert_eq!(
            standard_choice("Apache-2.0"),
            Some(vec!["Apache-2.0".into()])
        );
        assert_eq!(
            standard_choice("(MIT OR Apache-2.0) AND CC0-1.0"),
            Some(vec!["MIT".into(), "CC0-1.0".into()])
        );
        assert_eq!(standard_choice("BSD-3-Clause"), None);
    }

    #[gitti_test::test]
    fn texts_are_normalised() {
        assert_eq!(
            normalize_text("\u{feff}\r\n\r\nMIT  \r\n\tx\r\n==== y\n\n"),
            "MIT\n    x\n ==== y"
        );
    }
}
