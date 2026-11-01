#!/usr/bin/env bash
# Mutation-tests the changes since <base>: by default where HEAD left main.
# On a stacked branch, pass the stage's base so earlier stages stay out.
# Arguments after <base> go to cargo mutants, for example --list or -j 8.
# The summary reads ./mutants.out. The script passes --output ., which beats the
# CARGO_MUTANTS_OUTPUT variable and a config file, and it refuses an argument that moves
# the output or the tree (-o, --output, -d, --dir, --manifest-path).
# An argument that tests no mutant (--list, --list-files, --check and the like) ends with
# "mutation-test: NOT RUN", never PASS.
# Two passes keep it short. Pass 1 tests each mutant with the unit tests only (`--lib`,
# about 6 s against 47 s for the whole suite). Pass 2 runs only when pass 1 missed a mutant:
# it tests the mutants that pass 1 did not catch (--iterate) with the whole suite, so a
# mutant that only an end-to-end test kills is caught, not missed. The summary adds the counts.
# Arguments after -- choose the tests yourself: the script then runs one pass with them.
# Each mutant still costs one build, so a large diff takes an hour or more.
# It runs under nice -n 19 with 2 mutants at a time unless -j or CARGO_MUTANTS_JOBS says otherwise.
# The last line is a summary that starts "mutation-test: PASS" or "mutation-test: FAIL".
# A summary of a run that measured a commit ends "(HEAD <sha>)"; NOT RUN lines and the
# lines of a refused argument or of a refused tree do not.
# It refuses uncommitted Rust changes, because a review takes the log of a commit only.
# PASS needs no missed mutant; a timeout counts as caught.
# A run that tests no mutant, for example when the diff holds none, passes with "no mutant tested".
# A diff that changes no .rs file passes without running cargo mutants.
# More than 300 mutants (MUTANTS_MAX sets another limit) fails at once, before a pass starts.
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
base=${1:-$(git merge-base main HEAD)}
[[ $# -gt 0 ]] && shift
not_run=
test_args=
iterate=
for arg in "$@"; do
  case $arg in
    --)
      test_args=yes
      break
      ;;
    --iterate) iterate=yes ;;
    -o* | --output | --output=* | -d* | --dir | --dir=* | --manifest-path | --manifest-path=*)
      verdict="mutation-test: FAIL: $arg moves mutants.out or the tree; the summary needs the default"
      echo "$verdict"
      exit 1
      ;;
    --list | --list-files | --check | --version | --help | -h | --completions* | --emit-schema* | --Zmutate-file*)
      not_run=$arg
      ;;
  esac
done
# Refused before cargo starts: an hour of mutants on uncommitted code gives a log no review takes.
if [[ -z $not_run ]] && ! git diff --quiet HEAD -- '*.rs'; then
  verdict="mutation-test: FAIL: uncommitted Rust changes; commit them, then run again"
  echo "$verdict"
  exit 1
fi
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
# A large diff stops here, in seconds, not after an hour: 657 mutants took 1.5 hours in
# M8 part 8, mostly in pass 2. The caller then tests a part, or raises the limit.
max=${MUTANTS_MAX:-300}
if [[ -z $not_run && -z $test_args ]]; then
  total=$(cargo mutants --in-diff "$diff" --list "$@" 2>/dev/null | wc -l || true)
  if ((total > max)); then
    verdict="mutation-test: FAIL: $total mutants, more than $max; test a part with --file, or set MUTANTS_MAX=$total"
    echo "$verdict"
    exit 1
  fi
fi

# cargo mutants rotates the last mutants.out to mutants.out.old and writes a new one only
# when it tests mutants, so a pass counts only if lock.json is newer than this mark.
started=target/mutation-test.start
# One pass of cargo mutants over the diff with these extra arguments; sets status and tested.
# .cargo/mutants.toml leaves .llvm/ out of the copied tree, so point the
# build at the original.
pass() {
  touch "$started"
  status=0
  TMPDIR=$run CARGO_MUTANTS_JOBS=${CARGO_MUTANTS_JOBS:-2} \
    LLVM_SYS_231_PREFIX="$PWD/.llvm/23" nice -n 19 cargo mutants --in-diff "$diff" --output . "$@" || status=$?
  tested=
  [[ mutants.out/lock.json -nt $started ]] && tested=yes
  return 0
}
count() { if [[ -f mutants.out/$1.txt ]]; then wc -l <"mutants.out/$1.txt"; else echo 0; fi; }

if [[ -n $not_run || -n $test_args ]]; then
  pass "$@"
else
  pass "$@" -- --lib
fi
if [[ -n $not_run ]]; then
  verdict="mutation-test: NOT RUN: $not_run tests no mutant"
  echo "$verdict"
  exit "$status"
fi
caught=0
unviable=0
# Pass 2: exit 2 is a missed mutant and 3 a timeout; any other failure stops the run here.
if [[ -z $test_args && -n $tested ]] && ((status == 2 || status == 3)) && (($(count missed) > 0)); then
  caught=$(count caught)
  unviable=$(count unviable)
  echo "mutation-test: pass 1 (unit tests) missed $(count missed); pass 2 tests them with the whole suite"
  if [[ -n $iterate ]]; then pass "$@"; else pass --iterate "$@"; fi
fi
missed=0
if [[ -n $tested ]]; then
  missed=$(count missed)
  # A mutant that hangs the tests is caught, but cargo mutants exits 3 for a timeout even
  # when another mutant was missed, so only the missed count can pass it.
  ((status == 3 && missed == 0)) && status=0
  counts="$((caught + $(count caught))) caught, $((unviable + $(count unviable))) unviable, $missed missed, $(count timeout) timeouts"
else
  # No new output: the diff holds no mutant, or cargo mutants stopped before the first test.
  counts="no mutant tested"
fi
if ((status == 0 && missed == 0)); then
  verdict="mutation-test: PASS: $counts"
else
  verdict="mutation-test: FAIL: $counts"
  # A missed mutant explains the failure; any other exit status needs its own note.
  ((missed > 0)) || verdict+=" (cargo mutants exit $status, see the output above)"
fi
echo "$verdict (HEAD $head)"
[[ $verdict == "mutation-test: PASS:"* ]]
