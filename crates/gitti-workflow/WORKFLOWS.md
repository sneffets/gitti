# Workflow tests

A **workflow test** is a short, realistic sequence of user actions — stage two hunks and
commit them, cherry-pick a commit and later merge its branch, stash and pop, edit a file in
another program while gitti shows it — checked by its result in the repository and in the
app. Unit and feature tests check one function or one dialog; a workflow test checks that
the steps fit together the way a user chains them.

This file is the catalog: every workflow we want covered — its story, its steps, the
expected result — and which tests cover it today, on each of the three layers. A `—` is a
gap we know of; the totals at the end say where coverage stands. Tests carry the IDs of
the workflows they cover (`/// Workflow: ST-13`), and a test checks that this file and the
code agree (see [Keeping it in sync](#keeping-it-in-sync)).

## Layers

Each workflow can be checked on three layers. They share the story, the starting
repository and the expected repository state; each layer adds what only it can see.

| Layer | Binary | Drives the workflow through | Checks |
|---|---|---|---|
| **core** | `cargo test --workspace --test core` (`crates/gitti-core/tests/core`) | the `gitti_core::Repo` API, as the apps call it | the repository afterwards, read with plain git (log, index, files, state files) |
| **gitti** | `cargo test --workspace --test ui` (`crates/gitti-app/tests/ui`) | script steps (`GITTI_SCRIPT` syntax via `Ui::step` / `Ui::quick`: `select:`, `lines:`, `stagehunk:`, `menu:`, `palette:`, `action:`, `modalok`, …) in the real views, headless | the UI state (`testing::state`: lists, `repo_state`, `conflicted`, status, dialogs, git calls) **and** the repository |
| **gitty** | `cargo test --workspace --test tui` (`crates/gitty/tests/tui`) | the real `gitty` binary with `GITTY_SCRIPT` (keys, palette, menus) | `state` (incl. `repo_state=`, `conflicted=`) / `dump` / `cmds` output **and** the repository |

Core is the basis: it proves the git side is right. The gitti and gitty tests prove that
the app offers the step, asks what it should ask, runs the core call and shows the result —
they don't re-test every git corner case the core test already pins.

## The harness: `crates/gitti-workflow`

One workflow, three drivers. The shared part lives in the dev-only crate
`gitti-workflow` (std only; a dev-dependency of the three crates):

- **`Fixture`** — the starting state in three git processes: `git init`, one
  `git fast-import` for the whole history (branches, merges, tags, authors, fixed times so
  ids are the same in every run), one `git read-tree` for index and working tree; a staged
  state (`stage`), working-tree edits (`write`), config, remotes, hooks written as files.
  An operation state whose index stages matter (a merge that stopped) comes from the one
  real git command that makes it (`git_after_stops`), never from hand-written state files.
- **`ext`** — "another program": line endings for the whole file or some lines, a byte
  order mark, another code page (Latin-1, Windows-1252, UTF-16), the exec bit, a file
  swapped for a symlink or a folder, an old modification time, a git command in a terminal.
- **`Expect`** — the repository afterwards with few git processes: HEAD, refs and the
  operation state from the files; one `git log`; one `git cat-file --batch` for any number
  of index / commit contents; one `git status`. **`Checks`** collects every mismatch.
- **The workflows** — one module each in its area's module (`gitti_workflow::st`,
  `gitti_workflow::cp`, …): `fixture(dir)`, the constants the steps need, and
  `expect…(dir) -> Result<(), String>` for the state after a step. The drivers —
  `wf_*.rs` in the three test binaries — only take the steps and check what their app
  shows; the repository check is the same function on every layer.

Fixtures and checks run git without the user's global config. The apps under test read
it, so the fixture's own `.git/config` pins what it could change (identity, signing,
`autocrlf`, the conflict marker style).

## Time rule

Every test (and every gitti scenario, window included) takes **at most 2 s**, or it fails;
after 10 s the process is killed. git processes are the cost (~35 ms each on Windows):

- Build the starting state with `Fixture` (three git processes) — never one `git add` +
  `git commit` per commit.
- Start each test at the interesting state. A long story (branch, pick, edit, merge,
  resolve, commit) is split into steps whose starting states the fixture builds; the app
  drives only the steps under test (a gitti scenario two or three operations).
- Read the result with few git processes (`Expect`).
- Long or randomized variants (fuzzing, every combination) go to `release-tests/`, never
  into the daily suite.

## Legend

- `staging::stage_one_of_two_hunks` — a test that covers the workflow: `module::test` in
  the layer's binary (gitti: `suite::scenario`).
- `~name` — partial: the test covers part of the workflow (e.g. the dialog, not the
  result; or the command line instead of the app).
- `(unit)` — a unit test in `src/` of that crate.
- `—` — missing. `n/a` — the layer has nothing of its own to check.
- **Bold** stories are the everyday ones.

## Adding a workflow

1. Give it the next ID of its area and a row here: a one-line story, the steps, the
   expected result.
2. **The shared part**: a module in `crates/gitti-workflow/src/<area>.rs` with
   `fixture(dir)` (a `Fixture`), the names the steps need (messages, lines, commit ids in a
   `Setup`) and `expect…(dir)` per checked step (`Expect` + `Checks`).
3. **core first**: a `#[gitti_test::test]` in `crates/gitti-core/tests/core/wf_<area>.rs`
   — the fixture, the steps through `Repo`, the workflow's `expect`.
4. **gitti**: a scenario in `crates/gitti-app/tests/ui/wf_<area>.rs` (a suite registered in
   `main.rs`'s `SUITES`): the fixture in `repo_for`, script steps, asserts on what the user
   sees (`ui.ui_state()`, status, dialogs), then the same `expect` into `ui.fail`.
5. **gitty**: a `#[gitti_test::test]` in `crates/gitty/tests/tui/wf_<area>.rs`: the
   fixture, one gitty run (`common::gitty`), `state` / `dump` asserts, then the `expect`.
6. Mark each test with the workflow's ID: a doc-comment line `/// Workflow: CP-12` (several:
   `/// Workflow: ST-03, CP-07`) on the test function or scenario function, and name the
   tests in the row's columns. A layer that can't do the step yet (the app lacks the
   command) stays `—` and gets an open point; a missing product feature is never papered
   over by doing the step with plain git in an app test.
7. A step that runs into a product bug: the test checks what works, leaves a `TODO` comment
   naming the bug instead of asserting the broken behaviour, and the bug goes under
   [Open points](#open-points-found-by-workflow-tests).

## Keeping it in sync

`wf_catalog::catalog_and_tests_agree` (in the core test binary; reads files, no git)
checks both ways: every test named in a row exists and carries that row's ID in its
`/// Workflow:` line, and every ID a test carries is in this catalog with that test in its
row. Rename a test → change its row; add a test to a workflow → add it to the row and tag
it.

---

## ST — Staging hunks, lines and files

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| ST-01 | Stage one of two hunks | a file changed in two places; stage hunk 1 | index has hunk 1 only; working tree unchanged; hunk 2 still unstaged | `staging::stage_one_of_two_hunks` | `speed::stage_hunk_is_quick` | `staging::stage_a_hunk_and_unstage_a_file_with_path_limited_git` |
| ST-02 | Stage single lines | select one `+` and one `-` line of a hunk; stage | unselected `+` dropped, unselected `-` kept as context; a line of a replaced block (or its new version) stays in its place; exact index content | `staging::stage_single_lines_and_unstage_again`, `patch::stage_only_added_line` (unit), `staging::replaced_block_lines_stay_in_place`, `staging::uneven_blocks_and_hidden_whitespace_keep_lines_in_place` | `speed::stage_lines_is_quick` | `staging::stage_a_single_line_and_discard_lines_after_asking` |
| ST-03 | Unstage lines while another hunk stays staged | hunk 1 staged, part of hunk 2 staged; unstage lines of hunk 2 | hunk 1 stays staged, only the selected lines leave the index | `wf_staging::st03_unstage_lines_beside_a_staged_hunk`, `staging::unstage_lines_in_second_hunk_with_first_hunk_staged` | `wf_staging::st03_unstage_lines_beside_a_staged_hunk`, ~`speed::staging_keys` | `wf_staging::st03_unstage_lines_beside_a_staged_hunk` |
| ST-04 | Partial stage of a new file | untracked file; stage some lines | index has a new file with only those lines; rest untracked-side | `wf_staging::st04_partial_stage_of_a_new_file`, `staging::partial_stage_of_untracked_file` | `wf_staging::st04_partial_stage_of_a_new_file` | `wf_staging::st04_partial_stage_of_a_new_file` |
| ST-05 | Partial unstage of an added file | new file staged; unstage some lines | added file stays in the index without those lines | `wf_staging::st05_partial_unstage_of_an_added_file`, `staging::partial_unstage_of_added_file` | `wf_staging::st05_partial_unstage_of_an_added_file` | `wf_staging::st05_partial_unstage_of_an_added_file` |
| ST-06 | Partial stage of a deletion | tracked file deleted; stage some removed lines | index keeps the file minus those lines (not deleted) | `wf_staging::st06_partial_stage_of_a_deletion`, `staging::partial_stage_of_deleted_file` | `wf_staging::st06_partial_stage_of_a_deletion` | `wf_staging::st06_partial_stage_of_a_deletion` |
| ST-07 | Stage / unstage whole files | modified, untracked, deleted, a name with glob characters | each moves as a whole, literal paths only | `staging::working_state_after_whole_files_match_a_full_read`, `unix_fs::whole_file_operations_take_paths_literally` | `speed::stage_and_unstage_files_are_quick` | ~`staging::stage_a_hunk_and_unstage_a_file_with_path_limited_git` |
| ST-08 | Stage All / Unstage All | keys and section headers | everything moves; confirmations where they apply | `wf_staging::st08_stage_all_then_unstage_all`, ~`staging::discard_all_keeps_staged_changes` | `speed::staging_keys` | `staging::stage_all_unstage_all_discard_all_with_confirmations`, `staging::section_headers_stage_unstage_and_discard_all` |
| ST-09 | Stage a group | unstaged section grouped (tracked / untracked); stage one group | one `git add` of the group's paths, the other group stays | `wf_staging::st09_stage_a_group` | `speed::unstaged_groups_fold_and_stage_cheaply` | `views::unstaged_groups_fold_stage_and_discard_with_totals` |
| ST-10 | Stage with ignore-whitespace | `-w` view on; stage a hunk with whitespace-only neighbours | only the visible change staged (selection mapped onto the exact diff) | `wf_staging::st10_stage_ignoring_whitespace`, `staging::stage_hunk_while_ignoring_whitespace`, `non_utf8::ignore_whitespace_maps_lines_that_are_not_utf8`, `staging::uneven_blocks_and_hidden_whitespace_keep_lines_in_place` | `wf_staging::st10_stage_ignoring_whitespace` | `wf_staging::st10_stage_ignoring_whitespace` |
| ST-11 | Stage with expanded context | reveal hidden lines / whole file; select and stage | patch still exact | `staging::stage_and_unstage_lines_with_expanded_context` | `wf_staging::st11_stage_with_expanded_context` | ~`scroll::expanders_whole_file_and_changes_only_without_git` |
| ST-12 | Stage with minimal context | context lines set to 0 | hunks still apply | `wf_staging::st12_stage_with_minimal_context`, `staging::staging_works_with_minimal_context` | `wf_staging::st12_stage_with_minimal_context`, ~`keys::view_keys` | `wf_staging::st12_stage_with_minimal_context`, ~`menus::view_toggles_context_lines_and_history_order` |
| ST-13 | **Split a change into two commits** | two files changed; stage a hunk of `a` and a line of `b`; commit "first"; stage the rest; commit "second" | two commits holding exactly those changes; tree clean | `wf_pilot::st13_split_a_change_into_two_commits`, ~`staging::commit_via_cli` | `wf_pilot::st13_split_a_change_into_two_commits` | `wf_pilot::st13_split_a_change_into_two_commits`, `wf_pilot::st13_split_a_change_second_commit` |
| ST-14 | Stage inside a file tab | file tab of the working directory active; stage hunk / lines / file | tab stays and its mark follows; empty tab falls back | n/a | `speed::staging_in_a_file_tab_updates_the_tabs` | n/a |
| ST-15 | Keys during a running stage | press stage-all while a stage runs | runs after it, not refused | n/a | `speed::key_during_an_operation_waits_for_it` | `staging::keys_pressed_during_a_stage_run_after_it` |
| ST-16 | Refresh racing a stage | a full refresh starts before a stage ends | never shows the older state, not even one frame | n/a | `speed::refresh_racing_a_stage_shows_no_old_state` | — |
| ST-17 | View kept across staging | scrolled into a file; stage a hunk / the whole file | rows above stay; whole file → next file on top | n/a | `scroll::diff_stays_on_refresh_and_stage`, `scroll::diff_stage_whole_file_shows_the_next` | `scroll::scroll_position_is_kept_across_staging_and_refresh`, `scroll::staging_a_whole_file_moves_to_the_next_file` |
| ST-18 | Path-limited re-read equals a full read | after hunk / line / file operations | the cheap re-read gives what a full read gives | `staging::working_state_after_hunks_and_lines_match_a_full_read`, `staging::working_state_after_takes_directories` | ~`speed::staging_keys` | `speed::staging_runs_path_limited_git_and_its_own_writes_refresh_nothing` |
| ST-19 | Staging under a hostile global config | noprefix, mnemonicprefix, colour, textconv, external diff, … | stage / unstage / discard unaffected | `wf_staging::st19_staging_under_a_hostile_global_config`, `hostile_config::stage_hunk_lines_unstage_and_discard`, `hostile_config::untracked_deleted_and_trailing_whitespace` | `wf_staging::st19_staging_under_a_hostile_global_config` | — |
| ST-20 | Random line selections | random files and selections (fixed seeds) | byte-exact against a model | `non_utf8::random_byte_lines_stage_exactly` | n/a | n/a |
| ST-21 | Stage a line in a Unicode / wrapped line | wrapped CJK / RTL line; select one row; stage | the whole line staged exactly | — | `render_linux::unicode_staging` | `unicode::staging_copying_and_committing_unicode` |

## EX — Changes behind the app's back

Another program (editor, formatter, build, sync client, a terminal `git`) changes the
working tree or the index while the app shows an older state.

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| EX-01 | A shown file is saved by an editor | app shows `a`'s diff; another program writes `a` | only `a` re-read; the diff shows the new content | `linux_env::watcher_reports_edits_but_not_our_own_reads`, `watch::worktree_changes_need_only_their_paths` (unit) | `speed::watcher_rereads_only_what_changed` | `speed::outside_changes_refresh_only_what_they_touched` |
| EX-02 | Whole file LF → CRLF (no autocrlf) | an editor converts `a.txt` to CRLF; stage a middle line's change (its `-` and `+` line); discard another middle line's | the diff says the line endings changed (note row); the staged line reaches the index with CRLF, in its place, others untouched; discard restores LF in its place | `wf_eol::ex02_whole_file_to_crlf_stage_one_line_discard_another`, ~`staging::non_utf8_lines_stage_unstage_discard_byte_exact`, ~`eol_note::line_endings_and_bom` (unit), `staging::replaced_block_lines_stay_in_place`, `stash::stash_lines_keep_crlf_and_line_places` | `wf_eol::ex02_whole_file_to_crlf` | `wf_eol::ex02_whole_file_to_crlf` |
| EX-03 | Only some lines get CRLF | an editor rewrites 2 of 20 lines with CRLF; stage the first one's hunk | only those 2 lines show as changed (two hunks, a note says why); staging one keeps the other unstaged | `wf_eol::ex03_some_lines_get_crlf_stage_one` | `wf_eol::ex03_crlf_on_some_lines` | `wf_eol::ex03_crlf_on_some_lines` |
| EX-04 | CRLF file → LF | committed CRLF, editor saves LF; Stage All | every line changed, marked; Stage All gives an LF index (the staged diff's note says why) | `wf_eol::ex04_crlf_file_saved_with_lf_stage_all`, ~`eol_note::line_endings_and_bom` (unit) | `wf_eol::ex04_crlf_file_saved_with_lf`, ~`op_safety::merge_stop_stash_and_stage_all` | `wf_eol::ex04_crlf_file_saved_with_lf`, ~`op_safety::line_ending_only_changes_are_said` |
| EX-05 | `core.autocrlf=true` working tree | CRLF on disk, LF in the index; stage the file, unstage one hunk, discard it | only the edits show (no line-ending note); index stays LF; a discard writes CRLF back | `staging::line_staging_with_autocrlf_and_untracked_crlf_file` | `wf_eol::ex05_autocrlf_stage_unstage_discard` | `wf_eol::ex05_autocrlf_stage_unstage_discard` |
| EX-06 | `eol=crlf` / `text` attribute | file converted by attributes; stage one line's change, discard another hunk (stash: lines too) | index LF, working tree CRLF, both right | `wf_eol::ex06_eol_attribute_stage_and_discard_lines`, ~`stash::stash_lines_with_eol_conversion` | `wf_eol::ex06_eol_attribute_stage_and_discard` | `wf_eol::ex06_eol_attribute_stage_and_discard` |
| EX-07 | `.gitattributes` added that renormalizes | commit has CRLF; add `* text=auto` (untracked), an unfinished edit in another CRLF file; **Renormalize Line Endings…** (Repository menu, palette, the `.gitattributes` file's menu) | git shows no change of the CRLF file (with `text=auto` it keeps CRLF that is already stored); the dialog says what it does and what follows, leaves files with unstaged changes out unless ticked; afterwards the file is staged with LF as `git add --renormalize` gives it and the staged diff says why, its working-tree file is not rewritten; a deleted file is left out (git alone stops), conflicts refuse | `wf_eol::ex07_attributes_added_renormalize`, `wf_eol::ex07_renormalize_leaves_deleted_files_out_and_refuses_conflicts` | `wf_eol::ex07_renormalize_after_attributes` | `wf_eol::ex07_renormalize_after_attributes` |
| EX-08 | BOM added / removed | an editor adds a UTF-8 byte order mark to one file, drops another's; stage the one, discard the other | line 1 shows as changed and a note names the BOM; stage / discard exact | `wf_eol::ex08_bom_added_and_removed_stage_and_discard`, ~`eol_note::line_endings_and_bom` (unit) | `wf_eol::ex08_bom_added_and_removed` | `wf_eol::ex08_bom_added_and_removed` |
| EX-09 | Code page changed | a UTF-8 file re-saved as Windows-1252; stage one line's change | changed lines decoded, encoding named; a staged line moves its exact bytes | `wf_eol::ex09_saved_in_another_code_page_stage_one_line`, ~`non_utf8::eight_bit_lines_stage_unstage_discard_exactly` | `wf_eol::ex09_code_page_switch`, ~`speed::non_utf8_line_stages_exactly` | `wf_eol::ex09_code_page_switch`, ~`staging::latin1_line_stages_exactly` |
| EX-10 | Re-saved as UTF-16 | a text file becomes UTF-16 (binary for git) | listed as binary, whole-file stage only (exact bytes); with a `diff` attribute lines stage exactly | `wf_eol::ex10_saved_as_utf16_is_binary_whole_file_stages`, ~`unix_fs::text_becomes_binary`, `non_utf8::utf16_lines_stage_unstage_discard_exactly` | — | — |
| EX-11 | Trailing newline added / removed | an editor adds the last newline (and changes line 2) of one file, drops another's; stage the newline's hunk alone, discard the other | "no newline" handled on both sides; partial stage right | `unix_fs::no_eol_added_or_removed`, `staging::no_newline_at_eof`, `unix_fs::no_eol_both_sides_stage_and_unstage_one_side`, `staging::crlf_last_line_without_newline_gets_crlf` | `wf_eol::ex11_trailing_newline` | `wf_eol::ex11_trailing_newline` |
| EX-12 | Exec bit flipped | `chmod +x` / `-x` by another program | mode-only change; stage / unstage / discard; `core.fileMode=false` ignores it | `unix_fs::mode_only_change_stages_unstages_and_discards`, `unix_fs::removing_the_exec_bit_stages`, `unix_fs::core_filemode_false_ignores_the_exec_bit` | — | — |
| EX-13 | File replaced by a symlink (and back) | a tool swaps a file for a link | type change listed once per side; operations act on the whole change | `unix_fs::file_to_symlink_type_change`, `unix_fs::symlink_to_file_type_change` | — | — |
| EX-14 | File replaced by a folder (and back) | a build replaces `out` (file) by `out/` | deleted + untracked shown; stage / discard bring either state back | `unix_fs::file_replaced_by_directory`, `unix_fs::directory_replaced_by_file`, `unix_fs::directory_replaced_by_file_discard_in_steps` | — | — |
| EX-15 | File deleted while shown | the diff of `a` is shown; `a` is deleted; user stages a line of the old view | refused in plain words (or the view updated first); index unchanged | — | — | — |
| EX-16 | Discard from a stale view | `a` edited after it was shown; user discards a hunk of the old view | refused in plain words ("changed since it was shown"); nothing lost; view re-read | `op_safety::a_stale_view_stages_and_discards_nothing` | — | — |
| EX-17 | Stage from a stale view | `a` edited after it was shown; user stages lines of the old view | stages exactly what was shown (or refuses, as stash does); view re-read and consistent | `op_safety::a_stale_view_stages_and_discards_nothing` | `op_safety::hidden_untracked_and_stale_stage` | `op_safety::a_stale_stage_is_refused_plainly` |
| EX-18 | Index changed by a terminal `git add` | user stages the same hunk from the old view | no double change; plain-words error or no-op; view re-read | `op_safety::a_changed_index_is_noticed_too` | — | — |
| EX-19 | Edit lands while a stage runs | a write to the staged file during the operation | the final view equals the disk (no lost update) | — | ~`speed::refresh_racing_a_stage_shows_no_old_state` | — |
| EX-20 | File replaced with an older mtime | `cp -p` / sync client / unzip writes `a` with an old timestamp | the change still shows without a manual refresh | `watch::an_old_modification_time_is_not_taken_as_read` (unit) | — | — |
| EX-21 | Another git holds `index.lock` | briefly (an editor's git) / left by a crash | our write waits; a stale lock is offered for removal | `runner::write_waits_for_a_lock_released_meanwhile`, `runner::lasting_lock_fails_then_old_one_is_stale_and_removable` | — | `cancel::stale_index_lock_is_offered_for_removal` |
| EX-22 | Commit / branch made outside | a terminal commit or branch | history updated in place, selection kept | `repo_shapes::history_updates_reuse_what_did_not_change` | `scroll::commit_list_reload`, `speed::working_state_does_not_wait_for_the_history` | `speed::outside_ref_changes_update_the_history_in_place` |
| EX-23 | Tracked file made unreadable | `chmod 000` on a modified file | refresh works; the file listed without a diff | `unix_fs::unreadable_tracked_file_does_not_break_the_refresh`, `unix_fs::unreadable_directory_with_tracked_files` | — | — |
| EX-24 | Conflict resolved elsewhere while the merge editor is open | another program writes the resolution | taken in on focus; undo brings the conflict back | ~`merge_session::starts_from_an_edited_file_on_disk` (unit) | — | `merge_editor::merge_editor_takes_in_outside_edits` |

## CM — Commit, amend, reword

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| CM-01 | Commit staged changes | stage, type message, commit | one commit with exactly the staged changes; unstaged stay; box empties | `staging::commit_via_cli` | `keys::message_keys` | `staging::commit_message_keys_commit_and_amend` |
| CM-02 | **Amend: add a forgotten file** | stage `b`; amend with HEAD's message | HEAD replaced: same parent, tree now has `b`, message unchanged | `wf_commit::cm02_amend_adds_a_forgotten_file` | `wf_commit::cm02_amend_adds_a_forgotten_file`, ~`keys::message_keys` | `wf_commit::cm02_amend_adds_a_forgotten_file`, ~`staging::commit_message_keys_commit_and_amend` |
| CM-03 | Amend the message only | nothing staged, an unstaged change; amend with a new message | tree and parent unchanged, message replaced; no nothing-staged question; the change stays unstaged | `wf_commit::cm03_amend_the_message_only`, ~`hostile_config::typed_messages_keep_hash_lines` | `wf_commit::cm03_amend_the_message_only` | `staging::commit_message_keys_commit_and_amend` |
| CM-04 | **Amend with unstaged work around** | a staged fix; an unstaged line in the same file, a changed and an untracked file; amend with the message edited | only the staged part goes in, with the edited message; the rest stays as it was, unstaged | `wf_commit::cm04_amend_with_unstaged_work_around` | `wf_commit::cm04_amend_with_unstaged_work_around` | `wf_commit::cm04_amend_with_unstaged_work_around` |
| CM-05 | Amend draft per commit | edit the amend message; reopen | the edit comes back for that commit; gone once HEAD moved | `store::amend_drafts_are_kept_per_worktree_for_their_commit` (unit) | `commit::amend_draft_for_its_commit` | `staging::amend_draft_comes_back_for_its_commit` |
| CM-06 | Amend a pushed commit, then push | amend HEAD that origin has; push | refused in plain words; Push with Lease offered and works | `push::refused_push_of_an_amended_branch_offers_push_with_lease` | `wf_commit::cm06_amend_after_push_then_push_with_lease` | `remote::push_after_an_amend_offers_push_with_lease` |
| CM-07 | Amend while a merge / cherry-pick / revert is in progress | conflicts resolved; amend on; Commit; then amend off | refused with the reason before git runs (not during a rebase: an `edit` stop is where amending belongs); amend off, Commit concludes the operation | `wf_commit::cm07_amend_refused_while_a_merge_is_in_progress` | `wf_commit::cm07_amend_refused_while_a_merge_is_in_progress` | `wf_commit::cm07_amend_refused_while_a_merge_is_in_progress` |
| CM-08 | Nothing staged | Commit with nothing staged | asks: Stage All and Commit / Stage Tracked and Commit / Create Empty Commit — each does what it says | `repo_ops::nothing_staged_stage_all_or_empty_commit` | `commit::nothing_staged_choices` | `staging::nothing_staged_offers_stage_all_or_an_empty_commit` |
| CM-09 | Empty message | Commit with an empty box | refused, focus stays, no git | `wf_commit::cm09_empty_message_is_refused` | `wf_commit::cm09_empty_message_is_refused` | `staging::commit_message_keys_commit_and_amend` |
| CM-10 | Failing pre-commit hook | hook exits 1; fix; commit again | no commit, the hook's reason shown, message kept as draft; then committed, draft gone | `wf_commit::cm10_failing_pre_commit_hook_then_fixed` | `commit::draft_survives_a_restart` | `staging::commit_message_draft_survives_a_restart_until_committed` |
| CM-11 | commit-msg hook rewrites the message | hook appends a trailer | the commit has the hook's message (also what ↑ brings back); hook output shown while it runs | `wf_commit::cm11_commit_msg_hook_adds_a_trailer` | `wf_commit::cm11_commit_msg_hook_adds_a_trailer`, ~`commit::hook_output_shows_while_committing` | `wf_commit::cm11_commit_msg_hook_adds_a_trailer`, ~`progress::slow_hook_output_shows_while_committing` |
| CM-12 | Commit template | `commit.template` set; commit unedited, then edited | unedited refused before git; comment lines stripped | `commit_box::the_template_is_found_as_git_finds_it`, `commit_box::the_template_is_committed_only_edited_without_its_comments` | `commit::template_sign_off_and_earlier_messages` | `commit_box::template_fills_the_box_and_is_refused_unedited` |
| CM-13 | Sign off | Sign Off on; commit; amend | one trailer, never twice | `commit_box::sign_off_adds_the_trailer_once` | `commit::template_sign_off_and_earlier_messages` | `commit_box::sign_off_adds_the_trailer_and_is_remembered` |
| CM-14 | `#123` lines in a message | `commit.cleanup=strip` globally; message with `#` lines | kept as typed (commit, amend, tag, merge) | `hostile_config::typed_messages_keep_hash_lines` | `wf_commit::cm14_hash_lines_kept_under_cleanup_strip` | `wf_commit::cm14_hash_lines_kept_under_cleanup_strip` |
| CM-15 | Reword HEAD | staged (also changed again), unstaged and untracked work; Edit Commit Message on HEAD | message replaced, tree unchanged; the work stays exactly as it was, nothing staged committed | `repo_ops::reword_head_and_older_commit_keeps_staged_and_unstaged_changes`, `wf_commit::cm15_reword_head_keeps_the_work_in_progress` | `wf_commit::cm15_reword_head_keeps_the_work_in_progress`, ~`menus::branch_commit_stash_menus` | `menus::commit_menu_rewords_head`, `wf_commit::cm15_reword_head_keeps_the_work_in_progress` |
| CM-16 | Reword an older commit | staged (also changed again), unstaged and untracked work; reword commit 2 of 3 | history above rewritten, trees kept; the work stays exactly as it was (staged stays staged); no rebase or stash left | `repo_ops::reword_head_and_older_commit_keeps_staged_and_unstaged_changes`, `repo_ops::reword_needs_no_tools_but_git`, `hostile_config::reworded_messages_keep_hash_lines`, `wf_commit::cm16_reword_an_older_commit_keeps_the_work_in_progress` | `wf_commit::cm16_reword_an_older_commit_keeps_the_work_in_progress` | `menus::commit_menu_rewords_an_older_commit`, `wf_commit::cm16_reword_an_older_commit_keeps_the_work_in_progress` |
| CM-17 | Reword root / pushed / below a merge | reword the root; a pushed commit; one under a merge | root works; pushed and below-merge refused with the reason, nothing changed | ~`repo_ops::reword_the_root_commit_and_not_pushed_ones`, `wf_commit::cm17_reword_refused_for_pushed_and_below_a_merge`, `wf_commit::cm17_reword_the_root_commit` | `wf_commit::cm17_reword_refused_for_pushed_and_below_a_merge`, `wf_commit::cm17_reword_the_root_commit` | `wf_commit::cm17_reword_refused_for_pushed_and_below_a_merge`, `wf_commit::cm17_reword_the_root_commit` |
| CM-18 | Earlier messages | ↑ in the empty box; Recent Commit Messages… | newest first, own only | `commit_box::own_messages_are_the_users_newest` | `commit::template_sign_off_and_earlier_messages` | `commit_box::earlier_messages_by_up_and_the_picker` |
| CM-19 | First commit in a fresh repository | `git init`; stage; commit | root commit; history shows it; commands needing a commit come alive | ~`staging::history_of_repo_without_commits_has_worktree_row`, `wf_commit::cm19_first_commit_in_a_fresh_repository` | ~`menus::fresh_repository_greys_what_needs_a_commit`, `wf_commit::cm19_first_commit_in_a_fresh_repository` | `wf_commit::cm19_first_commit_in_a_fresh_repository` |
| CM-20 | Unicode / i18n messages | message with CJK, emoji; with `i18n.commitEncoding` = ISO-8859-1 a message with characters Latin-1 lacks | committed intact, shown decoded; in a legacy encoding: the box says first what it lacks and what is written instead, the commit is in that encoding with its header, the status line says what was replaced | `repo_shapes::commit_made_with_i18n_commit_encoding`, `repo_shapes::latin1_commit_message_is_decoded`, `wf_shapes::cm20_legacy_encoding` | `wf_shapes::cm20_unicode_messages`, `wf_shapes::cm20_legacy_encoding`, ~`render_linux::unicode_message` | `unicode::staging_copying_and_committing_unicode`, `wf_shapes::cm20_legacy_encoding` |
| CM-21 | Quit while a commit runs | ⌘Q / `q` while a hook holds the commit | asks; Quit When Done waits; Stop and Quit stops, draft kept | ~`runner::cancel_stops_a_hanging_hook`, `wf_commit::cm21_stop_a_commit_held_by_a_hook` | `commit::quit_while_git_runs_asks`, `commit::close_tab_while_git_runs_asks` | `cancel::quit_while_an_operation_runs_asks_first`, `cancel::quit_when_done_waits_for_the_operation`, `cancel::stop_and_quit_stops_the_operation` |

## SH — Stash

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| SH-01 | **Stash and pop** | modified file; Stash Changes…; pop | tree clean after stash; pop restores; list empty | `repo_ops::branch_tag_stash_remote_ops`, `wf_stash::sh01_stash_and_pop` | `wf_stash::sh01_stash_from_the_dialog_and_pop_from_its_menu`, ~`keys::overlay_keys` (stash only) | `wf_stash::sh01_stash`, `wf_stash::sh01_pop`, ~`menus::stash_dialog_fields_checkboxes_and_cancel` (stash only) |
| SH-02 | Stash including untracked | a new file; "Include untracked" | the file goes into the stash and comes back on pop | `wf_stash::sh02_stash_and_pop_including_untracked`, ~`stash::stash_paths_literal_and_all` | `menus::branch_commit_stash_menus` | `wf_stash::sh02_stash_including_untracked`, `wf_stash::sh02_pop_including_untracked`, ~`menus::stash_dialog_fields_checkboxes_and_cancel` |
| SH-03 | Stash `--all` | an ignored file too | ignored file stashed | `stash::stash_paths_literal_and_all`, `wf_stash::sh03_stash_and_pop_including_ignored` | `wf_stash::sh03_stash_including_ignored_files_and_pop` | `wf_stash::sh03_stash_including_ignored`, `wf_stash::sh03_pop_including_ignored` |
| SH-04 | Stash keeping the index | staged + unstaged; keep index | staged part stays staged, the rest stashed | `wf_stash::sh04_stash_keeping_the_index` | `wf_stash::sh04_stash_keeping_the_index` | `wf_stash::sh04_stash_keeping_the_index` |
| SH-05 | Stash only staged | staged part only | refused when git couldn't finish; else only staged stashed | `stash::stash_staged_only_refuses_what_git_cannot_finish` | `wf_stash::sh05_stash_only_the_staged_changes`, `wf_stash::sh05_staged_only_refused_next_to_unstaged_lines`, ~`menus::stash_dialogs` | `wf_stash::sh05_stash_only_the_staged_changes`, `wf_stash::sh05_staged_only_refused_next_to_unstaged_lines` |
| SH-06 | Stash selected lines | select lines of one hunk; Stash Selected Lines | only those lines leave; other hunk, other file and index stay | `stash::stash_unstaged_hunk_keeps_everything_else`, `stash::stash_unstaged_lines_over_staged_changes`, `stash::stash_staged_lines_keeps_unstaged_changes` | `menus::stash_dialogs` | `stash::stash_selected_lines` |
| SH-07 | Stash one file | cursor on a file; stash it | that file only; literal path | `stash::stash_paths_literal_and_all`, `stash::stash_lines_of_an_untracked_file` | `menus::stash_dialogs` | `stash::stash_file_and_rename` |
| SH-08 | Stash lines with odd bytes / EOL | Latin-1 + CRLF + no final newline; `eol=crlf` | exact bytes in the stash and in what stays | `stash::stash_lines_keeps_exact_bytes`, `stash::stash_lines_with_eol_conversion`, `stash::stash_lines_keep_crlf_and_line_places` | `wf_stash::sh08_stash_lines_with_odd_bytes` | `wf_stash::sh08_stash_lines_with_odd_bytes` |
| SH-09 | Stash lines from a stale view | file edited after it was shown; stash lines | refused: "changed since it was shown" | — | — | — |
| SH-10 | Rename a stash | Rename Stash… | message changed, order kept (also the only stash, reftable) | `stash::stash_rename_keeps_order`, `stash::stash_rename_the_only_stash`, `stash::stash_rename_in_a_reftable_repository` | — | `stash::stash_file_and_rename` |
| SH-11 | Drop a stash | sidebar menu Drop (asks) | that stash gone, others kept | `wf_stash::sh11_drop_one_stash_of_three` | `wf_stash::sh11_drop_one_stash_of_three`, ~`menus::branch_commit_stash_menus` | `menus::sidebar_menus_drop_a_stash_and_the_stashes_header` |
| SH-12 | Pop that conflicts, abort | pop over a conflicting commit; Abort | files back to HEAD; the stash entry stays | `conflicts::stash_pop_conflict_abort_keeps_the_stash` | `layout::stash_conflict_abort` | `merge_editor::stash_pop_conflict_aborts_from_the_palette_and_keeps_the_stash` |
| SH-13 | Apply that conflicts, resolve | apply conflicts; take the stash's side | resolved, no operation state left | `repo_ops::conflict_from_stash_apply`, `wf_stash::sh13_apply_conflicts_take_the_stash_side` | `wf_stash::sh13_apply_conflicts_take_the_stash_side` | `wf_stash::sh13_apply_stops_on_a_conflict`, `wf_stash::sh13_take_the_stash_side` |
| SH-14 | Branch from a stash | Create Branch from Stash… | branch at the stash's base, stash applied and dropped | `sidebar_ops::stash_branch_checks_out_a_branch_with_the_stash` | `menus::sidebar_menus` | `menus::sidebar_menus_drop_a_stash_and_the_stashes_header` |
| SH-15 | Look into a stash | select the stash row | its diff incl. untracked files; open a version | `history_diffs::stash_rows_in_history_and_stash_diff`, `navigate::stash_versions_include_untracked_files` | `wf_stash::sh15_look_into_a_stash` | `wf_stash::sh15_look_into_a_stash` |
| SH-16 | Stash while a merge is in progress | merge with conflicts resolved; Stash Changes | refused with the reason (a stash would end the merge) | `op_safety::no_stash_during_a_merge` | `op_safety::merge_stop_stash_and_stage_all` | `op_safety::during_a_merge_stash_waits_and_stage_all_asks` |
| SH-17 | Pop with the index restored | staged + unstaged stashed; Pop with Index… (asks, naming what is staged again) | staged part staged again (`--index`); when it doesn't apply to the index git refuses: nothing changed, said with the ways out | `wf_stash::sh17_pop_restoring_the_index`, `wf_stash::sh17_pop_with_the_index_refused_changes_nothing`, `wf_stash::sh17_pop_with_the_index_that_conflicts_says_the_index_is_not_restored`, `wf_stash::sh17_nothing_staged_in_the_stash_is_said` | `wf_stash::sh17_pop_with_index_from_the_stash_menu`, `wf_stash::sh17_pop_with_index_refused` | `wf_stash::sh17_pop_with_index_from_the_stash_menu`, `wf_stash::sh17_pop_with_index_refused` |
| SH-18 | Drop after the list changed outside | menu open on stash 1; a terminal `git stash` pushes another | the stash the user chose is dropped, or the action refuses | `op_safety::stash_commands_act_on_the_chosen_stash` | `wf_stash::sh18_drop_after_a_terminal_stash` | `wf_stash::sh18_drop_after_a_terminal_stash` |
| SH-19 | Pop blocked by an untracked file | the stash has `x`; an untracked `x` exists | refused in plain words; nothing changed; stash kept | `wf_stash::sh19_pop_blocked_by_an_untracked_file`, `wf_stash::sh19_what_is_in_the_way` | `wf_stash::sh19_pop_blocked_by_an_untracked_file` | `wf_stash::sh19_pop_blocked_by_an_untracked_file` |
| SH-20 | Branch from a stash, blocked | the stash has `x`; an untracked `x` exists (or a file the stash changes has local changes); Create Branch from Stash… | refused in plain words before anything changes (git alone would create and check out the branch, then stop half applied); still on `main`, no new branch, stash kept; with `x` moved away the branch is created at the stash's base | `wf_stash::sh20_branch_from_a_stash_blocked_by_an_untracked_file`, `wf_stash::sh20_branch_from_a_stash_blocked_by_local_changes`, `wf_stash::sh20_what_blocks_a_branch_from_a_stash` | `wf_stash::sh20_branch_from_a_stash_blocked_by_an_untracked_file` | `wf_stash::sh20_branch_from_a_stash_blocked_by_an_untracked_file` |

## BR — Branches

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| BR-01 | Create a branch and switch to it | Create Branch… on HEAD | branch exists, checked out | ~`repo_ops::branch_tag_stash_remote_ops`, `wf_branches::br01_create_a_branch_here_and_at_an_older_commit` | ~`keys::shortcut_list`, `wf_branches::br01_create_a_branch_at_head` | `repo_cmds::commit_list_marks_worktrees_and_branches_from_the_commit_menu` |
| BR-02 | Create a branch at an older commit | commit menu Create Branch Here… | branch at that commit, not checked out | ~`repo_ops::branch_tag_stash_remote_ops`, `wf_branches::br01_create_a_branch_here_and_at_an_older_commit` | `wf_branches::br02_create_a_branch_at_an_older_commit` | `menus::commit_menu_by_right_click_and_dot_creates_a_tag_and_a_branch` |
| BR-03 | Switch (clean) | branch switcher / sidebar ⏎ | HEAD moves; working tree matches | ~`switching::same_ignore_rules_cost_no_extra_git`, `wf_branches::br03_switch_clean_and_with_an_unrelated_change` | `keys::overlay_keys` | `navigation::sidebar_keys_move_fold_jump_and_checkout`, `palette::branch_switcher_moves_filters_and_checks_out`, `palette::branch_switcher_keys_no_matches_and_esc` |
| BR-04 | Switch with an unrelated local change | change in a file both branches have equal | switched; change carried over | ~`switching::discard_and_force_switch_drop_local_changes`, `wf_branches::br03_switch_clean_and_with_an_unrelated_change` | `wf_branches::br04_switch_carries_an_unrelated_change` | `wf_branches::br04_switch_carries_an_unrelated_change` |
| BR-05 | **Switch blocked by a local change → Stash & Switch** | change in a file the target changes | stashed, switched, re-applied | `repo_ops::switch_moves_local_changes_aside`, `wf_branches::br05_stash_and_switch_carries_the_change_or_keeps_the_stash` | `wf_branches::br05_switch_blocked_by_a_local_change_stash_and_switch` | `wf_branches::br05_switch_blocked_by_a_local_change_stash_and_switch` |
| BR-06 | Stash & Switch whose re-apply conflicts | overlapping change | switched, file conflicted, stash kept, said so | `repo_ops::switch_moves_local_changes_aside`, `wf_branches::br05_stash_and_switch_carries_the_change_or_keeps_the_stash` | `wf_branches::br06_stash_and_switch_whose_reapply_conflicts` | `wf_branches::br06_stash_and_switch_whose_reapply_conflicts` |
| BR-07 | Switch blocked by an untracked file | untracked `x` the target tracks | Stash & Switch keeps `x` in a stash; other untracked files stay | `repo_ops::switch_moves_blocking_untracked_files_aside` | `menus::switch_blocked_ways` | `repo_cmds::switch_blocked_by_an_untracked_file_stashes_it` |
| BR-08 | Delete & Switch / Discard & Switch / Force | the other ways out | each drops only what it says | `switching::folder_with_untracked_files_where_the_target_has_a_file`, `switching::discard_and_force_switch_drop_local_changes`, `switching::untracked_file_where_the_target_has_a_folder_is_listed_once` | `menus::switch_blocked_ways` | `repo_cmds::switch_blocked_offers_delete_and_force` |
| BR-09 | Many blocking files | ~200 blocking untracked files | all stashed in one go | `switching::many_blocking_untracked_files_are_stashed_in_one_go` | `wf_branches::br09_many_blocking_untracked_files_go_into_one_stash` | `wf_branches::br09_many_blocking_untracked_files_go_into_one_stash` |
| BR-10 | Switch to fewer ignore rules | target doesn't ignore `target/` | build output shows untracked; the app says why | `switching::switching_to_fewer_ignore_rules_shows_ignored_files_as_untracked` | `wf_branches::br10_switch_to_fewer_ignore_rules_says_why` | `repo_cmds::switch_to_fewer_ignore_rules_says_why_files_are_untracked` |
| BR-11 | Rename the current branch | Rename… | renamed; upstream config follows | ~`repo_ops::branch_tag_stash_remote_ops`, `wf_branches::br11_rename_the_current_branch_keeps_its_upstream` | `menus::branch_commit_stash_menus` | `menus::sidebar_menus_rename_a_branch_and_delete_a_tag` |
| BR-12 | Delete a merged branch | Delete… | gone | ~`repo_ops::branch_tag_stash_remote_ops`, `wf_branches::br12_delete_merged_then_unmerged_and_recover_it` | ~`menus::branch_commit_stash_menus`, `wf_branches::br12_delete_a_merged_branch` | ~`keybar::the_keys_shown_run_their_entries`, `wf_branches::br12_delete_a_merged_branch` |
| BR-13 | Delete an unmerged branch | Delete… | refused, Force Delete offered and naming the lost commits | `wf_branches::br12_delete_merged_then_unmerged_and_recover_it` | `wf_branches::br13_force_delete_names_the_commit_it_drops` | `repo_cmds::deleting_an_unmerged_branch_offers_force`, `wf_branches::br13_force_delete_names_the_commit_it_drops` |
| BR-14 | Switch to a remote branch | `origin/feat` without a local branch | local `feat` tracking it, checked out | ~`worktrees::plan_add_tracks_a_remote_branch`, `wf_branches::br14_switch_to_a_remote_branch_creates_a_tracking_branch` | `wf_branches::br14_switch_to_a_remote_branch` | `wf_branches::br14_switch_to_a_remote_branch` |
| BR-15 | Switch to a branch checked out in another worktree | ⏎ on it | offers that worktree instead of git's error | `worktrees::switching_to_a_branch_of_another_worktree` | `worktrees::add_open_remove` | — |
| BR-16 | Detached HEAD, then keep the work | check out a commit; commit; create a branch there | branch holds the commit; branch commands greyed while detached | ~`repo_shapes::detached_and_unborn_head`, `wf_branches::br16_commit_on_a_detached_head_and_keep_it` | ~`menus::detached_head_greys_branch_commands`, `wf_branches::br16_checkout_a_commit_and_commit_there`, `wf_branches::br16_keep_the_detached_work_in_a_branch` | ~`palette::palette_branch_commands_on_a_detached_head`, `wf_branches::br16_checkout_a_commit_and_commit_there`, `wf_branches::br16_keep_the_detached_work_in_a_branch` |
| BR-17 | Set / unset upstream | Set as Upstream | ahead / behind follow | `repo_ops::config_upstream_and_blobs` | ~`menus::branch_commit_stash_menus`, `wf_branches::br17_set_the_upstream` | `keybar::the_keys_shown_run_their_entries` |
| BR-18 | Leave a detached HEAD with work on it | detached at a commit no branch has; switch to `main` | switched; the status line names the commit left on no branch and how to keep it (reflog → Create Branch Here…); HEAD's reflog has it | `wf_branches::br18_leaving_a_detached_commit_warns` | `wf_branches::br18_leaving_a_detached_commit_warns` | `wf_branches::br18_leaving_a_detached_commit_warns` |
| BR-19 | Clean up branches | Clean Up Merged Branches…; Delete Branches… with Force | merged branch deleted, unmerged one only with its Force box | `wf_branches::br19_clean_up_merged_then_delete_with_force` | `branches::clean_up_and_delete` | `branches::clean_up_merged_deletes_the_merged_branch`, `branches::delete_branches_needs_force_for_an_unmerged_one` |

## MG — Merge

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| MG-01 | Fast-forward merge | `main` behind `feat`; merge `feat` | `main` at `feat`, no merge commit | `wf_merge::mg01_fast_forward_merge` | `wf_merge::mg01_fast_forward_merge` | `palette::palette_merge_picker_and_synonyms` |
| MG-02 | No-ff merge with a message | Merge… with "always create a merge commit" | two parents, the message | `repo_ops::merge_and_cherry_pick_options` | `wf_merge::mg02_no_ff_merge_with_a_message` | `wf_merge::mg02_no_ff_merge_with_a_message` |
| MG-03 | Squash merge | Merge… squash; commit | changes staged, no `MERGE_HEAD`, `SQUASH_MSG` in the commit box; one-parent commit with it | `wf_merge::mg03_squash_merge_then_commit`, ~`repo_ops::merge_and_cherry_pick_options`, ~`conflicts::squash_merge_message_in_a_linked_worktree` | `wf_merge::mg03_squash_merge_then_commit` | `wf_merge::mg03_squash_merge_then_commit` |
| MG-04 | Fast-forward only on diverged history | ff-only | refused in plain words (what blocks, nothing changed, merge without `--ff-only` or rebase), nothing changed | `repo_ops::merge_and_cherry_pick_options`, `wf_merge::mg04_fast_forward_only_on_diverged_branches_is_refused` | `wf_merge::mg04_fast_forward_only_on_diverged_branches` | `wf_merge::mg04_fast_forward_only_on_diverged_branches` |
| MG-05 | Clean three-way merge | diverged, different files | merge commit, both changes | `repo_ops::merge_and_cherry_pick_options` | `wf_merge::mg05_clean_three_way_merge` | `wf_merge::mg05_clean_three_way_merge` |
| MG-06 | **Content conflict → merge editor → commit** | same line changed; resolve in the editor; commit | box has git's message; Commit off while conflicted; merge concluded with two parents | `conflicts::committing_a_resolved_merge_concludes_it`, `repo_ops::merge_doc_from_conflict_stages` | `layout::merge_commit_flow` | `merge_editor::merge_message_prefilled_commit_blocked_then_concludes_the_merge` |
| MG-07 | Resolve by taking a side | ours for one file, theirs for another; Continue | each file as that side; merge concluded | `repo_ops::conflicts_from_a_merge_resolved_per_side` | `wf_merge::mg07_take_a_side_per_file_then_continue` | `wf_merge::mg07_take_a_side_per_file`, `wf_merge::mg07_continue_after_taking_sides`, ~`merge_editor::merge_editor_save_single_view_questions_editor_and_resolve` |
| MG-08 | Modify / delete conflict | one side deleted, other changed; keep / delete | header says so; keep stages it, delete removes it | `conflicts::delete_and_rename_conflicts_resolve_by_keeping_or_deleting` | `layout::delete_conflict_choices` | `merge_editor::deleted_file_conflict_offers_keep_and_delete` |
| MG-09 | Rename / rename conflict | both renamed `r.txt` differently; keep `feat`'s name, delete `main`'s and `r.txt`; Continue | both names listed; resolving keeps the chosen one | `conflicts::delete_and_rename_conflicts_resolve_by_keeping_or_deleting` | `wf_merge::mg09_both_renamed_keep_theirs_name` | `wf_merge::mg09_both_renamed_keep_theirs_name`, `wf_merge::mg09_delete_the_old_name_and_continue` |
| MG-10 | Abort a conflicting merge | Abort in the banner | back to the pre-merge commit and files | `repo_ops::cherry_pick_revert_reset_and_conflict_state` | `wf_merge::mg10_abort_a_conflicting_merge`, ~`layout::merge_commit_flow` | `wf_merge::mg10_abort_a_conflicting_merge` |
| MG-11 | Conclude with the box emptied / edited | empty box, or an edited message | git's message without `# Conflicts:` lines / the edited one | `conflicts::continue_concludes_a_merge_without_conflict_comments_or_with_the_edited_message` | `wf_merge::mg11_conclude_with_the_box_emptied`, `wf_merge::mg11_conclude_with_an_edited_message` | `merge_editor::merge_concluded_with_the_box_emptied` |
| MG-12 | Unrelated histories | merge a branch with its own root | refused, "Merge anyway" offered and works | `conflicts::unrelated_histories_merge_and_pull` | `menus::merge_unrelated_histories` | `wf_merge::mg12_unrelated_histories_merge_anyway`, ~`repo_cmds::pull_with_unrelated_histories_offers_merge_anyway` |
| MG-13 | Merge preview in the graph | during a merge | working row is the pending merge, dashed edge to `MERGE_HEAD` | `merge_preview::merge_head_makes_the_worktree_row_the_pending_merge`, `merge_preview::unreachable_merge_head_and_octopus` | `layout::merge_preview_graph` | `views::merge_preview_dashes_the_pending_merge` |
| MG-14 | Long conflict markers | `conflict-marker-size=12`, a `=======` heading in the text; Mark Resolved (asks); merge editor: theirs, save; Mark Resolved; Continue | the editor and the "still has markers" check use the size (the heading is text) | `conflicts::conflict_marker_size_attribute`, `merge_session::marker_size_attribute` (unit) | `wf_merge::mg14_long_conflict_markers` | `wf_merge::mg14_long_conflict_markers` |
| MG-15 | Conflict in a non-UTF-8 / CRLF file | Latin-1 conflict; autocrlf CRLF conflict | written back byte for byte in the working-tree form | `non_utf8::merge_editor_writes_non_utf8_files_exactly`, `non_utf8::merge_editor_writes_the_working_tree_form` | `wf_merge::mg15_latin1_and_crlf_conflicts_saved_exactly` | `merge_editor::merge_editor_saves_a_latin1_file_exactly` |
| MG-16 | Merge into a dirty tree (overlapping) | local change in a file the merge changes | git refuses; the app says what blocks and offers a way out (stash first); nothing changed | `wf_merge::mg16_merge_into_an_overlapping_dirty_tree_stash_and_merge` | `op_safety::blocked_merge_stash_and_merge` | `op_safety::a_blocked_merge_offers_ways_out` |
| MG-17 | Merge into a dirty tree (not overlapping) | local change elsewhere; conflicting merge; abort | the local change survives merge and abort | `wf_merge::mg17_dirty_tree_survives_a_conflicting_merge_and_its_abort` | `wf_merge::mg17_dirty_tree_survives_merge_and_abort` | `wf_merge::mg17_dirty_tree_survives_merge_and_abort` |
| MG-18 | Unstage All during a merge | conflicts resolved, files staged; Unstage All | files unstaged, **merge still in progress** (`MERGE_HEAD` kept) | `op_safety::unstage_all_keeps_a_merge_and_its_conflicts` | `op_safety::merge_stop_stash_and_stage_all` | `op_safety::unstage_all_keeps_the_merge` |
| MG-19 | Stage All with markers left | a conflicted file still has `<<<<<<<`; Stage All | asks first like Mark Resolved does | `op_safety::conflict_markers_left_are_found_before_stage_all` | `op_safety::merge_stop_stash_and_stage_all` | `op_safety::during_a_merge_stash_waits_and_stage_all_asks` |
| MG-20 | Merge in a linked worktree | conflict in a linked worktree; the app opened there; take theirs; Continue | state from its own git dir; concluded there; main worktree clean | `conflicts::merge_conflict_in_a_linked_worktree` | `wf_merge::mg20_merge_conflict_in_a_linked_worktree` | `wf_merge::mg20_merge_conflict_in_a_linked_worktree` |
| MG-21 | Submodule pointer conflict | both sides moved the pointer (submodule checked out); take theirs; Continue | listed as a submodule conflict showing ours → theirs and the commits between; no merge editor / external merge tool; taking a side records that side's commit (the checkout stays) | `submodules::conflicting_pointers`, `wf_merge::mg21_submodule_pointer_conflict_take_theirs` | `wf_merge::mg21_submodule_pointer_conflict_take_theirs` | `wf_merge::mg21_submodule_pointer_conflict_take_theirs` |
| MG-22 | Binary conflict | both sides changed a binary; take theirs; Continue | shown as a binary conflict (not "same as main"); no merge editor; take ours / theirs | `wf_merge::mg22_binary_conflict_take_theirs` | `wf_merge::mg22_binary_conflict_take_theirs` | `wf_merge::mg22_binary_conflict_take_theirs` |
| MG-23 | Merge conflict in a SHA-256 repository | MG-07 in a SHA-256 repository (the merge editor for one file, a side for the other); Continue | as in SHA-1 | `sha256::merge_conflict_worktrees_and_branch_ops` | `wf_merge::mg23_merge_conflict_in_a_sha256_repository` | `wf_merge::mg23_merge_conflict_in_a_sha256_repository`, `wf_merge::mg23_continue_in_a_sha256_repository` |
| MG-24 | Resolve with an external merge tool | conflicted file → Open in External Merge Tool | the tool's result staged; status says no conflict is left | `wf_merge::mg24_external_merge_tool_resolves_the_last_conflict`, ~`ext_tool::describes_the_merge_outcome` (unit) | `ext_tools::merge_tool` | `ext_tools::merge_tool_from_the_resolve_menu` |
| MG-25 | First external merge tool, set up on the way | no merge tool set; Open in External Merge Tool; choose one in the set-up dialog (this repository) | the dialog lists git's tools (own first, installed, missing); the choice written to the named config file; the tool resolves the file right after | `ext_tools::without_a_tool_the_choices_come_first_then_the_chosen_tool_runs` | `ext_tools::merge_tool_set_up_first` | `ext_tools::merge_tool_set_up_on_the_way`, ~`ext_tools::set_up_dialog_then_the_diff_tool_runs` (diff tool) |
| MG-26 | Resolve with a terminal merge tool | `merge.tool` runs vim; Open in External Merge Tool | gitty hands the tool its terminal; the file resolved and staged, backup kept; gitti refuses with the reason | `ext_tools::terminal_merge_tool_resolves_the_file` | n/a | `ext_tools::terminal_merge_tool_gets_the_terminal` |
| MG-27 | Conflict in a file `.gitattributes` marks binary | a text file `binary` by a nested `.gitattributes` (also `-merge`, `merge=binary`, a macro, `info/attributes`, `core.attributesFile`, `merge.default`), both sides changed it; take ours; Continue | git left ours without markers; shown as a binary conflict, no merge editor, the menu says why; `-diff` / `-text` alone or a driver of the user's named `binary` don't count | `wf_merge::mg27_conflict_in_a_file_gitattributes_marks_binary` | `wf_merge::mg27_conflict_in_a_file_gitattributes_marks_binary` | `wf_merge::mg27_conflict_in_a_file_gitattributes_marks_binary` |

## CP — Cherry-pick

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| CP-01 | **Cherry-pick one commit** | select a commit of `feat`; Cherry-pick | new commit on `main` with its change, author and message | `repo_ops::cherry_pick_revert_reset_and_conflict_state` | `wf_pick::cp01_pick_one_commit` | `wf_pick::cp01_pick_one_commit` |
| CP-02 | Cherry-pick with `-x` | Cherry-pick with Options…, record origin | message ends with "(cherry picked from commit …)" | `repo_ops::merge_and_cherry_pick_options` | `wf_pick::cp02_pick_with_record_origin` | `wf_pick::cp02_pick_with_record_origin`, ~`merge_editor::prepared_message_next_to_the_users_text` |
| CP-03 | Cherry-pick without committing | `-n` (and `-x`); then Commit | change staged; box gets git's prepared message; commit drops `MERGE_MSG` | `conflicts::cherry_pick_no_commit_record_origin_prepares_the_message` | `commit::prepared_message_after_cherry_pick` | `merge_editor::prepared_message_after_cherry_pick_no_commit`, `merge_editor::prepared_message_next_to_the_users_text`, `merge_editor::keep_mine_survives_a_restart` |
| CP-04 | Conflicting pick → resolve → Continue | conflict; resolve (take theirs); Continue | commit with the picked author; box message used when edited | `wf_pick::cp04_conflicting_pick_resolve_and_continue`, ~`conflicts::continue_a_cherry_pick_sequence_with_the_edited_message` | `wf_pick::cp04_take_theirs`, `wf_pick::cp04_continue_commits_the_resolution`, ~`commit::continue_with_the_edited_message` | `wf_pick::cp04_take_theirs`, `wf_pick::cp04_continue_commits_the_resolution`, ~`merge_editor::continue_with_the_edited_message` |
| CP-05 | Conflicting pick → Abort | conflict (said plainly, banner with Continue / Skip / Abort); Abort | HEAD and files as before | `wf_pick::cp05_conflicting_pick_abort` | `wf_pick::cp05_conflicting_pick_stops_then_abort` | `wf_pick::cp05_conflicting_pick_stops_then_abort` |
| CP-06 | Pick a merge commit | Cherry-pick on a merge | picks against the first parent (`-m 1`): one ordinary commit | `wf_pick::cp06_pick_a_merge_commit` | `wf_pick::cp06_pick_a_merge_commit` | `wf_pick::cp06_pick_a_merge_commit` |
| CP-07 | Several commits in sequence | mark A..C; Cherry-pick Marked Commits…; the second conflicts (where, in the status); resolve; Continue | the rest picked; state clean | `wf_pick::cp07_marked_sequence_stops_resolve_and_continue`, `conflicts::continue_a_cherry_pick_sequence_with_the_edited_message`, `conflicts::a_sequence_committed_by_hand_still_continues` | `wf_pick::cp07_marked_sequence_stops_at_the_second`, `wf_pick::cp07_resolve_and_continue_picks_the_rest`, ~`commit::continue_with_the_edited_message` (sequence started on the command line) | `wf_pick::cp07_marked_sequence_stops_at_the_second`, `wf_pick::cp07_resolve_and_continue_picks_the_rest`, ~`merge_editor::continue_with_the_edited_message` (same) |
| CP-08 | Pick several selected commits from the app | mark three commits (gitti: click, ⌘- / shift-click; gitty: space); Cherry-pick Marked Commits… (menu bar, palette, the marked commit's menu) | the dialog lists them oldest first; picked oldest first in one sequence; marks cleared | `wf_pick::cp08_pick_several_marked_commits_oldest_first` | `wf_pick::cp08_pick_three_marked_commits` | `wf_pick::cp08_pick_three_marked_commits` |
| CP-09 | **Pick a commit that is already applied** | pick `A` twice | the app says it is empty and offers Skip / Commit Empty / Abort | `op_safety::an_empty_pick_is_explained_and_skipped`, `wf_pick::cp09_pick_a_commit_twice_then_skip` | `wf_pick::cp09_pick_it_again_then_skip`, ~`op_safety::empty_pick_skip` | `wf_pick::cp09_pick_it_again_then_skip`, ~`op_safety::an_empty_pick_offers_skip` |
| CP-10 | Conflict resolved to the HEAD side | resolution equals HEAD; Continue | "nothing left of this commit": Skip offered; never stuck | `wf_pick::cp10_conflict_resolved_to_head_then_skip` | `wf_pick::cp10_conflict_resolved_to_head_then_skip` | `wf_pick::cp10_conflict_resolved_to_head_then_skip` |
| CP-11 | Conflicting `-x` with a custom comment char | `core.commentChar=;` | `; Conflicts:` lines dropped, origin line kept | `conflicts::conflicting_cherry_pick_strips_the_configured_comment_char` | `wf_pick::cp11_conflicting_pick_with_record_origin_and_comment_char` | `wf_pick::cp11_conflicting_pick_with_record_origin_and_comment_char` |
| CP-12 | **Pick, then merge the source branch** | pick `A` from `feat` onto `main`; later merge `feat` | merges cleanly; both `A` and its copy in history | `wf_pilot::cp12_pick_then_merge_the_source_branch` | `wf_pilot::cp12_pick_then_merge_the_source_branch` | `wf_pilot::cp12_pick_then_merge_the_source_branch` |
| CP-13 | Pick, change the same lines on main, merge the source | pick `A`; edit A's lines on `main`; merge `feat` | conflict exactly on those lines (expected); other files merged | `wf_pilot::cp13_pick_edit_then_merge_conflicts_there` | `wf_pilot::cp13_pick_edit_then_merge_conflicts_there` | `wf_pilot::cp13_pick_edit_then_merge_conflicts_there` |
| CP-14 | Pick with a different resolution, merge the source | pick `A` with a conflict resolved differently; merge `feat` | conflict again on those lines (git can't know); resolvable (keep the resolution, Continue) | `wf_history::cp14_pick_resolved_differently_then_merge_conflicts_again` | `wf_history::cp14_merge_stops_on_the_resolved_line`, `wf_history::cp14_keep_the_resolution_and_continue` | `wf_history::cp14_merge_stops_on_the_resolved_line`, `wf_history::cp14_keep_the_resolution`, `wf_history::cp14_continue_makes_the_merge` |
| CP-15 | Pick, revert the pick, merge the source | pick `A`; revert it; merge `feat` | `A`'s change is back after the merge (git's three-way result) and the diff shows it | `wf_history::cp15_pick_revert_then_merge_brings_the_change_back` | `wf_history::cp15_pick_revert_then_merge_brings_the_change_back` | `wf_history::cp15_pick_revert_then_merge_brings_the_change_back` |
| CP-16 | Pick into a dirty tree | local change in a file the pick changes | refused in plain words naming the file, ways out offered; nothing changed | `wf_pick::cp16_pick_into_a_dirty_tree_is_refused` | `wf_pick::cp16_pick_into_a_dirty_tree_is_refused` | `wf_pick::cp16_pick_into_a_dirty_tree_is_refused` |
| CP-17 | Pick HEAD | Cherry-pick on HEAD (also HEAD among marked commits) | greyed with the reason; no git | `availability::changes_stashes_and_selection_turn_them_on` (unit) | `wf_pick::cp17_pick_head_is_greyed`, ~`menus::unavailable_commands` | `wf_pick::cp17_pick_head_is_greyed`, ~`palette::palette_and_menu_grey_what_cannot_act_with_the_reason` |
| CP-18 | Pick a lost commit from the reflog | reflog entry → Cherry-pick | its change on HEAD | `wf_pick::cp18_pick_a_lost_commit_from_the_reflog` | `wf_pick::cp18_pick_a_lost_commit_from_the_reflog` | `wf_pick::cp18_pick_a_lost_commit_from_the_reflog` |

## RV — Revert

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| RV-01 | **Revert a commit** | Revert on a commit | a new commit undoing it ("Revert …") | `repo_ops::cherry_pick_revert_reset_and_conflict_state`, `wf_pick::rv05_revert_then_revert_the_revert` | `wf_pick::rv01_revert_then_revert_the_revert` | `wf_pick::rv01_revert_then_revert_the_revert` |
| RV-02 | Revert a merge commit | Revert on a merge | reverted against the first parent | `wf_pick::rv02_revert_a_merge_commit` | `wf_pick::rv02_revert_a_merge_commit` | `wf_pick::rv02_revert_a_merge_commit` |
| RV-03 | Revert conflict → Continue with an edited message | conflict; resolve; edit box; Continue | the edited message | `conflicts::continue_a_revert_with_the_edited_message` | `wf_pick::rv03_revert_conflict_continue_with_an_edited_message` | `wf_pick::rv03_revert_conflict_continue_with_an_edited_message` |
| RV-04 | Revert conflict → Abort | conflict; Abort | back to before | `wf_pick::rv04_revert_conflict_abort` | `wf_pick::rv04_revert_conflict_abort` | `wf_pick::rv04_revert_conflict_abort` |
| RV-05 | Revert the revert | revert `A`, then revert that | `A`'s change back | `wf_pick::rv05_revert_then_revert_the_revert` | `wf_pick::rv01_revert_then_revert_the_revert` | `wf_pick::rv01_revert_then_revert_the_revert` |
| RV-06 | Revert a change already undone | revert gives nothing | alone: nothing changed, said in plain words (git leaves no state); in a sequence: stops, Skip / Commit Empty / Abort offered — never stuck | `wf_pick::rv06_revert_a_change_already_undone` | `wf_pick::rv06_revert_a_change_already_undone` | `wf_pick::rv06_revert_a_change_already_undone` |
| RV-07 | Revert several selected commits | mark three commits; Revert Marked Commits… | the dialog says newest first; reverted newest first in one sequence | `wf_pick::rv07_revert_several_marked_commits_newest_first` | `wf_pick::rv07_revert_three_marked_commits` | `wf_pick::rv07_revert_three_marked_commits` |

## RB — Rebase

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| RB-01 | **Rebase the current branch onto another** | `feat` 2 commits, `main` 1 newer; Rebase onto `main` | `feat` linear on top of `main`, same messages / authors; the status line says where it is now | `wf_history::rb01_rebase_the_current_branch_onto_another` | `wf_history::rb01_rebase_onto_main_from_the_picker`, ~`menus::branch_commit_stash_menus` (picker) | `wf_history::rb01_rebase_onto_main_from_the_palette` |
| RB-02 | Rebase conflict → Continue with the edited message | conflict on commit 1; resolve; edit the box (git's message in it); Continue | rebased commit has the message; author kept | `conflicts::continue_a_rebase_with_the_edited_message` | `wf_history::rb02_continue_with_the_edited_message` | `wf_history::rb02_continue_with_the_edited_message` |
| RB-03 | Rebase conflict → Abort | stop (said plainly, banner with Continue / Skip / Abort); Abort | branch exactly as before | `wf_history::rb03_rebase_conflict_then_abort` | `wf_history::rb03_rebase_from_the_sidebar_stops_then_abort` | `wf_history::rb03_rebase_stops_then_abort` |
| RB-04 | The next commit conflicts too | resolve, Continue with an edited message, stops again | the stop said plainly; the box empties (its text went into commit 1); the second resolve finishes | `conflicts::continue_a_rebase_with_the_edited_message` | `wf_history::rb04_the_next_commit_conflicts_too` | `wf_history::rb04_the_next_commit_conflicts_too`, `wf_history::rb04_resolve_the_second_and_finish` |
| RB-05 | A step becomes empty | resolution equals upstream; Continue | git drops it (without a word); state clean; the app says it was left out — also when the next commit stops | `wf_history::rb05_a_step_resolved_to_the_base_is_left_out_and_said`, `wf_history::rb05_left_out_before_the_next_stop_is_said_with_it` | `wf_history::rb05_resolved_to_the_base_is_left_out_and_said` | `wf_history::rb05_resolved_to_the_base_is_left_out_and_said` |
| RB-06 | Rebase drops commits already upstream | `A` cherry-picked to `main` before | `A` not replayed; the status line says it was left out | `wf_history::rb06_commits_already_upstream_are_left_out` | `wf_history::rb06_a_commit_picked_before_is_left_out` | `wf_history::rb06_a_commit_picked_before_is_left_out` |
| RB-07 | Rebase with a dirty tree | local change | refused in plain words naming the file, ways out offered, nothing changed; with `rebase.autoStash` rebased and the change put back, said so | `wf_history::rb07_rebase_with_a_dirty_tree_is_refused`, `wf_history::rb07_rebase_with_autostash_keeps_the_change` | `wf_history::rb07_rebase_with_a_dirty_tree_is_refused` | `wf_history::rb07_rebase_with_a_dirty_tree_is_refused` |
| RB-08 | The apply backend refuses the box message | `rebase --apply` stopped (the box shows the patch's message); edit it; Continue | refused in plain words; box text kept, not silently dropped | `conflicts::continue_a_rebase_with_the_edited_message`, `wf_history::rb08_the_apply_backend_shows_and_guards_the_message` | `wf_history::rb08_the_apply_backend_keeps_the_box_text` | `wf_history::rb08_the_apply_backend_keeps_the_box_text` |
| RB-09 | Pull with rebase | diverged; Pull with… Rebase | local commits on top of the upstream's; strategy remembered on request | `pull::a_diverged_pull_asks_and_each_choice_does_what_it_says`, `pull::the_rebase_retry_ends_where_git_pull_would`, `pull::the_merge_retry_ends_where_git_pull_would`, `pull::pull_with_passes_the_choice_over_the_configuration` | `menus::pull_diverged` | `remote::pull_with_chooses_remembers_and_forgets_the_strategy` |

## RS — Reset and restore

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| RS-01 | Reset soft | reset `main` to HEAD~1 (soft) | HEAD moved; the undone commit's changes staged | `wf_reset::rs01_soft_reset_keeps_index_and_working_tree` | `wf_reset::rs01_soft_reset_from_the_commit_menu` | `wf_reset::rs01_soft_reset_from_the_commit_menu` |
| RS-02 | Reset mixed | mixed | changes unstaged | `wf_reset::rs02_mixed_reset_keeps_the_working_tree` | `wf_reset::rs02_mixed_reset_from_the_commit_menu` | `wf_reset::rs02_mixed_reset_from_the_commit_menu` |
| RS-03 | **Reset hard (asks)** | hard; confirm | HEAD, index and files at the target; untracked kept | `repo_ops::cherry_pick_revert_reset_and_conflict_state`, `wf_reset::rs03_hard_reset_says_what_is_lost_and_does_it` | `wf_reset::rs03_hard_reset_asks_naming_what_is_lost` | `wf_reset::rs03_hard_reset_asks_naming_what_is_lost` |
| RS-04 | Reset hard, recover from the reflog | reset hard; Show Reflog; Create Branch Here on the lost commit | the lost commit back on a branch | `reflog::heads_reflog_keeps_the_commit_a_reset_dropped` | `reflog::create_branch_keeps_a_lost_commit`, `reflog::lists_shows_and_navigates` | `reflog::reflog_lists_shows_navigates_and_recovers` |
| RS-05 | Reset to colliding paths | target has `X.h` and `x.h` (case-insensitive disk) | the app says which paths can't be checked out | `fs_paths::switching_and_resetting_to_colliding_paths_say_so` | `wf_reset::rs05_reset_to_colliding_paths_says_so` | `wf_reset::rs05_reset_to_colliding_paths_says_so` |
| RS-06 | Restore a file / folder from a commit | Restore from Selected Commit… | working tree (and index with `--staged`) as the commit; local change named first | `restore::restore_file_and_folder_from_a_commit` | `menus::restore_and_containing` | `menus::restore_from_a_commit_and_branches_containing_it` |
| RS-07 | Restore the version before a commit | renamed / added file | old name back, new removed; added file deleted | `restore::restore_the_version_before_a_commit` | `wf_reset::rs07_restore_the_version_before_a_commit` | `wf_reset::rs07_restore_the_version_before_a_rename`, `wf_reset::rs07_restore_and_stage_the_version_before_an_addition` |
| RS-08 | Reset the remote branch | Reset origin/main to Here… | origin moves back (lease), local untouched | `push::reset_the_remote_branch_to_a_commit` | `wf_reset::rs08_reset_the_remote_branch` | `remote::reset_remote_branch_from_the_palette` |
| RS-09 | Move another branch to a reflog commit | reflog of `feat`; move it | `feat` at that commit | `reflog::another_branch_moves_to_a_reflog_commit`, `wf_reset::rs09_move_another_branch_to_a_reflog_commit` | `wf_reset::rs09_move_another_branch_from_its_reflog` | `wf_reset::rs09_move_another_branch_from_its_reflog` |
| RS-10 | Undo a hard reset from the reflog | HEAD's reflog; the lost commit's entry ▸ Reset main to Here ▸ Hard (asks: nothing lost, untracked file stays) | main back at the commit, its files; untracked file kept | `wf_reset::rs10_undo_a_hard_reset_through_the_reflog` | `wf_reset::rs10_undo_a_hard_reset_from_the_reflog` | `wf_reset::rs10_undo_a_hard_reset_from_the_reflog` |

## TG — Tags

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| TG-01 | Lightweight and annotated tags | Create Tag… with / without a message; delete | as typed, multi-line, `#` lines kept | `repo_ops::branch_tag_stash_remote_ops`, `repo_ops::tag_details_lightweight_annotated_signed` | `menus::tag_message_and_show_tag` | `menus::commit_menu_by_right_click_and_dot_creates_a_tag_and_a_branch` |
| TG-02 | Show a tag | Show Tag | lightweight says so; annotated: tagger, message; signed verified | `repo_ops::tag_details_lightweight_annotated_signed` | `menus::tag_message_and_show_tag` | `menus::show_tag_lightweight_and_annotated` |
| TG-03 | Push tags | push one / all / follow-tags | on the remote | `push::push_to_several_remotes_with_a_new_name_tags_and_a_failing_one`, `wf_tags::tg03_push_one_tag_then_all` | `wf_tags::tg03_push_a_tag_then_all_tags`, ~`toolbar::push_steps_and_dialog` | `remote::push_dialog_to_two_remotes_under_a_new_name_with_tags` |
| TG-04 | Delete a tag locally and on the remote | Delete Tag…, also on origin | gone in both | ~`repo_ops::branch_tag_stash_remote_ops`, `wf_tags::tg04_delete_a_tag_on_the_remote_and_here` | `wf_tags::tg04_delete_a_tag_on_origin_then_here` | `menus::sidebar_menus_rename_a_branch_and_delete_a_tag` |
| TG-05 | Tag name already exists | Create Tag… `v1` again | refused in plain words before git | `wf_tags::tg05_an_existing_tag_name_is_refused_before_git` | `wf_tags::tg05_an_existing_name_is_refused_in_the_dialog` | `wf_tags::tg05_an_existing_name_is_refused_in_the_dialog` |
| TG-06 | Tags on tags / non-commits | — | listed and shown | `repo_shapes::tags_on_tags_and_on_non_commits` | `wf_tags::tg06_tags_of_tags_and_blobs_listed_and_shown` | `wf_tags::tg06_tags_of_tags_and_blobs_listed_and_shown` |
| TG-07 | Tag a detached commit | HEAD detached; Create Tag at HEAD… with a multi-line message (a `#` line) | annotated tag of that commit, message as typed; HEAD stays detached | `wf_tags::tg07_tag_a_detached_commit` | `wf_tags::tg07_tag_a_detached_commit` | `wf_tags::tg07_tag_a_detached_commit` |

## RM — Remotes (local bare remote)

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| RM-01 | Add / rename / re-URL / remove a remote | sidebar and dialogs | config as said | `repo_ops::branch_tag_stash_remote_ops`, `wf_remotes::rm01_add_change_url_rename_and_remove_a_remote` | ~`menus::menu_entries`, `wf_remotes::rm01_add_change_url_rename_and_remove_a_remote` | ~`keybar::the_keys_shown_run_their_entries`, `wf_remotes::rm01_add_a_remote`, `wf_remotes::rm01_change_the_url_and_rename_a_remote`, `wf_remotes::rm01_remove_a_remote` |
| RM-02 | Fetch | f / Fetch | remote-tracking branches updated; progress shown | ~`runner::fetch_streams_progress_meters`, `wf_remotes::rm02_fetch_then_pull_fast_forwards` | `keys::remote_keys`, `wf_remotes::rm02_fetch_then_pull_fast_forwards` | `remote::fetch_and_pull_with_a_local_remote`, `progress::fetch_progress_shows_while_fetching`, `wf_remotes::rm02_fetch_then_pull_fast_forwards`, `wf_remotes::rm02_fetch_shows_what_someone_pushed` |
| RM-03 | Pull fast-forward | behind only | fast-forwarded | `wf_remotes::rm02_fetch_then_pull_fast_forwards` | `keys::remote_keys`, `wf_remotes::rm02_fetch_then_pull_fast_forwards` | `remote::fetch_and_pull_with_a_local_remote`, `wf_remotes::rm02_fetch_then_pull_fast_forwards` |
| RM-04 | Pull diverged | no strategy configured | asks merge / rebase / ff-only; remembers on request; config wins | `pull::a_diverged_pull_asks_and_each_choice_does_what_it_says`, `pull::a_remembered_strategy_is_used_without_asking`, `pull::git_config_decides_over_the_remembered_strategy`, `pull::branch_rebase_config_decides_and_no_upstream_runs_no_pull`, `pull::pull_from_asks_and_takes_the_remembered_strategy` | `menus::pull_diverged` | `remote::diverged_pull_asks_for_a_strategy_and_remembers_it` |
| RM-05 | Pull that conflicts (merge) | diverged on the same line; Pull with Merge | conflict state, banner, resolve, commit concludes | `wf_remotes::rm05_pull_merge_conflict_resolved_and_committed` | `wf_remotes::rm05_pull_merge_stops_on_the_conflict`, `wf_remotes::rm05_resolve_and_commit_the_stopped_pull` | `wf_remotes::rm05_pull_merge_stops_on_the_conflict`, `wf_remotes::rm05_resolve_and_commit_the_stopped_pull` |
| RM-06 | Pull that conflicts (rebase) | diverged on the same line; Pull with Rebase | rebase stopped (said, not an error); Continue / Abort | `wf_remotes::rm06_pull_rebase_conflict_resolved_and_continued`, `wf_remotes::rm06_pull_rebase_conflict_aborted` | `wf_remotes::rm06_pull_rebase_stops_on_the_conflict`, `wf_remotes::rm06_resolve_and_continue_the_stopped_pull` | `wf_remotes::rm06_pull_rebase_stops_on_the_conflict`, `wf_remotes::rm06_resolve_and_continue_the_stopped_pull` |
| RM-07 | Pull into a dirty tree | local change in a file the pull changes; Stash and Pull | refused in plain words naming the file; nothing changed; Stash and Pull pulls and puts the change back | `wf_remotes::rm07_pull_into_a_dirty_tree_is_refused` | `wf_remotes::rm07_pull_into_a_dirty_tree_is_refused` | `wf_remotes::rm07_pull_into_a_dirty_tree_is_refused`, `wf_remotes::rm07_stash_and_pull` |
| RM-08 | Push a new branch | no upstream; Push | asks for the remote; pushes with `-u`; then plain Push | `push::main_push_and_pull_without_upstream_never_reach_the_remote`, `push::the_chosen_remote_becomes_the_upstream_of_push_and_pull` | `toolbar::push_steps_and_dialog`, `toolbar::pull_and_push_without_upstream` | `remote::pull_and_push_without_upstream_run_no_git` |
| RM-09 | Push rejected: someone pushed | Push | plain words; Pull or Fetch, Then Decide; lease pinned to what was fetched | `push::refused_push_after_someone_pushed` | `menus::push_refused_ways_out` | `remote::refused_push_fetch_then_push_with_lease` |
| RM-10 | Force with lease | rewritten history; Push… force with lease | lists what the remote loses; refused if moved since | `push::force_push_pins_the_lease_the_user_saw`, `push::force_push_options_list_what_each_remote_loses`, `push::force_with_lease_and_no_verify` | `wf_remotes::rm10_force_with_lease_after_someone_pushed` | `remote::push_options_force_with_lease_lists_what_the_remote_loses` |
| RM-11 | Push to several remotes | Push… two remotes, new name | each result; a failing one doesn't stop others | `push::push_to_several_remotes_with_a_new_name_tags_and_a_failing_one` | `toolbar::push_steps_and_dialog` | `remote::push_dialog_to_two_remotes_under_a_new_name_with_tags` |
| RM-12 | Pull from another remote; prune | Pull from…; Prune | other remote's branch; gone branches pruned | `push::pull_from_one_remote_and_prune_another` | `wf_remotes::rm12_pull_from_another_remote_and_prune` | `wf_remotes::rm12_pull_from_another_remote`, `wf_remotes::rm12_prune_stale_branches` |
| RM-13 | Delete a remote branch | sidebar Delete on `origin/feat` | gone on origin and locally | `wf_remotes::rm13_delete_a_remote_branch` | `wf_remotes::rm13_delete_a_remote_branch` | `wf_remotes::rm13_delete_a_remote_branch` |
| RM-14 | Credentials asked | an HTTP remote wanting a password | prompt names the command; secret hidden; Esc stops | `askpass_env` (binary) | ~`layout::askpass_key_icon` | `remote::askpass_prompt_takes_typing_enter_and_esc`, `cancel::esc_in_a_repeating_prompt_stops_the_operation`, `cancel::prompt_of_a_gone_helper_closes` |
| RM-15 | Cancel a hanging fetch | Esc on the busy label | stopped; idle again | ~`runner::cancel_stops_a_hanging_hook`, `wf_remotes::rm15_cancel_a_hanging_fetch` | — | `cancel::esc_cancels_a_hanging_fetch` |
| RM-16 | Failed pull's warning | a pull fails; next operation | warning stays until the next user operation | n/a | `toolbar::error_mark`, `toolbar::warnings_go_with_every_remote_op`, `toolbar::a_fetch_waits_for_the_pull`, `keys::warnings_clear` | `remote::error_mark_until_the_next_operation` |
| RM-17 | Clone from a local bare repository | Clone… | opened; branch of the source; refused into a non-empty folder | `repo_setup::clone_from_local_bare_repo` | `menus::clone_and_init` | `repo_cmds::clone_from_the_palette` |
| RM-18 | Unpushed / gone upstream marks | commits not pushed; upstream deleted | marks say so | `tab_marks::commits_not_pushed_with_and_without_an_upstream`, `tab_marks::a_local_upstream_compares_with_that_branch` | `layout::repo_tab_marks_follow` | `views::repo_marks_after_the_name` |

## WT — Worktrees

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| WT-01 | Add, list, lock, remove, prune | Add Worktree… (new / existing / detached) | as git lists; opened in a tab | `worktrees::add_list_lock_remove_prune` | `worktrees::add_open_remove`, `worktrees::live_plan` | `repo_cmds::add_worktree_says_what_happens` |
| WT-02 | Remove a dirty worktree / with its branch | Remove… also delete branch | Force Remove asked; unmerged branch kept until Force Delete | `worktrees::remove_with_branch` | `worktrees::detached_and_dirty_remove`, `worktrees::remove_with_branch` | `repo_cmds::remove_a_worktree_with_its_branch` |
| WT-03 | Stage and commit in a linked worktree | tab on the linked worktree | works; refs shared with main | `worktrees::repo_opened_in_a_linked_worktree`, `wf_modules::wt03_stage_and_commit_in_a_linked_worktree` | `worktrees::linked_tab_works` | `wf_modules::wt03_stage_and_commit_in_a_linked_worktree` |
| WT-04 | Sparse worktree | Add with folders | only those folders; clean status | `worktrees::sparse_worktree` | `worktrees::log_marks_and_sparse`, `worktrees::folder_picker` | `repo_cmds::add_a_sparse_worktree` |
| WT-05 | Move / repair / remove own | move; repair after a manual move; remove from its own tab | tab follows / main tab takes over | `worktrees::move_worktree`, `worktrees::repair_worktrees_moved_by_hand` | `worktrees::move_and_repair`, `worktrees::remove_from_own_tab` | `repo_cmds::remove_or_move_the_worktree_gitty_runs_in` |
| WT-06 | Worktree marks | dirty / unpushed worktrees | marks per entry | `tab_marks::other_worktrees_get_their_own_marks` | `worktrees::sidebar_marks` | `views::worktree_entries_carry_the_marks` |
| WT-07 | Track a remote branch in a new worktree | type `origin/feat` | local branch tracking it | `worktrees::plan_add_tracks_a_remote_branch`, `wf_modules::wt07_track_a_remote_branch_in_a_new_worktree` | `wf_modules::wt07_track_a_remote_branch_in_a_new_worktree` | `wf_modules::wt07_track_a_remote_branch_in_a_new_worktree` |
| WT-08 | Stash made in one worktree, popped in another | stash in A; pop in B | the shared stash list; applied in B | `wf_stash::wt08_stash_from_one_worktree_popped_in_another` | `wf_stash::wt08_pop_in_another_worktree` | `wf_stash::wt08_pop_in_another_worktree` |

## SM — Submodules

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| SM-01 | List and states | up to date / new commits / out of date / dirty / uninitialised / nested | as `git submodule status --recursive` | `submodules::list_matches_git_and_follows_the_states`, `submodules::new_commits_and_out_of_date_submodules`, `submodules::nested_dirty_state_comes_from_a_status_in_its_dirty_parent` | `submodules::sidebar_banner_update`, `submodules::stage2_nested_remote_remove` | `submodules::submodule_out_of_date_badge_and_update_from_the_palette` |
| SM-02 | Pointer change staged as one unit | new commits in the submodule; stage | the pointer, shown as commit summaries | `submodules::pointer_change_is_a_commit_summary_staged_as_one_unit`, `submodules::pointer_change_unstages_and_discards_as_one_unit` | `submodules::pointer_diff_rows` | `submodules::submodule_pointer_change_shows_commits_and_stages_as_one_unit` |
| SM-03 | Pull moves the pointer → Update | pull; Update Submodules | checked out at the recorded commit | `submodules::pull_that_moves_the_pointer_makes_it_out_of_date`, `submodules::update_nested_paths`, `submodules::sync_nested_paths`, `submodules::update_to_the_remote_branch` | `submodules::sidebar_banner_update` | `submodules::submodule_out_of_date_badge_and_update_from_the_palette` |
| SM-04 | Add a submodule | Add Submodule… (URL, local path) | staged with `.gitmodules`; file transport asked | `submodules::add_submodule_stages_it_with_its_default_path_and_branch`, `submodules::add_submodule_at_a_path_and_twice`, `submodules::add_submodule_from_a_local_path_asks_for_the_file_transport` | `submodules::add_local_path_asks` | `submodules::add_a_submodule_from_the_palette`, `submodules::add_a_submodule_from_a_local_path_asks_first` |
| SM-05 | Remove a submodule | Remove… with local work | warned; cleaned up | `submodules::remove_submodule_warns_about_local_work_and_cleans_up`, `submodules::remove_a_nested_submodule`, `submodules::remove_an_uninitialised_submodule`, `wf_modules::sm05_remove_a_submodule_with_local_work` | `submodules::stage2_nested_remote_remove` | `wf_modules::sm05_remove_a_submodule_with_local_work` |
| SM-06 | Check out a commit in a submodule | Check Out Commit in Submodule… | detached there; pointer change in the parent | `submodules::check_out_a_commit_in_the_submodule_records_it`, `submodules::submodule_checkout_risks`, `submodules::submodule_checkout_of_a_nested_one`, `submodules::submodule_checkout_of_an_uninitialised_one`, `wf_modules::sm06_check_out_a_commit_in_a_submodule` | `submodules::checkout_commit` | `wf_modules::sm06_check_out_a_commit_in_a_submodule` |
| SM-07 | Commit inside a submodule | commit in its own repository | the parent sees the new pointer | `wf_modules::sm07_commit_inside_a_submodule` | `submodules::own_commit_is_seen` | `wf_modules::sm07_commit_inside_a_submodule`, `wf_modules::sm07_record_the_submodule_commit_in_the_superproject` |
| SM-08 | Clone: uninitialised vs recursive | plain / recursive clone | states right | `submodules::uninitialised_after_a_plain_clone`, `submodules::a_recursive_clone_checks_out_the_nested_submodules` | `wf_modules::sm08_plain_clone_leaves_the_submodule_uninitialised`, `wf_modules::sm08_recursive_clone_asks_then_checks_it_out` | `submodules::uninitialised_submodule_copies_the_recorded_sha` |

## LF — Git LFS

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| LF-01 | LFS file changed | edit a tracked LFS file | content diff; whole-file stage only, partial refused | `lfs::working_lfs_files_stage_only_whole`, `lfs::whole_file_stage_runs_the_clean_filter`, `wf_modules::lf01_an_lfs_file_stages_only_whole` | `wf_modules::lf01_an_lfs_file_stages_only_whole` | `lfs::lfs_rows_whole_file_staging_and_the_notice` |
| LF-02 | Track with Git LFS | Track… in a folder | line in that `.gitattributes`; file stored as a pointer | `lfs::track_in_a_folder`, `lfs::track_a_folder_keeps_its_attributes_plain`, `lfs::track_sets_up_git_lfs_only_when_asked` | `lfs::track_without_git_lfs` | `lfs::track_with_git_lfs_from_the_diff_menu`, `lfs::track_without_git_lfs_says_how_to_install` |
| LF-03 | Push uploads the object | commit and push | object on the remote | `lfs::push_uploads_to_a_local_remote` | `lfs::push_without_the_lfs_hook_asks` | `lfs::push_without_the_lfs_hook_asks` |
| LF-04 | Missing objects downloaded | a commit whose object isn't local | Download fetches it; diff shows content | `lfs::download_fetches_missing_objects_from_the_remote`, `lfs::cached_diffs_reannotate_when_objects_arrive` | `lfs::download_missing_objects` | `lfs::download_missing_objects_from_the_diff_menu` |
| LF-05 | Pointers left / not set up / hooks | clone without git-lfs; Set Up | health explained and fixed | `lfs_health::not_set_up_then_set_up`, `lfs_health::pointer_files_in_the_working_tree`, `lfs_health::download_replaces_the_pointers` | `lfs::missing_git_lfs_banner`, `lfs::hooks_without_lfs_offer_removal` | `lfs::missing_git_lfs_badge_explained_and_hidden`, `lfs::hooks_without_lfs_removed` |
| LF-06 | Clone checks out content | clone a repo with LFS | content, nothing to commit | `lfs::clone_checks_out_the_content`, `wf_modules::lf06_clone_checks_out_the_content` | `wf_modules::lf06_clone_checks_out_the_content` | `wf_modules::lf06_clone_checks_out_the_content` |

## DC — Discard

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| DC-01 | Discard one line | select a line; discard (asks) | only that line goes back | `staging::discard_single_line`, `staging::replaced_block_lines_stay_in_place`, `staging::uneven_blocks_and_hidden_whitespace_keep_lines_in_place`, `staging::crlf_last_line_without_newline_gets_crlf` | `speed::staging_keys` | `staging::stage_a_single_line_and_discard_lines_after_asking` |
| DC-02 | Discard a hunk | discard the hunk (asks) | the hunk restored, others stay | `wf_staging::dc02_discard_a_hunk`, ~`hostile_config::stage_hunk_lines_unstage_and_discard` | `wf_staging::dc02_discard_a_hunk` | `wf_staging::dc02_discard_a_hunk` |
| DC-03 | Discard All keeps staged work | staged + unstaged; Discard All | unstaged gone (untracked too), staged stays | `staging::discard_all_keeps_staged_changes` | `speed::staging_keys` | `staging::stage_all_unstage_all_discard_all_with_confirmations` |
| DC-04 | Discard thousands of untracked files | `target/` with 1 000 files | one go; ignored file stays; the file watcher's echo of the deletions re-reads nothing | `discard::discard_all_deletes_thousands_of_untracked_files_in_one_go`, `discard::the_reread_after_a_discard_covers_the_deleted_folders`, `watch::our_whole_reread_covers_deleted_folders` (unit) | `wf_discard::dc04_discard_thousands_of_untracked_files` | `wf_discard::dc04_discard_thousands_of_untracked_files` |
| DC-05 | Discard with a nested repository | untracked folder that is a repository | kept and named unless asked | `discard::nested_repository_is_kept_and_named_unless_asked`, `discard::untracked_folder_entry_keeps_a_repository_inside` | `menus::discard_all_nested_repository` | `staging::discard_all_keeps_a_nested_repository_unless_asked` |
| DC-06 | A file can't be deleted | read-only folder (unix) | the others go; error names the file | `discard::a_file_that_cannot_be_deleted_does_not_stop_the_others`, `unix_fs::read_only_directory` | `wf_discard::dc06_a_file_that_cannot_be_deleted` | `wf_discard::dc06_a_file_that_cannot_be_deleted` |
| DC-07 | Discard a group | Untracked Files group; discard (asks) | only that group | `wf_staging::dc07_discard_a_group` | `wf_staging::dc07_discard_a_group` | `views::unstaged_groups_fold_stage_and_discard_with_totals` |
| DC-08 | Discard a deleted file / an untracked file | file deleted; discard | file back; untracked file removed | `wf_staging::dc08_discard_a_deleted_and_an_untracked_file`, ~`staging::working_state_after_whole_files_match_a_full_read` | `wf_staging::dc08_discard_a_deleted_and_an_untracked_file` | `wf_staging::dc08_discard_a_deleted_and_an_untracked_file` |
| DC-09 | Discard lines of a read-only file | 0444 file (unix) | rewritten, mode kept | `unix_fs::read_only_file_discard_lines` | `wf_discard::dc09_discard_lines_of_a_read_only_file` | `wf_discard::dc09_discard_lines_of_a_read_only_file` |

## RL — Undo and recovery

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| RL-01 | Find a commit a reset dropped | Show Reflog | "lost" entry marked; its commit shown | `reflog::heads_reflog_keeps_the_commit_a_reset_dropped` | `reflog::lists_shows_and_navigates`, `reflog::menu_bar_and_branch_menu_open_it` | `reflog::reflog_lists_shows_navigates_and_recovers` |
| RL-02 | Recover a deleted branch | delete `feat`; its reflog commit → Create Branch | `feat` back | `wf_branches::br12_delete_merged_then_unmerged_and_recover_it` | `wf_branches::rl02_recover_a_deleted_branch_from_the_reflog` | `wf_branches::rl02_recover_a_deleted_branch_from_the_reflog` |
| RL-03 | Undo an amend | amend; reflog → reset (soft) to the pre-amend commit | old HEAD back; what the amend added staged | `wf_history::rl03_undo_an_amend_through_the_reflog` | `wf_history::rl03_undo_an_amend_from_the_reflog` | `wf_history::rl03_undo_an_amend_from_the_reflog` |
| RL-04 | Undo a bad rebase | rebase; reflog → reset (hard) to before | branch as before | `wf_history::rl04_undo_a_rebase_through_the_reflog` | `wf_history::rl04_undo_a_rebase_from_the_reflog` | `wf_history::rl04_undo_a_rebase_from_the_reflog` |

## IG — Ignore rules and untracked files

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| IG-01 | Ignore a file | Add to .gitignore… | line written once; file gone from the list | `repo_setup::ignored_file_disappears_and_line_is_not_duplicated`, `repo_setup::folder_pattern_hides_everything_below` | `menus::gitignore_dialog` | `repo_cmds::ignore_from_the_diff_menu` |
| IG-02 | Ignore a tracked file | Stop tracking (`rm --cached`) | file kept on disk, untracked then ignored | `repo_setup::tracked_files_stay_tracked_unless_untracked` | `menus::gitignore_dialog` | `repo_cmds::ignore_a_tracked_file_and_info_exclude` |
| IG-03 | Folder `.gitignore` / `info/exclude` | target a folder's file | pattern relative to that folder | `repo_setup::folder_gitignore_patterns_apply_inside_the_folder`, `repo_setup::info_exclude_target` | `menus::folder_rule_files` | `repo_cmds::ignore_into_a_folder_gitignore` |
| IG-04 | `status.showUntrackedFiles` | `no` / `normal` | hidden said so; folder entry stages / discards whole | `config_safety::show_untracked_files_config_is_honoured`, `config_safety::untracked_folder_entry_stages_and_discards_as_a_whole` | `config_safety::hidden_untracked_note` | `config_safety::hidden_untracked_files_are_said_to_be_hidden` |
| IG-05 | Stage All with hidden untracked files | `showUntrackedFiles=no`, a hidden new file; Stage All | the hidden file is not staged without saying so | `op_safety::stage_all_leaves_hidden_untracked_files_out` | `op_safety::hidden_untracked_and_stale_stage` | `op_safety::stage_all_leaves_hidden_untracked_files_out` |
| IG-06 | Negation and global excludes | `!keep.log` in a nested `.gitignore`; `core.excludesFile` | as git decides | `wf_shapes::ig06_negation_and_excludes_file` | `wf_shapes::ig06_negation_and_excludes_file` | `wf_shapes::ig06_negation_and_excludes_file` |

## RN — Renames

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| RN-01 | Rename a file and stage both sides | `mv a b`; stage deletion and new file | one rename staged; unstage brings both back | `wf_shapes::rn01_stage_and_unstage_both_halves_of_a_rename`, ~`unix_fs::rename_only_in_case` | `wf_shapes::rn01_stage_and_unstage_both_halves_of_a_rename` | `wf_shapes::rn01_stage_and_unstage_both_halves_of_a_rename` |
| RN-02 | Rename with edits, stage some lines | `mv a b` + edit | lines of `b` stage; rename detected once both staged | `wf_shapes::rn02_stage_lines_of_a_renamed_file_then_its_deletion` | `wf_shapes::rn02_stage_lines_of_a_renamed_file_then_its_deletion` | `wf_shapes::rn02_stage_lines_of_a_renamed_file_then_its_deletion` |
| RN-03 | File history across a rename | Go to Commit with the file | commits through the rename | `search::file_history_follows_renames_and_deleted_files` | `menus::go_to_commit` | `views::go_to_commit_finds_file_history_and_messages` |
| RN-04 | Commit diff of a rename | select the commit | old → new name | `history_diffs::batched_commit_diffs_match_single_ones` | `wf_shapes::rn04_a_commit_that_renames` | `wf_shapes::rn04_a_commit_that_renames` |
| RN-05 | Merge across a rename | `feat` renames `a`, `main` edits `a` | clean merge, edit in the new name | `wf_merge::rn05_merge_across_a_rename` | `wf_merge::rn05_merge_across_a_rename` | `wf_merge::rn05_merge_across_a_rename` |

## BN — Binary files

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| BN-01 | Binary next to text | change both | binary stages whole; a binary patch is an error, not a broken index | `unix_fs::binary_next_to_text` | `wf_shapes::bn01_a_line_beside_binary_files` | `wf_shapes::bn01_stage_a_line_beside_binary_files`, `wf_shapes::bn01_stage_binary_files`, `wf_shapes::bn01_unstage_a_binary_file`, `wf_shapes::bn01_discard_a_binary_file` |
| BN-02 | Text becomes binary and back | — | listed right each time | `unix_fs::text_becomes_binary` | `wf_shapes::bn02_text_becomes_binary_and_back` | `wf_shapes::bn02_text_becomes_binary_and_back`, `wf_shapes::bn02_unstage_a_line_of_the_text_again` |
| BN-03 | Huge / binary file in a commit | select the commit | listed, not diffed; Load anyway for text | `history_diffs::huge_files_are_listed_but_not_diffed`, `history_diffs::stash_and_compare_guard_huge_files` | `speed::huge_file_in_a_commit_loads_on_demand` | `speed::huge_file_in_a_commit_loads_on_demand` |

## BG — Big and many

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| BG-01 | A commit with a huge diff | merge of 1 700 files selected, then its neighbour (as big, prefetched) | each shows after one load; neither loaded again | `wf_shapes::bg01_a_huge_merge_loads_once` | `speed::huge_commit_shows_after_one_load` | `wf_shapes::bg01_a_huge_merge_loads_once` |
| BG-02 | Very long lines | 200 KB lines | diff and stage byte-exact | `unix_fs::very_long_lines` | `wf_shapes::bg02_stage_and_discard_beside_very_long_lines` | `wf_shapes::bg02_stage_a_very_long_line`, `wf_shapes::bg02_discard_beside_very_long_lines` |
| BG-03 | Long history | 5 000+ commits | compact rows, partial history first | `repo_shapes::very_long_linear_history`, `repo_shapes::history_rows_are_compact`, `repo_shapes::partial_history_is_the_newest_slice_of_the_full_one` | `wf_shapes::bg03_a_long_history` | `wf_shapes::bg03_a_long_history` |
| BG-04 | Stage a group of > 200 paths | 250 untracked files; stage the group | all staged (chunked `git add`) | `wf_staging::bg04_stage_a_group_of_250_paths` | `wf_staging::bg04_stage_a_group_of_250_paths` | `wf_staging::bg04_stage_a_group_of_250_paths` |
| BG-05 | Watcher with thousands of folders | — | no loop, ignored trees unwatched | `linux_env::watcher_handles_thousands_of_directories`, `linux_env::watcher_leaves_ignored_trees_unwatched` | n/a | n/a |

## NU — Non-UTF-8 names and content

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| NU-01 | Latin-1 / Windows-1252 lines | stage / unstage / discard lines | exact bytes; encoding named | `non_utf8::eight_bit_lines_stage_unstage_discard_exactly`, `unix_fs::latin1_content_stages_lines` | `speed::non_utf8_line_stages_exactly` | `staging::latin1_line_stages_exactly` |
| NU-02 | UTF-16 with a `diff` attribute | lines | exact | `non_utf8::utf16_lines_stage_unstage_discard_exactly` | `wf_shapes::nu02_utf16_lines` | `wf_shapes::nu02_utf16_stage_and_discard_lines`, `wf_shapes::nu02_utf16_unstage_a_line` |
| NU-03 | `working-tree-encoding` | discard a line; stage one line of an untracked file | in its encoding on disk, UTF-8 in the index | `non_utf8::working_tree_encoding_and_untracked_files` | `wf_eol::nu03_working_tree_encoding` | `wf_eol::nu03_working_tree_encoding` |
| NU-04 | A file name that isn't UTF-8 | in the index only (any unix disk): unstage a line, stage / unstage the whole file; on disk (Linux): stage a line, stage / unstage / discard the whole file | listed with U+FFFD; git gets the name's bytes (arguments, pathspec lists, patch headers, the disk); a name two files show alike refused in plain words | `wf_shapes::nu04_a_name_that_is_not_utf8_in_the_index`, `wf_shapes::nu04_a_name_that_is_not_utf8_on_disk`, `unix_fs::non_utf8_name_is_listed`, `unix_fs::non_utf8_name_stages`, `non_utf8::names_that_are_not_utf8_are_refused` | `wf_shapes::nu04_a_name_that_is_not_utf8_in_the_index`, `wf_shapes::nu04_a_name_that_is_not_utf8_on_disk` | `wf_shapes::nu04_unstage_a_line_of_a_name_that_is_not_utf8`, `wf_shapes::nu04_stage_and_unstage_a_name_that_is_not_utf8`, `wf_shapes::nu04_a_name_that_is_not_utf8_on_disk` |
| NU-05 | Odd names everywhere | spaces, quotes, newlines, glob characters | hunks, lines, files right | `unix_fs::odd_names_tracked_hunks_and_lines`, `unix_fs::odd_names_tracked_discard_and_whole_files`, `unix_fs::odd_names_untracked_partial_stage_and_unstage`, `unix_fs::odd_names_untracked_discard_lines_and_files`, `unix_fs::odd_names_deleted_partial_stage_and_discard` | `wf_shapes::nu05_odd_names` | `wf_shapes::nu05_hunks_of_odd_names`, `wf_shapes::nu05_stage_odd_names_by_group`, `wf_shapes::nu05_unstage_odd_names`, `wf_shapes::nu05_discard_odd_names_by_group` |
| NU-06 | A commit's file whose name isn't UTF-8 | a name only commits have (added, changed, removed again; any unix disk): show the commit; Open This / Old Version; its file history; Restore This Version… (restored on Linux) | listed with U+FFFD; its bytes learned from the commit view (trees, git's diff) — the copies, the history, the restore plan and the restore reach the file | `wf_shapes::nu06_a_commits_file_whose_name_is_not_utf8` | `wf_shapes::nu06_a_commits_file_whose_name_is_not_utf8` | `wf_shapes::nu06_a_commits_file_whose_name_is_not_utf8` |
| NU-07 | Stash Selected Lines of a name that isn't UTF-8 | on disk (Linux): an edited `caf\xe9.txt`; select a line; Stash Selected Lines | the stash holds the line, the working tree loses it | `wf_shapes::nu07_stash_lines_of_a_name_that_is_not_utf8` | `wf_shapes::nu07_stash_lines_of_a_name_that_is_not_utf8` | `wf_shapes::nu07_stash_lines_of_a_name_that_is_not_utf8` |
| NU-08 | Ignore a tracked name that isn't UTF-8 | in the index only (any unix disk): Add to .gitignore… "This file", Stop tracking | `.gitignore` gets the name's bytes (git matches by bytes); the file leaves the index | `wf_shapes::nu08_ignore_a_name_that_is_not_utf8` | `wf_shapes::nu08_ignore_a_name_that_is_not_utf8` | `wf_shapes::nu08_ignore_a_name_that_is_not_utf8` |

## FS — File-system paths and file types

| ID | Story | Steps | Expected | core | gitti | gitty |
|---|---|---|---|---|---|---|
| FS-01 | Case collision in the index | `X.h` and `x.h` on a case-insensitive disk | marked, explained; staging asks; hide and show again | `fs_paths::index_paths_the_file_system_cant_hold_are_reported`, `fs_paths::hiding_with_skip_worktree_and_showing_again` | `fs_paths::marked_explained_and_noted`, `fs_paths::stage_asks_first`, `fs_paths::hide_and_show_again` | `fs_paths::colliding_paths_marked_staging_asks_hiding_settles` |
| FS-02 | A commit adds the other spelling | history commit adds `A.TXT` | marked before checkout | `fs_paths::a_commit_that_adds_the_other_spelling_is_marked_before_it_is_checked_out` | `fs_paths::history_commit_marked` | `fs_paths::history_commit_marked_before_checkout` |
| FS-03 | Rename only in case | `foo` → `Foo` | delete + untracked, a rename once staged | `unix_fs::rename_only_in_case`, `unix_fs::case_sensitive_names_are_separate_files` | `wf_shapes::fs03_rename_only_in_case` | `wf_shapes::fs03_rename_only_in_case` |
| FS-04 | NFC / NFD names | "café" both ways | two files where the disk keeps both | `unix_fs::unicode_nfc_and_nfd_names_are_separate_files` | `wf_shapes::fs04_nfc_and_nfd_names` | `wf_shapes::fs04_nfc_and_nfd_names` |
| FS-05 | Symlinks | new / changed / dangling / to a folder / deleted | the link, never its target | `unix_fs::new_symlink_stage_unstage_discard`, `unix_fs::untracked_symlink_preview_is_the_link`, `unix_fs::changed_symlink_target_hunk_and_lines`, `unix_fs::dangling_symlink`, `unix_fs::symlink_to_directory`, `unix_fs::deleted_symlink_hunks` | `wf_shapes::fs05_fs06_symlinks` | `wf_shapes::fs05_fs06_stage_symlinks`, `wf_shapes::fs05_discard_a_deleted_symlink` |
| FS-06 | Path beyond a symlink | file under a tracked symlink | staging fails cleanly | `unix_fs::path_beyond_a_symlink_is_refused` | `wf_shapes::fs05_fs06_symlinks` | `wf_shapes::fs05_fs06_stage_symlinks` |
| FS-07 | Special files | FIFO, hard links, empty files | refresh works; each operable | `unix_fs::fifo_in_worktree_does_not_block`, `unix_fs::hard_linked_files_stage_independently`, `unix_fs::empty_files` | `wf_shapes::fs07_special_files` | `wf_shapes::fs07_stage_special_files`, `wf_shapes::fs07_discard_a_hard_linked_file` |
| FS-08 | git refuses a `.git` alias / clone collisions | — | explained in plain words | `fs_paths::gits_refusal_of_a_dot_git_alias_in_plain_words`, `fs_paths::a_clone_explains_gits_collided_warning` | `wf_shapes::fs08_a_dot_git_alias_and_colliding_names` | `wf_shapes::fs08_a_dot_git_alias_and_colliding_names` |

---

## Totals

Counted per workflow: covered = at least one test that covers the whole workflow on that
layer (partial `~` and `n/a` don't count as covered).

| Area | Workflows | core | gitti | gitty |
|---|---|---|---|---|
| ST staging | 21 | 16 | 19 | 15 |
| EX external changes | 24 | 21 | 12 | 14 |
| CM commit / amend / reword | 21 | 21 | 21 | 21 |
| SH stash | 20 | 19 | 18 | 19 |
| BR branches | 19 | 19 | 19 | 18 |
| MG merge | 27 | 27 | 26 | 27 |
| CP cherry-pick | 18 | 18 | 18 | 18 |
| RV revert | 7 | 7 | 7 | 7 |
| RB rebase | 9 | 9 | 9 | 9 |
| RS reset / restore | 10 | 10 | 10 | 10 |
| TG tags | 7 | 7 | 7 | 7 |
| RM remotes | 18 | 17 | 16 | 18 |
| WT worktrees | 8 | 8 | 8 | 8 |
| SM submodules | 8 | 8 | 8 | 8 |
| LF LFS | 6 | 6 | 6 | 6 |
| DC discard | 9 | 9 | 9 | 9 |
| RL recovery | 4 | 4 | 4 | 4 |
| IG ignore | 6 | 6 | 6 | 6 |
| RN renames | 5 | 5 | 5 | 5 |
| BN binary | 3 | 3 | 3 | 3 |
| BG big / many | 5 | 5 | 4 | 4 |
| NU non-UTF-8 | 8 | 8 | 8 | 8 |
| FS paths | 8 | 8 | 8 | 8 |
| **Total** | **271** | **261** | **251** | **252** |

The release suite (`release-tests/`) holds older, slower duplicates of several of these
and the randomized `staging_fuzz`; it is not counted here.

## Open points found by workflow tests

Product issues a workflow test ran into. The test checks what works and leaves a `TODO`
at the step; once fixed, the assertion goes in and the entry goes.

| Workflow | What | Where the test notes it |
|---|---|---|
| RM-14, RM-15 (gitti) | Not testable headless: gpui's test executor runs an operation's job to its end on the test thread, so Esc can't reach a running fetch (as for CM-21), and a credential prompt from a real git can't be answered while its fetch blocks that thread. The prompt dialog itself is covered with the `ask:` step (`layout::askpass_key_icon`); the round trip in gitty. | — |
| RB-01, RB-03 (both apps) | Whether a rebase asks first differs: gitti's Rebase Current Branch onto… picker and the commit list's ref menu ask ("Rebase feat onto main? Rewrites the commits of the current branch."), gitti's sidebar branch menu and gitty (palette, branch menus) start it at once. A design choice: ask everywhere — and then say what follows (the commits get new ids; a branch pushed before needs a force push) — or nowhere. | gitti `wf_history::rb03_rebase_from_the_sidebar_stops_then_abort`, gitty `wf_history::rb01_rebase_onto_main_from_the_palette` |
