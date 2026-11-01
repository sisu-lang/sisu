"""Unit tests for compile_speed.py: python3 -m unittest discover -s scripts -p 'test_*.py'.

Each test that runs the script puts stubs in a directory. The stubs stand for the compilers, the
formatters and the other tools. The test makes that directory the whole PATH. A stub compiler logs
its arguments and writes a small program that logs its own run. The units are small fixture texts.
No test starts a real compiler, and no test checks an elapsed time.
"""
import os
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

import compile_speed

SCRIPT = Path(__file__).resolve().parent / 'compile_speed.py'
UNITS = {
    'unit.sisu': 'fn runXX() -> i64 { 45 }\n',
    'unit.rs': 'fn runXX() -> i64 { 45 }\n',
    'unit.cpp': 'long long runXX() { return 45; }\n',
    'unit.go': 'package main\n\nfunc runXX() int64 { return 45 }\n',
}
TOOLS = {'sisu': 'sisu', 'rustc': 'rustc', 'clangxx': 'clang++', 'go': 'go', 'cc': 'cc',
         'git': 'git', 'rustfmt': 'rustfmt', 'gofmt': 'gofmt', 'clang_format': 'clang-format',
         'perf': 'perf'}
# The stub writes this program at the path after `-o`.
# The program logs its run and prints the output of the stub.
PROGRAM = """#!/bin/sh
echo "ran $0" >> {log}
printf '{output}'
"""


def stub_text(tmp, name, output='45\\n', stderr='', fail=False, out=''):
    """The text of a stub for tool `name`.

    A compiler stub logs its arguments and $GOMAXPROCS, writes a program at the path after `-o`,
    and prints `stderr` to standard error. `fail` makes it exit 1. `out` is what a formatter
    prints to standard output.
    """
    log = tmp / 'log'
    if name == 'gofmt':
        # The real gofmt has no `--version`: it exits 2.
        version = 'case "$1" in --version) exit 2;; esac\n'
    else:
        version = f'case "$1" in --version|version|-V) echo "{name} 1.0"; exit 0;; esac\n'
    echo_stderr = f"echo '{stderr}' >&2\n" if stderr else ''
    exit_code = 'exit 1\n' if fail else 'exit 0\n'
    if name in ('rustfmt', 'gofmt', 'clang-format', 'perf', 'git'):
        body = f"echo '{out}'\n" if out else ''
        return f'#!/bin/sh\n{version}{body}{echo_stderr}{exit_code}'
    program = PROGRAM.format(log=log, output=output)
    return f"""#!/bin/sh
echo "{name} $* GOMAXPROCS=$GOMAXPROCS" >> {log}
case "$1 $2" in "tool link") echo "{name} link 1.0"; exit 0;; esac
{version}out=
outdir=
prev=
for a in "$@"; do
    if [ "$prev" = -o ]; then out=$a; fi
    if [ "$prev" = --out-dir ]; then outdir=$a; fi
    prev=$a
done
{echo_stderr}if [ -n "$out" ]; then
    /bin/cat > "$out" <<'PROGRAM'
{program}PROGRAM
    /bin/chmod +x "$out"
fi
if [ -n "$outdir" ]; then echo > "$outdir/stub.out"; fi
{exit_code}"""


def stubs(tmp, omit=(), **overrides):
    """Writes a stub for each tool into tmp/bin and returns an environment whose PATH is only that.

    A keyword is a tool (`rustc`, `clangxx`, `go`, `sisu`, `rustfmt`, `gofmt`, `clang_format`) and
    its value is the keyword arguments of `stub_text`, or a whole script as a string. `omit` names
    the tools to leave out.
    """
    bin_dir = tmp / 'bin'
    bin_dir.mkdir(exist_ok=True)
    for key, name in TOOLS.items():
        if key in omit:
            continue
        change = overrides.get(key, {})
        text = change if isinstance(change, str) else stub_text(tmp, name, **change)
        path = bin_dir / name
        path.write_text(text)
        path.chmod(path.stat().st_mode | stat.S_IXUSR)
    return {'PATH': str(bin_dir)}


class CompileSpeed(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        self.units = self.tmp / 'units'
        self.units.mkdir()
        for name, text in UNITS.items():
            (self.units / name).write_text(text)

    def run_script(self, *args, env, time_bound='60', timeout=None):
        """Runs the script. A later `--sisu` or `--out` in `args` replaces the default."""
        return subprocess.run(
            [sys.executable, str(SCRIPT), '--sisu', str(self.tmp / 'bin' / 'sisu'), '--units',
             str(self.units), '--out', str(self.tmp / 'out'), '--time-bound', time_bound, *args],
            env=env, capture_output=True, text=True, check=False, timeout=timeout)

    def kill_stub_sleep(self):
        """Kills the process that the stub in the time-bound test left, if the script did not."""
        try:
            os.kill(int((self.tmp / 'pid').read_text()), signal.SIGKILL)
        except (OSError, ValueError):
            pass

    def log_lines(self):
        log = self.tmp / 'log'
        return log.read_text().splitlines() if log.exists() else []

    def test_unit_with_a_different_result_should_stop_the_comparison(self):
        """
        GIVEN a Rust unit whose result differs from the Sisu unit's
        WHEN `scripts/compile_speed.py` runs
        THEN it exits with code 1
        AND it names Rust, the size and the two outputs
        """
        for output, named in (('46\\n', ('45', '46')), ('45', ())):
            with self.subTest(output=output):
                env = stubs(self.tmp, rustc={'output': output})
                r = self.run_script('--sizes', '1', '--check-only', env=env)
                self.assertEqual(r.returncode, 1, r.stdout)
                self.assertTrue(r.stderr.startswith('compile_speed: '), r.stderr)
                for text in ('Rust', 'size 1', *named):
                    self.assertIn(text, r.stderr)

    def test_unit_with_a_warning_should_stop_the_comparison(self):
        """
        GIVEN a Rust unit that gives a `dead_code` warning
        WHEN `scripts/compile_speed.py` runs
        THEN it exits with code 1
        AND it names Rust, the size and the warning
        """
        warning = 'warning: function `f` is never used'
        env = stubs(self.tmp, rustc={'stderr': warning})
        r = self.run_script('--sizes', '1', '--check-only', env=env)
        self.assertEqual(r.returncode, 1)
        for text in ('Rust', 'size 1', warning):
            self.assertIn(text, r.stderr)

    def test_unit_not_in_the_formatters_style_should_stop_the_script(self):
        """
        GIVEN a Rust unit that is not in rustfmt's style, with rustfmt on `PATH`
        WHEN `scripts/compile_speed.py` runs
        THEN it exits with code 1
        AND it names Rust and the unit file
        """
        for tool, change, language, unit in (
                ('rustfmt', {'fail': True}, 'Rust', 'unit.rs'),
                ('gofmt', {'out': 'unit.go'}, 'Go', 'unit.go')):
            with self.subTest(tool):
                env = stubs(self.tmp, **{tool: change})
                r = self.run_script('--sizes', '1', '--check-only', env=env)
                self.assertEqual(r.returncode, 1, r.stdout)
                self.assertIn(language, r.stderr)
                self.assertIn(unit, r.stderr)

    def test_cpp_unit_with_an_include_should_stop_the_script(self):
        """
        GIVEN a C++ unit with `#include <vector>`
        WHEN `scripts/compile_speed.py` runs
        THEN it exits with code 1 and names the `#include` line
        """
        head = 'extern "C" int printf(const char*, ...);\n\n'
        tail = '\nlong long runXX() { return 45; }\n'
        for label, directive in (('plain', '#include <vector>'),
                                 ('comment in the directive', '#/**/ include <vector>'),
                                 ('comment over lines', '/* a\nb */ #include <vector>'),
                                 ('line splice', '#inc\\\nlude <vector>')):
            with self.subTest(label):
                (self.units / 'unit.cpp').write_text(head + directive + tail)
                r = self.run_script('--sizes', '1', '--check-only', env=stubs(self.tmp))
                self.assertEqual(r.returncode, 1)
                # The directive starts on line 3 or, after a two-line comment, on line 4.
                line = 4 if label == 'comment over lines' else 3
                self.assertIn(f'unit.cpp:{line}', r.stderr)
                self.assertNotIn('Traceback', r.stderr)

    def test_check_only_should_check_agreement_and_time_nothing(self):
        """
        GIVEN four fixture units and stub compilers for the four languages
        WHEN `scripts/compile_speed.py --check-only` runs
        THEN it builds and runs each input at the smallest size and compares the outputs
        AND it prints no table
        AND the exit code is 0
        """
        # The environment has another value, so the script must set GOMAXPROCS itself.
        env = {**stubs(self.tmp), 'GOMAXPROCS': '2'}
        r = self.run_script('--sizes', '3,1', '--check-only', env=env)
        self.assertEqual(r.returncode, 0, r.stderr)
        log = self.log_lines()
        go_builds = [line for line in log if line.startswith('go ') and ' -o ' in line]
        self.assertTrue(go_builds[0].endswith('GOMAXPROCS=1'), go_builds)
        for name in ('sisu', 'rustc', 'clang++', 'go'):
            builds = [line for line in log if line.startswith(f'{name} ') and ' -o ' in line]
            self.assertEqual(len(builds), 1, (name, log))
            self.assertIn('/1/input.', builds[0])
        self.assertEqual(len([line for line in log if line.startswith('ran ')]), 4, log)
        self.assertFalse([line for line in log if '-Wl,--version' in line], log)
        self.assertFalse([line for line in r.stdout.splitlines() if '|' in line], r.stdout)

    def test_compile_that_runs_over_the_time_bound_should_stop_the_script(self):
        """
        GIVEN a compile command that does not end within the total time bound
        WHEN `scripts/compile_speed.py` runs it
        THEN it stops the command
        AND it exits with code 1 and names the input
        """
        # The background sleep does not hold the pipes, so only the kill of the group ends it.
        script = (f'#!/bin/sh\n/bin/sleep 300 >/dev/null 2>&1 &\necho $! > {self.tmp}/pid\nwait\n')
        env = stubs(self.tmp, rustc=script)
        self.addCleanup(self.kill_stub_sleep)
        # The timeout is only a stop for a script that does not kill the group.
        r = self.run_script('--sizes', '1', '--check-only', env=env, time_bound='1', timeout=20)
        self.assertEqual(r.returncode, 1)
        self.assertIn('Rust, size 1: ran over the time bound', r.stderr)
        pid = int((self.tmp / 'pid').read_text())
        gone = False
        for _ in range(50):
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                gone = True
            else:
                # A killed child that nobody has waited for yet is a zombie.
                try:
                    gone = Path(f'/proc/{pid}/stat').read_text().rsplit(') ', 1)[1][0] == 'Z'
                except FileNotFoundError:
                    gone = True
            if gone:
                break
            time.sleep(0.1)
        self.assertTrue(gone, f'process {pid} is still running')

    def test_deadline_should_give_each_command_the_time_left(self):
        """
        GIVEN a total time bound of 10 seconds
        WHEN a command runs, 4 seconds pass, and a second command runs
        THEN the second command gets a timeout of 6 seconds
        """
        now = [100.0]
        timeouts = []

        class FakePopen:
            def __init__(self, cmd, **kwargs):
                self.returncode = 0

            def communicate(self, timeout=None):
                timeouts.append(timeout)
                now[0] += 4
                return '', ''

        deadline = compile_speed.Deadline(10, clock=lambda: now[0])
        with mock.patch.object(compile_speed.subprocess, 'Popen', FakePopen):
            deadline.run(['a'], 'first')
            deadline.run(['b'], 'second')
        self.assertEqual(timeouts, [10, 6])

    def test_deadline_should_kill_and_wait_for_the_group_on_every_failure(self):
        """
        GIVEN a command that times out, is interrupted, or exits with a code that is not 0
        WHEN `Deadline.run` runs it
        THEN it kills the process group of the command, waits for it, and then raises
        """
        timeout = subprocess.TimeoutExpired(['a'], 1)
        # (label, what the first communicate does, exit code, run arguments, expected error)
        cases = (
            ('timeout', timeout, 0, {}, compile_speed.Fail),
            ('interrupt', KeyboardInterrupt(), 0, {}, KeyboardInterrupt),
            ('failed exit', ('', 'bad\n'), 1, {}, compile_speed.Fail),
            ('failed exit with check off', ('', 'bad\n'), 1, {'check': False}, None),
        )
        for label, first, code, kwargs, error in cases:
            with self.subTest(label):
                calls = []

                class FakePopen:
                    pid = 4242

                    def __init__(self, cmd, **kw):
                        self.returncode = code

                    def communicate(self, timeout=None):
                        calls.append(timeout)
                        if len(calls) == 1 and isinstance(first, BaseException):
                            raise first
                        return first if len(calls) == 1 else ('', '')

                deadline = compile_speed.Deadline(10)
                with mock.patch.object(compile_speed.subprocess, 'Popen', FakePopen), \
                        mock.patch.object(compile_speed.os, 'killpg') as killpg:
                    if error is None:
                        deadline.run(['a'], 'what', **kwargs)
                    else:
                        with self.assertRaises(error):
                            deadline.run(['a'], 'what', **kwargs)
                killpg.assert_called_once_with(4242, signal.SIGKILL)
                self.assertEqual(len(calls), 2, calls)
        # A clean exit leaves the group alone.
        with mock.patch.object(compile_speed.subprocess, 'Popen') as popen, \
                mock.patch.object(compile_speed.os, 'killpg') as killpg:
            popen.return_value.communicate.return_value = ('', '')
            popen.return_value.returncode = 0
            compile_speed.Deadline(10).run(['a'], 'what')
        killpg.assert_not_called()

    def test_failing_compiler_should_stop_the_script_with_its_stderr(self):
        """
        GIVEN a C++ compiler that is on `PATH` and exits with code 1
        WHEN `scripts/compile_speed.py` runs
        THEN it exits with code 1
        AND stderr names C++, the command and the first lines of the compiler's stderr
        """
        env = stubs(self.tmp, clangxx={'stderr': 'error: no', 'fail': True})
        r = self.run_script('--sizes', '1', '--check-only', env=env)
        self.assertEqual(r.returncode, 1)
        for text in ('C++', 'clang++', 'error: no'):
            self.assertIn(text, r.stderr)
        self.assertNotIn('Traceback', r.stderr)
        noexec = self.tmp / 'noexec'
        noexec.write_text('#!/bin/sh\n')
        noexec.chmod(0o644)
        regular = self.tmp / 'regular'
        regular.write_text('')
        garbage = '#!/bin/sh\nprintf "\\377" >&2\nexit 1\n'
        # A compiler stub whose program exits 3 and writes to stderr: the bytes of a program are not text.
        broken_program = ('#!/bin/sh\nout=\nprev=\nfor a in "$@"; do [ "$prev" = -o ] && out=$a;'
                          ' prev=$a; done\nprintf "#!/bin/sh\\necho boom >&2\\nexit 3\\n" > "$out"\n'
                          '/bin/chmod +x "$out"\n')
        cases = (
            ('program that exits 3', {'rustc': broken_program}, (), 'boom'),
            ('bytes that are not text', {'clangxx': garbage}, (), 'C++'),
            ('sisu without the execute bit', {}, ('--sisu', str(noexec)), 'Sisu'),
            ('out that names a file', {}, ('--out', str(regular)), str(regular)),
        )
        for label, overrides, extra, named in cases:
            with self.subTest(label):
                r = self.run_script('--sizes', '1', '--check-only', *extra,
                                    env=stubs(self.tmp, **overrides))
                self.assertEqual(r.returncode, 1, r.stdout)
                self.assertTrue(r.stderr.startswith('compile_speed: '), r.stderr)
                self.assertIn(named, r.stderr)
                self.assertNotIn('Traceback', r.stderr)

    def test_missing_formatter_should_skip_only_its_style_check(self):
        """
        GIVEN a `PATH` with no `gofmt`
        WHEN `scripts/compile_speed.py` runs
        THEN the style check of Go is skipped, and the other style checks run
        """
        env = stubs(self.tmp, omit=('gofmt',), clang_format={'fail': True})
        r = self.run_script('--sizes', '1', '--check-only', env=env)
        self.assertEqual(r.returncode, 1)
        self.assertIn('C++', r.stderr)
        env = stubs(self.tmp, omit=('gofmt',))
        r = self.run_script('--sizes', '1', '--check-only', env=env)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertRegex(r.stdout, r'skipped.*Go style check|Go style check.*skipped')

    def test_generate_should_write_the_header_once_and_each_copy_with_its_number(self):
        """
        GIVEN a unit and a number of copies
        WHEN `generate` runs
        THEN the header is once in the input, each copy has its own number, and `main` adds each result
        """
        text = compile_speed.generate('rust', 'fn runXX() -> i64 { 1 }\n', 3)
        self.assertIn('fn run0', text)
        self.assertIn('fn run2', text)
        self.assertNotIn('XX', text)
        adds = [line for line in text.splitlines() if '+ run' in line]
        self.assertEqual(len(adds), 3, text)
        for i, line in enumerate(adds):
            self.assertIn(f'run{i}()', line)
        go = compile_speed.generate(
            'go', 'package main\n\nimport "fmt"\n\nfunc runXX() int64 { return 1 }\n', 2)
        self.assertEqual(go.count('package main'), 1)
        self.assertEqual(go.count('import "fmt"'), 1)
        self.assertIn('func run0', go)
        self.assertIn('func run1', go)

    def test_command_should_run_each_compiler_on_one_thread(self):
        """
        GIVEN each row and language
        WHEN `command` builds the command
        THEN each compiler runs on one thread, with the edition or standard of the spec
        """
        src, out = Path('d/input.x'), Path('d/prog')

        def has(cmd, *words):
            return any(cmd[i:i + len(words)] == list(words) for i in range(len(cmd)))

        for row in compile_speed.ROWS:
            for lld in (False, True):
                with self.subTest(row=row, lld=lld):
                    exe = row.startswith('executable')
                    rust = compile_speed.command(row, 'rust', src, out, lld)
                    self.assertTrue(has(rust, '--edition', '2024'), rust)
                    self.assertTrue(has(rust, '-C', 'codegen-units=1'), rust)
                    self.assertEqual(has(rust, '-C', 'link-arg=-Wl,--threads=1'), lld and exe)
                    cpp = compile_speed.command(row, 'cpp', src, out, lld)
                    self.assertIn('-std=c++20', cpp)
                    self.assertEqual('-Wl,--threads=1' in cpp, lld and exe)
                    sisu = compile_speed.command(row, 'sisu', src, out, lld)
                    self.assertNotIn('--linker', sisu)
                    if '--emit' in sisu:
                        self.assertNotIn('-o', sisu)
                    go = compile_speed.command(row, 'go', src, out, lld)
                    if row == 'LLVM IR, no optimization':
                        self.assertIsNone(go)
                    else:
                        self.assertIn('-p=1', go)
                        self.assertTrue(any('-c=1' in arg for arg in go), go)

if __name__ == '__main__':
    unittest.main()
