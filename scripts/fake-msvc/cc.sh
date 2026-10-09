#!/bin/sh
# Fake C compiler/archiver for `cargo check` cross checks: creates empty outputs.
for a in "$@"; do
  case "$a" in
    -Fo*) : > "${a#-Fo}" ;;
    /Fo*) : > "${a#/Fo}" ;;
    -out:*) : > "${a#-out:}" ;;
    /OUT:*) : > "${a#/OUT:}" ;;
  esac
done
prev=""
for a in "$@"; do
  if [ "$prev" = "-o" ]; then : > "$a"; fi
  prev="$a"
done
exit 0
