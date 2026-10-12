//! The workflow catalog (`crates/gitti-workflow/WORKFLOWS.md`) and the tests agree, both
//! ways: every test a catalog row names exists and carries the row's ID in its
//! `/// Workflow: …` line; every ID a test carries is a row of the catalog that names
//! that test. The catalog's "Totals" table matches its rows (`totals_match_the_catalog`).
//! Reads files only (no git).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// A catalog column: its crate, test binary folder and source folder (for `(unit)`).
const LAYERS: [(&str, &str, &str); 3] = [
    ("core", "gitti-core/tests/core", "gitti-core/src"),
    ("gitti", "gitti-app/tests/ui", "gitti-app/src"),
    ("gitty", "gitty/tests/tui", "gitty/src"),
];

/// The tag line's prefix.
const TAG: &str = "/// Workflow:";

/// A test as the catalog names it: layer (index into [`LAYERS`]), `module::fn`, unit test.
type TestRef = (usize, String, bool);

fn crates() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn catalog_text() -> String {
    fs::read_to_string(crates().join("gitti-workflow/WORKFLOWS.md")).unwrap()
}

/// The ID of a catalog row (`ST-13`): the first cell alone (other tables cite IDs with
/// more).
fn row_id(line: &str) -> Option<&str> {
    let id = line.strip_prefix("| ")?.split(" |").next().unwrap_or("");
    let is_id = id.len() == 5
        && id.as_bytes()[2] == b'-'
        && id[..2].bytes().all(|b| b.is_ascii_uppercase())
        && id[3..].bytes().all(|b| b.is_ascii_digit());
    is_id.then_some(id)
}

/// The last three cells of a catalog row: core, gitti, gitty (test names hold no `|`).
fn layer_cells(line: &str) -> [&str; 3] {
    let cells: Vec<&str> = line.trim().trim_end_matches('|').rsplitn(4, '|').collect();
    assert!(cells.len() == 4, "catalog row {line:?}: fewer than four cells");
    [cells[2], cells[1], cells[0]]
}

/// Whether a layer's cell covers the whole workflow, as the Totals count it: one test
/// (`module::test`, or a whole test binary marked `(binary)`) not marked partial (`~`);
/// `—` and `n/a` don't count.
fn covers(cell: &str) -> bool {
    let parts: Vec<&str> = cell.split('`').collect();
    (1..parts.len()).step_by(2).any(|i| {
        let partial = parts[i - 1].trim_end().ends_with('~');
        let after = parts.get(i + 1).copied().unwrap_or("");
        !partial && (parts[i].contains("::") || after.trim_start().starts_with("(binary)"))
    })
}

/// The catalog's rows: ID → the tests of each layer.
fn catalog() -> BTreeMap<String, Vec<TestRef>> {
    let text = catalog_text();
    let mut rows = BTreeMap::new();
    for line in text.lines() {
        let Some(id) = row_id(line) else { continue };
        let mut tests = Vec::new();
        for (layer, cell) in layer_cells(line).into_iter().enumerate() {
            for (i, part) in cell.split('`').enumerate() {
                if i % 2 == 0 || !part.contains("::") {
                    continue;
                }
                let after = cell.split('`').nth(i + 1).unwrap_or("");
                tests.push((layer, part.to_string(), after.trim_start().starts_with("(unit)")));
            }
        }
        assert!(rows.insert(id.to_string(), tests).is_none(), "catalog row {id} twice");
    }
    rows
}

/// The IDs of the `/// Workflow:` line in the doc comment / attributes above line `at`.
fn tags_above(lines: &[&str], at: usize) -> Vec<String> {
    let mut ids = Vec::new();
    for l in lines[..at].iter().rev() {
        let t = l.trim();
        if let Some(list) = t.strip_prefix(TAG) {
            ids.extend(list.split(',').map(|s| s.trim().trim_end_matches('.').to_string()));
        } else if !(t.starts_with("///") || t.starts_with("#[") || t.starts_with("//")) {
            break;
        }
    }
    ids
}

/// The line of `fn name` in `text`.
fn fn_line(lines: &[&str], name: &str) -> Option<usize> {
    lines.iter().position(|l| {
        let t = l.trim_start();
        let t = t.strip_prefix("pub ").unwrap_or(t);
        t.strip_prefix("fn ")
            .and_then(|r| r.strip_prefix(name))
            .is_some_and(|r| r.starts_with('(') || r.starts_with('<'))
    })
}

/// The files a test of `layer` named `path` (`module::fn`) can be in.
fn candidates(layer: usize, path: &str, unit: bool, all: &[(usize, bool, PathBuf, String)]) -> Vec<usize> {
    let (module, _) = path.rsplit_once("::").unwrap();
    let module = module.trim_end_matches("::tests");
    let module_file = module.replace("::", "/");
    let last = module.rsplit("::").next().unwrap();
    all.iter()
        .enumerate()
        .filter(|(_, (l, u, p, _))| {
            if *l != layer || *u != unit {
                return false;
            }
            let p = p.to_string_lossy().replace('\\', "/");
            if unit {
                p.ends_with(&format!("/{last}.rs")) || p.ends_with(&format!("/{last}/mod.rs"))
            } else {
                p.ends_with(&format!("/{module_file}.rs"))
            }
        })
        .map(|(i, _)| i)
        .collect()
}

#[gitti_test::test]
fn catalog_and_tests_agree() {
    let rows = catalog();
    assert!(rows.len() > 200, "the catalog has {} rows", rows.len());
    // Every source file of the three crates: (layer, unit = under src/, path, text).
    let mut all = Vec::new();
    for (layer, (_, tests, src)) in LAYERS.iter().enumerate() {
        for (dir, unit) in [(tests, false), (src, true)] {
            let mut files = Vec::new();
            rs_files(&crates().join(dir), &mut files);
            for f in files {
                let text = fs::read_to_string(&f).unwrap();
                all.push((layer, unit, f, text));
            }
        }
    }
    let mut problems = Vec::new();
    // Catalog → code: each named test exists and carries the ID.
    for (id, tests) in &rows {
        for (layer, path, unit) in tests {
            let name = path.rsplit("::").next().unwrap();
            let found = candidates(*layer, path, *unit, &all).into_iter().find_map(|i| {
                let lines: Vec<&str> = all[i].3.lines().collect();
                fn_line(&lines, name).map(|at| tags_above(&lines, at))
            });
            let layer_name = LAYERS[*layer].0;
            match found {
                None => problems.push(format!("{id}: {layer_name} test {path} not found")),
                Some(tags) if !tags.contains(id) => {
                    problems.push(format!("{id}: {layer_name} test {path} lacks `{TAG} {id}` (has {tags:?})"))
                }
                Some(_) => {}
            }
        }
    }
    // Code → catalog: each tag names a row that names this test.
    for (layer, unit, file, text) in &all {
        let lines: Vec<&str> = text.lines().collect();
        for (i, l) in lines.iter().enumerate() {
            let Some(list) = l.trim().strip_prefix(TAG) else { continue };
            let Some(f) = (i + 1..lines.len()).find_map(|j| {
                let t = lines[j].trim();
                if t.starts_with("///") || t.starts_with("#[") || t.starts_with("//") {
                    return None;
                }
                let t = t.strip_prefix("pub ").unwrap_or(t);
                Some(t.strip_prefix("fn ").map(|r| r.split(['(', '<']).next().unwrap_or("").to_string()))
            }) else {
                continue;
            };
            let Some(f) = f else {
                problems.push(format!("{}:{}: `{TAG}` not on a function", file.display(), i + 1));
                continue;
            };
            let module = if *unit {
                file.file_stem().unwrap().to_string_lossy().into_owned()
            } else {
                let base = crates().join(LAYERS[*layer].1);
                let rel = file.strip_prefix(&base).unwrap_or(file).with_extension("");
                rel.to_string_lossy().replace(['/', '\\'], "::")
            };
            for id in list.split(',').map(|s| s.trim().trim_end_matches('.')) {
                if !rows.contains_key(id) {
                    problems.push(format!("{}:{}: {id} is not in the catalog", file.display(), i + 1));
                    continue;
                }
                let listed = rows[id].iter().any(|(l, p, _)| {
                    *l == *layer && p.rsplit("::").next() == Some(f.as_str()) && (*unit || p.rsplit_once("::").unwrap().0 == module)
                });
                if !listed {
                    problems.push(format!(
                        "{}:{}: {id} tags {module}::{f}, but the catalog's {id} row doesn't name it ({})",
                        file.display(),
                        i + 1,
                        LAYERS[*layer].0
                    ));
                }
            }
        }
    }
    assert!(
        problems.is_empty(),
        "WORKFLOWS.md and the tests disagree ({}):\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// The "Totals" table counts what the catalog rows say: per area (the first two letters
/// of its IDs) the workflows and, per layer, those covered ([`covers`]); the Total row
/// their sums. Several branches each bumping the counts from the same base end up wrong
/// after the merge — this names the number to fix.
#[gitti_test::test]
fn totals_match_the_catalog() {
    const COLUMNS: [&str; 4] = ["Workflows", "core", "gitti", "gitty"];
    let text = catalog_text();
    // Per area, in catalog order: [workflows, core, gitti, gitty].
    let mut counted: Vec<(String, [usize; 4])> = Vec::new();
    for line in text.lines() {
        let Some(id) = row_id(line) else { continue };
        let area = &id[..2];
        if counted.last().is_none_or(|(a, _)| a != area) {
            assert!(
                !counted.iter().any(|(a, _)| a == area),
                "catalog rows of {area} are not together in one table"
            );
            counted.push((area.to_string(), [0; 4]));
        }
        let n = &mut counted.last_mut().unwrap().1;
        n[0] += 1;
        for (layer, cell) in layer_cells(line).into_iter().enumerate() {
            n[layer + 1] += usize::from(covers(cell));
        }
    }
    let totals = text.split("\n## Totals").nth(1).expect("WORKFLOWS.md has no \"## Totals\" section");
    let totals = totals.split("\n## ").next().unwrap_or(totals);
    let mut problems = Vec::new();
    let mut seen = Vec::new();
    let mut total_row = None;
    for line in totals.lines() {
        let Some(rest) = line.strip_prefix("| ") else { continue };
        let cells: Vec<&str> = rest.trim_end().trim_end_matches('|').split('|').map(str::trim).collect();
        let name = cells[0].trim_matches('*');
        if name == "Area" || name.starts_with("---") {
            continue;
        }
        let numbers: Vec<Option<usize>> = cells[1..].iter().map(|c| c.trim_matches('*').parse().ok()).collect();
        if numbers.len() != 4 || numbers.iter().any(Option::is_none) {
            problems.push(format!("Totals row {name:?}: not four numbers ({cells:?})"));
            continue;
        }
        let numbers: Vec<usize> = numbers.into_iter().flatten().collect();
        if name == "Total" {
            total_row = Some(numbers);
            continue;
        }
        let area = name.split_whitespace().next().unwrap_or("");
        seen.push(area.to_string());
        let Some((_, want)) = counted.iter().find(|(a, _)| a == area) else {
            problems.push(format!("Totals row {name:?}: no catalog rows of {area} — remove the row"));
            continue;
        };
        for (col, (has, want)) in COLUMNS.iter().zip(numbers.iter().zip(want)) {
            if has != want {
                problems.push(format!(
                    "Totals row {name:?}, column {col}: says {has}, the {area} rows give {want} — change it to {want}"
                ));
            }
        }
    }
    for (area, _) in &counted {
        if !seen.contains(area) {
            problems.push(format!("Totals: no row for the area {area} — add one"));
        }
    }
    let sums: Vec<usize> = (0..4).map(|c| counted.iter().map(|(_, n)| n[c]).sum()).collect();
    match total_row {
        None => problems.push("Totals: no **Total** row".to_string()),
        Some(has) => {
            for (col, (has, want)) in COLUMNS.iter().zip(has.iter().zip(&sums)) {
                if has != want {
                    problems.push(format!(
                        "Totals row \"Total\", column {col}: says {has}, the catalog rows give {want} — change it to {want}"
                    ));
                }
            }
        }
    }
    assert!(
        problems.is_empty(),
        "WORKFLOWS.md's Totals disagree with its rows ({}):\n{}",
        problems.len(),
        problems.join("\n")
    );
}
