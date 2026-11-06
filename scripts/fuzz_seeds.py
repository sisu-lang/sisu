"""Writes the seeds of a fuzz run of the program target: the files seed-0 to seed-127, each of
4096 bytes, into a directory. scripts/fuzz.sh passes that directory to the fuzzer as a corpus.

`generate::seed` in crates/sisuc/src/generate/coverage.rs owns the rule, and this file is a copy,
because a shell step cannot call a test-only function. `FUZZ_SEEDS` there is the number of seeds.
A seed is 512 outputs of splitmix64, with the seed's number as the start state, each written as
a little-endian 64-bit word. One pin covers the bytes and the order of all 128 seeds: a hash of
them, kept in the Rust test and in scripts/test_fuzz_seeds.py. The Python test holds the two
sides to that one value. A change to the rule in one place fails a test until the other follows.

Usage: python3 fuzz_seeds.py <directory>. The directory must exist.
"""

import os
import struct
import sys

SEEDS = 128
SEED_WORDS = 512
MASK = (1 << 64) - 1


def splitmix64(state: int) -> tuple[int, int]:
    """The next state and the next output."""
    state = (state + 0x9E3779B97F4A7C15) & MASK
    z = state
    z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & MASK
    z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & MASK
    return state, z ^ (z >> 31)


def seed(n: int) -> bytes:
    state = n
    words = []
    for _ in range(SEED_WORDS):
        state, word = splitmix64(state)
        words.append(word)
    return struct.pack(f"<{SEED_WORDS}Q", *words)


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print("usage: fuzz_seeds.py <directory>", file=sys.stderr)
        return 2
    for n in range(SEEDS):
        with open(os.path.join(argv[1], f"seed-{n}"), "wb") as f:
            f.write(seed(n))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
