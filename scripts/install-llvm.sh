#!/usr/bin/env bash
# Installs the pinned LLVM release into .llvm/23, the prefix that
# .cargo/config.toml hands to llvm-sys. The system LLVM is left alone.
# With --audit it also installs opt and clang, with clang's resource directory
# for the ASan runtime, which the audit-sisu skill's toolbox runs.
# Safe to re-run: it does nothing when the pinned version, with ld.lld (and
# with --audit, opt and clang), is already there.
set -euo pipefail

version=23.1.3
sha256=ca9fe385a5d9e2d88132dc0e23a00e9f87f38fa75f67207b5e457d208e71d013
url="https://github.com/llvm/llvm-project/releases/download/llvmorg-${version}/LLVM-${version}-Linux-X64.tar.xz"

root="$(cd "$(dirname "$0")/.." && pwd)"
dest="$root/.llvm/23"
audit=false
[[ "${1:-}" == --audit ]] && audit=true

if [[ "$(cat "$dest/VERSION" 2>/dev/null)" == "$version" && -x "$dest/bin/ld.lld" ]] &&
    { ! $audit || [[ -x "$dest/bin/opt" && -x "$dest/bin/clang" ]]; }; then
    echo "LLVM $version is already in $dest"
    exit 0
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl --fail --location --retry 3 --no-progress-meter --output "$tmp/llvm.tar.xz" "$url"
echo "$sha256  $tmp/llvm.tar.xz" | sha256sum --check --quiet

rm -rf "$dest"
mkdir -p "$dest"
# Only what llvm-sys builds against: llvm-config, the C API headers, and the
# static LLVM and Polly libraries that `llvm-config --libnames` lists; and lld,
# with its ld.lld link, for the `--linker lld` tests. That is about 570 MB of
# the 12 GB release; the rest (clang and other tools) would not fit the 10 GB
# GitHub Actions cache.
top="LLVM-${version}-Linux-X64"
members=("$top/bin/llvm-config" "$top/bin/lld" "$top/bin/ld.lld" "$top/include/llvm" "$top/include/llvm-c"
    "$top/lib/libLLVM*.a" "$top/lib/libPolly*.a")
if $audit; then
    members+=("$top/bin/opt" "$top/bin/clang" "$top/bin/clang-23" "$top/lib/clang")
fi
tar -xJf "$tmp/llvm.tar.xz" -C "$dest" --strip-components=1 --wildcards "${members[@]}"
# Written last, so an interrupted run is redone on the next call.
echo "$version" >"$dest/VERSION"
echo "Installed LLVM $version in $dest"
