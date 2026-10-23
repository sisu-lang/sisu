"""Rejects a `#[test]` whose name does not say what should hold, and an end-to-end test without
GIVEN, WHEN and THEN lines above it (CODING_STANDARDS.md, Tests). The tests in
test_names_baseline.txt were named before the rule and are skipped.

Usage: python3 test_names.py FILE...
"""

import os
import re
import sys

BASELINE = os.path.join(os.path.dirname(__file__), "test_names_baseline.txt")
END_TO_END = re.compile(r"(^|/)crates/[^/]+/tests/")
FN = re.compile(r"\bfn\s+(\w+)")


def tests(lines: list[str]) -> list[tuple[int, str, list[str]]]:
    """(line, name, comment lines) of each `#[test]` function. The comment is the run of `//`
    lines above the function's attributes."""
    found = []
    for i, line in enumerate(lines):
        if line.strip() != "#[test]":
            continue
        j = i
        while j < len(lines) and not FN.search(lines[j]):
            j += 1
        if j == len(lines):
            continue
        top = i
        while top > 0 and lines[top - 1].strip().startswith("#["):
            top -= 1
        comment = []
        while top > 0 and lines[top - 1].strip().startswith("//"):
            top -= 1
            comment.insert(0, lines[top].strip().lstrip("/").strip())
        found.append((j + 1, FN.search(lines[j]).group(1), comment))
    return found


def has_given_when_then(comment: list[str]) -> bool:
    """Whether lines that start with GIVEN, WHEN and THEN come in that order."""
    words = iter(line.split(" ", 1)[0] for line in comment)
    return all(want in words for want in ("GIVEN", "WHEN", "THEN"))


def main(paths: list[str], baseline: str = BASELINE) -> int:
    with open(baseline, encoding="utf-8") as f:
        old = {line.strip() for line in f if line.strip() and not line.startswith("#")}
    bad = 0
    for path in paths:
        with open(path, encoding="utf-8") as f:
            lines = f.read().splitlines()
        for n, name, comment in tests(lines):
            if f"{path}::{name}" in old:
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
