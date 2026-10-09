# Release tests — NOT for daily work

Old slow tests and long randomized ones (e.g. `staging_fuzz`). Not compiled by default
(commented-out `[[test]]` blocks at the end of the crates' `Cargo.toml`). Run **only before
a release or overnight** — never in daily work, never per merge. A bug they find gets a
fast daily test. The daily suite is `cargo test --workspace --no-fail-fast` (every test
≤ 2 s).

Nothing here is compiled by default: cargo only builds tests under `crates/*/tests/`.
The switch is a commented-out `[[test]]` block at the end of each crate's `Cargo.toml`:

| Suite | Switch in | Run |
|---|---|---|
| `core/` — gitti-core integration tests, `staging_fuzz` | `crates/gitti-core/Cargo.toml` | `cargo test --workspace --test release_core [-- MODULE::]` |
| `tui/` — gitty end-to-end tests | `crates/gitty/Cargo.toml` | `cargo test --workspace --test release_tui [-- MODULE::]` |
| `ui/` — gitti headless UI scenarios (own harness) | `crates/gitti-app/Cargo.toml` | `cargo test --workspace --test release_ui [-- FILTER]` |

Uncomment the block, run, comment it out again before committing (never commit it
uncommented).

- The tests have hang guards instead of the daily limits: a UI scenario 10 min
  (`GITTI_TEST_FULL=1`: 100 min), a scripted gitty run 10 min. Plain `#[test]` is fine
  here (the 2 s guard only checks `crates/`).
- `GITTI_TEST_FULL=1` runs every UI sweep's whole matrix (sizes × fonts × …).
- `core/staging_fuzz.rs`: random files, edits and line selections, staged / unstaged /
  discarded and checked against a model. `GITTI_FUZZ_SEED`, `GITTI_FUZZ_ROUNDS` (300),
  `GITTI_FUZZ_SECS` (overnight: 28800). A failure prints the seed and round to replay.
- They may have rotted since: a failure can be the test, not the product. Fix or delete
  the test then — and if it found a real bug, add a fast test for it to the daily suite.
- New long or randomized tests go here too, not into `crates/*/tests/`.
