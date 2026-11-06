"""Unit test for fuzz_seeds.py: python3 -m unittest discover -s scripts -p 'test_*.py'.

The pin is FNV-1a 64 over the bytes of seed-0 to seed-127, in number order. It covers the bytes and
the order of all 128 seeds. `fuzz_seeds_should_match_the_pinned_values` in
crates/sisuc/src/generate/coverage.rs pins the same value. This test also finds its own value in
that file. A change to the pin on one side fails it until the other side follows.
"""

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).absolute().parent
SCRIPT = HERE / "fuzz_seeds.py"
RUST = HERE / ".." / "crates" / "sisuc" / "src" / "generate" / "coverage.rs"
PIN = 0x20E312F6E6AEC322
PIN_IN_RUST = f"{PIN:#_x}"


def fnv1a64(hash_value: int, data: bytes) -> int:
    for byte in data:
        hash_value = ((hash_value ^ byte) * 0x100000001B3) & ((1 << 64) - 1)
    return hash_value


class FuzzSeeds(unittest.TestCase):
    def test_seeds_should_match_the_pinned_values(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            subprocess.run([sys.executable, SCRIPT, directory], check=True)
            self.assertEqual(
                sorted(p.name for p in Path(directory).iterdir()),
                sorted(f"seed-{n}" for n in range(128)),
            )
            hash_value = 0xCBF29CE484222325
            for n in range(128):
                with Path(directory, f"seed-{n}").open("rb") as f:
                    data = f.read()
                self.assertEqual(len(data), 4096, f"the length of seed-{n}")
                hash_value = fnv1a64(hash_value, data)
            self.assertEqual(hash_value, PIN)
        with RUST.open(encoding="utf-8") as f:
            self.assertTrue(PIN_IN_RUST in f.read(), f"the pin {PIN_IN_RUST} in the Rust test")


if __name__ == "__main__":
    unittest.main()
