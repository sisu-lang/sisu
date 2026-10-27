"""Rejects a `#[test]` whose name does not say what should hold, and an end-to-end test without
GIVEN, WHEN and THEN lines above it (CODING_STANDARDS.md, Tests). The tests in
test_names_baseline.txt were named before the rule and are skipped.

With `--diff REF`, it also rejects a baseline test whose body differs from REF: the change is
the time to rename it. With `--baseline`, it rejects a line of the baseline that names no test.

Usage: python3 test_names.py [--diff REF] FILE...
       python3 test_names.py --baseline
"""

import argparse
import collections
import os
import re
import sys

import changed_tests

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASELINE = os.path.join(ROOT, "scripts", "test_names_baseline.txt")
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


def main(paths: list[str], baseline: str = BASELINE, changed: frozenset = frozenset()) -> int:
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
                if (path, name) in changed:
                    print(f"{path}:{n}: test `{name}` is in test_names_baseline.txt and its body "
                          "changed: rename it to <subject>_should_<result> and remove its "
                          "baseline line")
                    bad = 1
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


def check_baseline(baseline: str = BASELINE, root: str = ROOT) -> int:
    """Prints each line of `baseline` whose file, under `root`, has no test of that name."""
    have: dict[str, collections.Counter] = {}
    seen: collections.Counter = collections.Counter()
    bad = 0
    with open(baseline, encoding="utf-8") as f:
        entries = f.read().splitlines()
    for n, entry in enumerate(entries, 1):
        if not entry.strip() or entry.startswith("#"):
            continue
        path, _, name = entry.strip().rpartition("::")
        if path not in have:
            try:
                with open(os.path.join(root, path), encoding="utf-8") as f:
                    have[path] = collections.Counter(t[1] for t in tests(f.read().splitlines()))
            except FileNotFoundError:
                have[path] = collections.Counter()
        seen[entry] += 1
        if seen[entry] > have[path][name]:
            print(f"{os.path.relpath(baseline, root)}:{n}: `{entry.strip()}` names no test")
            bad = 1
    return bad


def cli(argv: list[str], baseline: str = BASELINE) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--diff", metavar="REF")
    parser.add_argument("--baseline", action="store_true", dest="check")
    parser.add_argument("files", nargs="*")
    args = parser.parse_args(argv)
    if args.check:
        return check_baseline(baseline)
    changed = frozenset(changed_tests.changed(args.diff)) if args.diff and args.files else frozenset()
    return main(args.files, baseline, changed)


if __name__ == "__main__":
    sys.exit(cli(sys.argv[1:]))
