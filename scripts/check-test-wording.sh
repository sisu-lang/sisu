#!/usr/bin/env bash
# Fails when a unit test reads a diagnostic's wording: its message, or its label
# or help text beyond whether there is one. The text of a secondary label
# (`.secondary()`) and of a note (`.notes()`) counts too. Unit tests assert codes and
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
# A read of `.notes()` or `.secondary()` is flagged unless it is on this allow list:
#   `.notes()` and `.secondary()` followed by `.len()` or `.is_empty()`;
#   `.secondary()` followed by an index and `.0` (the span);
#   `.secondary()` followed by `, []` (an empty check);
#   `.secondary()` followed by `.iter()` and `.map(|(name, _)|` or `.map(|&(name, _)|`,
#     which bind spans only, over lines;
#   `let [(a, _), (b, _)] = <path>.secondary() else`, which binds spans only;
#   `panic!("...", <path>.secondary())`: a failure message shows the value, pins no text.
# `Diagnostic::secondary` and `Diagnostic::notes` are always flagged.
# A regex cannot follow a binding, so `let s = d.secondary();` is flagged: read the
# spans or the count in place.
# shortcut: a `let [...] = x.secondary() else` pattern that names a text element, or has
# another shape than `(name, _)` elements, is flagged. So is a `panic!` with more than one
# argument. Add the form to the allow list when a test needs it.
# shortcut: the allow list reads text, not types: `.secondary().iter().map(|(s, _)| ..)`
# passes for any body, and a `.0` of an index passes whatever follows it.
# An assert_eq! or assert_ne! of two `format!("{x:?}")` strings counts too: the
# Debug text of a diagnostic holds its wording. The argument of `format!` may
# hold calls, with parentheses nested to any depth.
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
  my $span = qr/\(\w+,\s*_\)/;
  my $path = qr/[\w.\[\]]+/;
  my $ok = qr/
      \blet\s*\[\s*$span(?:\s*,\s*$span)*\s*,?\s*\]\s*=\s*$path\.secondary\(\)\s+else\b
    | \bpanic!\(\s*"(?:[^"\\]|\\.)*"\s*,\s*$path\.secondary\(\)\s*\)
  /x;
  my $count = qr/\.(?:len|is_empty)\(\)/;
  my $spans = qr/$count|\[[^\]\n]*\]\.0\b|,\s*\[\]|\s*\.iter\(\)\)?\s*\.map\(\|&?$span\|/;
  while (/(?<ok>$ok)|\.message\(\)|\bDiagnostic::(?:message|secondary|notes)\b|\.secondary\(\)(?!$spans)|\.notes\(\)(?!$count)|(?:\.(?:help|label)\(\)|\bDiagnostic::(?:help|label)\b(?:\([^()]*\))?+)(?!\.is_(?:some|none)\(\)|,\s*None\b)/g) {
    next if defined $+{ok};
    my $line = 1 + (substr($_, 0, $-[0]) =~ tr/\n//);
    print "$ARGV:$line: a unit test reads diagnostic wording; assert the code, and pin the text in a tests/diagnostics fixture\n";
    $bad = 1;
  }
  my $parens = qr/(\((?:[^()]++|(?-1))*+\))/;
  my $debug = qr/format!\(\s*"\{[\w.]*:\#?\?\}"\s*(?:,\s*(?:[^(),]++|$parens)+)?\)/;
  pos = $start;
  while (/\bassert_(?:eq|ne)!\(\s*$debug\s*,\s*$debug/g) {
    my $line = 1 + (substr($_, 0, $-[0]) =~ tr/\n//);
    print "$ARGV:$line: a unit test compares the Debug text of two values, which holds a diagnostic\x27s wording; compare the codes, spans and fix edits\n";
    $bad = 1;
  }
  END { exit($bad ? 1 : 0) }
' "${files[@]}"
