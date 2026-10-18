#!/usr/bin/env bash
# Runs scripts/coverage.sh, then scripts/mutants.sh, on the changes since <base>,
# and always ends with one verdict line that starts "verify: PASS" or "verify: FAIL".
# PASS needs no uncovered region touching the changes and no surviving mutant.
# A region you explain instead of covering still fails the verdict, so the
# mutants run anyway: one job reports both. A failing test run stops it early.
# A diff that changes no .rs file passes without running either.
#   scripts/verify.sh [base]
set -euo pipefail

verdict=
report=
# Any early exit, from set -e included, still prints a verdict line.
trap 'rm -f "$report"; [[ -n $verdict ]] || echo "verify: FAIL: stopped early (see the error above)"' EXIT

cd "$(git rev-parse --show-toplevel)"
base=${1:-$(git merge-base main HEAD)}
# Coverage and mutants measure Rust code only, so a diff without any has nothing to check.
if git diff --quiet "$base" -- '*.rs'; then
	verdict="verify: PASS: no Rust changes since $base"
	echo "$verdict"
	exit 0
fi
report=$(mktemp)

if ! scripts/coverage.sh "$base" | tee "$report"; then
	verdict="verify: FAIL: the coverage run failed; mutants not run"
	echo "$verdict"
	exit 1
fi
header='Uncovered regions touching changed code:'
if ! grep -qxF "$header" "$report"; then
	verdict="verify: FAIL: the coverage report has no \"$header\" list"
	echo "$verdict"
	exit 1
fi
uncovered=$(sed -n "/^$header\$/,/^\$/p" "$report" | sed '1d;/^$/d;/^(none)$/d' | wc -l)

# cargo mutants leaves mutants.out alone when it finds nothing to mutate, so
# drop the last run's counts first.
rm -rf mutants.out
status=0
scripts/mutants.sh "$base" || status=$?
count() { if [[ -f mutants.out/$1.txt ]]; then wc -l <"mutants.out/$1.txt"; else echo 0; fi; }
if [[ -d mutants.out ]]; then
	mutants="$(count caught) caught, $(count unviable) unviable, $(count missed) missed, $(count timeout) timeouts"
else
	mutants='none to test'
fi

if ((uncovered == 0 && status == 0)); then
	verdict="verify: PASS: no uncovered region touches the changes; mutants: $mutants"
	echo "$verdict"
	exit 0
fi
reasons=
((uncovered == 0)) || reasons="$uncovered uncovered regions touch the changes (listed above): cover or explain each; "
reasons+="mutants: $mutants"
((status == 0)) || reasons+=" (cargo mutants exit $status, see mutants.out/)"
verdict="verify: FAIL: $reasons"
echo "$verdict"
exit 1
