#!/usr/bin/env bash
# Installs gitti + gitty for the current user: binaries into ~/.local/bin, the icon into the
# hicolor theme and the desktop entry (its name = the windows' app_id, so the shell shows our
# icon for running windows too).
#   scripts/install-linux.sh
set -euo pipefail
cd "$(dirname "$0")/.."
ID=io.github.sneffets.gitti
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
# Icon files from assets/icon/*.svg (rewrites only what changed, so usually no rebuild).
cargo run --release -q -p gitti-app --example make_icons
cargo build --release -p gitti-app -p gitty
install -Dm755 target/release/gitti "$HOME/.local/bin/gitti"
install -Dm755 target/release/gitty "$HOME/.local/bin/gitty"
for png in assets/icon/hicolor/*/apps/$ID.png; do
  size=$(basename "$(dirname "$(dirname "$png")")")
  install -Dm644 "$png" "$DATA/icons/hicolor/$size/apps/$ID.png"
done
install -Dm644 "assets/linux/$ID.desktop" "$DATA/applications/$ID.desktop"
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$DATA/icons/hicolor" || true
command -v update-desktop-database >/dev/null && update-desktop-database -q "$DATA/applications" || true
echo "installed gitti + gitty into ~/.local/bin (make sure it is on PATH)"
