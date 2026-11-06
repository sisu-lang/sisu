"""Tests for nightly_check.py and nightly_smoke.sh: python3 -m unittest discover -s scripts -p 'test_nightly_check.py'."""
import os
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import unittest

import nightly_check as nc

SCRIPTS = os.path.dirname(os.path.abspath(__file__))
CHECK = os.path.join(SCRIPTS, 'nightly_check.py')
SMOKE = os.path.join(SCRIPTS, 'nightly_smoke.sh')
NAMES = ['sisu/sisu', 'sisu/libsisu_runtime_mimalloc.a', 'sisu/libsisu_runtime_system.a',
         'sisu/LICENSE', 'sisu/THIRD-PARTY-LICENSES.txt']

# Lines of `objdump -T` and `objdump -p` from the build machine, with their tabs.
STRONG = '0000000000000000      DF *UND*\t0000000000000000 (%s) f\n'
WEAK = '0000000000000000  w   DF *UND*\t0000000000000000 (%s) f\n'
NEEDED = '  NEEDED               %s\n'
FIXTURE_T = ('0000000000000000      DF *UND*\t0000000000000000 (GLIBC_2.34) __libc_start_main\n'
             '0000000000000000  w   DF *UND*\t0000000000000000 (GLIBC_2.39) pidfd_spawnp\n'
             '0000000000000000      DF *UND*\t0000000000000000 (GLIBCXX_3.4.21) '
             '_ZNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEE9_M_createERmm\n')
FIXTURE_P = NEEDED % 'libm.so.6'
OK_T = STRONG % 'GLIBC_2.34'
OK_P = NEEDED % 'libc.so.6'


def minos_text(cmd, line):
    return 'Load command 9\n      cmd %s\n  cmdsize 32\n%s\n' % (cmd, line)


class Versions(unittest.TestCase):
    def test_version_should_want_the_picked_commit(self):
        text = 'sisu 0.1.0\ncommit: %s\nhost: x\nllvm: 23.1.3\n'
        self.assertEqual(nc.version_problems(text % 'abc1234', '', 'abc1234def'), [])
        self.assertTrue(nc.version_problems(text % 'abc1235', '', 'abc1234def'))
        self.assertTrue(nc.version_problems(text % 'unknown', '', 'abc1234def'))
        self.assertTrue(nc.version_problems('sisu 0.1.0\nhost: x\n', '', 'abc1234def'))
        weak = "version 'GLIBC_2.39' not found\n"
        self.assertTrue(nc.version_problems(text % 'abc1234', weak, 'abc1234def'))


class Linux(unittest.TestCase):
    def test_linux_should_pass_at_the_floor(self):
        t = (STRONG % 'GLIBC_2.35') + (STRONG % 'GLIBC_2.2.5') + (STRONG % 'GLIBCXX_3.4.30')
        self.assertEqual(nc.linux_problems(t, FIXTURE_P), [])

    def test_linux_should_ignore_a_weak_glibc_reference(self):
        self.assertEqual(nc.linux_problems(FIXTURE_T, FIXTURE_P), [])
        self.assertEqual(nc.linux_problems(WEAK % 'GLIBC_2.39', FIXTURE_P), [])

    def test_linux_should_fail_above_the_glibc_floor(self):
        problems = nc.linux_problems(OK_T + STRONG % 'GLIBC_2.36', OK_P)
        self.assertTrue(any('2.36' in p for p in problems))
        self.assertTrue(nc.linux_problems(OK_T + STRONG % 'GLIBC_2.100', OK_P))

    def test_linux_should_fail_above_the_glibcxx_floor(self):
        self.assertTrue(nc.linux_problems(OK_T + STRONG % 'GLIBCXX_3.4.31', OK_P))
        self.assertTrue(nc.linux_problems(OK_T + STRONG % 'GLIBCXX_3.4.100', OK_P))

    def test_linux_should_fail_on_a_library_off_the_list(self):
        problems = nc.linux_problems(OK_T, OK_P + NEEDED % 'libfoo.so.1')
        self.assertTrue(any('libfoo.so.1' in p for p in problems))
        for name in ('libm.so.6', 'libz.so.1', 'libzstd.so.1', 'libstdc++.so.6',
                     'libgcc_s.so.1', 'libc.so.6', 'ld-linux-x86-64.so.2',
                     'ld-linux-aarch64.so.1'):
            self.assertEqual(nc.linux_problems(OK_T, NEEDED % name), [], name)

    def test_linux_should_fail_on_output_with_no_glibc_reference(self):
        self.assertTrue(nc.linux_problems('', ''))


class MacOS(unittest.TestCase):
    def test_macos_links_should_allow_only_system_paths(self):
        head = 'target/release/sisu:\n'
        lines = ('\t/usr/lib/libz.1.dylib (compatibility version 1.0.0, current version 1.3.1)\n'
                 '\t/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation (x)\n')
        self.assertEqual(nc.macos_link_problems(head + lines), [])
        bad = '\t/opt/homebrew/lib/libzstd.1.dylib (compatibility version 1.0.0)\n'
        problems = nc.macos_link_problems(head + lines + bad)
        self.assertTrue(any('/opt/homebrew/lib/libzstd.1.dylib' in p for p in problems))
        # The file's own name is not a load path, even when it is not a system path.
        self.assertEqual(nc.macos_link_problems('/opt/work/sisu:\n' + lines), [])

    def test_minos_should_fail_above_the_floor(self):
        build = 'LC_BUILD_VERSION'
        for line in ('    platform 1\n       minos 15.0\n         sdk 26.0\n    ntools 1\n'
                     '     tool 3\n  version 1100.0', '       minos 11.0'):
            self.assertEqual(nc.minos_problems('a.o', minos_text(build, line)), [], line)
        old = minos_text('LC_VERSION_MIN_MACOSX', '  version 11.0\n      sdk 11.0')
        self.assertEqual(nc.minos_problems('a.o', old), [])
        for minos in ('16.0', '15.1'):
            problems = nc.minos_problems('b.o', minos_text(build, '       minos ' + minos))
            self.assertTrue(any('b.o' in p for p in problems), minos)
        self.assertTrue(nc.minos_problems('c.o', 'Load command 0\n      cmd LC_SEGMENT_64\n'))


class Tar(unittest.TestCase):
    def test_tar_should_want_exact_names(self):
        self.assertEqual(nc.tar_problems(NAMES), [])
        self.assertTrue(nc.tar_problems(NAMES + ['sisu/._sisu']))
        self.assertTrue(nc.tar_problems([n for n in NAMES if n != 'sisu/LICENSE']))
        self.assertTrue(nc.tar_problems(NAMES + ['sisu/extra']))


class Pack(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(prefix='sisu-pack-')
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)
        self.release = os.path.join(self.root, 'release')
        self.out = os.path.join(self.root, 'out', 'dist')
        self.third = os.path.join(self.root, 'third-party.txt')
        os.makedirs(self.release)
        for name in ('sisu', 'libsisu_runtime_mimalloc.a', 'libsisu_runtime_system.a'):
            self.write(os.path.join(self.release, name), 'content of ' + name)
        os.chmod(os.path.join(self.release, 'sisu'), 0o755)
        self.write(self.third, 'notices')

    def write(self, path, text):
        with open(path, 'w') as f:
            f.write(text)

    def pack(self):
        return subprocess.run([sys.executable, CHECK, 'pack', 'x86_64-linux', self.release,
                               self.third, self.out], capture_output=True, text=True)

    def test_pack_should_write_the_exact_tarball(self):
        done = self.pack()
        self.assertEqual(done.returncode, 0, done.stderr)
        with tarfile.open(os.path.join(self.out, 'sisu-x86_64-linux.tar.gz')) as tar:
            self.assertEqual(sorted(tar.getnames()), sorted(NAMES))
            self.assertEqual(tar.extractfile('sisu/sisu').read(), b'content of sisu')
            self.assertEqual(tar.extractfile('sisu/THIRD-PARTY-LICENSES.txt').read(), b'notices')
            self.assertTrue(tar.getmember('sisu/sisu').mode & stat.S_IXUSR)
            with open(os.path.join(SCRIPTS, '..', 'LICENSE'), 'rb') as f:
                self.assertEqual(tar.extractfile('sisu/LICENSE').read(), f.read())

    def test_pack_should_fail_on_a_missing_file(self):
        os.remove(os.path.join(self.release, 'libsisu_runtime_system.a'))
        done = self.pack()
        self.assertEqual(done.returncode, 1)
        self.assertIn('libsisu_runtime_system.a', done.stderr)
        self.assertEqual([n for n in os.listdir(self.out) if n.endswith('.tar.gz')], [])


class Smoke(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(prefix='sisu-smoke-')
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)
        self.bin = os.path.join(self.root, 'bin')
        self.programs = os.path.join(self.root, 'programs')
        self.log = os.path.join(self.root, 'log')
        os.makedirs(self.bin)
        os.makedirs(self.programs)

    def fake(self, body='cat "${4%.sisu}.out"'):
        path = os.path.join(self.bin, 'sisu')
        with open(path, 'w') as f:
            f.write('#!/bin/sh\necho "$@" >> "$LOG"\n' + body + '\n')
        os.chmod(path, 0o755)

    def program(self, name, out='hello\n'):
        with open(os.path.join(self.programs, name + '.sisu'), 'w') as f:
            f.write('fn main() {}\n')
        if out is not None:
            with open(os.path.join(self.programs, name + '.out'), 'w') as f:
                f.write(out)

    def smoke(self):
        env = dict(os.environ, LOG=self.log)
        return subprocess.run(['bash', SMOKE, self.bin, self.programs], env=env,
                              capture_output=True, text=True)

    def calls(self):
        if not os.path.exists(self.log):
            return []
        with open(self.log) as f:
            return f.read().splitlines()

    def test_smoke_should_pass_when_each_program_prints_its_output(self):
        self.fake()
        self.program('a')
        self.program('b', 'two\nlines\n')
        done = self.smoke()
        self.assertEqual(done.returncode, 0, done.stderr)
        for name in ('a', 'b'):
            path = os.path.join(self.programs, name + '.sisu')
            for allocator in ('mimalloc', 'system'):
                self.assertIn('run --allocator %s %s' % (allocator, path), self.calls())

    def test_smoke_should_run_only_programs_with_an_out_file(self):
        self.fake()
        self.program('with_out')
        self.program('no_out', None)
        done = self.smoke()
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertTrue(self.calls())
        self.assertFalse(any('no_out' in c for c in self.calls()))

    def test_smoke_should_fail_on_a_wrong_output(self):
        self.fake('if [ "$3" = system ]; then echo wrong; else cat "${4%.sisu}.out"; fi')
        self.program('prog')
        done = self.smoke()
        self.assertEqual(done.returncode, 1)
        self.assertIn('prog.sisu', done.stderr)
        self.assertIn('system', done.stderr)

    def test_smoke_should_fail_on_a_stderr_line(self):
        self.fake('cat "${4%.sisu}.out"; echo warning >&2')
        self.program('prog')
        self.assertEqual(self.smoke().returncode, 1)

    def test_smoke_should_fail_on_a_non_zero_exit(self):
        self.fake('cat "${4%.sisu}.out"; exit 3')
        self.program('prog')
        self.assertEqual(self.smoke().returncode, 1)

    def test_smoke_should_fail_with_no_programs(self):
        self.fake()
        self.assertEqual(self.smoke().returncode, 1)


if __name__ == '__main__':
    unittest.main()
