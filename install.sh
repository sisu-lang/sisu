#!/bin/sh
# Installs Sisu into ~/.sisu/bin. Run it with:
#   curl --proto '=https' --tlsv1.2 -fsSL https://github.com/sisu-lang/sisu/releases/latest/download/install.sh | sh
# The whole body is in functions and the last line is `main`, so a cut download runs nothing.
set -eu

die() {
  printf 'install.sh: %s\n' "$1" >&2
  exit 1
}

unsupported() {
  die "cannot install on $1: Sisu supports x86-64 and ARM64 Linux with glibc 2.35 or newer, and ARM64 macOS 15 or newer."
}

# Sets tarball.
detect_macos() {
  # A Rosetta terminal reports x86_64, so this key decides. A missing key means an Intel Mac.
  arm=$(sysctl -n hw.optional.arm64 2>/dev/null || true)
  [ "$arm" = 1 ] || unsupported 'Intel macOS'
  version=$(sw_vers -productVersion)
  major=${version%%.*}
  case $major in
    '' | *[!0-9]*) unsupported "macOS $version" ;;
  esac
  [ "$major" -ge 15 ] || unsupported "macOS $version"
  tarball='sisu-aarch64-macos.tar.gz'
}

# Sets tarball.
detect_linux() {
  case $arch in
    x86_64 | aarch64) ;;
    *) unsupported "$arch Linux" ;;
  esac
  case $(ldd --version 2>&1 || true) in
    *musl*) unsupported 'musl Linux' ;;
  esac
  libc=$(getconf GNU_LIBC_VERSION 2>/dev/null || true)
  version=${libc#glibc }
  case $version in
    [0-9]*.[0-9]*) ;;
    *) unsupported 'Linux with an unknown C library' ;;
  esac
  major=${version%%.*}
  minor=${version#*.}
  minor=${minor%%.*}
  case $major$minor in
    *[!0-9]*) unsupported 'Linux with an unknown C library' ;;
  esac
  if [ "$major" -lt 2 ] || { [ "$major" -eq 2 ] && [ "$minor" -lt 35 ]; }; then
    unsupported "glibc $version"
  fi
  tarball=sisu-$arch-linux.tar.gz
}

# Sets os and tarball.
detect() {
  os=$(uname -s)
  arch=$(uname -m)
  case $os in
    Linux) detect_linux ;;
    Darwin) detect_macos ;;
    *) unsupported "$os" ;;
  esac
}

# Reads the file on stdin: sha256sum puts a backslash before the hash for a name with a backslash.
sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum <"$1"
  else
    shasum -a 256 <"$1"
  fi
}

# Sets sisu and tmp. A tmp that a killed run left goes away first.
setup() {
  sisu=$HOME/.sisu
  tmp=$sisu/tmp
  rm -rf "$tmp" 2>/dev/null || die "cannot write to $sisu."
  mkdir -p "$tmp" 2>/dev/null || die "cannot write to $sisu."
  # dash does not run an EXIT trap on a signal.
  trap 'rm -rf "$tmp"' EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
}

fetch() {
  base=https://github.com/sisu-lang/sisu/releases/latest/download/
  # shortcut: SHA256SUMS and the tarball can come from two builds, if users report a mismatch
  # write the tag into the installer
  for file in "$tarball" SHA256SUMS; do
    if [ -n "${SISU_DOWNLOAD_DIR:-}" ]; then
      [ -f "$SISU_DOWNLOAD_DIR/$file" ] || die "$SISU_DOWNLOAD_DIR has no $file."
      cp "$SISU_DOWNLOAD_DIR/$file" "$tmp/$file"
    else
      curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$tmp/$file" "$base$file" ||
        die "cannot download $base$file."
    fi
  done
}

verify() {
  dotted=$(printf '%s' "$tarball" | sed 's/[.]/[.]/g')
  want=$(grep "[ *]$dotted\$" "$tmp/SHA256SUMS" | cut -d ' ' -f 1 | head -n 1)
  got=$(sha256 "$tmp/$tarball" | cut -d ' ' -f 1)
  if [ -z "$want" ] || [ "$got" != "$want" ]; then
    die "the checksum of $tarball does not match SHA256SUMS. Run the install line again."
  fi
}

unpack() {
  # GNU tar reads a backslash in the argument of -C as an escape, so go into the directory.
  (cd "$tmp" && tar -xzf "$tarball")
  for file in sisu libsisu_runtime_mimalloc.a libsisu_runtime_system.a; do
    [ -f "$tmp/sisu/$file" ] || die "the tarball has no $file."
  done
  "$tmp/sisu/sisu" --version >/dev/null 2>&1 || die 'this machine cannot run the Sisu build.'
}

# Swaps the whole directory, so that no failure leaves a sisu beside archives of another build.
swap() {
  bin=$sisu/bin
  old=$sisu/bin.old
  # With no bin and a bin.old, a killed run left the earlier install in bin.old: keep it.
  if [ -e "$bin" ]; then
    rm -rf "$old" 2>/dev/null || die "cannot write to $sisu."
    mv "$bin" "$old" 2>/dev/null || die "cannot write to $sisu."
  fi
  if ! mv "$tmp/sisu" "$bin" 2>/dev/null; then
    if [ -e "$old" ]; then
      mv "$old" "$bin" 2>/dev/null || die "cannot write to $sisu."
    fi
    die "cannot write to $sisu."
  fi
  rm -rf "$old" 2>/dev/null || die "cannot write to $sisu."
}

compiler_hint() {
  if [ "$os" = Darwin ]; then
    if xcode-select -p >/dev/null 2>&1; then
      return 0
    fi
    fix='Run: xcode-select --install'
  else
    if command -v cc >/dev/null 2>&1; then
      return 0
    fi
    if command -v apt-get >/dev/null 2>&1; then
      fix='Run: sudo apt install build-essential'
    else
      fix="Install a C compiler as \`cc\`."
    fi
  fi
  printf 'Sisu needs a C compiler to build programs. %s\n' "$fix"
}

path_hint() {
  case ":$PATH:" in
    *":$HOME/.sisu/bin:"*) ;;
    *)
      # shellcheck disable=SC2016 # the user's shell expands these lines, not this script
      printf '%s\n' 'export PATH="$HOME/.sisu/bin:$PATH"' 'fish_add_path $HOME/.sisu/bin'
      ;;
  esac
}

main() {
  [ -n "${HOME:-}" ] || die 'HOME is not set.'
  detect
  command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1 ||
    die 'this machine has no sha256sum or shasum.'
  setup
  fetch
  verify
  unpack
  swap
  compiler_hint
  path_hint
  printf 'Sisu is installed in %s/.sisu/bin.\n' "$HOME"
}

main
