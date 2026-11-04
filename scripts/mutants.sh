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
# about 6 s against 47 s for the whole suite). Pass 2 runs only when pass 1 missed a mutant
# or timed out. It tests the mutants that pass 1 did not catch in two groups, chosen by the file
# in the name of each mutant. A mutant in a LIB_ONLY file is tested with the unit tests again,
# with the pass 2 wait: no integration test reaches that code, and the whole suite would cost
# about 3 to 4 minutes for each mutant and give the same result. Every other mutant is tested
# with the whole suite, so a mutant that only an end-to-end test kills is caught, not missed.
# Each group selects its mutants by name with one --re for each mutant, not with --iterate, so
# that the groups do not read each other's results. A --re, -F, --regex, --examine-re,
# --examine-regex or --iterate of the caller is not passed to pass 2: it would widen the set.
# The summary adds the counts of all the passes.
# Arguments after -- choose the tests yourself: the script then runs one pass with them.
# Each mutant still costs one build, so a large diff takes an hour or more.
# Every pass passes --caught, so that the log lists each caught mutant, not only the misses.
# It runs under nice -n 19 with 2 mutants at a time unless -j or CARGO_MUTANTS_JOBS says otherwise.
# Each mutant runs 8 tests at a time unless NEXTEST_TEST_THREADS says otherwise.
# Two whole suites at one test for each core link hundreds of programs at once. Then the disk stops.
# The last line is a summary that starts "mutation-test: PASS" or "mutation-test: FAIL".
# A summary of a run that measured a commit ends "(HEAD <sha>)"; NOT RUN lines and the
# lines of a refused argument or of a refused tree do not.
# It refuses uncommitted Rust changes, because a review takes the log of a commit only.
# PASS needs no missed mutant; a timeout counts as caught.
# Pass 2 waits 600 s for each mutant. MUTANTS_PASS2_TIMEOUT sets another number.
# A --timeout, -t or --timeout-multiplier of the caller replaces it.
# Without it, cargo mutants takes the wait from its baseline run, about 2 x 41 s in one run.
# With 2 mutants at a time, the whole suite is slower than that.
# A mutant that every test passes would then time out and count as caught.
# After 600 s, a timeout of pass 2 is a hang. Pass 1 keeps the wait from the baseline.
# Its tests are fast.
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
# Then the lock of the whole clone (scripts/heavy-run.sh). A second run in this worktree fails
# at once above, and does not wait here.
# shellcheck source=scripts/heavy-run.sh
source scripts/heavy-run.sh
heavy_lock make mutation-test
[[ -z ${LOG:-} ]] || exec >"$LOG" 2>&1

# The files whose code only the library's unit tests reach: no integration test, no binary and
# no module outside this list reaches it. scripts/test_mutants.py checks that this list matches
# the facts. The paths are prefixes of the file in the name of a mutant.
# A wrong entry never hides a survivor: a mutant of a listed file that only an integration test
# would kill shows as missed, not as caught. Then the entry is wrong, and the fix is to delete it.
LIB_ONLY=(
  crates/sisuc/src/fuzz.rs
  crates/sisuc/src/fuzz_program.rs
  crates/sisuc/src/generate/
)

verdict=
diff=
lists=
run=
head=unknown
# Any early exit, from set -e included, still prints a summary line.
cleanup() {
  rm -f "$diff"
  [[ -z $lists ]] || rm -rf "$lists"
  [[ -z $run ]] || rm -rf "$run" "$run.lock"
  [[ -n $verdict ]] || echo "mutation-test: FAIL: stopped early (see the error above) (HEAD $head)"
}
trap cleanup EXIT

# The summary names the commit it measured, so a log is checked by its last line alone.
head=$(git rev-parse HEAD)
# origin/main, not main: a local main falls behind, and then the range takes in merged work.
base=${1:-$(git merge-base origin/main HEAD)}
[[ $# -gt 0 ]] && shift
not_run=
test_args=
caller_timeout=
for arg in "$@"; do
  case $arg in
    --)
      test_args=yes
      break
      ;;
    -t* | --timeout | --timeout=* | --timeout-multiplier | --timeout-multiplier=*) caller_timeout=yes ;;
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
  TMPDIR=$run CARGO_MUTANTS_JOBS=${CARGO_MUTANTS_JOBS:-2} NEXTEST_TEST_THREADS=${NEXTEST_TEST_THREADS:-8} \
    LLVM_SYS_231_PREFIX="$PWD/.llvm/23" nice -n 19 cargo mutants --in-diff "$diff" --output . --caught "$@" || status=$?
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
missed=0
timeouts=0
if [[ -n $tested ]]; then
  caught=$(count caught)
  unviable=$(count unviable)
  missed=$(count missed)
  timeouts=$(count timeout)
fi

# Pass 2 tests the group of the names in file $2 with the tests in $3 (the label is $1), and adds
# the counts. The remaining arguments go last to cargo mutants. A group with no mutant does not run.
run_group() {
  local label=$1 names=$2 tests=$3 n re
  shift 3
  n=$(wc -l <"$names")
  ((n > 0)) || return 0
  echo "mutation-test: pass 2, $label group: $n mutants, tests: $tests"
  local args=("${pass2_args[@]}")
  # A name holds regex characters, so each one is escaped and anchored.
  while IFS= read -r re; do args+=(--re "$re"); done < <(sed 's/[][\\.+*?()|{}^$]/\\&/g; s/^/^/; s/$/$/' "$names")
  pass "${args[@]}" "$@"
  if [[ -n $tested ]]; then
    caught=$((caught + $(count caught)))
    unviable=$((unviable + $(count unviable)))
    missed=$((missed + $(count missed)))
    timeouts=$((timeouts + $(count timeout)))
  fi
  # A missed mutant (2) or a timeout (3) is a result; any other code is a failure, which wins.
  case $status in
    0) ;;
    2 | 3) if [[ $group_status == 0 ]]; then group_status=$status; fi ;;
    *) group_status=$status ;;
  esac
  return 0
}

# Pass 2: exit 2 is a missed mutant and 3 a timeout; any other failure stops the run here.
if [[ -z $test_args && -n $tested ]] && ((status == 2 || status == 3)) && ((missed + timeouts > 0)); then
  echo "mutation-test: pass 1 (unit tests) missed $missed and timed out on $timeouts; pass 2 tests them in two groups"
  # Each pass rotates mutants.out, so the names of pass 1 are saved first.
  lists=$(mktemp -d)
  survivors=$lists/survivors
  lib_names=$lists/lib
  suite_names=$lists/suite
  for kind in missed timeout; do
    if [[ -f mutants.out/$kind.txt ]]; then cat "mutants.out/$kind.txt"; fi
  done | sort -u >"$survivors"
  touch "$lib_names" "$suite_names"
  while IFS= read -r name; do
    group=$suite_names
    for prefix in "${LIB_ONLY[@]}"; do [[ $name == "$prefix"* ]] && group=$lib_names; done
    printf '%s\n' "$name" >>"$group"
  done <"$survivors"
  # An explicit --timeout beats the baseline wait; cargo mutants reads a new baseline in each run.
  pass2_args=()
  [[ -n $caller_timeout ]] || pass2_args=(--timeout "${MUTANTS_PASS2_TIMEOUT:-600}")
  # Only the script selects mutants in pass 2.
  skip=
  for arg in "$@"; do
    if [[ -n $skip ]]; then
      skip=
      continue
    fi
    case $arg in
      --re | -F | --regex | --examine-re | --examine-regex) skip=yes ;;
      --re=* | -F* | --regex=* | --examine-re=* | --examine-regex=* | --iterate) ;;
      *) pass2_args+=("$arg") ;;
    esac
  done
  missed=0
  timeouts=0
  group_status=0
  run_group lib-only "$lib_names" "the unit tests (--lib)" -- --lib
  run_group whole-suite "$suite_names" "the whole suite"
  status=$group_status
fi
# A mutant that hangs the tests is caught, but cargo mutants exits 3 for a timeout even
# when another mutant was missed, so only the missed count can pass it.
((status == 3 && missed == 0)) && status=0
if [[ -n $tested ]]; then
  counts="$caught caught, $unviable unviable, $missed missed, $timeouts timeouts"
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
