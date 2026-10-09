#!/bin/sh
# Regenerates the README screenshots in docs/images from a fixed state of a public example
# repository (VectorCraft, MIT OR Apache-2.0). macOS only: gitti's `shot:` script step needs
# it. Needs git, cargo, python3, sips and cwebp (`brew install webp`).
#
#   scripts/readme-shots/shoot.sh            # everything
#   scripts/readme-shots/shoot.sh gitty      # only the terminal shots (gitti | gitty | setup)
#
# The demo lives in $GITTI_SHOTS_DIR (default /tmp/gitti-demo): its path shows in the
# screenshots, so keep it neutral (no user name). gitti and gitty run with their own git
# identity (Jane Doe) and settings there, never yours.
set -eu

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
HERE=$ROOT/scripts/readme-shots
DEMO=${GITTI_SHOTS_DIR:-/tmp/gitti-demo}
REPO=$DEMO/vectorcraft
OUT=$DEMO/out
IMG=$ROOT/docs/images
URL=https://github.com/storytold/vectorcraft.git
REV=c2711df30bca1ab8efe0dc37712afae94e55776a
WHAT=${1:-all}

for tool in git cargo python3 sips cwebp; do
    command -v $tool >/dev/null || { echo "shoot.sh: $tool is missing" >&2; exit 1; }
done

export GIT_CONFIG_GLOBAL=$DEMO/gitconfig GIT_CONFIG_NOSYSTEM=1
export GITTI_CONFIG_DIR=$DEMO/config EDITOR=true VISUAL=true

# The example repository at $REV: refs newer than it removed, remote branches thinned to the
# ones labelled near the top of the graph, every author / committer e-mail that isn't a
# noreply address replaced by one (the screenshots show the commit header), three worktrees
# (one sparse, one locked) and an uncommitted edit to stage lines from.
setup() {
    rm -rf "$DEMO"
    mkdir -p "$DEMO/worktrees" "$DEMO/config" "$OUT"
    printf '[user]\n\tname = Jane Doe\n\temail = jane@example.com\n' > "$DEMO/gitconfig"
    git clone --quiet "$URL" "$REPO"
    cd "$REPO"
    git checkout --quiet -B main "$REV"
    git update-ref refs/remotes/origin/main main
    top=$(git rev-list --date-order -n 45 main)
    for r in $(git for-each-ref --format='%(refname)' refs/remotes/origin refs/tags); do
        case $r in */HEAD|*/origin/main) continue ;; esac
        t=$(git rev-parse "$r^{commit}")
        git merge-base --is-ancestor "$t" main || { git update-ref -d "$r"; continue; }
        case $r in refs/remotes/*) echo "$top" | grep -q "$t" || git update-ref -d "$r" ;; esac
    done
    FILTER_BRANCH_SQUELCH_WARNING=1 git filter-branch -f --env-filter '
fix() { case "$1" in *@users.noreply.github.com|noreply@*) echo "$1";; *) s=$(printf "%s" "$2" | tr "[:upper:]" "[:lower:]" | tr -c "a-z0-9\n" "-" | sed "s/--*/-/g; s/^-//; s/-$//"); echo "${s:-user}@users.noreply.github.com";; esac; }
GIT_AUTHOR_EMAIL=$(fix "$GIT_AUTHOR_EMAIL" "$GIT_AUTHOR_NAME"); GIT_COMMITTER_EMAIL=$(fix "$GIT_COMMITTER_EMAIL" "$GIT_COMMITTER_NAME"); export GIT_AUTHOR_EMAIL GIT_COMMITTER_EMAIL' \
        --tag-name-filter cat -- --all >/dev/null 2>&1
    git for-each-ref --format='%(refname)' refs/original | xargs -n1 git update-ref -d
    git reflog expire --expire=now --all && git gc --quiet --prune=now
    git branch --quiet --set-upstream-to=origin/main main
    git worktree add --quiet -b i18n/french "$DEMO/worktrees/i18n-french" main~3
    git worktree add --quiet -b fix/pen-tool "$DEMO/worktrees/pen-tool" main~20
    git worktree lock --reason "on the external drive" "$DEMO/worktrees/pen-tool"
    git worktree add --quiet --no-checkout -b docs/roadmap "$DEMO/worktrees/docs" main
    git -C "$DEMO/worktrees/docs" sparse-checkout set --cone docs
    git -C "$DEMO/worktrees/docs" checkout --quiet
    python3 - <<'EOF'
import pathlib
p = pathlib.Path("crates/pdf/src/presets.rs"); s = p.read_text()
s = s.replace("PDF 1.6, images kept", "PDF 1.7, images kept")
s = s.replace("Compatibility::Pdf16,\n        compression: compression(images(300.0, 450.0, codec, JpegQuality::Maximum)",
              "Compatibility::Pdf17,\n        compression: compression(images(300.0, 450.0, codec, JpegQuality::High)")
s = s.replace("/// A PDF/X preset:", """/// Settings for the screen: PDF 1.5, images resampled to 150 ppi, no bleed.
fn screen() -> PdfSettings {
    PdfSettings {
        compatibility: Compatibility::Pdf15,
        compression: compression(images(150.0, 225.0, ImageCodec::Jpeg, JpegQuality::Medium), 300.0),
        ..Default::default()
    }
}

/// A PDF/X preset:""")
p.write_text(s)
EOF
}

# A commit with four children (for the "Which child?" chooser), and two of them — siblings,
# so no single commit's diff shows their difference — for compare mode.
pick_commits() {
    cd "$REPO"
    CHOOSER=$(git rev-list --all --children -n 400 | awk 'NF >= 5 { print $1; exit }')
    BASE=$(git log --format=%H --grep='^Fix #733: PDF export keeps type as text' -n 1 main)
    OTHER=$(git log --format=%H --grep='^PDF import: slanted type keeps its slant' -n 1 main)
}

gitti_shot() { # NAME STEPS — runs gitti on the demo, saves docs/images/NAME.webp
    cd "$REPO"
    GITTI_SCRIPT="sleep:2000;$2;sleep:800;shot:$OUT/$1.png;quit" "$ROOT/target/debug/gitti" . \
        > "$OUT/$1.log" 2>&1 || { cat "$OUT/$1.log" >&2; exit 1; }
    sips -Z 2000 "$OUT/$1.png" --out "$OUT/$1-small.png" >/dev/null
    cwebp -quiet -lossless -z 9 "$OUT/$1-small.png" -o "$IMG/$1.webp"
    echo "docs/images/$1.webp"
}

gitty_shot() { # NAME STEPS — runs gitty headless (200x50) on the demo, saves docs/images/NAME.svg
    cd "$REPO"
    cells=""; y=0
    while [ $y -lt 50 ]; do cells="$cells;cells:$y"; y=$((y + 1)); done
    GITTY_SIZE=200x50 GITTY_SCRIPT_LIMIT=60 GITTY_SCRIPT="wait;sleep:1000;$2$cells" \
        "$ROOT/target/debug/gitty" . > "$OUT/$1.cells"
    python3 "$HERE/term2svg.py" "$OUT/$1.cells" "$IMG/$1.svg" "gitty — vectorcraft"
    echo "docs/images/$1.svg"
}

repeat() { # KEY N — "press:KEY;" N times
    i=0; while [ $i -lt "$2" ]; do printf 'press:%s;' "$1"; i=$((i + 1)); done
}

[ "$WHAT" = gitti ] || [ "$WHAT" = gitty ] || setup
[ "$WHAT" = setup ] && exit 0
[ -d "$REPO" ] || { echo "shoot.sh: no demo in $DEMO — run it without arguments first" >&2; exit 1; }
(cd "$ROOT" && cargo build --quiet -p gitti-app --features screenshot && cargo build --quiet -p gitty)
mkdir -p "$IMG" "$OUT"
pick_commits

if [ "$WHAT" != gitty ]; then
    git -C "$REPO" reset --quiet
    # Leaves the screen() function staged: the later shots (and gitty's) show that state.
    gitti_shot gitti-staging "lines:21-30;stage;sleep:1000;lines:12-15"
    gitti_shot gitti-search "changes:marquee pasteboard;sleep:7000;changehit:0;sleep:1500"
    gitti_shot gitti-chooser "select:$CHOOSER;sleep:800;focuslist;key:alt-shift-up"
    gitti_shot gitti-compare "select:$BASE;sleep:600;action:gitti::MarkCompare;sleep:600;select:$OTHER;sleep:2000"
    gitti_shot gitti-worktrees "worktree:open,i18n-french;sleep:2000;worktree:open,docs;sleep:2000;tab:0;sleep:1500;rclickon:side:wt-docs"
    gitti_shot gitti-add-worktree "key:cmd-alt-w;sleep:1000;field:0=feat/screen-preset"
    # Last: the layout is a saved setting (shared with gitty) — reset the demo's settings after.
    gitti_shot gitti-two-line "palette:Two-Line Commit List;sleep:1200"
    rm -rf "$DEMO/config" && mkdir -p "$DEMO/config"
fi
if [ "$WHAT" != gitti ]; then
    # Rows in gitty's commit list (0 = the working directory): 15 = Fix #733, 16 = the
    # commit with four children, 10 = PDF import.
    gitty_shot gitty-main "sleep:500;wait"
    gitty_shot gitty-search "press:/;type:marquee pasteboard;sleep:6000;wait;key:enter;sleep:1500;wait"
    gitty_shot gitty-chooser "$(repeat j 16)wait;press:alt-shift-up;sleep:800"
    gitty_shot gitty-compare "$(repeat j 15)wait;press:C;wait;$(repeat k 5)sleep:1500;wait"
fi
