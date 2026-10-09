#!/usr/bin/env bash
# End-to-end UI smoke test: builds a demo repo, drives the real app via GITTI_SCRIPT,
# checks the resulting git index, and leaves screenshots in $OUT.
#   scripts/smoke.sh [out-dir]
set -euo pipefail
cd "$(dirname "$0")/.."
OUT="${1:-${TMPDIR:-/tmp}/gitti-smoke}"
rm -rf "$OUT" && mkdir -p "$OUT"
DEMO="$OUT/demo"

cargo build -q -p gitti-app --features screenshot
BIN=./target/debug/gitti
# Isolated default settings (font size etc. affect click coordinates).
export GITTI_CONFIG_DIR="$OUT/config"

mkdir -p "$DEMO" && cd "$DEMO"
git init -q -b main
git config user.name Demo && git config user.email demo@example.com && git config commit.gpgsign false
for i in $(seq 1 40); do echo "line $i" >> a.txt; done
git add -A && git commit -qm "Initial import"
git switch -qc feature && echo feat > f.txt && git add f.txt && git commit -qm "Add feature file"
git switch -q main && sed -i.bak 's/^line 30$/line thirty/' a.txt && rm a.txt.bak && git commit -qam "Tweak line 30"
git merge -q --no-edit feature
sed -i.bak 's/^line 2$/line two/; s/^line 20$/line twenty/' a.txt && rm a.txt.bak
cd - > /dev/null

row_of() { # row index of an Unstaged/Staged added line with given text
  GITTI_SCRIPT="sleep:1500;dump;quit" "$BIN" "$DEMO" 2>&1 | grep "src: $1" | grep "Added $2\$" | awk '{print $1}'
}

R=$(row_of Unstaged "line twenty")
GITTI_SCRIPT="sleep:1500;shot:$OUT/1-initial.png;lines:$R-$R;stage;sleep:1200;shot:$OUT/2-staged-line.png;quit" "$BIN" "$DEMO" 2>/dev/null
STAGED=$(git -C "$DEMO" diff --cached --unified=0 | grep '^[+-][^+-]' || true)
[ "$STAGED" = "+line twenty" ] || { echo "FAIL stage: got '$STAGED'"; exit 1; }

R=$(row_of Staged "line twenty")
GITTI_SCRIPT="sleep:1500;lines:$R-$R;unstage;sleep:1200;shot:$OUT/3-unstaged.png;quit" "$BIN" "$DEMO" 2>/dev/null
[ -z "$(git -C "$DEMO" diff --cached)" ] || { echo "FAIL unstage"; exit 1; }

GITTI_SCRIPT="sleep:1500;select:0;msg:Smoke commit\\n\\nBody text;sleep:200;shot:$OUT/4-message.png;commit;sleep:1500;shot:$OUT/5-committed.png;quit" "$BIN" "$DEMO" 2>/dev/null
[ -n "$(git -C "$DEMO" diff --cached)" ] && { echo "FAIL: index not empty"; exit 1; }
# Nothing was staged → commit must be refused, HEAD unchanged.
[ "$(git -C "$DEMO" log -1 --format=%s)" != "Smoke commit" ] || { echo "FAIL: empty commit went through"; exit 1; }
git -C "$DEMO" add a.txt
GITTI_SCRIPT="sleep:1500;msg:Smoke commit\\n\\nBody text;commit;sleep:1500;quit" "$BIN" "$DEMO" 2>/dev/null
[ "$(git -C "$DEMO" log -1 --format=%B | head -3)" = "$(printf 'Smoke commit\n\nBody text')" ] || { echo "FAIL commit: $(git -C "$DEMO" log -1 --format=%B)"; exit 1; }

# Layout invariants (`layout_check.rs`) in the real window: worktree and commit view.
CHECK=$(GITTI_SCRIPT="sleep:1500;check;select:1;sleep:800;check;quit" "$BIN" "$DEMO" 2>/dev/null | grep '^\[check\]')
echo "$CHECK" | grep -q FAIL && { echo "FAIL: layout check"; echo "$CHECK"; exit 1; }
[ "$(echo "$CHECK" | grep -c ok)" = 2 ] || { echo "FAIL: layout check did not run: $CHECK"; exit 1; }

# Context menu → "Create Branch Here…" dialog on the first commit row (elements by their
# layout probe names, see `probe.rs`; menu items relative to the right click, logical px).
GITTI_SCRIPT="sleep:1500;rclickon:commit-row:1;sleep:300;shot:$OUT/6-menu.png;clickrel:70,40;sleep:300;shot:$OUT/7-dialog.png;answer:from-menu;sleep:1500;quit" "$BIN" "$DEMO" 2>/dev/null
git -C "$DEMO" rev-parse -q --verify refs/heads/from-menu > /dev/null || { echo "FAIL: branch via context menu"; exit 1; }

# Askpass: a fake ssh that asks for a passphrase through $SSH_ASKPASS like real ssh does.
AP="$OUT/askpass" && mkdir -p "$AP" && git init -q --bare "$AP/remote.git"
git -C "$DEMO" push -q "$AP/remote.git" main
cat > "$AP/fake-ssh.sh" <<'SSH'
#!/bin/sh
[ "$1" = "-G" ] && exit 0
[ "$SSH_ASKPASS_REQUIRE" = force ] || exit 255
PASS=$("$SSH_ASKPASS" "Enter passphrase for key 'test': ")
[ "$PASS" = "hunter2" ] || { echo "Permission denied (publickey)." >&2; exit 255; }
for last; do :; done
exec sh -c "$last"
SSH
chmod +x "$AP/fake-ssh.sh"
git -C "$DEMO" remote add sshremote "ssh://fakehost$AP/remote.git"
git -C "$DEMO" config core.sshCommand "$AP/fake-ssh.sh"
# The passphrase dialog turns on secure keyboard entry (macOS) and releases it again.
SECURE=$(GITTI_SCRIPT="sleep:1500;fetch;sleep:1500;shot:$OUT/8-askpass.png;dump;answer:hunter2;sleep:2500;dump;quit" "$BIN" "$DEMO" 2>&1 | sed -n 's/^\[dump\] secure input: //p' | tr '\n' ' ')
git -C "$DEMO" rev-parse -q --verify refs/remotes/sshremote/main > /dev/null || { echo "FAIL: askpass fetch"; exit 1; }
# (A locked screen holds secure input system-wide — loginwindow — so the check can't tell.)
if [ "$(uname)" = Darwin ]; then
  LOCKED=$(ioreg -n Root -d1 | grep -c '"CGSSessionScreenIsLocked"=Yes' || true)
  if [ "$LOCKED" != 0 ]; then
    echo "note: screen locked, secure input check skipped"
  else
    [ "$SECURE" = "true false " ] || { echo "FAIL: secure input during passphrase dialog: '$SECURE'"; exit 1; }
  fi
fi

# Command palette → "Stash Changes…" → dialog; then the stash shows up as a graph row.
echo "stash me" >> "$DEMO/a.txt"
GITTI_SCRIPT="sleep:1500;palette:stash changes;sleep:300;answer:via-palette;sleep:1500;dump;shot:$OUT/9-stashed.png;quit" "$BIN" "$DEMO" 2>/dev/null
git -C "$DEMO" stash list | grep -q via-palette || { echo "FAIL: stash via palette"; exit 1; }

# Branch switcher (secondary-b), first entry = first other local branch ("feature").
GITTI_SCRIPT="sleep:1500;key:secondary-b;sleep:300;shot:$OUT/10-switcher.png;key:enter;sleep:1500;quit" "$BIN" "$DEMO" 2>/dev/null
[ "$(git -C "$DEMO" branch --show-current)" = "feature" ] || { echo "FAIL: branch switcher ($(git -C "$DEMO" branch --show-current))"; exit 1; }

# Repository git settings dialog: first field = user.name (local scope).
GITTI_SCRIPT="sleep:1500;config;sleep:300;shot:$OUT/11-config.png;answer:Smoke User;sleep:1500;quit" "$BIN" "$DEMO" 2>/dev/null
[ "$(git -C "$DEMO" config --local user.name)" = "Smoke User" ] || { echo "FAIL: config dialog"; exit 1; }

# Command line in the git log panel.
GITTI_SCRIPT="sleep:1500;cmd:tag smoke-tag;sleep:1500;shot:$OUT/12-cmdline.png;quit" "$BIN" "$DEMO" 2>/dev/null
git -C "$DEMO" rev-parse -q --verify refs/tags/smoke-tag > /dev/null || { echo "FAIL: command line"; exit 1; }

# The command log is persisted (JSON lines per repo).
grep -q '"smoke-tag"' "$GITTI_CONFIG_DIR"/logs/*.jsonl || { echo "FAIL: persistent command log"; exit 1; }

# Settings view: click the "Wrap long lines" switch in the Diff section.
GITTI_SCRIPT="sleep:1500;settings:diff;sleep:300;clickon:setting:sw-wrap;sleep:300;shot:$OUT/13-settings.png;quit" "$BIN" "$DEMO" 2>/dev/null
# Settings live in SQLite (one row per field); the switch writes just that field.
DB="$GITTI_CONFIG_DIR/gitti.db"
[ "$(sqlite3 "$DB" "SELECT value FROM settings WHERE key = 'wrap_lines'")" = "true" ] || { echo "FAIL: settings switch"; exit 1; }
[ -z "$(sqlite3 "$DB" "SELECT key FROM settings WHERE key = 'ui_font_size'")" ] || { echo "FAIL: unchanged settings were written"; exit 1; }

# Graph navigation: from the merge commit, alt-down follows the first parent,
# alt-shift-down takes the merged (second) parent.
nav() { GITTI_SCRIPT="sleep:1500;select:$1;sleep:300;key:$2;sleep:600;dump;quit" "$BIN" "$DEMO" 2>&1 | sed -n 's/^selected commit: //p'; }
MERGE=$(git -C "$DEMO" rev-list --merges -1 main)
[ "$(nav "$MERGE" alt-down)" = "$(git -C "$DEMO" rev-parse "$MERGE^1")" ] || { echo "FAIL: follow branch down"; exit 1; }
[ "$(nav "$MERGE" alt-shift-down)" = "$(git -C "$DEMO" rev-parse "$MERGE^2")" ] || { echo "FAIL: other way down"; exit 1; }
# "feature" is checked out: going up from its tip stays on it (→ working directory row);
# the merge that merged it is the other way.
[ "$(nav "$(git -C "$DEMO" rev-parse "$MERGE^2")" alt-shift-up)" = "$MERGE" ] || { echo "FAIL: other way up"; exit 1; }
[ "$(nav "$(git -C "$DEMO" rev-parse "$MERGE^1")" alt-up)" = "$MERGE" ] || { echo "FAIL: follow branch up"; exit 1; }

# A second repository tab; tabs are not persisted by scripted runs, the second opens and is active.
OTHER_REPO="$OUT/other" && mkdir -p "$OTHER_REPO" && git -C "$OTHER_REPO" init -q -b main
GITTI_SCRIPT="sleep:1500;open:$OTHER_REPO;sleep:1000;shot:$OUT/14-tabs.png;dump;quit" "$BIN" "$DEMO" 2>/dev/null

# "Ignore whitespace" on: hunk/line staging still works and leaves whitespace-only
# changes unstaged.
sqlite3 "$DB" "INSERT OR REPLACE INTO settings (key, value) VALUES ('ignore_whitespace', 'true')"
git -C "$DEMO" stash -q -u 2>/dev/null || true
sed -i.bak 's/^line 7$/line seven/; s/^line 9$/line 9   /' "$DEMO/a.txt" && rm "$DEMO/a.txt.bak"
R=$(row_of Unstaged "line seven")
[ -n "$R" ] || { echo "FAIL: -w view misses the change"; exit 1; }
GITTI_SCRIPT="sleep:1500;lines:$R-$R;stage;sleep:1200;shot:$OUT/15-ignore-ws-staged.png;quit" "$BIN" "$DEMO" 2>/dev/null
STAGED=$(git -C "$DEMO" diff --cached --unified=0 -- a.txt | grep '^[+-][^+-]' | tr '\n' '|' || true)
[ "$STAGED" = "+line seven|" ] || { echo "FAIL: stage line with ignore whitespace: got '$STAGED'"; exit 1; }
git -C "$DEMO" diff -- a.txt | grep -q '^+line 9   $' || { echo "FAIL: whitespace change must stay unstaged"; exit 1; }

# Stash apply with a conflict (no MERGE_HEAD): the file shows up under "Conflicted Files";
# "Resolve Conflicts" is a split button: its ▾ (right end) → Take Theirs resolves the file
# with the stashed version.
CONF="$OUT/conflict" && rm -rf "$CONF" && mkdir -p "$CONF" && git -C "$CONF" init -q -b main
git -C "$CONF" config user.name T && git -C "$CONF" config user.email t@example.com
printf 'fn main() {\n    let x = 1;\n}\n' > "$CONF/main.rs"
git -C "$CONF" add . && git -C "$CONF" commit -qm init
sed -i.bak 's/let x = 1;/let x = 42;/' "$CONF/main.rs" && rm "$CONF/main.rs.bak" && git -C "$CONF" stash -q
sed -i.bak 's/let x = 1;/let x = 2;/' "$CONF/main.rs" && rm "$CONF/main.rs.bak" && git -C "$CONF" commit -qam change
git -C "$CONF" stash apply > /dev/null 2>&1 && { echo "FAIL: expected a stash conflict"; exit 1; }
GITTI_SCRIPT="sleep:1500;select:0;sleep:1200;shot:$OUT/16-conflict.png;clickon:resolve-split:*@-8,-10;sleep:500;shot:$OUT/17-resolve-menu.png;clickrel:-165,96;sleep:1500;quit" "$BIN" "$CONF" 2>/dev/null
[ "$(git -C "$CONF" status --porcelain)" = "M  main.rs" ] || { echo "FAIL: take theirs ($(git -C "$CONF" status --porcelain))"; exit 1; }
grep -q 'let x = 42;' "$CONF/main.rs" || { echo "FAIL: take theirs content"; exit 1; }

# Merge editor: a merge conflict next to a clean one-sided change; take theirs for the
# conflict via its "◀ Take Theirs" button, then "Save & Mark Resolved".
MRG="$OUT/merge" && rm -rf "$MRG" && mkdir -p "$MRG" && git -C "$MRG" init -q -b main
git -C "$MRG" config user.name T && git -C "$MRG" config user.email t@example.com
printf 'top\nsep\nx = 1\nend\n' > "$MRG/f.txt"
git -C "$MRG" add . && git -C "$MRG" commit -qm init && git -C "$MRG" switch -qc feature
printf 'top\nsep\nx = 2\nend\n' > "$MRG/f.txt" && git -C "$MRG" commit -qam feature && git -C "$MRG" switch -q main
printf 'TOP\nsep\nx = 3\nend\n' > "$MRG/f.txt" && git -C "$MRG" commit -qam main
git -C "$MRG" merge feature > /dev/null 2>&1 && { echo "FAIL: expected a merge conflict"; exit 1; }
# Top rows: header "changed in ours" (0), TOP, sep, header "conflict" (3).
GITTI_SCRIPT="sleep:1500;select:0;sleep:800;mergeeditor:0;sleep:1200;win:merge;shot:$OUT/18-merge-editor.png;clickon:merge-take-theirs:3;sleep:400;shot:$OUT/19-merge-resolved.png;clickon:merge-save-resolve;sleep:1500;quit" "$BIN" "$MRG" 2>/dev/null
[ "$(git -C "$MRG" status --porcelain)" = "M  f.txt" ] || { echo "FAIL: merge editor save ($(git -C "$MRG" status --porcelain))"; exit 1; }
[ "$(cat "$MRG/f.txt")" = "$(printf 'TOP\nsep\nx = 2\nend')" ] || { echo "FAIL: merge editor result: $(cat "$MRG/f.txt")"; exit 1; }

echo "OK — screenshots in $OUT"
