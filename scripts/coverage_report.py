"""Report the uncovered regions and missed branches that a change touches.

The script turns a cargo-llvm-cov export, its LCOV file and a git diff into the report.
The branch list appears only when the export holds branch data (a `--branch` run).

Usage: python3 coverage_report.py COVERAGE.json LCOV.info CHANGES.diff
Exits 0 for any valid report, however low the coverage, and 1 when an input is unusable.
"""

import json
import re
import subprocess
import sys
from collections.abc import Callable
from pathlib import Path
from typing import Any, NamedTuple

# Integration tests, and unit-test modules in their own file (src/**/tests.rs, src/**/tests/).
TEST_DIR = re.compile(r"^crates/[^/]+/(?:tests/|src/(?:.+/)?tests(?:\.rs$|/))")
# A panic that states an invariant, which no test should reach.
INVARIANT = re.compile(r"\b(?:unreachable|panic)!\(")
# An assert's message arguments run only when it fails, so on a line that ran they stay at zero.
ASSERT = re.compile(r"\b(?:debug_)?assert(?:_eq|_ne)?!\(")
# What `unmeasured` walks back over: the longest statement it follows, in lines.
CONTINUATION = 30
# A string literal or a `//` comment, which `test_module_lines` strips before it counts braces.
BRACE_NOISE = re.compile(r'"(?:\\.|[^"\\])*"|//.*')
TEST_MOD = re.compile(r"^(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{")
HUNK = re.compile(r"^@@ -\d+(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")


class Region(NamedTuple):
    """A span of source that has a zero count, or a function that never ran."""

    start: int
    start_col: int
    end: int
    end_col: int
    never_run: bool


def changed_lines(diff: str) -> dict[str, set[int]]:
    """Return the new-side line numbers of added lines, per Rust file that is not test code.

    Test code is what `TEST_DIR` matches.
    """
    result: dict[str, set[int]] = {}
    path = None
    line = old_left = new_left = 0
    for text in diff.splitlines():
        if old_left > 0 or new_left > 0:  # inside a hunk, "+++ " is an added line
            if text.startswith("+"):
                if path:
                    result.setdefault(path, set()).add(line)
                line += 1
                new_left -= 1
            elif text.startswith("-"):
                old_left -= 1
            elif text.startswith(" "):
                line += 1
                old_left -= 1
                new_left -= 1
        elif text.startswith("+++ "):
            name = text[4:].removeprefix("b/")
            ok = name.endswith(".rs") and not TEST_DIR.match(name)
            path = name if ok else None
        elif text.startswith("diff --git "):
            path = None
        elif m := HUNK.match(text):
            old_left = int(1 if m[1] is None else m[1])
            line = int(m[2])
            new_left = int(1 if m[3] is None else m[3])
    return result


def relative(path: str, root: str) -> str | None:
    """Return `path` relative to `root`, or None when `path` is not under `root`."""
    return path[len(root) + 1 :] if path.startswith(root + "/") else None


def uncovered(export: dict[str, Any], root: str) -> dict[str, list[Region]]:
    """Return the zero-count regions per file; a function that never ran is one never_run region."""
    # A source function is its file plus the span of its first region; its instantiations
    # have different mangled names, so they are grouped by that key and merged span by span.
    functions: dict[
        tuple[str, tuple[int, ...]], tuple[int, dict[tuple[str, int, int, int, int], int]]
    ] = {}
    for record in export["data"][0]["functions"]:
        regions = [r for r in record["regions"] if r[7] == 0]
        if not regions:
            continue
        names = record["filenames"]
        first = regions[0]
        count, spans = functions.setdefault((names[first[5]], tuple(first[:4])), (0, {}))
        functions[(names[first[5]], tuple(first[:4]))] = (max(count, record["count"]), spans)
        for r in regions:
            key = (names[r[5]], *r[:4])
            spans[key] = max(spans.get(key, 0), r[4])
    result: dict[str, list[Region]] = {}
    for (name, _first), (count, spans) in functions.items():
        if count == 0:
            path = relative(name, root)
            if path is None:
                continue
            # The head region alone often misses the body, so the function spans all its regions.
            own = [k for k in spans if k[0] == name]
            top = min(own, key=lambda k: k[1:])
            result.setdefault(path, []).append(
                Region(top[1], top[2], max(k[3] for k in own), 0, never_run=True)
            )
            continue
        for (file, *span), n in spans.items():
            if n == 0 and (path := relative(file, root)) is not None:
                result.setdefault(path, []).append(Region(*span, never_run=False))
    return {p: sorted(rs) for p, rs in result.items()}


def line_hits(lcov: str, root: str) -> dict[str, dict[int, int]]:
    """Return the hit count of each instrumented line, per file under `root`, from LCOV text."""
    result: dict[str, dict[int, int]] = {}
    hits: dict[int, int] = {}
    for text in lcov.splitlines():
        if text.startswith("SF:"):
            path = relative(text[3:], root)
            hits = result.setdefault(path, {}) if path else {}
        elif text.startswith("DA:"):
            number, count = text[3:].split(",")[:2]
            hits[int(number)] = hits.get(int(number), 0) + int(count)
    return result


def branch_misses(lcov: str, root: str) -> dict[str, dict[int, tuple[int, int]]]:
    """Return (missed, total) branch outcomes per line with a missed one, per file under `root`.

    A `BRDA:<line>,<block>,<branch>,<taken>` record is one outcome; it is missed when `taken`
    is `-` (the branch was never reached) or 0.
    """
    result: dict[str, dict[int, tuple[int, int]]] = {}
    seen: dict[int, tuple[int, int]] = {}
    for text in lcov.splitlines():
        if text.startswith("SF:"):
            path = relative(text[3:], root)
            seen = {}
            if path:
                result[path] = seen
        elif text.startswith("BRDA:"):
            number, _block, _branch, taken = text[5:].split(",")
            missed, total = seen.get(int(number), (0, 0))
            seen[int(number)] = (missed + (taken in ("-", "0")), total + 1)
    return {path: {n: mt for n, mt in lines.items() if mt[0] > 0} for path, lines in result.items()}


def report(
    export: dict[str, Any], lcov: str, diff: str, root: str, read_line: Callable[[str, int], str]
) -> str:
    """Return the figures, the uncovered regions in changed files, then the figures again."""
    # The figures come last too: a `make coverage` log is long, and its end is what a reader sees.
    totals = export["data"][0]["totals"]
    changed = changed_lines(diff)
    hits = line_hits(lcov, root)
    instrumented = [h[n] for p, ns in changed.items() for n in ns if n in (h := hits.get(p, {}))]
    if instrumented:
        covered = sum(1 for h in instrumented if h > 0)
        pct = (
            f"changed lines {100 * covered / len(instrumented):.1f}% "
            f"({covered} of {len(instrumented)})"
        )
    else:
        pct = "changed lines: none"
    branches = totals.get("branches", {})
    measured = branches.get("count", 0) > 0
    branch_pct = f"branches {branches['percent']:.1f}%, " if measured else ""
    head = (
        f"Coverage: lines {totals['lines']['percent']:.1f}%, "
        f"regions {totals['regions']['percent']:.1f}%, {branch_pct}{pct}."
    )

    touching: list[str] = []
    other: list[str] = []
    for path, regions in sorted(uncovered(export, root).items()):
        if path not in changed:
            continue
        for r in regions:
            line = read_line(path, r.start)
            source = line.strip()[:80]
            if r.never_run:
                text = f"{path}:{r.start}: never run: {source}"
            else:
                text = f"{path}:{r.start}:{r.start_col}-{r.end}:{r.end_col}: {source}"
            hit = any(
                n in changed[path] for n in range(r.start, r.end + 1)
            ) and not INVARIANT.search(line)
            if not r.never_run and ASSERT.search(line) and hits.get(path, {}).get(r.start, 0) > 0:
                hit = False
            (touching if hit else other).append(text)
    text = (
        f"{head}\n\nUncovered regions touching changed code:\n"
        + "\n".join(touching or ["(none)"])
        + "\n\nOther uncovered regions in changed files (advisory):\n"
        + "\n".join(other or ["(none)"])
    )
    if measured:
        text += "\n\nMissed branches on changed lines:\n" + "\n".join(
            missed_branches(lcov, changed, root, read_line) or ["(none)"]
        )
    return f"{text}\n\n{head}"


def test_module_lines(path: str, last: int, read_line: Callable[[str, int], str]) -> set[int]:
    """Return the lines up to `last` that an inline `#[cfg(test)] mod name {` encloses.

    shortcut: braces are counted after a rough strip of strings and `//` comments. A raw
    string or a char literal with a brace can end a module early. Use `syn` if that happens.
    """
    result: set[int] = set()
    armed = False
    depth = 0
    for number in range(1, last + 1):
        text = read_line(path, number).strip()
        code = BRACE_NOISE.sub("", text)
        if depth > 0:
            result.add(number)
            depth += code.count("{") - code.count("}")
        elif text.startswith("#[cfg(test)]"):
            armed = True
        elif armed and TEST_MOD.match(text):
            result.add(number)
            depth = code.count("{") - code.count("}")
            armed = False
        elif armed and not text.startswith(("#[", "//")):
            armed = False
    return result


def unmeasured(path: str, number: int, read_line: Callable[[str, int], str]) -> bool:
    """Return whether line `number` is one whose branches no test should take.

    That is an assert, or a line that an assert spans, or a `let ... else {` whose body is
    an invariant panic, as in the region lists.
    """
    line = read_line(path, number)
    if ASSERT.search(line) or INVARIANT.search(line):
        return True
    if line.rstrip().endswith("else {") and INVARIANT.search(read_line(path, number + 1)):
        return True
    # A statement goes on while the lines before it end in neither `;` nor a brace.
    for before in range(number - 1, max(number - CONTINUATION, 0), -1):
        text = read_line(path, before).rstrip()
        if not text or text.endswith((";", "{", "}")):
            return False
        if ASSERT.search(text):
            return True
    return False


def missed_branches(
    lcov: str, changed: dict[str, set[int]], root: str, read_line: Callable[[str, int], str]
) -> list[str]:
    """Return one line per changed line that has a missed branch outcome.

    The list leaves out what `unmeasured` names and the lines of inline test modules. The
    region lists keep their own rules.
    """
    result = []
    for path, lines in sorted(branch_misses(lcov, root).items()):
        wanted = [n for n in sorted(lines) if n in changed.get(path, ())]
        if not wanted:
            continue
        in_tests = test_module_lines(path, wanted[-1], read_line)
        for number in wanted:
            if number in in_tests or unmeasured(path, number, read_line):
                continue
            missed, total = lines[number]
            source = read_line(path, number).strip()[:80]
            result.append(f"{path}:{number}: missed {missed} of {total}: {source}")
    return result


def main(argv: list[str]) -> int:
    """Print the report for the three files in `argv`, and return the exit code."""
    cache: dict[str, list[str]] = {}

    def read_line(path: str, number: int) -> str:
        if path not in cache:
            try:
                cache[path] = Path(f"{root}/{path}").read_text(errors="replace").splitlines()
            except OSError:
                cache[path] = []
        lines = cache[path]
        return lines[number - 1] if 0 < number <= len(lines) else ""

    try:
        if len(argv) != 3:  # noqa: PLR2004  the three paths of the usage line
            raise ValueError(  # noqa: TRY301  the handler below prints the message and returns 1
                "usage: coverage_report.py COVERAGE.json LCOV.info CHANGES.diff"
            )
        texts = [Path(path).read_text(errors="replace") for path in argv]
        export = json.loads(texts[0])
        data = export["data"][0]
        for kind in ("lines", "regions"):
            if not isinstance(data["totals"][kind]["percent"], (int, float)):
                raise ValueError(  # noqa: TRY004, TRY301  the handler of this `try` prints the message, and TypeError would change the type name that it prints
                    f"totals.{kind}.percent is not a number"
                )
        if not isinstance(data["functions"], list):
            raise ValueError("functions is not a list")  # noqa: TRY004, TRY301  as for `percent`: the handler prints the message, and TypeError would change the type name
        root = subprocess.run(
            ["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True
        ).stdout.strip()
        text = report(export, texts[1], texts[2], root, read_line)
    except (
        OSError,
        ValueError,
        LookupError,
        TypeError,
        AttributeError,
        subprocess.CalledProcessError,
    ) as e:
        reason = " ".join(f"{type(e).__name__}: {e}".split())
        print(f"coverage_report: {reason}", file=sys.stderr)
        return 1
    print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
