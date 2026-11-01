"""Rejects `{:?}` in diagnostic text, the code of `registry.rs` before its tests. Rust's `{:?}`
escapes a value the Rust way; diagnostic text quotes Sisu source.

Usage: python3 registry_debug_format.py FILE...
"""

import re
import sys

# The tests: an inline `#[cfg(test)] mod name {`, which clippy's items_after_test_module keeps at
# the end of the file. A `#[cfg(test)]` step counter before it is production code.
TESTS = re.compile(r"^#\[cfg\(test\)\]\n(?:#\[.*\n)*(?:pub(?:\([^)\n]*\))? )?mod \w+ \{", re.M)


def main(paths: list[str]) -> int:
    bad = 0
    for path in paths:
        with open(path, encoding="utf-8") as f:
            text = f.read()
        tests = TESTS.search(text)
        code = text[:tests.start()] if tests else text
        for n, line in enumerate(code.splitlines(), 1):
            if ":?}" in line:
                print(f"{path}:{n}: diagnostic text uses {{:?}}; escape the Sisu way")
                bad = 1
    return bad


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
