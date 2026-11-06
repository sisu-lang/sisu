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
NAMES = ['sisu/', 'sisu/sisu', 'sisu/libsisu_runtime_mimalloc.a', 'sisu/libsisu_runtime_system.a',
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
        sha = 'abc1234def'
        self.assertEqual(nc.version_problems(text % 'abc1234', '', sha), [])
        weak = "version 'GLIBC_2.39' not found\n"
        for name, out, err in (('another commit', text % 'abc1235', ''),
                               ('unknown commit', text % 'unknown', ''),
                               ('no commit line', 'sisu 0.1.0\nhost: x\n', ''),
                               ('stderr', text % 'abc1234', weak)):
            with self.subTest(name):
                self.assertTrue(nc.version_problems(out, err, sha))


class Linux(unittest.TestCase):
    def test_linux_should_pass_at_the_floor(self):
        t = (STRONG % 'GLIBC_2.35') + (STRONG % 'GLIBC_2.2.5') + (STRONG % 'GLIBCXX_3.4.30')
        self.assertEqual(nc.linux_problems(t, FIXTURE_P), [])

    def test_linux_should_ignore_a_weak_glibc_reference(self):
        self.assertEqual(nc.linux_problems(FIXTURE_T, FIXTURE_P), [])
        self.assertEqual(nc.linux_problems(WEAK % 'GLIBC_2.39', FIXTURE_P), [])

    def test_linux_should_fail_above_the_glibc_floor(self):
        problems = nc.linux_problems(OK_T + STRONG % 'GLIBC_2.36', OK_P)
        self.assertTrue(any('2.36' in p for p in problems), problems)
        self.assertTrue(nc.linux_problems(OK_T + STRONG % 'GLIBC_2.100', OK_P), 'GLIBC_2.100')

    def test_linux_should_fail_above_the_glibcxx_floor(self):
        for version in ('GLIBCXX_3.4.31', 'GLIBCXX_3.4.100'):
            with self.subTest(version):
                self.assertTrue(nc.linux_problems(OK_T + STRONG % version, OK_P))

    def test_linux_should_fail_on_a_library_off_the_list(self):
        problems = nc.linux_problems(OK_T, OK_P + NEEDED % 'libfoo.so.1')
        self.assertTrue(any('libfoo.so.1' in p for p in problems), problems)
        for name in ('libm.so.6', 'libz.so.1', 'libzstd.so.1', 'libstdc++.so.6',
                     'libgcc_s.so.1', 'libc.so.6', 'ld-linux-x86-64.so.2',
                     'ld-linux-aarch64.so.1'):
            with self.subTest(name):
                self.assertEqual(nc.linux_problems(OK_T, NEEDED % name), [])
        for name in ('libzstd.so.2', 'libc.so.999', 'ld-linux-evil.so.9'):
            with self.subTest(name):
                self.assertTrue(nc.linux_problems(OK_T, NEEDED % name))

    def test_linux_should_fail_on_output_with_no_glibc_reference(self):
        self.assertTrue(nc.linux_problems('', OK_P))

    def test_linux_should_fail_on_output_with_no_needed_line(self):
        self.assertTrue(nc.linux_problems(OK_T, ''))


class MacOS(unittest.TestCase):
    def test_macos_links_should_allow_only_system_paths(self):
        head = 'target/release/sisu:\n'
        lines = ('\t/usr/lib/libz.1.dylib (compatibility version 1.0.0, current version 1.3.1)\n'
                 '\t/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation (x)\n')
        self.assertEqual(nc.macos_link_problems(head + lines), [])
        bad = '\t/opt/homebrew/lib/libzstd.1.dylib (compatibility version 1.0.0)\n'
        problems = nc.macos_link_problems(head + lines + bad)
        self.assertTrue(any('/opt/homebrew/lib/libzstd.1.dylib' in p for p in problems), problems)
        # The file's own name is not a load path, even when it is not a system path.
        self.assertEqual(nc.macos_link_problems('/opt/work/sisu:\n' + lines), [])
        # A tool that prints no load path cannot pass.
        for name, out in (('empty', ''), ('header only', 'x:\n')):
            with self.subTest(name):
                self.assertTrue(nc.macos_link_problems(out))

    def test_minos_should_fail_above_the_floor(self):
        build = 'LC_BUILD_VERSION'
        for line in ('    platform 1\n       minos 15.0\n         sdk 26.0\n    ntools 1\n'
                     '     tool 3\n  version 1100.0', '       minos 11.0', '       minos 9.0'):
            with self.subTest(line):
                self.assertEqual(nc.minos_problems('a.o', minos_text(build, line)), [])
        old = minos_text('LC_VERSION_MIN_MACOSX', '  version 11.0\n      sdk 11.0')
        self.assertEqual(nc.minos_problems('a.o', old), [])
        for minos in ('16.0', '15.1', '100.0'):
            with self.subTest(minos):
                problems = nc.minos_problems('b.o', minos_text(build, '       minos ' + minos))
                self.assertTrue(any('b.o' in p for p in problems), problems)
        self.assertTrue(nc.minos_problems('c.o', 'Load command 0\n      cmd LC_SEGMENT_64\n'))


class Tar(unittest.TestCase):
    def test_tar_should_want_exact_names(self):
        self.assertEqual(nc.tar_problems(NAMES), [])
        for name, names in (('._ file', NAMES + ['sisu/._sisu']),
                            ('no LICENSE', [n for n in NAMES if n != 'sisu/LICENSE']),
                            ('no directory', [n for n in NAMES if n != 'sisu/']),
                            ('extra', NAMES + ['sisu/extra']),
                            ('duplicate', NAMES + ['sisu/sisu'])):
            with self.subTest(name):
                self.assertTrue(nc.tar_problems(names))


class PackBase(unittest.TestCase):
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

    def pack(self, target='x86_64-linux', env=None):
        return subprocess.run([sys.executable, CHECK, 'pack', target, self.release,
                               self.third, self.out], capture_output=True, text=True, env=env)


class Pack(PackBase):
    def test_pack_should_write_the_exact_tarball(self):
        done = self.pack()
        self.assertEqual(done.returncode, 0, done.stderr)
        with tarfile.open(os.path.join(self.out, 'sisu-x86_64-linux.tar.gz')) as tar:
            self.assertEqual(sorted(tar.getnames()), sorted(n.rstrip('/') for n in NAMES))
            for name in ('sisu', 'libsisu_runtime_mimalloc.a', 'libsisu_runtime_system.a'):
                self.assertEqual(tar.extractfile('sisu/' + name).read(),
                                 ('content of ' + name).encode(), name)
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

    def test_pack_should_fail_on_an_unknown_target(self):
        done = self.pack('riscv-linux')
        self.assertEqual(done.returncode, 1)
        self.assertIn('riscv-linux', done.stderr)


def fake_tool(directory, name, body):
    path = os.path.join(directory, name)
    with open(path, 'w') as f:
        f.write('#!/bin/sh\necho "%s $*" >> "$LOG"\n%s\n' % (name, body))
    os.chmod(path, 0o755)


def breaks(call):
    """Shell for a fake: fail with a message when $FAIL names this call."""
    return 'if [ "$FAIL" = %s ]; then echo "%s broke" >&2; exit 1; fi' % (call, call)


class ToolBase(unittest.TestCase):
    """Runs nightly_check.py with fake tools first on PATH. A fake fails when $FAIL names its call."""

    def setUp(self):
        self.root = tempfile.mkdtemp(prefix='sisu-tools-')
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)
        self.bin = os.path.join(self.root, 'bin')
        self.log = os.path.join(self.root, 'log')
        os.makedirs(self.bin)

    def check(self, *args, fail='', path=None, **env):
        path = path if path is not None else self.bin + os.pathsep + os.environ['PATH']
        env = dict(os.environ, PATH=path, LOG=self.log, FAIL=fail, **env)
        return subprocess.run([sys.executable, CHECK, *args], capture_output=True, text=True,
                              env=env)

    def calls(self):
        if not os.path.exists(self.log):
            return []
        with open(self.log) as f:
            return f.read().splitlines()


class MacOSTools(ToolBase):
    def setUp(self):
        super().setUp()
        fake_tool(self.bin, 'otool', r'''
case "$1" in
-L) %s
    printf 'sisu:\n\t%%s (compatibility version 1.0.0)\n' "${LOAD:-/usr/lib/libz.1.dylib}";;
-l) %s
    printf '      cmd LC_BUILD_VERSION\n     minos %%s\n' "${MINOS:-15.0}";;
esac''' % (breaks('otool-L'), breaks('otool-l')))
        fake_tool(self.bin, 'codesign', breaks('codesign'))
        fake_tool(self.bin, 'ar', r'''
case "$1" in
-t) %s
    printf '%%s\n' "__.SYMDEF SORTED" "a.native";;
-x) %s
    : > "$3";;
esac''' % (breaks('ar-t'), breaks('ar-x')))
        self.archives = [os.path.join(self.root, n) for n in ('libLLVMSupport.a', 'libzstd.a')]

    def macos(self, **kwargs):
        return self.check('macos', 'sisu', *self.archives, **kwargs)

    def test_macos_should_call_each_tool(self):
        done = self.macos()
        self.assertEqual(done.returncode, 0, done.stderr)
        calls = self.calls()
        for archive in self.archives:
            for call in ('ar -t ' + archive, 'ar -x %s a.native' % archive):
                self.assertIn(call, calls)
        self.assertIn('otool -L sisu', calls)
        self.assertIn('codesign -v sisu', calls)
        # The first member is `__.SYMDEF SORTED`, which is skipped; `a.native` is read.
        read = [c for c in calls if c.startswith('otool -l ') and c.endswith('/a.native')]
        self.assertEqual(len(read), 2, calls)
        self.assertFalse(any('SYMDEF' in c for c in calls if c.startswith('ar -x')))

    def test_macos_should_fail_when_a_tool_fails(self):
        for fail in ('otool-L', 'codesign', 'ar-t', 'ar-x', 'otool-l'):
            with self.subTest(fail):
                done = self.macos(fail=fail)
                self.assertEqual(done.returncode, 1)
                self.assertIn(fail + ' broke', done.stderr)

    def test_macos_should_fail_on_a_load_path_off_the_system(self):
        done = self.macos(LOAD='/opt/homebrew/lib/libzstd.1.dylib')
        self.assertEqual(done.returncode, 1)
        self.assertIn('/opt/homebrew/lib/libzstd.1.dylib', done.stderr)

    def test_macos_should_fail_on_an_object_above_the_floor(self):
        done = self.macos(MINOS='16.0')
        self.assertEqual(done.returncode, 1)
        self.assertIn('a.native', done.stderr)


class LinuxAndVersionTools(ToolBase):
    def setUp(self):
        super().setUp()
        fake_tool(self.bin, 'objdump', r'''
case "$1" in
-T) %s
    printf '%%s' "$OBJDUMP_T";;
-p) %s
    printf '  NEEDED               %%s\n' "${NEEDED:-libc.so.6}";;
esac''' % (breaks('objdump-T'), breaks('objdump-p')))
        fake_tool(self.bin, 'sisu', breaks('sisu') + '''
if [ -n "$WARN" ]; then echo "$WARN" >&2; fi
printf "sisu 0.1.0\\ncommit: abc1234\\n"''')

    def test_linux_should_fail_when_a_tool_fails(self):
        good = self.check('linux', 'sisu', OBJDUMP_T=OK_T)
        self.assertEqual(good.returncode, 0, good.stderr)
        for fail in ('objdump-T', 'objdump-p'):
            with self.subTest(fail):
                done = self.check('linux', 'sisu', fail=fail, OBJDUMP_T=OK_T)
                self.assertEqual(done.returncode, 1)
                self.assertIn(fail + ' broke', done.stderr)

    def test_linux_should_fail_on_a_library_off_the_list_in_the_tool_output(self):
        done = self.check('linux', 'sisu', OBJDUMP_T=OK_T, NEEDED='libfoo.so.1')
        self.assertEqual(done.returncode, 1)
        self.assertIn('libfoo.so.1', done.stderr)

    def test_version_should_fail_on_another_commit(self):
        sisu = os.path.join(self.bin, 'sisu')
        done = self.check('version', sisu, 'fff1234')
        self.assertEqual(done.returncode, 1)
        self.assertIn('abc1234', done.stderr)

    def test_version_should_fail_on_a_stderr_line(self):
        sisu = os.path.join(self.bin, 'sisu')
        done = self.check('version', sisu, 'abc1234def', WARN='weak version not found')
        self.assertEqual(done.returncode, 1)
        self.assertIn('weak version not found', done.stderr)

    def test_version_should_fail_when_the_tool_fails(self):
        sisu = os.path.join(self.bin, 'sisu')
        self.assertEqual(self.check('version', sisu, 'abc1234def').returncode, 0)
        done = self.check('version', sisu, 'abc1234def', fail='sisu')
        self.assertEqual(done.returncode, 1)
        self.assertIn('sisu broke', done.stderr)

    def test_checks_should_fail_when_a_tool_is_missing(self):
        empty = os.path.join(self.root, 'empty')
        os.makedirs(empty)
        for args in (('linux', 'sisu'), ('version', os.path.join(empty, 'no-sisu'), 'abc1234')):
            with self.subTest(args[0]):
                done = self.check(*args, path=empty)
                self.assertEqual(done.returncode, 1)
                self.assertIn('cannot run', done.stderr)


class PackTools(PackBase):
    def setUp(self):
        super().setUp()
        self.bin = os.path.join(self.root, 'bin')
        self.log = os.path.join(self.root, 'log')
        os.makedirs(self.bin)
        # A tar that forwards to the real one, records COPYFILE_DISABLE on the create call, and
        # adds a name to the listing when $EXTRA is set.
        fake_tool(self.bin, 'tar', r'''
case "$1" in
-czf) echo "COPYFILE_DISABLE=$COPYFILE_DISABLE" >> "$LOG";;
-tzf) %(real)s "$@"; [ -n "$EXTRA" ] && echo "$EXTRA"; exit 0;;
esac
exec %(real)s "$@"''' % {'real': shutil.which('tar')})

    def pack_with(self, **extra):
        env = dict(os.environ, PATH=self.bin + os.pathsep + os.environ['PATH'], LOG=self.log,
                   **extra)
        return self.pack(env=env)

    def test_pack_should_disable_copyfile_in_the_create_call(self):
        done = self.pack_with()
        self.assertEqual(done.returncode, 0, done.stderr)
        with open(self.log) as f:
            self.assertIn('COPYFILE_DISABLE=1', f.read())

    def test_pack_should_fail_on_an_extra_name_in_the_listing(self):
        done = self.pack_with(EXTRA='sisu/._sisu')
        self.assertEqual(done.returncode, 1)
        self.assertIn('._sisu', done.stderr)
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

    def test_smoke_should_name_a_program_before_it_runs(self):
        # A hung program must be named in the log, so the fake sleeps past the timeout.
        self.fake('sleep 5')
        self.program('hung')
        env = dict(os.environ, LOG=self.log)
        with self.assertRaises(subprocess.TimeoutExpired) as raised:
            subprocess.run(['bash', SMOKE, self.bin, self.programs], env=env,
                           capture_output=True, timeout=1)
        out = (raised.exception.stdout or b'').decode()
        self.assertIn('hung.sisu', out)
        self.assertIn('mimalloc', out)

    def test_smoke_should_fail_with_no_programs(self):
        self.fake()
        self.assertEqual(self.smoke().returncode, 1)


if __name__ == '__main__':
    unittest.main()
