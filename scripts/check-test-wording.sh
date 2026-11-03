#!/usr/bin/env bash
# Fails when a unit test reads a diagnostic's wording: its message, its label,
# or its help text beyond whether there is one. Unit tests assert codes and
# positions; the tests/diagnostics fixtures pin the wording (CODING_STANDARDS.md,
# Tests). The writers' own tests in render.rs, json.rs and report.rs may read it.
# So may crates/sisuc/src/check/tests/parity.rs, which holds only `parity_found`: it compares the
# output of a generic call with that of its concrete twin and pins no text.
# Test code is files under tests/, tests.rs, and an inline
# `#[cfg(test)] mod name { ... }`, which clippy's items_after_test_module keeps
# at the end of its file. A `#[cfg(test)]` step counter is production code.
# shortcut: a `#[cfg(test)] fn` helper before the test module is checked as
# production code too, so its wording reads pass; move it into the module if
# a hit matters.
# The path forms, such as `Diagnostic::message`, count too.
# So does an assert_eq! or assert_ne! of two `format!("{x:?}")` strings: the
# Debug text of a diagnostic holds its wording.
# shortcut: the Debug pair is flagged for any type, not only a diagnostic;
# compare such values directly, or exempt the file when a hit is right.
# shortcut: between `#[cfg(test)]` and `mod`, the pattern takes blank lines,
# `//` comments and attributes with one level of inner brackets, not a
# `/* */` comment; add the form when a file uses it.
#   scripts/check-test-wording.sh <file.rs>...
set -euo pipefail

# Each exempt file by its full path, relative or absolute, so the same file
# name in another crate is checked.
exempt=(
  crates/sisuc/src/diagnostic/render.rs
  crates/sisuc/src/diagnostic/json.rs
  crates/sisuc/src/report.rs
  crates/sisuc/src/check/tests/parity.rs
)
files=()
for file in "$@"; do
  for path in "${exempt[@]}"; do
    [[ $file == "$path" || $file == */"$path" ]] && continue 2
  done
  files+=("$file")
done
[[ ${#files[@]} -eq 0 ]] && exit 0

perl -0777 -ne '
  $start = $ARGV =~ m{/tests/|/tests\.rs$} ? 0 : /^#\[cfg\(test\)\]\n(?:#\[(?:[^][]|\[[^]]*\])*\]\n|\/\/[^\n]*\n|\n)*(?:pub(?:\([^)\n]*\))? )?mod \w+ \{/m ? $-[0] : length;
  pos = $start;
  while (/\.(?:message|label)\(\)|\bDiagnostic::(?:message|label)\b|(?:\.help\(\)|\bDiagnostic::help\b(?:\([^()]*\))?+)(?!\.is_(?:some|none)\(\)|,\s*None\b)/g) {
    my $line = 1 + (substr($_, 0, $-[0]) =~ tr/\n//);
    print "$ARGV:$line: a unit test reads diagnostic wording; assert the code, and pin the text in a tests/diagnostics fixture\n";
    $bad = 1;
  }
  my $debug = qr/format!\(\s*"\{[\w.]*:\#?\?\}"\s*(?:,\s*[^(),]+)?\)/;
  pos = $start;
  while (/\bassert_(?:eq|ne)!\(\s*$debug\s*,\s*$debug/g) {
    my $line = 1 + (substr($_, 0, $-[0]) =~ tr/\n//);
    print "$ARGV:$line: a unit test compares the Debug text of two values, which holds a diagnostic\x27s wording; compare the codes, spans and fix edits\n";
    $bad = 1;
  }
  END { exit($bad ? 1 : 0) }
' "${files[@]}"
