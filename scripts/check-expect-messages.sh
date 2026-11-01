#!/usr/bin/env bash
# Fails when an `expect` message in non-test Rust code lacks "should": the
# message states the invariant that should hold (CODING_STANDARDS.md, Code).
# Test code is skipped: files under tests/, tests.rs, and an inline
# `#[cfg(test)] mod name { ... }`, which clippy's items_after_test_module keeps
# at the end of its file. A `#[cfg(test)]` step counter is production code.
# shortcut: between `#[cfg(test)]` and `mod`, the pattern takes blank lines,
# `//` comments and attributes with one level of inner brackets, not a
# `/* */` comment; add the form when a file uses it.
#   scripts/check-expect-messages.sh <file.rs>...
set -euo pipefail

files=()
for file in "$@"; do
  [[ $file == */tests/* || $file == */tests.rs ]] || files+=("$file")
done
[[ ${#files[@]} -eq 0 ]] && exit 0

perl -0777 -ne '
  s/^#\[cfg\(test\)\]\n(?:#\[(?:[^][]|\[[^]]*\])*\]\n|\/\/[^\n]*\n|\n)*(?:pub(?:\([^)\n]*\))? )?mod \w+ \{.*//ms;
  while (/\.expect\(\s*"((?:[^"\\]|\\.)*)"/g) {
    next if $1 =~ /\bshould\b/;
    my $line = 1 + (substr($_, 0, $-[0]) =~ tr/\n//);
    print "$ARGV:$line: expect message should say what should hold, with \"should\": \"$1\"\n";
    $bad = 1;
  }
  END { exit($bad ? 1 : 0) }
' "${files[@]}"
