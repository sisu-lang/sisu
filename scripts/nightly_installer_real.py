"""Tests of install.sh that need a real machine, for the nightly build workflow:

  SISU_ARTIFACTS=<dir> python3 scripts/nightly_installer_real.py

<dir> holds the tarball of this host, install.sh and SHA256SUMS. The Linux tests run on Linux and
need Docker; the macOS tests run on macOS. The name does not match `test_*.py`, so the pre-commit
hook does not run it: the workflow runs it on `ubuntu-24.04` and `macos-15`.
"""
import os
import select
import subprocess
import sys
import tempfile
import time
import unittest

ARTIFACTS = os.environ.get('SISU_ARTIFACTS')
LINUX = sys.platform.startswith('linux')
MAC = sys.platform == 'darwin'
READY_SECONDS = 60
# `print` to a pipe is held until the program ends, so only `eprint` shows that the build is over.
WAIT_PROGRAM = 'fn main() {\n    eprint("ready")\n    print(read_line() ?? "none")\n}\n'
HELLO_PROGRAM = 'fn main() {\n    print("hello")\n}\n'
HINT = 'sudo apt install build-essential'


def environment(home, path=None):
    env = dict(os.environ, HOME=home, SISU_DOWNLOAD_DIR=ARTIFACTS)
    if path:
        env['PATH'] = path
    return env


def install(home, path=None):
    return subprocess.run(['sh', os.path.join(ARTIFACTS, 'install.sh')], env=environment(home, path),
                          capture_output=True, text=True, timeout=300, check=False)


def docker(image, command):
    """Runs `sh -c command` in the image with ARTIFACTS at /dl. The pull is quiet, so that its
    progress does not reach the stderr of the test."""
    subprocess.run(['docker', 'pull', '-q', image], capture_output=True, check=True, timeout=600)
    return subprocess.run(
        ['docker', 'run', '--rm', '-v', f'{ARTIFACTS}:/dl:ro', '-e', 'SISU_DOWNLOAD_DIR=/dl', image,
         'sh', '-c', command],
        capture_output=True, text=True, timeout=600, check=False)


def write(path, text, mode=0o644):
    with open(path, 'w', encoding='utf-8') as f:
        f.write(text)
    os.chmod(path, mode)


class InstallerRealTest(unittest.TestCase):
    def home(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        return directory.name

    def sisu(self, home):
        return os.path.join(home, '.sisu', 'bin', 'sisu')

    @staticmethod
    def stop(proc):
        proc.kill()
        proc.wait()
        for pipe in (proc.stdin, proc.stdout, proc.stderr):
            if pipe:
                pipe.close()

    def wait_for_ready(self, proc):
        """Reads the stderr of the process until it holds `ready`, for at most READY_SECONDS."""
        seen = b''
        deadline = time.monotonic() + READY_SECONDS
        while b'ready' not in seen:
            left = deadline - time.monotonic()
            if left <= 0:
                self.fail(f'the program printed no `ready` in {READY_SECONDS} s; stderr: {seen!r}')
            if select.select([proc.stderr], [], [], left)[0]:
                chunk = os.read(proc.stderr.fileno(), 4096)
                if not chunk:
                    self.fail(f'the program ended before it printed `ready`; stderr: {seen!r}')
                seen += chunk

    @unittest.skipUnless(LINUX, 'Linux only')
    def test_alpine_should_stop_with_one_sentence(self):
        result = docker('alpine:3', 'sh /dl/install.sh')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(result.stderr.strip().splitlines()), 1, result.stderr)
        self.assertIn('musl', result.stderr)

    @unittest.skipUnless(LINUX, 'Linux only')
    def test_old_glibc_should_stop_with_one_sentence(self):
        result = docker('ubuntu:20.04', 'sh /dl/install.sh')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(result.stderr.strip().splitlines()), 1, result.stderr)
        self.assertIn('glibc 2.31', result.stderr)

    @unittest.skipUnless(LINUX, 'Linux only')
    def test_ubuntu_without_a_compiler_should_install_and_name_the_command(self):
        result = docker('ubuntu:22.04', 'sh /dl/install.sh && "$HOME/.sisu/bin/sisu" --version')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(HINT, result.stdout)

    @unittest.skipUnless(LINUX or MAC, 'Linux or macOS only')
    def test_install_over_a_running_sisu_should_succeed(self):
        home = self.home()
        first = install(home)
        self.assertEqual(first.returncode, 0, first.stderr)
        program = os.path.join(home, 'wait.sisu')
        write(program, WAIT_PROGRAM)
        proc = subprocess.Popen([self.sisu(home), 'run', program], env=environment(home),
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.addCleanup(self.stop, proc)
        self.wait_for_ready(proc)
        self.assertIsNone(proc.poll(), 'sisu ended before the second install')
        second = install(home)
        self.assertEqual(second.returncode, 0, second.stderr)
        out, err = proc.communicate(b'line\n', timeout=60)
        self.assertEqual(proc.returncode, 0, err)
        self.assertEqual(out.decode().strip(), 'line')
        for name in ('sisu', 'libsisu_runtime_mimalloc.a', 'libsisu_runtime_system.a'):
            self.assertTrue(os.path.isfile(os.path.join(home, '.sisu', 'bin', name)), name)

    @unittest.skipUnless(LINUX or MAC, 'Linux or macOS only')
    def test_build_should_refuse_a_deleted_runtime_archive(self):
        home = self.home()
        self.assertEqual(install(home).returncode, 0)
        os.remove(os.path.join(home, '.sisu', 'bin', 'libsisu_runtime_mimalloc.a'))
        program = os.path.join(home, 'hello.sisu')
        write(program, HELLO_PROGRAM)
        result = subprocess.run(
            [self.sisu(home), 'build', '--allocator', 'mimalloc', program, '-o', os.path.join(home, 'hello')],
            env=environment(home), capture_output=True, text=True, timeout=300, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('runtime library not found at', result.stderr)
        self.assertIn('libsisu_runtime_mimalloc.a; run the Sisu install line again', result.stderr)

    @unittest.skipUnless(MAC, 'macOS only')
    def test_mac_with_a_rosetta_uname_should_get_the_arm64_build(self):
        home = self.home()
        fake = self.home()
        write(os.path.join(fake, 'uname'),
              '#!/bin/sh\nif [ "${1:-}" = -m ]; then echo x86_64; else exec /usr/bin/uname "$@"; fi\n', 0o755)
        result = install(home, fake + os.pathsep + os.environ['PATH'])
        self.assertEqual(result.returncode, 0, result.stderr)
        version = subprocess.run([self.sisu(home), '--version'], capture_output=True, text=True,
                                 timeout=60, check=False)
        self.assertEqual(version.returncode, 0, version.stderr)

    @unittest.skipUnless(MAC, 'macOS only')
    def test_mac_without_command_line_tools_should_name_the_command(self):
        home = self.home()
        fake = self.home()
        write(os.path.join(fake, 'xcode-select'), '#!/bin/sh\nexit 2\n', 0o755)
        result = install(home, fake + os.pathsep + os.environ['PATH'])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('xcode-select --install', result.stdout)


if __name__ == '__main__':
    if not ARTIFACTS:
        print('nightly_installer_real.py: set SISU_ARTIFACTS to the directory with the tarball, '
              'install.sh and SHA256SUMS.', file=sys.stderr)
        sys.exit(2)
    unittest.main()
