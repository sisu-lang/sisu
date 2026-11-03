#!/usr/bin/env bash
# Runs a command that loads the whole machine, such as the test suite, coverage or mutation
# tests. While it runs, it holds one lock for all the worktrees of this clone. Two such runs at
# the same time link hundreds of programs at once. Then the disk stops for every other process.
# A second run says which run it waits for, and waits.
# A script that takes a lock of its own first (scripts/mutants.sh) sources this file and calls
# heavy_lock. When the lock cannot be taken (no flock on macOS, no Git directory, a read-only
# .git), the command runs with no lock and a warning.
#   scripts/heavy-run.sh <command> [args...]

heavy_lock() {
  local dir lock
  if ! command -v flock >/dev/null; then
    echo "heavy-run: no flock; running with no lock" >&2
    return 0
  fi
  # The common directory is the same for every worktree of the clone.
  if ! dir=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null) ||
    ! { : >>"$dir/sisu-heavy-run.lock"; } 2>/dev/null; then
    echo "heavy-run: no lock in the clone's .git; running with no lock" >&2
    return 0
  fi
  lock=$dir/sisu-heavy-run.lock
  exec {heavy_fd}>>"$lock"
  if ! flock -n "$heavy_fd"; then
    echo "heavy-run: waiting for this run to end: $(cat "$lock")" >&2
    flock "$heavy_fd"
  fi
  # The command inherits the open lock, so the lock lasts until it and its children end.
  printf '%s in %s (pid %s)\n' "${*:-$0}" "$PWD" "$$" >"$lock"
}

if [[ ${BASH_SOURCE[0]} == "$0" ]]; then
  set -euo pipefail
  heavy_lock "$@"
  exec "$@"
fi
