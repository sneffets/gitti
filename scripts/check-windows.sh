#!/usr/bin/env bash
# Type-checks the whole workspace for Windows from macOS/Linux (`cargo check` only: C code of
# build scripts is "compiled" by stub tools that write empty outputs, nothing is linked).
#   scripts/check-windows.sh
set -euo pipefail
cd "$(dirname "$0")/.."
T=x86_64-pc-windows-msvc
rustup target list --installed | grep -qx "$T" || rustup target add "$T"
STUBS="$PWD/scripts/fake-msvc"
export PATH="$STUBS:$PATH"
export CC_x86_64_pc_windows_msvc="$STUBS/cc.sh" CXX_x86_64_pc_windows_msvc="$STUBS/cc.sh" \
  AR_x86_64_pc_windows_msvc="$STUBS/cc.sh"
cargo check --workspace --target "$T" "$@"
