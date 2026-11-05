"""Unit test for fuzz_seeds.py: python3 -m unittest discover -s scripts -p 'test_*.py'.

The test pins the same values as `fuzz_seeds_should_match_the_pinned_values` in
crates/sisuc/src/generate/coverage.rs, so a change to the rule in one file fails both tests.
"""
import os
import struct
import subprocess
import sys
import tempfile
import unittest

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'fuzz_seeds.py')


def xor_of_words(data: bytes) -> int:
    result = 0
    for word in struct.unpack('<512Q', data):
        result ^= word
    return result


class FuzzSeeds(unittest.TestCase):
    def test_seeds_should_match_the_pinned_values(self):
        with tempfile.TemporaryDirectory() as directory:
            subprocess.run([sys.executable, SCRIPT, directory], check=True)
            names = sorted(os.listdir(directory))
            self.assertEqual(names, sorted(f'seed-{n}' for n in range(128)))
            for n, xor in [(0, 0xa24fddd8729255f9), (127, 0x3d7eca94fe32c652)]:
                with open(os.path.join(directory, f'seed-{n}'), 'rb') as f:
                    data = f.read()
                self.assertEqual(len(data), 4096, f'the length of seed-{n}')
                self.assertEqual(xor_of_words(data), xor, f'the XOR of the words of seed-{n}')
            for name in names:
                self.assertEqual(os.path.getsize(os.path.join(directory, name)), 4096, name)


if __name__ == '__main__':
    unittest.main()
