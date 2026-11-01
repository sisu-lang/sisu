#!/usr/bin/env bash
# Fails when a unit test reads a diagnostic's wording: its message, its label,
# or its help text beyond whether there is one. Unit tests assert codes and
# positions; the tests/diagnostics fixtures pin the wording (CODING_STANDARDS.md,
# Tests). The writers' own tests in render.rs, json.rs and report.rs may read it.
# Test code is files under tests/, tests.rs, and an inline
# `#[cfg(test)] mod name { ... }`, which clippy's items_after_test_module keeps
# at the end of its file. A `#[cfg(test)]` step counter is production code.
# shortcut: a `#[cfg(test)] fn` helper before the test module is checked as
# production code too, so its wording reads pass; move it into the module if
# a hit matters.
# shortcut: between `#[cfg(test)]` and `mod`, the pattern takes blank lines,
# `//` comments and attributes with one level of inner brackets, not a
# `/* */` comment; add the form when a file uses it.
#   scripts/check-test-wording.sh <file.rs>...
set -euo pipefail

files=()
for file in "$@"; do
  [[ $file == */diagnostic/render.rs || $file == */diagnostic/json.rs || $file == */src/report.rs ]] || files+=("$file")
done
[[ ${#files[@]} -eq 0 ]] && exit 0

perl -0777 -ne '
  pos = $ARGV =~ m{/tests/|/tests\.rs$} ? 0 : /^#\[cfg\(test\)\]\n(?:#\[(?:[^][]|\[[^]]*\])*\]\n|\/\/[^\n]*\n|\n)*(?:pub(?:\([^)\n]*\))? )?mod \w+ \{/m ? $-[0] : length;
  while (/\.(?:message|label)\(\)|\.help\(\)(?!\.is_(?:some|none)\(\)|,\s*None\b)/g) {
    my $line = 1 + (substr($_, 0, $-[0]) =~ tr/\n//);
    print "$ARGV:$line: a unit test reads diagnostic wording; assert the code, and pin the text in a tests/diagnostics fixture\n";
    $bad = 1;
  }
  END { exit($bad ? 1 : 0) }
' "${files[@]}"
