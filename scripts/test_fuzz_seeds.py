"""Unit test for fuzz_seeds.py: python3 -m unittest discover -s scripts -p 'test_*.py'.

The pin is FNV-1a 64 over the bytes of seed-0 to seed-127, in number order. It covers the bytes and
the order of all 128 seeds. `fuzz_seeds_should_match_the_pinned_values` in
crates/sisuc/src/generate/coverage.rs pins the same value. This test also finds its own value in
that file. A change to the pin on one side fails it until the other side follows.
"""
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, 'fuzz_seeds.py')
RUST = os.path.join(HERE, '..', 'crates', 'sisuc', 'src', 'generate', 'coverage.rs')
PIN = 0x20e312f6e6aec322
PIN_IN_RUST = f'{PIN:#_x}'


def fnv1a64(hash_value: int, data: bytes) -> int:
    for byte in data:
        hash_value = ((hash_value ^ byte) * 0x100000001b3) & ((1 << 64) - 1)
    return hash_value


class FuzzSeeds(unittest.TestCase):
    def test_seeds_should_match_the_pinned_values(self):
        with tempfile.TemporaryDirectory() as directory:
            subprocess.run([sys.executable, SCRIPT, directory], check=True)
            self.assertEqual(sorted(os.listdir(directory)), sorted(f'seed-{n}' for n in range(128)))
            hash_value = 0xcbf29ce484222325
            for n in range(128):
                with open(os.path.join(directory, f'seed-{n}'), 'rb') as f:
                    data = f.read()
                self.assertEqual(len(data), 4096, f'the length of seed-{n}')
                hash_value = fnv1a64(hash_value, data)
            self.assertEqual(hash_value, PIN)
        with open(RUST, encoding='utf-8') as f:
            self.assertTrue(PIN_IN_RUST in f.read(), f'the pin {PIN_IN_RUST} in the Rust test')


if __name__ == '__main__':
    unittest.main()
