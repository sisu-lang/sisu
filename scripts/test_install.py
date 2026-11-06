"""Tests for install.sh, run on fake system commands: python3 -m unittest discover -s scripts -p 'test_install.py'.

SISU_TEST_SH names the shell (default /bin/sh) with its arguments, such as 'bash --posix'.
"""
import hashlib
import io
import os
import re
import resource
import shlex
import shutil
import signal
import stat
import subprocess
import tarfile
import tempfile
import time
import unittest

INSTALL = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), 'install.sh')
BASE = 'https://github.com/sisu-lang/sisu/releases/latest/download/'
FILES = ('sisu', 'libsisu_runtime_mimalloc.a', 'libsisu_runtime_system.a')
TOOLS = 'mkdir rm mv cp tar gzip grep sed cut head cat'.split()
# What a fake command that fails runs: a real one prints its own error.
FAIL = 'echo "fake: failed" >&2; exit 1'
GNU_LDD = 'ldd (GNU libc) 2.39'
# The shell is found once, before the Machine replaces PATH, as an absolute path.
SH = shlex.split(os.environ.get('SISU_TEST_SH', '/bin/sh'))
SH[0] = shutil.which(SH[0]) or SH[0]
if not (os.path.isfile(SH[0]) and os.access(SH[0], os.X_OK)):
    raise RuntimeError('SISU_TEST_SH names no executable shell: %s' % SH[0])
# A command that hashes its stdin with SHA-256, for the fake shasum.
SHA256 = [shutil.which('sha256sum')] if shutil.which('sha256sum') \
    else [shutil.which('shasum'), '-a', '256']
SLEEP = shutil.which('sleep')
UNSUPPORTED = ('cannot install on {}: Sisu supports x86-64 and ARM64 Linux with glibc 2.35 or '
               'newer, and ARM64 macOS 15 or newer.')


def script(path, body):
    with open(path, 'w') as f:
        f.write('#!/bin/sh\n' + body)
    os.chmod(path, 0o755)


class Machine:
    """A temporary HOME, a download directory, and a PATH with only fake system commands and
    links to the tools the installer needs."""

    def __init__(self, test, profile='linux-x86_64', cc=True, apt=True, clt=True, prefix=''):
        self.root = tempfile.mkdtemp(prefix='sisu-install-')
        test.addCleanup(shutil.rmtree, self.root, ignore_errors=True)
        self.profile = profile
        self.home = os.path.join(self.root, prefix + 'home')
        self.dl = os.path.join(self.root, 'dl')
        self.fake = os.path.join(self.root, 'fake')
        self.tools = os.path.join(self.root, 'tools')
        for d in (self.home, self.dl, self.fake, self.tools):
            os.makedirs(d)
        self.path = self.fake + os.pathsep + self.tools
        self.sisu = os.path.join(self.home, '.sisu')
        for tool in TOOLS + [shutil.which('sha256sum') and 'sha256sum' or 'shasum']:
            os.symlink(shutil.which(tool), os.path.join(self.tools, tool))
        self._fake_commands(profile, cc, apt, clt)

    def _fake_commands(self, profile, cc, apt, clt):
        system, machine, libc, sysctl, version = 'Linux', 'x86_64', '2.39', None, None
        if profile in ('linux-x86_64', 'linux-musl', 'linux-unknown-libc'):
            pass
        elif profile == 'linux-aarch64':
            machine = 'aarch64'
        elif profile == 'linux-armv7l':
            machine = 'armv7l'
        elif profile.startswith('linux-glibc-'):
            libc = profile[len('linux-glibc-'):]
        elif profile == 'freebsd':
            system, machine = 'FreeBSD', 'amd64'
        elif profile.startswith('macos-'):
            system = 'Darwin'
            machine = 'x86_64' if profile in ('macos-rosetta', 'macos-intel', 'macos-intel-zero') \
                else 'arm64'
            sysctl = {'macos-intel': ("echo \"sysctl: unknown oid 'hw.optional.arm64'\" >&2\n"
                                      "exit 1\n"),
                      'macos-intel-zero': 'echo 0\n'}.get(profile, 'echo 1\n')
            version = profile[len('macos-'):]
            if version in ('arm64', 'rosetta', 'intel', 'intel-zero'):
                version = '15.0'
        else:
            raise ValueError(profile)
        script(self._f('uname'), 'case "$1" in -s) echo %s;; -m) echo %s;; esac\n'
               % (system, machine))
        if system == 'Linux':
            if profile == 'linux-musl':
                script(self._f('ldd'), 'echo "musl libc (x86_64)" >&2\necho "Version 1.2.4" >&2\n'
                       'exit 1\n')
                script(self._f('getconf'), 'exit 1\n')
            else:
                script(self._f('ldd'), 'echo "%s"\n' % GNU_LDD)
                script(self._f('getconf'),
                       'exit 1\n' if profile == 'linux-unknown-libc'
                       else 'echo "glibc %s"\n' % libc)
            self.tarball_name = 'sisu-%s-linux.tar.gz' % machine
        elif system == 'Darwin':
            script(self._f('sysctl'), sysctl)
            script(self._f('sw_vers'), 'echo "%s"\n' % version)
            script(self._f('xcode-select'),
                   'echo /Library/Developer/CommandLineTools\n' if clt else 'exit 2\n')
            self.tarball_name = 'sisu-aarch64-macos.tar.gz'
        else:
            self.tarball_name = 'sisu-x86_64-linux.tar.gz'
        if cc:
            script(self._f('cc'), 'exit 0\n')
        if apt:
            script(self._f('apt-get'), 'exit 0\n')

    def _f(self, name):
        return os.path.join(self.fake, name)

    def fake_command(self, name, body):
        script(self._f(name), body)

    def fake_curl(self, before=''):
        """A curl that logs each call, runs `before`, and serves the file of the URL from dl."""
        self.curl_log = os.path.join(self.root, 'curl.log')
        script(self._f('curl'), 'printf "%%s\\n" "$*" >> %s\n%s\n'
               'while [ $# -gt 0 ]; do\n'
               '  case "$1" in -o) shift; out=$1;; https://*) url=$1;; esac\n  shift\ndone\n'
               'cp %s/"${url##*/}" "$out"\n'
               % (shlex.quote(self.curl_log), before, shlex.quote(self.dl)))

    def curl_calls(self):
        if not os.path.exists(self.curl_log):
            return []
        with open(self.curl_log) as f:
            return [line.split() for line in f]

    def tarball(self, name=None, build='new', omit=(), sisu_exit=0, pad=0):
        """Writes the tarball and its line in SHA256SUMS to the download directory."""
        name = name or self.tarball_name
        stub = '#!/bin/sh\necho "sisu %s"\nexit %d\n#%s\n' % (build, sisu_exit, 'x' * pad)
        path = os.path.join(self.dl, name)
        with tarfile.open(path, 'w:gz') as tar:
            for f in FILES + ('LICENSE',):
                if f in omit:
                    continue
                data = (stub if f == 'sisu' else build + '\n').encode()
                info = tarfile.TarInfo('sisu/' + f)
                info.size = len(data)
                info.mode = 0o755 if f == 'sisu' else 0o644
                tar.addfile(info, io.BytesIO(data))
        with open(path, 'rb') as f:
            digest = hashlib.sha256(f.read()).hexdigest()
        with open(os.path.join(self.dl, 'SHA256SUMS'), 'a') as f:
            f.write('%s  %s\n' % (digest, name))
        return name

    def sums(self, text):
        with open(os.path.join(self.dl, 'SHA256SUMS'), 'w') as f:
            f.write(text)

    def earlier_install(self, build='old'):
        os.makedirs(os.path.join(self.sisu, 'bin'))
        for f in FILES:
            stub = '#!/bin/sh\necho "sisu %s"\nexit 0\n' % build
            with open(os.path.join(self.sisu, 'bin', f), 'w') as out:
                out.write(stub if f == 'sisu' else build + '\n')
        os.chmod(os.path.join(self.sisu, 'bin', 'sisu'), 0o755)

    def _env(self, env):
        e = {'HOME': self.home, 'PATH': self.path, 'SISU_DOWNLOAD_DIR': self.dl}
        for k, v in (env or {}).items():
            if v is None:
                e.pop(k, None)
            else:
                e[k] = v
        return e

    def start(self, env=None, preexec_fn=None):
        return subprocess.Popen(SH + [INSTALL], env=self._env(env), stdin=subprocess.DEVNULL,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                                preexec_fn=preexec_fn)

    def run(self, env=None, on_stdin=False, preexec_fn=None):
        """on_stdin pipes the script to the shell, as the install line does."""
        kw = {}
        if on_stdin:
            with open(INSTALL) as f:
                kw['input'] = f.read()
            argv = SH
        else:
            kw['stdin'] = subprocess.DEVNULL
            argv = SH + [INSTALL]
        return subprocess.run(argv, env=self._env(env), capture_output=True, text=True,
                              timeout=120, preexec_fn=preexec_fn, **kw)

    def builds(self):
        """File name -> build, for the files in ~/.sisu/bin."""
        found = {}
        for f in FILES:
            path = os.path.join(self.sisu, 'bin', f)
            if os.path.exists(path):
                with open(path) as fh:
                    text = fh.read(200)
                m = re.search(r'echo "sisu (\w+)"', text)
                found[f] = m.group(1) if f == 'sisu' and m else text.strip()
        return found

    def tmp_exists(self):
        return os.path.exists(os.path.join(self.sisu, 'tmp'))


ALL_OLD = {f: 'old' for f in FILES}
ALL_NEW = {f: 'new' for f in FILES}


class InstallTest(unittest.TestCase):
    def machine(self, profile='linux-x86_64', tarball=True, **kw):
        m = Machine(self, profile, **kw)
        if tarball:
            m.tarball()
        return m

    def assertInstalled(self, m, r):
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(m.builds(), ALL_NEW)

    def assertKept(self, m, r):
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(m.builds(), ALL_OLD)
        self.assertFalse(m.tmp_exists())

    def test_unsupported_system_should_stop_with_one_sentence(self):
        rows = [('linux-armv7l', 'armv7l Linux'), ('linux-musl', 'musl'),
                ('linux-glibc-2.31', 'glibc 2.31'), ('linux-unknown-libc', 'unknown C library'),
                ('macos-intel', 'Intel macOS'), ('macos-intel-zero', 'Intel macOS'),
                ('macos-14.7', 'macOS 14'), ('freebsd', 'FreeBSD')]
        for profile, named in rows:
            with self.subTest(profile):
                m = self.machine(profile)
                r = m.run()
                self.assertNotEqual(r.returncode, 0)
                self.assertEqual(len(r.stderr.splitlines()), 1, r.stderr)
                for text in (named, 'glibc 2.35', 'macOS 15'):
                    self.assertIn(text, r.stderr)
                self.assertFalse(os.path.exists(m.sisu))

    def test_unsupported_system_should_keep_the_earlier_install(self):
        for profile in ('linux-armv7l', 'linux-musl', 'linux-glibc-2.31', 'linux-unknown-libc',
                        'macos-intel', 'macos-intel-zero', 'macos-14.7', 'freebsd'):
            with self.subTest(profile):
                m = self.machine(profile)
                m.earlier_install()
                r = m.run()
                self.assertKept(m, r)
                self.assertIn('cannot install on', r.stderr)

    def test_version_below_the_floor_should_compare_as_numbers(self):
        for profile, installs in [('linux-glibc-2.4', False), ('linux-glibc-2.9', False),
                                  ('macos-9.6', False), ('linux-glibc-2.100', True),
                                  ('linux-glibc-3.0', True), ('macos-100.0', True)]:
            with self.subTest(profile):
                m = self.machine(profile)
                r = m.run()
                if installs:
                    self.assertInstalled(m, r)
                else:
                    self.assertNotEqual(r.returncode, 0)
                    self.assertIn('cannot install on', r.stderr)

    def test_system_at_the_floor_should_install(self):
        for profile in ('linux-glibc-2.35', 'macos-15.0', 'macos-15', 'macos-26.1'):
            with self.subTest(profile):
                m = self.machine(profile)
                self.assertInstalled(m, m.run())

    def test_platform_should_install_its_own_tarball(self):
        for profile, name in [('linux-x86_64', 'sisu-x86_64-linux.tar.gz'),
                              ('linux-aarch64', 'sisu-aarch64-linux.tar.gz'),
                              ('macos-arm64', 'sisu-aarch64-macos.tar.gz')]:
            with self.subTest(profile):
                m = self.machine(profile, tarball=False)
                m.tarball(name)
                r = m.run()
                self.assertInstalled(m, r)
                self.assertEqual(r.stdout.splitlines()[-1],
                                 'Sisu is installed in %s/.sisu/bin.' % m.home)

    def test_rosetta_terminal_should_install_the_arm64_build(self):
        m = self.machine('macos-rosetta', tarball=False)
        m.tarball('sisu-aarch64-macos.tar.gz')
        self.assertInstalled(m, m.run())

    def test_missing_compiler_should_install_and_name_the_command(self):
        rows = [(dict(cc=False), 'sudo apt install build-essential'),
                (dict(cc=False, apt=False), 'Install a C compiler as `cc`.'),
                (dict(profile='macos-arm64', clt=False), 'xcode-select --install')]
        for kw, command in rows:
            with self.subTest(kw):
                m = self.machine(**kw)
                r = m.run()
                self.assertInstalled(m, r)
                self.assertIn('Sisu needs a C compiler to build programs.', r.stdout)
                self.assertIn(command, r.stdout)
                version = subprocess.run([os.path.join(m.sisu, 'bin', 'sisu'), '--version'],
                                         capture_output=True, text=True)
                self.assertEqual(version.returncode, 0)

    def test_usable_compiler_should_print_no_hint(self):
        for profile in ('linux-x86_64', 'macos-arm64'):
            with self.subTest(profile):
                m = self.machine(profile)
                r = m.run()
                self.assertEqual(r.returncode, 0, r.stderr)
                self.assertNotIn('C compiler', r.stdout)

    def test_cut_download_should_change_nothing(self):
        m = Machine(self)
        with open(INSTALL, 'rb') as f:
            size = len(f.read())
        loop = ('i=0\nwhile [ "$i" -le "$2" ]; do\n'
                '  head -c "$i" "$1" | %s >/dev/null 2>&1 || true\n'
                '  i=$((i + 1))\ndone\necho "$i"\n') % ' '.join(shlex.quote(a) for a in SH)
        r = subprocess.run(['/bin/sh', '-c', loop, 'loop', INSTALL, str(size - 2)],
                           env=m._env(None), capture_output=True, text=True, timeout=300)
        self.assertEqual(r.stdout.strip(), str(size - 1), r.stderr)
        self.assertEqual(os.listdir(m.home), [])

    def test_installer_should_run_only_definitions_before_main(self):
        with open(INSTALL) as f:
            lines = f.read().splitlines()
        self.assertEqual(lines[-1], 'main')
        definition = re.compile(r'^[A-Za-z_]\w*\(\) \{(.*\})?$')
        for n, line in enumerate(lines[:-1], 1):
            self.assertTrue(
                line == '' or line.startswith('#') or line in ('set -eu', '}')
                or definition.match(line) or line.startswith((' ', '\t')),
                'line %d runs before main: %s' % (n, line))

    def test_checksum_mismatch_should_keep_the_earlier_install(self):
        for row in ('a wrong hash', 'no line for this tarball', 'a name that differs at a dot'):
            with self.subTest(row):
                m = self.machine()
                m.earlier_install()
                name = m.tarball_name
                with open(os.path.join(m.dl, 'SHA256SUMS')) as f:
                    good = f.read().split()[0]
                if row == 'a wrong hash':
                    m.sums('%s  %s\n' % ('0' * 64, name))
                elif row == 'no line for this tarball':
                    m.sums('%s  other.tar.gz\n' % ('0' * 64))
                else:
                    m.sums('%s  %s\n' % (good, name.replace('.', 'X')))
                r = m.run()
                self.assertKept(m, r)
                self.assertIn('does not match', r.stderr)
                self.assertIn('Run the install line again', r.stderr)

    def test_checksum_should_read_only_this_tarballs_line(self):
        m = self.machine()
        with open(os.path.join(m.dl, 'SHA256SUMS')) as f:
            mine = f.read()
        m.sums('%s  sisu-aarch64-linux.tar.gz\n%s%s  sisu-aarch64-macos.tar.gz\n'
               % ('1' * 64, mine, '2' * 64))
        self.assertInstalled(m, m.run())

    def test_shasum_should_check_when_sha256sum_is_missing(self):
        for row, good in (('good', True), ('bad', False)):
            with self.subTest(row):
                m = self.machine()
                for tool in ('sha256sum', 'shasum'):
                    if os.path.lexists(os.path.join(m.tools, tool)):
                        os.remove(os.path.join(m.tools, tool))
                if good:
                    m.fake_command('shasum', 'exec %s\n' % ' '.join(map(shlex.quote, SHA256)))
                else:
                    m.fake_command('shasum', 'echo "%s  -"\n' % ('0' * 64))
                r = m.run()
                if good:
                    self.assertInstalled(m, r)
                else:
                    self.assertNotEqual(r.returncode, 0)
                    self.assertIn('does not match', r.stderr)

    def test_no_checksum_tool_should_stop_with_one_sentence(self):
        m = self.machine()
        m.earlier_install()
        for tool in ('sha256sum', 'shasum'):
            if os.path.lexists(os.path.join(m.tools, tool)):
                os.remove(os.path.join(m.tools, tool))
        r = m.run()
        self.assertKept(m, r)
        self.assertEqual(r.stderr, 'install.sh: this machine has no sha256sum or shasum.\n')

    def test_failed_unpack_should_keep_the_earlier_install(self):
        def limit():
            resource.setrlimit(resource.RLIMIT_FSIZE, (1_000_000, 1_000_000))
            signal.signal(signal.SIGXFSZ, signal.SIG_IGN)

        m = self.machine(tarball=False)
        m.tarball(pad=8_000_000)
        m.earlier_install()
        r = m.run(preexec_fn=limit)
        self.assertKept(m, r)
        self.assertIn('tar', r.stderr)
        self.assertNotIn('the tarball has no', r.stderr)
        self.assertNotIn('does not match', r.stderr)
        self.assertNotIn('cannot run', r.stderr)

    def test_wrong_layout_should_keep_the_earlier_install(self):
        for missing in FILES:
            with self.subTest(missing):
                m = self.machine(tarball=False)
                m.tarball(omit=(missing,))
                m.earlier_install()
                r = m.run()
                self.assertKept(m, r)
                self.assertIn(missing, r.stderr)

    def test_build_that_cannot_start_should_keep_the_earlier_install(self):
        m = self.machine(tarball=False)
        m.tarball(sisu_exit=1)
        m.earlier_install()
        r = m.run()
        self.assertEqual(m.builds(), ALL_OLD)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn('cannot run the Sisu build', r.stderr)

    def fake_mv(self, m, fails):
        """A mv that logs each call, and exits 1 when $1 matches the `fails` pattern."""
        log = os.path.join(m.root, 'mv.log')
        m.fake_command('mv', 'echo "$*" >> %s\ncase "$1" in %s) %s;; esac\nexec %s "$@"\n'
                       % (shlex.quote(log), fails, FAIL, shlex.quote(shutil.which('mv'))))
        return log

    def one_line(self, m, r):
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(r.stderr, 'install.sh: cannot write to %s.\n' % m.sisu)

    def test_failed_swap_should_restore_the_earlier_install(self):
        for state in ('bin', 'only bin.old', 'no ~/.sisu'):
            with self.subTest(state):
                m = self.machine()
                if state == 'bin':
                    m.earlier_install()
                elif state == 'only bin.old':
                    m.earlier_install()
                    os.rename(os.path.join(m.sisu, 'bin'), os.path.join(m.sisu, 'bin.old'))
                log = self.fake_mv(m, '*.sisu/tmp/sisu')
                r = m.run()
                self.one_line(m, r)
                self.assertFalse(os.path.exists(os.path.join(m.sisu, 'bin.old')))
                self.assertFalse(m.tmp_exists())
                if state == 'no ~/.sisu':
                    self.assertFalse(os.path.exists(os.path.join(m.sisu, 'bin')))
                    continue
                self.assertEqual(m.builds(), ALL_OLD)
                if state == 'bin':
                    with open(log) as f:
                        calls = [line.replace(m.home, '~').strip() for line in f]
                    self.assertEqual(calls, ['~/.sisu/bin ~/.sisu/bin.old',
                                             '~/.sisu/tmp/sisu ~/.sisu/bin',
                                             '~/.sisu/bin.old ~/.sisu/bin'])

    def test_failed_restore_should_keep_the_earlier_install_in_bin_old(self):
        m = self.machine()
        m.earlier_install()
        self.fake_mv(m, '*.sisu/tmp/sisu | *.sisu/bin.old')
        r = m.run()
        self.one_line(m, r)
        self.assertFalse(os.path.exists(os.path.join(m.sisu, 'bin')))
        self.assertEqual(sorted(os.listdir(os.path.join(m.sisu, 'bin.old'))), sorted(FILES))
        self.assertFalse(m.tmp_exists())

    def test_failed_rename_of_bin_should_keep_the_earlier_install(self):
        m = self.machine()
        m.earlier_install()
        self.fake_mv(m, '*.sisu/bin')
        r = m.run()
        self.one_line(m, r)
        self.assertEqual(m.builds(), ALL_OLD)
        self.assertFalse(os.path.exists(os.path.join(m.sisu, 'bin.old')))
        self.assertFalse(m.tmp_exists())

    def test_failed_final_removal_should_stop_after_the_swap(self):
        m = self.machine()
        m.earlier_install()
        m.fake_command('rm', 'case "$2" in *.sisu/bin.old) [ -e "$2" ] && { %s; };; esac\n'
                       'exec %s "$@"\n' % (FAIL, shlex.quote(shutil.which('rm'))))
        r = m.run()
        self.one_line(m, r)
        self.assertNotIn('Sisu is installed', r.stdout)
        self.assertEqual(m.builds(), ALL_NEW)
        with open(os.path.join(m.sisu, 'bin.old', 'sisu')) as f:
            self.assertIn('sisu old', f.read())
        self.assertFalse(m.tmp_exists())

    def test_failed_setup_command_should_stop_with_one_line(self):
        for command, fails, stale in [('rm', '*.sisu/tmp', False), ('mkdir', '*.sisu/tmp', False),
                                      ('rm', '*.sisu/bin.old', True)]:
            with self.subTest('%s %s' % (command, fails)):
                m = self.machine()
                m.earlier_install()
                if stale:
                    os.makedirs(os.path.join(m.sisu, 'bin.old'))
                m.fake_command(command, 'case "$2" in %s) %s;; esac\nexec %s "$@"\n'
                               % (fails, FAIL, shlex.quote(shutil.which(command))))
                r = m.run()
                self.one_line(m, r)
                self.assertEqual(m.builds(), ALL_OLD)

    def test_swap_should_work_from_each_starting_state(self):
        for state in ('no ~/.sisu', 'only bin.old', 'bin and a stale bin.old'):
            with self.subTest(state):
                m = self.machine()
                if state != 'no ~/.sisu':
                    old = os.path.join(m.sisu, 'bin.old')
                    os.makedirs(old)
                    with open(os.path.join(old, 'stale'), 'w') as f:
                        f.write('stale\n')
                if state == 'bin and a stale bin.old':
                    os.makedirs(os.path.join(m.sisu, 'bin'))
                r = m.run()
                self.assertInstalled(m, r)
                self.assertFalse(os.path.exists(os.path.join(m.sisu, 'bin.old')))
                self.assertFalse(m.tmp_exists())

    def test_download_should_ask_for_the_right_files(self):
        m = self.machine()
        m.fake_curl()
        r = m.run(env={'SISU_DOWNLOAD_DIR': None}, on_stdin=True)
        self.assertInstalled(m, r)
        calls = m.curl_calls()
        self.assertEqual(len(calls), 2)
        for call in calls:
            for token in ('--proto', '=https', '--tlsv1.2', '-fsSL', '--retry'):
                self.assertIn(token, call)
        self.assertEqual(sorted(c[-1] for c in calls),
                         sorted([BASE + m.tarball_name, BASE + 'SHA256SUMS']))

    def test_download_dir_should_not_call_curl(self):
        m = self.machine()
        m.fake_curl()
        self.assertInstalled(m, m.run())
        self.assertEqual(m.curl_calls(), [])

    def test_missing_file_in_download_dir_should_stop_with_one_sentence(self):
        for missing in ('tarball', 'SHA256SUMS'):
            with self.subTest(missing):
                m = self.machine()
                m.earlier_install()
                name = m.tarball_name if missing == 'tarball' else 'SHA256SUMS'
                os.remove(os.path.join(m.dl, name))
                r = m.run()
                self.assertKept(m, r)
                self.assertEqual(r.stderr, 'install.sh: %s has no %s.\n' % (m.dl, name))

    def test_path_lines_should_print_only_when_needed(self):
        m = self.machine()
        r = m.run()
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn('export PATH="$HOME/.sisu/bin:$PATH"\n', r.stdout)
        self.assertIn('fish_add_path $HOME/.sisu/bin\n', r.stdout)
        m = self.machine()
        r = m.run(env={'PATH': os.path.join(m.sisu, 'bin') + os.pathsep + m.path})
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertNotIn('PATH=', r.stdout)
        self.assertNotIn('fish_add_path', r.stdout)

    def test_signal_should_remove_tmp(self):
        def default_int():
            signal.signal(signal.SIGINT, signal.SIG_DFL)

        for sig, code in ((signal.SIGTERM, 143), (signal.SIGINT, 130)):
            with self.subTest(sig.name):
                m = self.machine()
                m.earlier_install()
                marker = os.path.join(m.root, 'marker')
                m.fake_curl(': > %s\n%s 0.3' % (shlex.quote(marker), shlex.quote(SLEEP)))
                p = m.start(env={'SISU_DOWNLOAD_DIR': None}, preexec_fn=default_int)
                deadline = time.time() + 5
                while not os.path.exists(marker) and time.time() < deadline:
                    time.sleep(0.01)
                if not os.path.exists(marker):
                    p.kill()
                    p.communicate()
                    self.fail('curl did not start within 5 s')
                p.send_signal(sig)
                p.communicate(timeout=30)
                self.assertEqual(p.returncode, code)
                self.assertEqual(m.builds(), ALL_OLD)
                self.assertFalse(m.tmp_exists())

    def test_home_with_a_space_should_install(self):
        m = self.machine(prefix='a b/')
        self.assertInstalled(m, m.run())

    def test_home_with_a_backslash_should_install(self):
        m = self.machine(prefix='a\\b/')
        self.assertInstalled(m, m.run())

    def test_home_unset_should_stop_with_one_sentence(self):
        m = self.machine()
        r = m.run(env={'HOME': None})
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(r.stderr, 'install.sh: HOME is not set.\n')

    @unittest.skipIf(os.geteuid() == 0, 'root can write to a read-only directory')
    def test_unwritable_sisu_directory_should_keep_the_earlier_install(self):
        m = self.machine()
        m.earlier_install()
        os.chmod(m.sisu, 0o555)
        self.addCleanup(os.chmod, m.sisu, stat.S_IRWXU)
        r = m.run()
        self.one_line(m, r)
        self.assertEqual(m.builds(), ALL_OLD)

    def test_sisu_path_that_is_a_file_should_stop_with_one_sentence(self):
        m = self.machine()
        with open(m.sisu, 'w') as f:
            f.write('not a directory\n')
        r = m.run()
        self.one_line(m, r)
        with open(m.sisu) as f:
            self.assertEqual(f.read(), 'not a directory\n')

    def test_failed_download_should_keep_the_earlier_install(self):
        m = self.machine()
        m.earlier_install()
        m.fake_curl('echo "curl: (22) The requested URL returned error: 404" >&2\nexit 22')
        r = m.run(env={'SISU_DOWNLOAD_DIR': None})
        self.assertKept(m, r)
        self.assertIn('cannot download', r.stderr)
        self.assertIn(BASE + m.tarball_name, r.stderr)

    def test_stale_tmp_should_not_reach_the_new_install(self):
        m = self.machine()
        os.makedirs(os.path.join(m.sisu, 'tmp', 'sisu'))
        for junk in ('sisu/junk', 'other'):
            with open(os.path.join(m.sisu, 'tmp', junk), 'w') as f:
                f.write('junk\n')
        r = m.run()
        self.assertInstalled(m, r)
        self.assertFalse(os.path.exists(os.path.join(m.sisu, 'bin', 'junk')))
        self.assertFalse(m.tmp_exists())


if __name__ == '__main__':
    unittest.main()
