# Sourced by the install scripts: sets PROFILE, the cargo profile of the build. Fat LTO
# (`release`) needs more than 6 GB in one rustc for gitti, so below 12 GB of RAM the lean
# `release-lowmem` (Cargo.toml) is used. GITTI_BUILD_PROFILE=release|release-lowmem overrides.
# Without /proc/meminfo (macOS) it stays `release`.
PROFILE="${GITTI_BUILD_PROFILE:-}"
if [ -z "$PROFILE" ]; then
  PROFILE=release
  mem_kb=$(awk '/^MemTotal:/ {print $2}' /proc/meminfo 2>/dev/null || true)
  if [ -n "$mem_kb" ] && [ "$mem_kb" -lt $((12 * 1024 * 1024)) ]; then
    PROFILE=release-lowmem
    echo "$((mem_kb / 1024)) MB RAM: building with profile release-lowmem (no LTO, needs less memory; GITTI_BUILD_PROFILE=release overrides)"
  fi
fi
case "$PROFILE" in
  release | release-lowmem) ;;
  *) echo "GITTI_BUILD_PROFILE must be release or release-lowmem, not '$PROFILE'" >&2; exit 2 ;;
esac
