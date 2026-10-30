#!/usr/bin/env bash
# Builds sisu at each COMMIT once for a review run, so that no reviewer builds it again. Each
# build goes to RUN/bin/<sha>/ with the two runtime archives, which sisu links from its own
# directory. A commit that RUN/bin already has is not built again. All commits share one export
# directory and one target directory in RUN, so a later round rebuilds only the workspace crates.
# Prints the path of each sisu.
#   build_sisu.sh REPO RUN COMMIT...
set -euo pipefail

repo=$1 run=$2
shift 2
llvm=$(readlink -f "$repo/.llvm")
for ref in "$@"; do
  sha=$(git -C "$repo" rev-parse --verify "$ref^{commit}")
  bin=$run/bin/$sha
  if [[ ! -x $bin/sisu ]]; then
    # An export, not a worktree: `git worktree add` leaves an entry that a sandbox cannot prune.
    rm -rf "$run/src"
    mkdir -p "$run/src" "$bin"
    git -C "$repo" archive "$sha" | tar -x -C "$run/src"
    # git archive gives each file the commit's time, so an older commit would look fresh to cargo.
    find "$run/src" -type f -exec touch {} +
    ln -s "$llvm" "$run/src/.llvm"
    (cd "$run/src" && CARGO_TARGET_DIR=$run/target cargo build --quiet -p sisuc -p sisu-runtime-mimalloc -p sisu-runtime-system)
    cp "$run/target/debug/sisu" "$run/target/debug/libsisu_runtime_mimalloc.a" \
      "$run/target/debug/libsisu_runtime_system.a" "$bin/"
  fi
  echo "$bin/sisu"
done
