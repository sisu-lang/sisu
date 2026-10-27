"""Finds the tests whose body a git diff changes (CODING_STANDARDS.md, Tests).

Usage: python3 changed_tests.py REF
"""

import re
import subprocess
import sys

import test_names

HUNK = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")


def bodies(lines: list[str]) -> list[tuple[str, int, int]]:
    """(name, first line, last line), counted from 1, of each test that `test_names.tests()` finds.
    The body starts at the `fn` line. A `fn` line that ends with `}` is the whole body. Otherwise
    the body ends at the first later line that is `}` at the indent of the `fn` line.

    shortcut: a raw string that holds a `}` at the indent of its `fn` ends the body early, so a
    later change in that body is missed. Upgrade to a lexer if such a miss is ever seen. The miss
    never flags a test whose body did not change."""
    found = []
    for first, name, _ in test_names.tests(lines):
        text = lines[first - 1]
        closing = text[: len(text) - len(text.lstrip())] + "}"
        last = first
        if not text.rstrip().endswith("}"):
            last = next((n for n in range(first + 1, len(lines) + 1) if lines[n - 1].rstrip() == closing),
                        len(lines))
        found.append((name, first, last))
    return found


def git(repo: str, *args: str) -> str:
    return subprocess.run(["git", "-C", repo, *args], check=True, capture_output=True,
                          text=True, encoding="utf-8").stdout


def hit(lines: list[str], start: int, count: int) -> set[str]:
    """The tests whose body meets the range of `count` lines from `start`; a count of 0 is empty."""
    return {name for name, first, last in bodies(lines)
            if count > 0 and start <= last and start + count - 1 >= first}


def changed(ref: str, repo: str = ".") -> set[tuple[str, str]]:
    """The (new path, test name) pairs whose body `git diff -U0 -M -l10000 REF` changes, for `.rs`
    files only. A deleted test is left out."""
    diff = git(repo, "-c", "core.quotepath=off", "diff", "-U0", "-M", "-l10000", ref).splitlines()
    found: set[tuple[str, str]] = set()
    old_path = new_path = ""
    old_lines: list[str] | None = None
    new_lines: list[str] = []
    left = [0, 0]  # lines of the current hunk not yet read, on the old and the new side
    for line in diff:
        if left[0] or left[1]:  # a hunk line; one that starts `---` or `+++` is not a header
            left[line.startswith("+")] -= line[:1] in ("-", "+")
            continue
        if line.startswith("--- "):
            old_path = line[4:].split("\t")[0].removeprefix("a/") if line != "--- /dev/null" else ""
        elif line.startswith("+++ "):
            new_path = line[4:].split("\t")[0].removeprefix("b/") if line != "+++ /dev/null" else ""
            old_lines = new_lines = None
        elif (m := HUNK.match(line)) and new_path.endswith(".rs"):
            a, b, c, d = (int(x) if x is not None else 1 for x in m.groups())
            left = [b, d]
            if old_lines is None:
                old_lines = git(repo, "show", f"{ref}:{old_path}").splitlines() if old_path else []
                with open(f"{repo}/{new_path}", encoding="utf-8") as f:
                    new_lines = f.read().splitlines()
            new_names = {name for name, _, _ in bodies(new_lines)}
            found |= {(new_path, n) for n in hit(old_lines, a, b) & new_names}
            found |= {(new_path, n) for n in hit(new_lines, c, d)}
    return found


if __name__ == "__main__":
    for path, name in sorted(changed(sys.argv[1])):
        print(f"{path}: {name}")
