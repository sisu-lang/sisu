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
# Any Debug `format!` in the first two arguments of `assert_eq!` or `assert_ne!` fails, unless
# one bare `format!` matches a plain string literal.
# The argument of `format!` can hold calls with nested parentheses.
# shortcut: the check flags any type, not only a diagnostic; compare values directly, or exempt
# the file when a hit is right.
# shortcut: turbofish commas (`x.collect::<Map<K, V>>()`) at top level split early.
# shortcut: `.to_string()`, `String::from("...")`, a `const` string, `&format!(...)`, and
# `format!(...).as_str()` fail against a literal.
# shortcut: `assert!(format!("{a:?}") == b)` and a pre-collected Debug list compared with a
# literal list pass.
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
  # `exempt` skips both checks for this file; `pairs.rs` compares MIR `Block` Debug text and never
  # touches `Diagnostic`.
  crates/sisuc/src/mir/tests/pairs.rs
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
  my $count = qr/\s*\.(?:len|is_empty)\(\)/;
  my $spans = qr/$count|\[[^\]\n]*\]\.0\b|,\s*\[\]|\s*\.iter\(\)\)?\s*\.map\(\|&?$span\|/;
  while (/(?<ok>$ok)|\.message\(\)|\bDiagnostic::(?:message|secondary|notes)\b|\.secondary\(\)(?!$spans)|\.notes\(\)(?!$count)|(?:\.(?:help|label)\(\)|\bDiagnostic::(?:help|label)\b(?:\([^()]*\))?+)(?!\s*\.is_(?:some|none)\(\)|,\s*None\b)/g) {
    next if defined $+{ok};
    my $line = 1 + (substr($_, 0, $-[0]) =~ tr/\n//);
    print "$ARGV:$line: a unit test reads diagnostic wording; assert the code, and pin the text in a tests/diagnostics fixture\n";
    $bad = 1;
  }
  my $parens = qr/(\((?:[^()]++|(?-1))*+\))/;
  my $debug = qr/format!\(\s*"\{[\w.]*:\#?\?\}"\s*(?:,\s*(?:[^(),]++|$parens)+)?\)/;
  my $str_lit = qr/(?:b?"(?:[^"\\]|\\.)*"|b?r(#*)".*?"\1)/s;
  pos = $start;
  while (/\bassert_(?:eq|ne)!\(/g) {
    my $call_start = $-[0];
    my $args_start = pos($_);
    my $saved_pos = $args_start;
    my @commas;
    my $depth = 1;
    my $end_pos;
    while ($depth > 0 && pos($_) < length($_)) {
      if (/\G\/\/[^\n]*/gc) {
        next;
      }
      if (/\Gb?r(#*)".*?"\1/sgc) {
        next;
      }
      if (/\Gb?"(?:[^"\\]|\\.)*"/sgc) {
        next;
      }
      if (/\G\x27(?:\\.[^\x27]*|[^\x27\\])\x27/gc) {
        next;
      }
      if (/\G[\(\[\{]/gc) {
        $depth++;
        next;
      }
      if (/\G[\)\]\}]/gc) {
        $depth--;
        if ($depth == 0) {
          $end_pos = $-[0];
          last;
        }
        next;
      }
      if ($depth == 1 && /\G,/gc) {
        push @commas, $-[0];
        next;
      }
      /\G[^"\x27\/\(\[\{\)\]\},br]+|\G./sgc;
    }
    pos($_) = $saved_pos;
    next unless defined($end_pos) && @commas;
    my $lhs = substr($_, $args_start, $commas[0] - $args_start);
    my $rhs_end = @commas > 1 ? $commas[1] : $end_pos;
    my $rhs = substr($_, $commas[0] + 1, $rhs_end - ($commas[0] + 1));
    $lhs =~ s/^\s+|\s+$//g;
    $rhs =~ s/^\s+|\s+$//g;
    next unless $lhs =~ /$debug/ || $rhs =~ /$debug/;
    my $allowed =
      ($lhs =~ /\A$debug\z/ && $rhs =~ /\A$str_lit\z/) ||
      ($rhs =~ /\A$debug\z/ && $lhs =~ /\A$str_lit\z/);
    next if $allowed;
    my $line = 1 + (substr($_, 0, $call_start) =~ tr/\n//);
    print "$ARGV:$line: a unit test compares the Debug text of two values, which holds a diagnostic\x27s wording; compare the codes, spans and fix edits\n";
    $bad = 1;
  }
  END { exit($bad ? 1 : 0) }
' "${files[@]}"
