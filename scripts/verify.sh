#!/usr/bin/env bash
# Runs scripts/coverage.sh, then scripts/mutants.sh, on the changes since <base>,
# and always ends with one verdict line that starts "verify: PASS" or "verify: FAIL"
# and ends "(HEAD <sha>)", the commit it measured.
# PASS needs no uncovered region touching the changes and no missed mutant; a timeout counts as caught.
# A region you explain instead of covering still fails the verdict, so the
# mutants run anyway: one job reports both. A failing test run stops it early.
# A diff that changes no .rs file passes without running either.
# With a log, all output goes there, opened only once this run holds the worktree's lock:
# a shell redirect would truncate a running verify's log before a second one could refuse.
#   scripts/verify.sh [base] [log]
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
mkdir -p target
exec {lock}>target/verify.lock
if ! flock -n "$lock"; then
	echo "verify: FAIL: another make verify is running in this worktree; wait for its completion notification"
	exit 1
fi
[[ -z ${2:-} ]] || exec >"$2" 2>&1

verdict=
report=
# Any early exit, from set -e included, still prints a verdict line.
trap 'rm -f "$report"; [[ -n $verdict ]] || echo "verify: FAIL: stopped early (see the error above) (HEAD ${head:-unknown})"' EXIT

# The verdict names the commit it measured, so a log is checked by its last line alone.
head=$(git rev-parse HEAD)
git diff --quiet HEAD -- '*.rs' || head+=' with uncommitted changes'
base=${1:-$(git merge-base main HEAD)}
# Coverage and mutants measure Rust code only, so a diff without any has nothing to check.
if git diff --quiet "$base" -- '*.rs'; then
	verdict="verify: PASS: no Rust changes since $base"
	echo "$verdict (HEAD $head)"
	exit 0
fi
report=$(mktemp)

if ! scripts/coverage.sh "$base" | tee "$report"; then
	verdict="verify: FAIL: the coverage run failed; mutants not run"
	echo "$verdict (HEAD $head)"
	exit 1
fi
header='Uncovered regions touching changed code:'
if ! grep -qxF "$header" "$report"; then
	verdict="verify: FAIL: the coverage report has no \"$header\" list"
	echo "$verdict (HEAD $head)"
	exit 1
fi
# The list also goes to a file, so a reader of a long log finds it without a search.
listed=target/verify-uncovered.txt
sed -n "/^$header\$/,/^\$/p" "$report" | sed '1d;/^$/d;/^(none)$/d' >"$listed"
uncovered=$(wc -l <"$listed")

# cargo mutants leaves mutants.out alone when it finds nothing to mutate, so
# drop the last run's counts first.
rm -rf mutants.out
status=0
scripts/mutants.sh "$base" || status=$?
count() { if [[ -f mutants.out/$1.txt ]]; then wc -l <"mutants.out/$1.txt"; else echo 0; fi; }
# A mutant that hangs the tests is caught, but cargo mutants exits 3 for a timeout even
# when another mutant was missed, so only the missed count can pass it.
((status == 3 && $(count missed) == 0)) && status=0
if [[ -d mutants.out ]]; then
	mutants="$(count caught) caught, $(count unviable) unviable, $(count missed) missed, $(count timeout) timeouts"
else
	mutants='none to test'
fi

if ((uncovered == 0 && status == 0)); then
	verdict="verify: PASS: no uncovered region touches the changes; mutants: $mutants"
	echo "$verdict (HEAD $head)"
	exit 0
fi
reasons=
((uncovered == 0)) || reasons="$uncovered uncovered regions touch the changes (listed in $listed): cover or explain each; "
reasons+="mutants: $mutants"
((status == 0)) || reasons+=" (cargo mutants exit $status, see mutants.out/)"
verdict="verify: FAIL: $reasons"
echo "$verdict (HEAD $head)"
exit 1
