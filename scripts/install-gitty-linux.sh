#!/usr/bin/env bash
# Installs only gitty (the terminal UI) for the current user: no GPUI system libraries needed,
# no icon or desktop entry. Works on macOS too.
#   scripts/install-gitty-linux.sh          # build and copy into ~/.local/bin (as install-linux.sh)
#   scripts/install-gitty-linux.sh --cargo  # `cargo install` into ~/.cargo/bin; cargo tracks it
#                                           # (`cargo install --list`, `cargo uninstall gitty`)
# --locked keeps the pins of Cargo.lock (cc 1.2.x for tree-sitter-sequel); `cargo install`
# would otherwise resolve the newest versions.
# Below 12 GB of RAM it builds without LTO (profile `release-lowmem`, scripts/build-profile.sh).
set -euo pipefail
cd "$(dirname "$0")/.."
. scripts/steps.sh
. scripts/build-profile.sh
case "${1:-}" in
  --cargo)
    step "cargo install gitty (cargo profile $PROFILE)"
    build_note
    cargo install --locked --profile "$PROFILE" --path crates/gitty
    done_steps "installed gitty into ${CARGO_INSTALL_ROOT:-${CARGO_HOME:-$HOME/.cargo}}/bin (make sure it is on PATH)"
    ;;
  "")
    step "building gitty (cargo profile $PROFILE)"
    build_note
    cargo build --profile "$PROFILE" --locked -p gitty
    step "installing into ~/.local/bin, licences"
    mkdir -p "$HOME/.local/bin"
    install -m755 "target/$PROFILE/gitty" "$HOME/.local/bin/gitty"
    # Licences shipped with the binary (gitty --licenses prints its third-party part too).
    DOC="${XDG_DATA_HOME:-$HOME/.local/share}/doc/gitty"
    for f in COPYING LICENSE-MIT LICENSE-APACHE licenses/THIRD-PARTY-LICENSES.txt; do
      install -Dm644 "$f" "$DOC/$(basename "$f")"
    done
    done_steps "installed gitty into ~/.local/bin (make sure it is on PATH), licences into $DOC"
    ;;
  *)
    echo "usage: $0 [--cargo]" >&2
    exit 2
    ;;
esac
