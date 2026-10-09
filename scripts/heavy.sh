#!/usr/bin/env bash
# Runs a heavy command (cargo build / clippy / test) under a machine-wide guard, for
# several builds started in parallel (e.g. from several worktrees):
#   - at most GITTI_HEAVY_MAX (default 1; with 2, disk writeback + linking can already push
#     memory pressure over 90 % and make systemd-oomd kill other processes) heavy commands
#     at once, machine-wide (flock semaphore); `--all` takes every slot (the full test
#     suite runs alone);
#   - CARGO_BUILD_JOBS = GITTI_HEAVY_JOBS (default 4) unless already set: linking a test
#     binary takes ~4.4 GB (rust-lld), and cargo links as many in parallel as it has jobs;
#   - Linux: in its own systemd user scope (no memory cap), so `systemd-cgls` / the
#     scope's memory.current show what each run takes. No cap on purpose: a scope sitting
#     at a MemoryMax stalls in reclaim, that stall counts as pressure for the whole user
#     service, and systemd-oomd then kills some *other* process. The limits that help are
#     the job count and the number of runs at once.
#   - a time limit (GITTI_HEAVY_TIMEOUT seconds, default 2700 = 45 min), so a hung test
#     can't hold a slot forever while other runs wait for it.
# Why: several parallel cold builds can create enough memory pressure that systemd-oomd
# kills other processes (up to the whole terminal session); this runs one heavy cargo
# command at a time. Without flock / systemd-run (e.g. macOS) the command just runs.
#
# Usage: scripts/heavy.sh [--all] <command> [args…]
#   e.g. scripts/heavy.sh cargo clippy --workspace --all-targets
#        scripts/heavy.sh --all cargo test --workspace
set -u

all=0
if [ "${1:-}" = "--all" ]; then
    all=1
    shift
fi
[ $# -gt 0 ] || { echo "usage: $0 [--all] <command> [args…]" >&2; exit 2; }

max=${GITTI_HEAVY_MAX:-1}
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-${GITTI_HEAVY_JOBS:-4}}

if command -v flock >/dev/null 2>&1; then
    dir=${XDG_RUNTIME_DIR:-/tmp}/gitti-heavy
    mkdir -p "$dir"
    if [ $all = 1 ]; then
        # Blocking, in order; the others only try non-blocking, so no deadlock.
        for i in $(seq 1 "$max"); do
            exec {fd}>"$dir/$i.lock"
            flock "$fd"
        done
    else
        waited=0
        while :; do
            got=0
            for i in $(seq 1 "$max"); do
                exec {fd}>"$dir/$i.lock"
                if flock -n "$fd"; then got=1; break; fi
                exec {fd}>&-
            done
            [ $got = 1 ] && break
            [ $waited = 0 ] && echo "heavy.sh: waiting for a free build slot ($max busy)…" >&2
            waited=1
            sleep 3
        done
    fi
fi

limit=""
command -v timeout >/dev/null 2>&1 && limit="timeout --kill-after=30 ${GITTI_HEAVY_TIMEOUT:-2700}"

# The lock fds stay with this shell only: children must not inherit them (a lingering
# process would otherwise hold the slot).
close_fds() {
    for f in /proc/$BASHPID/fd/*; do
        n=${f##*/}
        [ "$n" -gt 2 ] 2>/dev/null && eval "exec $n>&-"
    done
}

if command -v systemd-run >/dev/null 2>&1 && systemd-run --user --scope --quiet true 2>/dev/null; then
    ( close_fds; exec systemd-run --user --scope --quiet -- $limit "$@" )
else
    ( close_fds; exec $limit "$@" )
fi
