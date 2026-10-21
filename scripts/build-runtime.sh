#!/usr/bin/env bash
# Builds sisu-runtime for the end-to-end tests: the nextest setup script.
# Under scripts/coverage.sh it builds the runtime without instrumentation, in
# its own target directory, and copies it next to the instrumented sisu. The
# test build shares sisu's target directory and has already put an
# instrumented runtime there, which cargo would consider fresh.
#   scripts/build-runtime.sh
set -euo pipefail

# Not git rev-parse: cargo-mutants runs this in a copy of the tree without .git.
cd "$(dirname "${BASH_SOURCE[0]}")/.."
if [[ -z ${SISU_COVERAGE_TARGET_DIR:-} ]]; then
	# A bare cargo llvm-cov run would leave an instrumented runtime in target/debug.
	[[ -z ${CARGO_LLVM_COV:-} ]] || {
		echo "build-runtime: run coverage through scripts/coverage.sh, not cargo llvm-cov directly" >&2
		exit 1
	}
	cargo build --quiet -p sisu-runtime
else
	# cargo-llvm-cov instruments through RUSTC_WRAPPER, so a build without it is plain.
	env -u RUSTC_WRAPPER -u __CARGO_LLVM_COV_RUSTC_WRAPPER \
		cargo build --quiet -p sisu-runtime --target-dir target/llvm-cov-runtime
	# --remove-destination: cargo hardlinks the old file to its copy in deps/.
	cp --remove-destination target/llvm-cov-runtime/debug/libsisu_runtime.a \
		"$SISU_COVERAGE_TARGET_DIR/debug/libsisu_runtime.a"
fi
