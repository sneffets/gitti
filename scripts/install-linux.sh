#!/usr/bin/env bash
# Installs gitti + gitty for the current user: binaries into ~/.local/bin, the icon into the
# hicolor theme and the desktop entry (its name = the windows' app_id, so the shell shows our
# icon for running windows too).
#   scripts/install-linux.sh
# Below 12 GB of RAM it builds without LTO (profile `release-lowmem`, scripts/build-profile.sh).
set -euo pipefail
cd "$(dirname "$0")/.."
. scripts/steps.sh
. scripts/build-profile.sh
ID=io.github.sneffets.gitti
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
step "icons: checking assets/icon/*.svg against sources.hash (renders only if they changed)"
cargo run -p xtask -- icons
step "building gitti + gitty (cargo profile $PROFILE)"
build_note
cargo build --profile "$PROFILE" -p gitti-app -p gitty
step "installing into ~/.local/bin, icons, desktop entry, licences"
install -Dm755 "target/$PROFILE/gitti" "$HOME/.local/bin/gitti"
install -Dm755 "target/$PROFILE/gitty" "$HOME/.local/bin/gitty"
for png in assets/icon/hicolor/*/apps/$ID.png; do
  size=$(basename "$(dirname "$(dirname "$png")")")
  install -Dm644 "$png" "$DATA/icons/hicolor/$size/apps/$ID.png"
done
install -Dm644 "assets/linux/$ID.desktop" "$DATA/applications/$ID.desktop"
# Licences shipped with the binaries: our own, and every third-party component's.
for f in COPYING LICENSE-MIT LICENSE-APACHE licenses/THIRD-PARTY-LICENSES.txt licenses/LUCIDE.txt; do
  install -Dm644 "$f" "$DATA/doc/gitti/$(basename "$f")"
done
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$DATA/icons/hicolor" || true
command -v update-desktop-database >/dev/null && update-desktop-database -q "$DATA/applications" || true
done_steps "installed gitti + gitty into ~/.local/bin (make sure it is on PATH), licences into $DATA/doc/gitti"
