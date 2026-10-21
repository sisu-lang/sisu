#!/usr/bin/env bash
# Installs the pinned LLVM release into .llvm/23, the prefix that
# .cargo/config.toml hands to llvm-sys. The system LLVM is left alone.
# With --audit it also installs opt and clang, with clang's resource directory
# for the ASan runtime, which the audit-sisu skill's toolbox runs.
# Safe to re-run: it does nothing when the pinned version, with ld.lld (and
# with --audit, opt and clang), is already there.
set -euo pipefail

version=23.1.3
case "$(uname -s)-$(uname -m)" in
Linux-x86_64)
    platform=Linux-X64
    sha256=ca9fe385a5d9e2d88132dc0e23a00e9f87f38fa75f67207b5e457d208e71d013
    ;;
Linux-aarch64)
    platform=Linux-ARM64
    sha256=56d005e339c979a696ed6c244e6472e16f3b1371e48987a07e6145bda7ee0d03
    ;;
Darwin-arm64)
    platform=macOS-ARM64
    sha256=3dcfd88aec10be204a24037bcb0dc0ad777882e9dbe8ac60e2639060d8a794b1
    ;;
*)
    echo "install-llvm.sh: no LLVM release for $(uname -s) $(uname -m); Sisu builds on x86-64 and aarch64 Linux and arm64 macOS" >&2
    exit 1
    ;;
esac
url="https://github.com/llvm/llvm-project/releases/download/llvmorg-${version}/LLVM-${version}-${platform}.tar.xz"

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
# macOS has neither sha256sum nor GNU tar; its bsdtar matches wildcards without a flag.
if [[ "$(uname -s)" == Darwin ]]; then
    echo "$sha256  $tmp/llvm.tar.xz" | shasum -a 256 --check --quiet
    tar=(tar)
else
    echo "$sha256  $tmp/llvm.tar.xz" | sha256sum --check --quiet
    tar=(tar --wildcards)
fi

rm -rf "$dest"
mkdir -p "$dest"
# Only what llvm-sys builds against: llvm-config, the C API headers, and the
# static LLVM and Polly libraries that `llvm-config --libnames` lists; and lld,
# with its ld.lld link, for the `--linker lld` tests. That is about 570 MB of
# the 12 GB release; the rest (clang and other tools) would not fit the 10 GB
# GitHub Actions cache.
top="LLVM-${version}-${platform}"
members=("$top/bin/llvm-config" "$top/bin/lld" "$top/bin/ld.lld" "$top/include/llvm" "$top/include/llvm-c"
    "$top/lib/libLLVM*.a" "$top/lib/libPolly*.a")
if $audit; then
    members+=("$top/bin/opt" "$top/bin/clang" "$top/bin/clang-23" "$top/lib/clang")
fi
"${tar[@]}" -xJf "$tmp/llvm.tar.xz" -C "$dest" --strip-components=1 "${members[@]}"
# Written last, so an interrupted run is redone on the next call.
echo "$version" >"$dest/VERSION"
echo "Installed LLVM $version in $dest"
