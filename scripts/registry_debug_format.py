"""Rejects `{:?}` in diagnostic text, the code of `registry.rs` before its tests. Rust's `{:?}`
escapes a value the Rust way; diagnostic text quotes Sisu source.

Usage: python3 registry_debug_format.py FILE...
"""

import sys


def main(paths: list[str]) -> int:
    bad = 0
    for path in paths:
        with open(path, encoding="utf-8") as f:
            for n, line in enumerate(f, 1):
                if line.startswith("#[cfg(test)]"):
                    break
                if ":?}" in line:
                    print(f"{path}:{n}: diagnostic text uses {{:?}}; escape the Sisu way")
                    bad = 1
    return bad


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
