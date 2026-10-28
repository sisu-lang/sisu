"""Finds the tests whose body a git diff changes (CODING_STANDARDS.md, Tests).

Usage: python3 changed_tests.py REF
"""

import bisect
import os
import re
import subprocess
import sys

import test_names

HUNK = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")


# A `}` that ends a line, alone or before a comment: `} // why`, `} /* x */`.
CLOSE = re.compile(r"\}\s*(?://.*|/\*.*)?$")


def bodies(lines: list[str]) -> list[tuple[str, int, int]]:
    """(name, first line, last line), counted from 1, of each test that `test_names.tests()` finds.
    The body starts at the `fn` line. A `fn` line that ends with `}` is the whole body. Otherwise
    the body ends at the first later `}` line at the indent of the `fn` line. A comment can follow
    each `}`. The body ends before the next test's `fn` line. So the bodies are in order and do not
    overlap.

    shortcut: a raw string that holds a `}` at the indent of its `fn` ends the body early, so a
    later change in that body is missed. Upgrade to a lexer if such a miss is ever seen. The miss
    never flags a test whose body did not change."""
    tests = test_names.tests(lines)
    found = []
    for i, (first, name, _) in enumerate(tests):
        text = lines[first - 1]
        closing = re.compile(re.escape(text[: len(text) - len(text.lstrip())]) + CLOSE.pattern)
        stop = tests[i + 1][0] - 1 if i + 1 < len(tests) else len(lines)  # the line before the next `fn`
        last = first
        # shortcut: a `fn` line that holds a closed inner block is a one-line body; rustfmt never writes that.
        if not CLOSE.search(text.rstrip()):
            last = next((n for n in range(first + 1, stop + 1) if closing.fullmatch(lines[n - 1].rstrip())),
                        max(stop, first))
        found.append((name, first, last))
    return found


def git(repo: str, *args: str) -> str:
    """The output of `git -C repo ARGS`. A byte that is not UTF-8 in some tracked text file must
    not stop the check, so it is kept as a surrogate. GIT_DIFF_OPTS would beat `-U0`; the other
    `GIT_*` variables, such as GIT_DIR in a hook, stay."""
    env = {key: value for key, value in os.environ.items() if key != "GIT_DIFF_OPTS"}
    return subprocess.run(["git", "-C", repo, *args], check=True, capture_output=True, text=True,
                          encoding="utf-8", errors="surrogateescape", env=env).stdout


def hit(found: list[tuple[str, int, int]], start: int, count: int) -> set[str]:
    """The tests whose body meets the range of `count` lines from `start`; a count of 0 is empty.
    `found` is in order and its bodies do not overlap, so a bisect finds the first test to meet it."""
    end = start + count - 1
    found_at = bisect.bisect_left(found, start, key=lambda body: body[2])
    meeting = set()
    while count > 0 and found_at < len(found) and found[found_at][1] <= end:
        meeting.add(found[found_at][0])
        found_at += 1
    return meeting


# These flags make the output the same under any user config: no color, no external diff driver
# or text conversion, the `a/` and `b/` prefixes, and no merge of near hunks, which would add
# unchanged lines to a range.
DIFF = ("-c", "core.quotepath=off", "diff", "-U0", "-M", "-l10000", "--no-color", "--no-ext-diff",
        "--no-textconv", "--inter-hunk-context=0", "--src-prefix=a/",
        "--dst-prefix=b/")


def changed(ref: str, repo: str = ".") -> set[tuple[str, str]]:
    """The (new path, test name) pairs whose body `git diff -U0 -M -l10000 REF` changes, for `.rs`
    files only. A deleted test is left out.

    shortcut: `test_names.tests()` does not find a test that a macro writes (`fn $name`) or that
    libtest-mimic makes (`reference`). So it is never a changed test. Map a changed macro row to its
    test if a caller needs it.

    shortcut: a path that git quotes (it holds a quote, a backslash or a control character) keeps
    its quotes, so its file is not found. No such path is in the tree; read `-z` output if one is
    ever added."""
    diff = git(repo, *DIFF, ref).splitlines()
    found: set[tuple[str, str]] = set()
    old_path = new_path = ""
    old: list[tuple[str, int, int]] | None = None  # the bodies of the file at REF
    new: list[tuple[str, int, int]] = []  # the bodies of the file now
    new_names: set[str] = set()
    left = [0, 0]  # lines of the current hunk not yet read, on the old and the new side
    for line in diff:
        if left[0] or left[1]:  # a hunk line; one that starts `---` or `+++` is not a header
            left[line.startswith("+")] -= line[:1] in ("-", "+")
            continue
        if line.startswith("--- "):
            old_path = line[4:].split("\t")[0].removeprefix("a/") if line != "--- /dev/null" else ""
        elif line.startswith("+++ "):
            new_path = line[4:].split("\t")[0].removeprefix("b/") if line != "+++ /dev/null" else ""
            old = None
        elif m := HUNK.match(line):
            a, b, c, d = (int(x) if x is not None else 1 for x in m.groups())
            left = [b, d]  # for every file, so a `-- a` line of a text file is not a header
            if not new_path.endswith(".rs"):
                continue
            if old is None:  # the first hunk of the file
                text = git(repo, "show", f"{ref}:{old_path}") if old_path else ""
                old = bodies(text.splitlines())
                with open(f"{repo}/{new_path}", encoding="utf-8", errors="surrogateescape") as f:
                    new = bodies(f.read().splitlines())
                new_names = {name for name, _, _ in new}
            found |= {(new_path, n) for n in hit(old, a, b) & new_names}
            found |= {(new_path, n) for n in hit(new, c, d)}
    return found


if __name__ == "__main__":
    for path, name in sorted(changed(sys.argv[1])):
        print(f"{path}: {name}")
