#!/usr/bin/env bash
# Builds the two runtime archives, sisu-runtime-mimalloc and
# sisu-runtime-system, for the end-to-end tests: the nextest setup script.
# Under scripts/coverage.sh it builds them without instrumentation, in their
# own target directory, and copies them next to the instrumented sisu. The
# test build shares sisu's target directory and has already put instrumented
# archives there, which cargo would consider fresh.
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
	cargo build --quiet -p sisu-runtime-mimalloc -p sisu-runtime-system
else
	# cargo-llvm-cov instruments through RUSTC_WRAPPER, so a build without it is plain.
	env -u RUSTC_WRAPPER -u __CARGO_LLVM_COV_RUSTC_WRAPPER \
		cargo build --quiet -p sisu-runtime-mimalloc -p sisu-runtime-system \
		--target-dir target/llvm-cov-runtime
	for archive in libsisu_runtime_mimalloc.a libsisu_runtime_system.a; do
		# --remove-destination: cargo hardlinks the old file to its copy in deps/.
		cp --remove-destination "target/llvm-cov-runtime/debug/$archive" \
			"$SISU_COVERAGE_TARGET_DIR/debug/$archive"
	done
fi
