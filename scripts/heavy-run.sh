#!/usr/bin/env bash
# Runs a command that loads the whole machine, such as the test suite, coverage or mutation
# tests, while it holds one lock for all the worktrees of this clone. Two such runs at the same
# time link hundreds of programs at once and stop the disk for every other process. A second
# run waits for the first and says which run it waits for.
# Without `flock` (macOS has none), the command runs with no lock.
#   scripts/heavy-run.sh <command> [args...]
set -euo pipefail

if ! command -v flock >/dev/null; then
  exec "$@"
fi
# The common directory is the same for every worktree of the clone, and a sandbox can write it.
lock="$(git rev-parse --path-format=absolute --git-common-dir)/sisu-heavy-run.lock"
exec {fd}>>"$lock"
if ! flock -n "$fd"; then
  echo "heavy-run: waiting for this run to end: $(cat "$lock")" >&2
  flock "$fd"
fi
printf '%s in %s (pid %s)\n' "$*" "$PWD" "$$" >"$lock"
# The command inherits the open lock, so the lock lasts until it ends.
exec "$@"
