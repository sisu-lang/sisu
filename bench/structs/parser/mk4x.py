#!/usr/bin/env python3
"""Write renamed copies of a Sisu source: mk4x.py IN OUT [COPIES].

Copy k adds the suffix `_k` to each top-level `fn`, `struct`, `enum`, `interface`, `class` and
`const` name, and to each variant of each enum. So no name clashes, and the parser accepts the
whole text. COPIES is 4 by default.
"""

import argparse
import re
from pathlib import Path

ITEM = re.compile(r"(fn|struct|enum|interface|class|const)\s+(\w+)")
VARIANT = re.compile(r"\s+(\w+)\s*$")


def top_names(src: str) -> set[str]:
    """Return the top-level item names and the enum variants of `src`."""
    names: set[str] = set()
    in_enum = False
    for line in src.split("\n"):
        item = ITEM.match(line)
        if item:
            names.add(item.group(2))
            in_enum = item.group(1) == "enum"
        elif in_enum:
            variant = VARIANT.match(line)
            if line.startswith("}"):
                in_enum = False
            elif variant:
                names.add(variant.group(1))
    return names


def copies(src: str, count: int) -> str:
    """Return `count` renamed copies of `src`, joined by line breaks."""
    names = sorted(top_names(src), key=len, reverse=True)
    word = re.compile(r"\b(" + "|".join(names) + r")\b")
    return "\n".join(word.sub(lambda m, k=k: f"{m.group(1)}_{k}", src) for k in range(1, count + 1))


def main() -> None:
    """Read IN, and write the copies to OUT."""
    parser = argparse.ArgumentParser(description="Write renamed copies of a Sisu source.")
    parser.add_argument("source", type=Path)
    parser.add_argument("out", type=Path)
    parser.add_argument("count", type=int, nargs="?", default=4)
    args = parser.parse_args()
    args.out.write_text(copies(args.source.read_text(), args.count))


if __name__ == "__main__":
    main()
