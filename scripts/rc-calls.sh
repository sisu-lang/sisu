#!/usr/bin/env bash
# Compares the `sisu_rc.*` call sites of two sisuc binaries, per program, function and helper.
# Each program of the snapshot list is compiled with `build --emit ir-raw`; a count that differs
# prints a tab-separated line `program function helper base N head M`. Exits 1 if a head count
# is higher than the base's, unless `scripts/rc-calls.allow` lists that `program<TAB>function<TAB>
# helper` (one per line, each a leak fix with its Valgrind case). Prints `rc-calls: identical`
# when nothing differs.
#   scripts/rc-calls.sh BASE_SISUC HEAD_SISUC
set -euo pipefail

[[ $# -eq 2 ]] || { echo "usage: scripts/rc-calls.sh BASE_SISUC HEAD_SISUC" >&2; exit 2; }
base_sisuc=$(realpath "$1")
head_sisuc=$(realpath "$2")
cd "$(dirname "${BASH_SOURCE[0]}")/.."
allow=scripts/rc-calls.allow
[[ -e $allow ]] || allow=/dev/null
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# "function<TAB>helper<TAB>count" for each `sisu_rc.*` call site group of one program: the
# program's raw IR from the binary, then the count into the file.
count() {
  "$1" build --emit ir-raw "$2" >"$tmp/ll" 2>&1 || { echo "rc-calls: $1 failed on $2" >&2; exit 2; }
  awk '/^define/ { match($0, /@"?[^"(]+/); f = substr($0, RSTART, RLENGTH) }
       /call .*@"?sisu_rc\./ { match($0, /@"?sisu_rc\.[^"(]+/); n[f "\t" substr($0, RSTART, RLENGTH)]++ }
       END { for (k in n) print k "\t" n[k] }' "$tmp/ll" | sort
}

differs=0 raised=0
for f in crates/sisuc/tests/run/*.sisu crates/sisuc/tests/run-fail/*.sisu \
         crates/sisuc/tests/programs/*.sisu bench/*/main.sisu bench/features/*/main.sisu; do
  [[ -e $f ]] || continue
  count "$base_sisuc" "$f" >"$tmp/base"
  count "$head_sisuc" "$f" >"$tmp/head"
  out=$(awk -F'\t' -v prog="$f" -v allow="$allow" -v basefile="$tmp/base" '
    BEGIN { while ((getline line < allow) > 0) ok[line] = 1 }
    { key = $1 "\t" $2 }
    FILENAME == basefile { b[key] = $3; keys[key]; next }
    { h[key] = $3; keys[key] }
    END {
      for (k in keys) {
        if (b[k] + 0 == h[k] + 0) continue
        flag = ""
        if (h[k] + 0 > b[k] + 0 && !((prog "\t" k) in ok)) { flag = "  RAISED"; bad = 1 }
        printf "%s\t%s\tbase %d head %d%s\n", prog, k, b[k] + 0, h[k] + 0, flag
      }
      exit bad
    }' "$tmp/base" "$tmp/head" | sort) || raised=1
  [[ -z $out ]] || { echo "$out"; differs=1; }
done

if ((raised)); then
  echo "rc-calls: a head count is higher than the base's"
  exit 1
fi
((differs)) || echo "rc-calls: identical"
