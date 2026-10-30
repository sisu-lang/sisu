"""Unit tests for review-sisu's build_sisu.sh: python3 -m unittest discover -s scripts -p 'test_*.py'.

Each test runs the script on a small repository with a fake `cargo` on PATH, so nothing compiles.
"""
import os
import shutil
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / '.agents/skills/review-sisu/scripts/build_sisu.sh'
# A pre-commit hook runs these tests with GIT_DIR and its kin set to the outer repository.
ENV = {k: v for k, v in os.environ.items() if not k.startswith('GIT_')}
# The fake cargo counts its runs and writes a sisu and the two runtime archives that name the
# exported source, so a test sees which commit each build came from.
FAKE_CARGO = """#!/usr/bin/env bash
echo run >>"$COUNT"
mkdir -p "$CARGO_TARGET_DIR/debug"
for f in sisu libsisu_runtime_mimalloc.a libsisu_runtime_system.a; do cp version "$CARGO_TARGET_DIR/debug/$f"; done
chmod +x "$CARGO_TARGET_DIR/debug/sisu"
"""


class BuildSisu(unittest.TestCase):
    def setUp(self):
        self.repo = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.repo)
        self.run_dir = self.repo / 'run'
        (self.repo / '.llvm').mkdir()
        bin_dir = self.repo / 'fake-bin'
        bin_dir.mkdir()
        cargo = bin_dir / 'cargo'
        cargo.write_text(FAKE_CARGO)
        cargo.chmod(cargo.stat().st_mode | stat.S_IXUSR)
        self.env = dict(ENV, PATH=f'{bin_dir}:{ENV["PATH"]}', COUNT=str(self.repo / 'count'),
                        GIT_COMMITTER_DATE='2001-01-01T00:00:00')
        self.git = ['git', '-C', str(self.repo), '-c', 'user.name=t', '-c', 'user.email=t@t']
        subprocess.run(self.git[:3] + ['init', '-q'], check=True, env=ENV)
        (self.repo / '.gitignore').write_text('.llvm\nfake-bin\nrun\ncount\n')
        for version in ('one', 'two'):
            (self.repo / 'version').write_text(f'{version}\n')
            subprocess.run(self.git + ['add', '-A'], check=True, env=ENV)
            subprocess.run(self.git + ['commit', '-q', '-m', version], check=True, env=self.env)

    def build(self, *refs):
        return subprocess.run(['bash', str(SCRIPT), str(self.repo), str(self.run_dir), *refs],
                              capture_output=True, text=True, check=True, env=self.env).stdout.split()

    def sha(self, ref):
        return subprocess.run(self.git[:3] + ['rev-parse', ref], capture_output=True, text=True,
                              check=True, env=ENV).stdout.strip()

    def runs(self):
        return len((self.repo / 'count').read_text().splitlines())

    def test_each_commit_should_get_its_own_sisu_and_archives(self):
        paths = self.build('HEAD~1', 'HEAD')
        self.assertEqual(paths, [str(self.run_dir / 'bin' / self.sha(r) / 'sisu') for r in ('HEAD~1', 'HEAD')])
        for path, version in zip(paths, ('one', 'two')):
            for name in ('sisu', 'libsisu_runtime_mimalloc.a', 'libsisu_runtime_system.a'):
                self.assertEqual((Path(path).parent / name).read_text(), f'{version}\n')

    def test_a_commit_built_before_should_not_build_again(self):
        self.build('HEAD')
        self.build('HEAD', 'HEAD')
        self.assertEqual(self.runs(), 1)

    def test_the_export_should_be_newer_than_the_commit(self):
        # git archive dates each file 2001 here, which cargo would take as older than its last build.
        self.build('HEAD')
        self.assertGreater((self.run_dir / 'src' / 'version').stat().st_mtime, 978307200 + 86400)


if __name__ == '__main__':
    unittest.main()
