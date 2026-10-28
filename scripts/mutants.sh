#!/usr/bin/env bash
# Mutation-tests the changes since <base>: by default where HEAD left main.
# On a stacked branch, pass the stage's base so earlier stages stay out.
# Arguments after <base> go to cargo mutants, for example --list or -j 8.
# Each mutant costs one build and one full test run, so this takes an hour or more.
# It runs under nice -n 19 with 2 mutants at a time unless -j or CARGO_MUTANTS_JOBS says otherwise.
# The last line is a summary that starts "mutation-test: PASS" or "mutation-test: FAIL"
# and ends "(HEAD <sha>)", the commit it measured.
# PASS needs no missed mutant; a timeout counts as caught.
# A diff that changes no .rs file passes without running cargo mutants.
# One run holds the worktree's lock; a second run prints a FAIL line and stops.
# With LOG=<file>, all output goes there, opened only once this run holds the lock:
# a shell redirect would truncate a running log before a second run could refuse.
#   [LOG=<file>] scripts/mutants.sh [base] [cargo-mutants args...]
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
mkdir -p target
exec {worktree_lock}>target/mutation-test.lock
if ! flock -n "$worktree_lock"; then
  echo "mutation-test: FAIL: another make mutation-test is running in this worktree; wait for it to end"
  exit 1
fi
[[ -z ${LOG:-} ]] || exec >"$LOG" 2>&1

verdict=
diff=
run=
head=unknown
# Any early exit, from set -e included, still prints a summary line.
cleanup() {
  rm -f "$diff"
  [[ -z $run ]] || rm -rf "$run" "$run.lock"
  [[ -n $verdict ]] || echo "mutation-test: FAIL: stopped early (see the error above) (HEAD $head)"
}
trap cleanup EXIT

# The summary names the commit it measured, so a log is checked by its last line alone.
head=$(git rev-parse HEAD)
git diff --quiet HEAD -- '*.rs' || head+=' with uncommitted changes'
base=${1:-$(git merge-base main HEAD)}
[[ $# -gt 0 ]] && shift
# Mutation testing changes Rust code only, so a diff without any has nothing to mutate.
if git diff --quiet "$base" -- '*.rs'; then
  verdict="mutation-test: PASS: no Rust changes since $base"
  echo "$verdict (HEAD $head)"
  exit 0
fi

diff=$(mktemp)
# cargo mutants copies the tree into $TMPDIR and removes the copy only when it
# exits normally, so a killed run leaks gigabytes. Each run gets a directory it
# holds a lock on until it exits; a free lock marks a leaked one.
scratch=${TMPDIR:-/tmp}/sisu-mutants
mkdir -p "$scratch"
for stale in "$scratch"/*.lock; do
  [[ -e $stale ]] || continue
  if flock -n "$stale" true; then rm -rf "${stale%.lock}" "$stale"; fi
done
run=$(mktemp -d "$scratch/run.XXXXXX")
exec {held}>"$run.lock"
flock "$held"
git diff "$base" >"$diff"

# cargo mutants leaves mutants.out alone when it finds nothing to mutate, so
# drop the last run's counts first.
rm -rf mutants.out
# .cargo/mutants.toml leaves .llvm/ out of the copied tree, so point the
# build at the original.
status=0
TMPDIR=$run CARGO_MUTANTS_JOBS=${CARGO_MUTANTS_JOBS:-2} \
  LLVM_SYS_231_PREFIX="$PWD/.llvm/23" nice -n 19 cargo mutants --in-diff "$diff" "$@" || status=$?

count() { if [[ -f mutants.out/$1.txt ]]; then wc -l <"mutants.out/$1.txt"; else echo 0; fi; }
missed=$(count missed)
# A mutant that hangs the tests is caught, but cargo mutants exits 3 for a timeout even
# when another mutant was missed, so only the missed count can pass it.
((status == 3 && missed == 0)) && status=0
counts="$(count caught) caught, $(count unviable) unviable, $missed missed, $(count timeout) timeouts"
if ((status == 0 && missed == 0)); then
  verdict="mutation-test: PASS: $counts"
else
  verdict="mutation-test: FAIL: $counts"
  # A missed mutant explains the failure; any other exit status needs its own note.
  ((missed > 0)) || verdict+=" (cargo mutants exit $status, see mutants.out/)"
fi
echo "$verdict (HEAD $head)"
[[ $verdict == "mutation-test: PASS:"* ]]
