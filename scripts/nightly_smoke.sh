#!/usr/bin/env bash
# Runs each program of a directory that has an `.out` file, with each allocator, and checks that
# its stdout equals the `.out` file, its stderr is empty and its exit code is 0.
#   scripts/nightly_smoke.sh <bin dir> <programs dir>
# It needs bash and coreutils only, because the clean images have no Python. It also runs on
# the bash 3.2 of macOS, so it has no mapfile and no associative array.
set -u

if [ $# -ne 2 ]; then
  echo "usage: $0 <bin dir> <programs dir>" >&2
  exit 2
fi
bin=$1
programs=$2
tmp=$(mktemp -d) || exit 1
trap 'rm -rf "$tmp"' EXIT

ran=0
failed=0
for program in "$programs"/*.sisu; do
  expected=${program%.sisu}.out
  [ -f "$expected" ] || continue
  for allocator in mimalloc system; do
    ran=$((ran + 1))
    "$bin/sisu" run --allocator "$allocator" "$program" >"$tmp/stdout" 2>"$tmp/stderr"
    code=$?
    if [ "$code" -ne 0 ]; then
      echo "$program with --allocator $allocator: exit code $code" >&2
      failed=1
    fi
    if [ -s "$tmp/stderr" ]; then
      echo "$program with --allocator $allocator: stderr is not empty:" >&2
      cat "$tmp/stderr" >&2
      failed=1
    fi
    if ! cmp -s "$tmp/stdout" "$expected"; then
      echo "$program with --allocator $allocator: stdout differs from $expected" >&2
      failed=1
    fi
  done
done

if [ "$ran" -eq 0 ]; then
  echo "no program with an .out file in $programs" >&2
  exit 1
fi
exit "$failed"
