#!/usr/bin/env bash
# Measures the coverage of the changes since <base>: by default where HEAD left
# main. On a stacked branch, pass the stage's base so earlier stages stay out.
# It runs the nextest suite under cargo-llvm-cov and honors NEXTEST_PROFILE.
# It fails only when the instrumented run fails, never on low coverage.
# Programs link an uninstrumented runtime (scripts/build-runtime.sh), so the
# runtime's coverage comes from its unit tests only.
# git diff leaves out untracked files, so commit new files or `git add -N`
# them first.
#   scripts/coverage.sh [base]
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
base=${1:-$(git merge-base main HEAD)}

# cargo-llvm-cov passes its target directory to cargo as an argument, so tell
# the nextest setup script (scripts/build-runtime.sh) where sisu will look.
export SISU_COVERAGE_TARGET_DIR="$PWD/target/llvm-cov-target"

cargo llvm-cov clean --workspace
# This test exhausts file descriptors, so the profiler's "Too many open files"
# line breaks its exact stderr check.
cargo llvm-cov nextest --workspace --locked --no-report \
	-E 'not (package(sisu-runtime) & test(=tests::print_panics_when_standard_output_cannot_be_duplicated))'

out=target/llvm-cov
mkdir -p "$out"
cargo llvm-cov report --json --output-path "$out/coverage.json"
cargo llvm-cov report --lcov --output-path "$out/lcov.info"
git diff --no-color --no-ext-diff -U0 --src-prefix=a/ --dst-prefix=b/ "$base" >"$out/changes.diff"

python3 scripts/coverage_report.py target/llvm-cov/coverage.json target/llvm-cov/lcov.info target/llvm-cov/changes.diff
