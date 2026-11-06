"""Reject a `//` comment line over 100 columns.

rustfmt breaks code at 100 columns where it can, but on stable it does not wrap comments
(`wrap_comments` is unstable).

Usage: python3 comment_width.py FILE...
"""

import sys
from pathlib import Path

WIDTH = 100


def main(paths: list[str]) -> int:
    """Print each comment line over `WIDTH` columns in the files `paths`; return the exit code."""
    bad = 0
    for path in paths:
        with Path(path).open(encoding="utf-8") as f:
            for n, raw in enumerate(f, 1):
                line = raw.rstrip("\n")
                if line.lstrip().startswith("//") and len(line) > WIDTH:
                    print(f"{path}:{n}: comment is {len(line)} columns; wrap it at {WIDTH}")
                    bad = 1
    return bad


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
