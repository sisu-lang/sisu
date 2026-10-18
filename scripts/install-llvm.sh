#!/usr/bin/env bash
# Installs the pinned LLVM release into .llvm/22, the prefix that
# .cargo/config.toml hands to llvm-sys. The system LLVM is left alone.
# Safe to re-run: it does nothing when the pinned version, with ld.lld, is
# already there.
set -euo pipefail

version=22.1.8
sha256=df0e1ecf16caf3489a272a5eea4eec9b0d82878f6477fa309504f918a0006384
url="https://github.com/llvm/llvm-project/releases/download/llvmorg-${version}/LLVM-${version}-Linux-X64.tar.xz"

root="$(cd "$(dirname "$0")/.." && pwd)"
dest="$root/.llvm/22"

if [[ "$(cat "$dest/VERSION" 2>/dev/null)" == "$version" && -x "$dest/bin/ld.lld" ]]; then
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
# with its ld.lld link, for the `--linker lld` tests. That is about 560 MB of
# the 12 GB release; the rest (clang and other tools) would not fit the 10 GB
# GitHub Actions cache.
top="LLVM-${version}-Linux-X64"
tar -xJf "$tmp/llvm.tar.xz" -C "$dest" --strip-components=1 --wildcards \
    "$top/bin/llvm-config" "$top/bin/lld" "$top/bin/ld.lld" "$top/include/llvm" "$top/include/llvm-c" \
    "$top/lib/libLLVM*.a" "$top/lib/libPolly*.a"
# Written last, so an interrupted run is redone on the next call.
echo "$version" >"$dest/VERSION"
echo "Installed LLVM $version in $dest"
