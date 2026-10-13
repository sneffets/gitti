# Sourced by the Linux install scripts: `check_deps PACKAGE...` checks the build
# requirements before the long build starts. On Debian / Ubuntu (and derivatives with dpkg)
# it checks the given apt packages and, if some are missing, prints the `sudo apt install`
# command for exactly those and stops. Elsewhere it names the Debian packages so the
# equivalents can be found, and goes on. `PACKAGE/COMMAND` also counts as present when the
# command exists (pkg-config may come as pkgconf, a compiler without build-essential).
# cargo and git are checked everywhere. GITTI_SKIP_DEPS=1 skips the package check (e.g.
# libraries built by hand).
check_deps() {
  local missing_cmd=0
  if ! command -v cargo >/dev/null 2>&1; then
    echo "cargo not found: install Rust with rustup (https://rustup.rs), then run this script again." >&2
    echo "  (Distribution packages of Rust are often too old; edition 2024 needs Rust 1.85 or newer.)" >&2
    missing_cmd=1
  fi
  local pkgs=("$@")
  # git is needed at run time, not for the build: both apps call the git CLI.
  command -v git >/dev/null 2>&1 || pkgs+=(git)
  if [ "$(uname -s)" != Linux ]; then
    # install-gitty-linux.sh works on macOS too: its compiler comes with the Xcode tools.
    if ! command -v git >/dev/null 2>&1; then
      echo "git not found: gitty runs the git CLI, install git (macOS: xcode-select --install)." >&2
      missing_cmd=1
    fi
    [ "$missing_cmd" = 0 ] || exit 1
    return 0
  fi
  if [ -n "${GITTI_SKIP_DEPS:-}" ]; then
    [ "$missing_cmd" = 0 ] || exit 1
    return 0
  fi
  if ! command -v dpkg-query >/dev/null 2>&1 || ! is_debian_like; then
    echo "    Not Debian / Ubuntu: the build needs these packages (Debian names), install the"
    echo "    equivalents of your distribution if the build fails:"
    echo "      ${pkgs[*]%%/*}"
    [ "$missing_cmd" = 0 ] || exit 1
    return 0
  fi
  local missing=() names=() p cmd
  for p in "${pkgs[@]}"; do
    cmd=""
    case "$p" in */*) cmd=${p#*/}; p=${p%%/*} ;; esac
    names+=("$p")
    [ -n "$cmd" ] && command -v "$cmd" >/dev/null 2>&1 && continue
    # One line per installed architecture (multiarch): any of them counts.
    dpkg-query -W -f='${db:Status-Status}\n' "$p" 2>/dev/null | grep -qx installed || missing+=("$p")
  done
  if [ ${#missing[@]} -gt 0 ]; then
    echo "Missing packages: ${missing[*]}" >&2
    echo "Install them with:" >&2
    echo >&2
    echo "  sudo apt install ${missing[*]}" >&2
    echo >&2
    echo "then run this script again (GITTI_SKIP_DEPS=1 skips this check)." >&2
    exit 1
  fi
  echo "    all required packages are installed: ${names[*]}"
  [ "$missing_cmd" = 0 ] || exit 1
}

# Debian, Ubuntu and their derivatives (Mint, Pop!_OS, Raspberry Pi OS, …) by /etc/os-release.
is_debian_like() {
  local id="" id_like=""
  [ -r /etc/os-release ] || return 1
  id=$(. /etc/os-release && echo "${ID:-}")
  id_like=$(. /etc/os-release && echo "${ID_LIKE:-}")
  case " $id $id_like " in
    *" debian "* | *" ubuntu "*) return 0 ;;
  esac
  return 1
}
