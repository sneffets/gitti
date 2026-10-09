#!/usr/bin/env bash
# Installs only gitty (the terminal UI) for the current user: no GPUI system libraries needed,
# no icon or desktop entry. Works on macOS too.
#   scripts/install-gitty-linux.sh          # build and copy into ~/.local/bin (as install-linux.sh)
#   scripts/install-gitty-linux.sh --cargo  # `cargo install` into ~/.cargo/bin; cargo tracks it
#                                           # (`cargo install --list`, `cargo uninstall gitty`)
# --locked keeps the pins of Cargo.lock (cc 1.2.x for tree-sitter-sequel); `cargo install`
# would otherwise resolve the newest versions.
set -euo pipefail
cd "$(dirname "$0")/.."
case "${1:-}" in
  --cargo)
    cargo install --locked --path crates/gitty
    echo "installed gitty into ${CARGO_INSTALL_ROOT:-${CARGO_HOME:-$HOME/.cargo}}/bin (make sure it is on PATH)"
    ;;
  "")
    cargo build --release --locked -p gitty
    mkdir -p "$HOME/.local/bin"
    install -m755 target/release/gitty "$HOME/.local/bin/gitty"
    echo "installed gitty into ~/.local/bin (make sure it is on PATH)"
    ;;
  *)
    echo "usage: $0 [--cargo]" >&2
    exit 2
    ;;
esac
