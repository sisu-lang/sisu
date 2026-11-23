"""Reject a `#[cfg(test)]` module or function that branch coverage still counts.

The nightly `Coverage` job gates on the branch coverage of product code. A test module counts
too unless it has `#[cfg_attr(coverage_nightly, coverage(off))]`. The failing side of each of
its asserts is a branch that no passing test takes.

Usage: python3 coverage_off.py FILE...
"""

import re
import sys
from pathlib import Path

ATTR = "#[cfg_attr(coverage_nightly, coverage(off))]"
ITEM = re.compile(r"(pub(\([^)]*\))? )?(const )?(unsafe )?(mod|fn) ")


def item_after(lines: list[str], start: int) -> tuple[int, bool]:
    """Return the index of the item after the attribute at `start`, and whether `ATTR` is on it.

    Blank lines, `//` comments and attributes, also ones that rustfmt splits over lines, come
    between.
    """
    marked = False
    i = start + 1
    while i < len(lines):
        line = lines[i].strip()
        if not line or line.startswith("//"):
            i += 1
        elif line.startswith("#["):
            depth = 0
            while i < len(lines):
                depth += lines[i].count("[") - lines[i].count("]")
                marked = marked or ATTR in lines[i]
                i += 1
                if depth <= 0:
                    break
        else:
            break
    return i, marked


def main(paths: list[str]) -> int:
    """Print each unmarked `#[cfg(test)]` module or function in `paths`; return the exit code."""
    bad = 0
    for path in paths:
        lines = Path(path).read_text(encoding="utf-8").split("\n")
        for n, line in enumerate(lines):
            if line.strip() != "#[cfg(test)]":
                continue
            at, marked = item_after(lines, n)
            if at < len(lines) and ITEM.match(lines[at].strip()) and not marked:
                print(f"{path}:{at + 1}: test-only item needs {ATTR} after #[cfg(test)]")
                bad = 1
    return bad


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
