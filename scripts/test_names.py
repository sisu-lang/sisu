"""Rejects a `#[test]` whose name does not say what should hold, and an end-to-end test without
GIVEN, WHEN and THEN lines above it (CODING_STANDARDS.md, Tests). The tests in
test_names_baseline.txt were named before the rule and are skipped.

Usage: python3 test_names.py FILE...
"""

import collections
import os
import re
import sys

BASELINE = os.path.join(os.path.dirname(__file__), "test_names_baseline.txt")
END_TO_END = re.compile(r"(^|/)crates/[^/]+/tests/")
# At the start of a line, so `fn` inside an attribute such as `#[should_panic(expected = "fn f")]`
# is not the test's name. rustfmt puts `#[test]` and `fn` on lines of their own.
FN = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:(?:async|const|unsafe)\s+)*fn\s+(\w+)")


def tests(lines: list[str]) -> list[tuple[int, str, list[str]]]:
    """(line, name, doc lines) of each `#[test]` function. The doc is the run of `///` lines above
    the function's attributes."""
    found = []
    for i, line in enumerate(lines):
        if line.strip() != "#[test]":
            continue
        # The first line after the attributes, comments and blank lines is the function.
        j, depth = i + 1, 0
        while j < len(lines):
            text = lines[j].strip()
            if depth > 0 or text.startswith("#["):
                depth += text.count("[") - text.count("]")
            elif text and not text.startswith("//"):
                break
            j += 1
        fn = FN.search(lines[j]) if j < len(lines) else None
        if fn is None:  # a test a macro writes, such as `fn $name()`: its name is not in the file
            continue
        top = i
        while top > 0:
            text = lines[top - 1].strip()
            if text.startswith("#["):
                top -= 1
            elif text.endswith("]") and not text.startswith("//"):  # the end of a long attribute
                top -= 1
                while top > 0 and not lines[top].strip().startswith("#["):
                    top -= 1
            else:
                break
        comment = []
        while top > 0 and lines[top - 1].strip().startswith("///"):
            top -= 1
            comment.insert(0, lines[top].strip()[3:].strip())
        found.append((j + 1, fn.group(1), comment))
    return found


def has_given_when_then(comment: list[str]) -> bool:
    """Whether lines that start with GIVEN, WHEN and THEN come in that order."""
    words = iter(line.split(" ", 1)[0] for line in comment)
    return all(want in words for want in ("GIVEN", "WHEN", "THEN"))


def main(paths: list[str], baseline: str = BASELINE) -> int:
    # A count, so a new test that reuses an old name in another module of the file is checked.
    with open(baseline, encoding="utf-8") as f:
        old = collections.Counter(l.strip() for l in f if l.strip() and not l.startswith("#"))
    bad = 0
    for path in paths:
        with open(path, encoding="utf-8") as f:
            lines = f.read().splitlines()
        for n, name, comment in tests(lines):
            if old[f"{path}::{name}"] > 0:
                old[f"{path}::{name}"] -= 1
                continue
            if "_should_" not in name:
                print(f"{path}:{n}: test name `{name}` should say what should hold: "
                      "<subject>_should_<result>")
                bad = 1
            if END_TO_END.search(path) and not has_given_when_then(comment):
                print(f"{path}:{n}: end-to-end test `{name}` needs GIVEN, WHEN and THEN lines "
                      "in its doc comment")
                bad = 1
    return bad


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
