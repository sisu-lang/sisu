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

# A trimmed copy of the `-bench` file of a real run: `go build -gcflags='-bench=<file> -c=1' -p=1`
# of a small program (go1.27.2, 15 lines). Its `fe:parse` time is 29747546 ns.
GO_BENCH = """commit: go1.27.2
goos: linux
goarch: amd64
BenchmarkCompile:main:fe:init                       1     5813643 ns/op      6.94 %
BenchmarkCompile:main:fe:parse                      1    29747546 ns/op     35.51 %    15 lines    504 lines/s
BenchmarkCompile:main:fe:subtotal                   1    48121701 ns/op     57.44 %
BenchmarkCompile:main:total                         1    83781280 ns/op    100.00 %
"""
GO_FE_PARSE = 0.029747546
# The same file for the empty program: its `fe:parse` time is 4747546 ns, so 25 ms less.
GO_BENCH_EMPTY = GO_BENCH.replace('29747546', ' 4747546')
# perf runs the command after `--` and then prints a count, as `perf stat -x,` does.
PERF = """#!/bin/sh
case "$*" in "stat -e instructions:u true") exit 0;; esac
while [ "$1" != -- ]; do shift; done
shift
"$@"
code=$?
echo '1234,,instructions:u,100,100.00,,' >&2
exit $code
"""


def stub_text(tmp, name, output='45\\n', stderr='', fail=False, out='', bench='',
              linker='GNU ld stub 1.0', empty_bench='', version_fail=False, linker_on_stderr=False):
    """The text of a stub for tool `name`.

    A compiler stub logs its arguments, the first line of its last argument and $GOMAXPROCS. It
    writes a program at the path after `-o`, and prints `stderr` to standard error. `fail` makes
    it exit 1. `out` is what a formatter prints to standard output. `bench` is the text that a
    Go stub appends to the file of a `-bench=<file>` flag, and `empty_bench` is what it appends
    when the file is in a directory `empty`. `linker` is what a stub prints for `-Wl,--version`.
    `version_fail` makes the version command print an error and exit 1. `linker_on_stderr` makes a
    stub print the linker line to standard error, in the form `warning: linker stdout: <line>`
    that rustc uses.
    """
    log = tmp / 'log'
    if name == 'gofmt':
        # The real gofmt has no `--version`: it exits 2.
        version = 'case "$1" in --version) exit 2;; esac\n'
    else:
        shown = 'echo "error: version failed" >&2; exit 1' if version_fail \
            else f'echo "{name} 1.0"; exit 0'
        version = f'case "$1" in --version|version|-V) {shown};; esac\n'
    echo_stderr = f"echo '{stderr}' >&2\n" if stderr else ''
    exit_code = 'exit 1\n' if fail else 'exit 0\n'
    if name in ('rustfmt', 'gofmt', 'clang-format', 'perf', 'git'):
        body = f"echo '{out}'\n" if out else ''
        return f'#!/bin/sh\n{version}{body}{echo_stderr}{exit_code}'
    program = PROGRAM.format(log=log, output=output)
    linker_echo = f'echo "warning: linker stdout: {linker}" >&2' if linker_on_stderr \
        else f'echo "{linker}"'
    bench_block = ''
    if bench:
        put = '/bin/cat >> "$bench" <<\'{0}\'\n{1}{0}\n'
        bench_block = ('if [ -n "$bench" ]; then\ncase "$bench" in\n*/empty/*) '
                       + put.format('EMPTY', empty_bench or bench) + ';;\n*) '
                       + put.format('BENCH', bench) + ';;\nesac\nfi\n')
    return f"""#!/bin/sh
last=
for a in "$@"; do last=$a; done
first=
if [ -f "$last" ]; then first=$(/usr/bin/head -n 1 "$last"); fi
echo "{name} $* first=[$first] CCACHE_DISABLE=$CCACHE_DISABLE GOMAXPROCS=$GOMAXPROCS" >> {log}
case "$1 $2" in "tool link") echo "{name} link 1.0"; exit 0;; esac
case "$*" in *-Wl,--version*) {linker_echo}; exit 0;; esac
{version}out=
outdir=
bench=
prev=
for a in "$@"; do
    if [ "$prev" = -o ]; then out=$a; fi
    if [ "$prev" = --out-dir ]; then outdir=$a; fi
    case "$a" in -gcflags=*)
        # Go splits the flags like quoted.Split: on spaces, and a word that starts with a quote ends
        # at the next quote.
        flags=${{a#-gcflags=}}
        eval "set -- $flags"
        for f in "$@"; do case "$f" in -bench=*) bench=${{f#-bench=}};; esac; done;;
    esac
    prev=$a
done
{bench_block}{echo_stderr}if [ -n "$out" ]; then
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

    def gone(self, pid):
        """True when the process `pid` ends within 5 seconds."""
        for _ in range(50):
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                return True
            # A killed child that nobody has waited for yet is a zombie.
            try:
                if Path(f'/proc/{pid}/stat').read_text().rsplit(') ', 1)[1][0] == 'Z':
                    return True
            except (FileNotFoundError, ProcessLookupError):
                # The process ended between the two reads.
                return True
            time.sleep(0.1)
        return False

    def kill_pidfile(self, path):
        """Kills the process whose pid is in `path`, if a test left it."""
        try:
            os.kill(int(path.read_text()), signal.SIGKILL)
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
        for output, named in (('46\\n', ('45', '46')), ('45', ()), ('45\\r\\n', ())):
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
        # (label, the text of the directive, its first line, the physical line that the message shows)
        rows = (('plain', '#include <vector>', 3, '#include <vector>'),
                ('comment in the directive', '#/**/ include <vector>', 3, '#/**/ include <vector>'),
                ('comment over lines', '/* a\nb */ #include <vector>', 4, 'b */ #include <vector>'),
                ('line splice', '#inc\\\nlude <vector>', 3, '#inc\\'),
                ('digraph', '%:include <vector>', 3, '%:include <vector>'),
                ('import', '#import <vector>', 3, '#import <vector>'),
                ('include_next', '#include_next <vector>', 3, '#include_next <vector>'))
        for label, directive, line, shown in rows:
            with self.subTest(label):
                (self.units / 'unit.cpp').write_text(head + directive + tail)
                r = self.run_script('--sizes', '1', '--check-only', env=stubs(self.tmp))
                self.assertEqual(r.returncode, 1)
                self.assertIn(f'unit.cpp:{line}', r.stderr)
                self.assertIn(shown, r.stderr)
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
        self.addCleanup(self.kill_pidfile, self.tmp / 'pid')
        # The timeout is only a stop for a script that does not kill the group.
        r = self.run_script('--sizes', '1', '--check-only', env=env, time_bound='1', timeout=20)
        self.assertEqual(r.returncode, 1)
        self.assertIn('Rust, size 1: ran over the time bound', r.stderr)
        pid = int((self.tmp / 'pid').read_text())
        self.assertTrue(self.gone(pid), f'process {pid} is still running')

    def test_deadline_should_give_each_command_the_time_left(self):
        """
        GIVEN a total time bound of 10 seconds
        WHEN a command runs, 4 seconds pass, and a second command runs
        THEN the second command gets a timeout of 6 seconds
        """
        now = [100.0]
        timeouts = []

        class FakePopen:
            pid = 4242

            def __init__(self, cmd, **kwargs):
                self.returncode = 0

            def communicate(self, timeout=None):
                # The call without a timeout is the wait after the kill of the group.
                if timeout is not None:
                    timeouts.append(timeout)
                    now[0] += 4
                return '', ''

        deadline = compile_speed.Deadline(10, clock=lambda: now[0])
        with mock.patch.object(compile_speed.subprocess, 'Popen', FakePopen), \
                mock.patch.object(compile_speed.os, 'killpg'):
            deadline.run(['a'], 'first')
            deadline.run(['b'], 'second')
        self.assertEqual(timeouts, [10, 6])

    def test_deadline_should_kill_and_wait_for_the_group_on_every_failure(self):
        """
        GIVEN a command that times out, is interrupted, exits with a code that is not 0, or exits
        WHEN `Deadline.run` runs it
        THEN it kills the process group of the command, waits for it, and then raises or returns
        """
        timeout = subprocess.TimeoutExpired(['a'], 1)
        # (label, what the first communicate does, exit code, run arguments, expected error)
        cases = (
            ('timeout', timeout, 0, {}, compile_speed.Fail),
            ('interrupt', KeyboardInterrupt(), 0, {}, KeyboardInterrupt),
            ('failed exit', ('', 'bad\n'), 1, {}, compile_speed.Fail),
            ('failed exit with check off', ('', 'bad\n'), 1, {'check': False}, None),
            ('clean exit', ('', ''), 0, {}, None),
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
        # A clean exit that leaves a child with no pipe: the child ends before `run` returns.
        pidfile = self.tmp / 'child-pid'
        self.addCleanup(self.kill_pidfile, pidfile)
        compile_speed.Deadline(10).run(
            ['/bin/sh', '-c', f'/bin/sleep 300 >/dev/null 2>&1 & echo $! > {pidfile}'], 'what')
        pid = int(pidfile.read_text())
        self.assertTrue(self.gone(pid), f'process {pid} is still running')

    def test_failure_should_stop_the_script_with_a_message_and_no_traceback(self):
        """
        GIVEN a compiler that exits with code 1, a program that fails, a tool that a signal stops,
        a path that cannot be used, or stderr that is not text
        WHEN `scripts/compile_speed.py` runs
        THEN it exits with code 1
        AND stderr starts with `compile_speed:` and names the tool or path, and has no traceback
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
        killed = '#!/bin/sh\nkill -9 $$\n'
        garbage = '#!/bin/sh\nprintf "\\377" >&2\nexit 1\n'
        # A compiler stub whose program exits 3 and writes to stderr: the bytes of a program are not text.
        broken_program = ('#!/bin/sh\nout=\nprev=\nfor a in "$@"; do [ "$prev" = -o ] && out=$a;'
                          ' prev=$a; done\nprintf "#!/bin/sh\\necho boom >&2\\nexit 3\\n" > "$out"\n'
                          '/bin/chmod +x "$out"\n')
        cases = (
            ('program that exits 3', {'rustc': broken_program}, (), 'boom'),
            ('compiler stopped by a signal', {'clangxx': killed}, (), 'SIGKILL'),
            ('formatter stopped by a signal', {'rustfmt': killed}, (), 'SIGKILL'),
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
                self.assertNotIn('code -9', r.stderr)
                self.assertNotIn('style of', r.stderr)

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

    def table_rows(self, stdout):
        """The cells of each row of the markdown table in `stdout`, as lists of text."""
        return [[c.strip() for c in line.strip('|').split('|')] for line in stdout.splitlines()
                if line.startswith('| ') and not line.startswith(('| Row', '| ---'))]

    def test_tokens_should_count_words_numbers_literals_and_each_punctuation_character(self):
        """
        GIVEN source text in each language
        WHEN `tokens` counts it
        THEN a word, a number, a literal and each other character is one token
        AND a comment is none
        """
        rows = (
            ('fn f(x: i64) -> i64 { x + 1 } // c\n', 'u.rs', 15, 'the arrow is two'),
            ('print("a {b} c")', 'u.sisu', 4, 'one string with an interpolation'),
            ('print("a {f("{x}")} c")', 'u.sisu', 4, 'a nested interpolation'),
            ('printf("{")', 'u.cpp', 4, 'no interpolation outside Sisu'),
            ('"a\\"b"', 'u.rs', 1, 'an escaped quote'),
            ('0..10', 'u.sisu', 4, 'a range is two numbers and two points'),
            ('1.2_3 + 1e5 + 2.5E-3', 'u.go', 5, 'numbers with a point, an exponent and a sign'),
            ("x = 'a'", 'u.go', 3, 'a character literal'),
            ('/* a */', 'u.go', 0, 'a block comment'),
            ('a /* b\nc */ d', 'u.go', 2, 'a block comment over lines'),
            ('x := 5u8', 'u.rs', 4, 'a suffix is part of the number'),
            ('print("{\'"\'}")', 'u.sisu', 4, 'a quote character literal in an interpolation'),
            ('print("{\'{\'}")', 'u.sisu', 4, 'a brace character literal in an interpolation'),
            (r"""print("{'\''}{'{'}")""", 'u.sisu', 4, 'an escaped quote character literal'),
            (r"""print("{'\\'}{'"'}")""", 'u.sisu', 4, 'an escaped backslash literal'),
        )
        for text, path, expected, label in rows:
            with self.subTest(label):
                self.assertEqual(compile_speed.tokens(text, path), expected)

    def test_tokens_should_stop_on_a_quote_with_no_close_on_its_line(self):
        """
        GIVEN source text with a `'` that has no closing `'` on its line
        WHEN `tokens` counts it
        THEN it raises Fail with the file and the line
        """
        for text, named in (("fn f<'a>()", 'u.rs:1'), ("x\ny = 'a\n'", 'u.rs:2')):
            with self.subTest(text), self.assertRaises(compile_speed.Fail) as caught:
                compile_speed.tokens(text, 'u.rs')
            self.assertIn(named, str(caught.exception))

    def test_go_fe_parse_should_read_one_phase_time(self):
        """
        GIVEN the `-bench` file of a real Go run
        WHEN `go_fe_parse` reads it
        THEN it gives the `fe:parse` time in seconds
        """
        self.assertAlmostEqual(compile_speed.go_fe_parse(GO_BENCH), GO_FE_PARSE, places=9)

    def test_go_run_without_one_fe_parse_line_should_stop_the_script(self):
        """
        GIVEN the `-bench` file of a cached build, or one with a repeated line
        WHEN `go_fe_parse` reads it
        THEN it raises Fail that names Go
        """
        header = 'commit: go1.27.2\n'
        parse = GO_BENCH.splitlines()[4] + '\n'
        rows = (('empty', ''), ('no fe:parse line', header),
                ('two headers', header + header + parse), ('two fe:parse lines', header + parse * 2))
        for label, text in rows:
            with self.subTest(label), self.assertRaises(compile_speed.Fail) as caught:
                compile_speed.go_fe_parse(text)
            self.assertIn('Go', str(caught.exception))

    def test_past_empty_should_print_a_dash_when_the_difference_is_not_positive(self):
        """
        GIVEN a time, the time of the empty program and a count of tokens
        WHEN `past_empty` gives the rate
        THEN it is a dash when the time less the empty time is 0 or less
        """
        self.assertEqual(compile_speed.past_empty(1000, 0.5, 0.5), '-')
        self.assertEqual(compile_speed.past_empty(1000, 0.4, 0.5), '-')
        self.assertEqual(compile_speed.past_empty(1000, 1.5, 0.5), '1.0k')

    def test_missing_toolchain_should_be_skipped(self):
        """
        GIVEN a `PATH` with no `go`, and a `perf` that fails
        WHEN `scripts/compile_speed.py` runs
        THEN it exits with code 0
        AND each Go cell of the table says `skipped`
        AND each Sisu, Rust and C++ cell has a time and no instruction count
        """
        env = stubs(self.tmp, omit=('go',), perf={'fail': True})
        r = self.run_script('--sizes', '1', '--runs', '1', env=env)
        self.assertEqual(r.returncode, 0, r.stderr)
        rows = self.table_rows(r.stdout)
        self.assertEqual(len(rows), 20, r.stdout)
        for cells in rows:
            if cells[2] == 'Go':
                self.assertEqual(cells[5], 'skipped', cells)
            else:
                self.assertRegex(cells[5], r'^\d+\.\d+ m?s$', cells)
                self.assertEqual(cells[-1], '-', cells)

    def test_full_run_should_print_a_row_for_each_command_language_and_size(self):
        """
        GIVEN stub compilers for the four languages, a stub `perf`, and two sizes
        WHEN `scripts/compile_speed.py --sizes 2,1 --runs 2` runs twice on one `--out`
        THEN the table has one line for each row, size and language
        AND the Go LLVM IR cells say `none`
        AND the Go type-check cells have the `fe:parse` time
        AND the Go type-check past-empty cells use the `fe:parse` time of the empty program
        AND the Go type-check instruction cells are `-` and every other filled one is a number
        AND no Go input has the same first line twice
        AND each command has `--runs` samples, and the empty program has them once for each command
        AND every command has `CCACHE_DISABLE=1`
        AND the header has a version line for each tool and a linker line for each language
        """
        env = stubs(self.tmp, go={'bench': GO_BENCH, 'empty_bench': GO_BENCH_EMPTY}, perf=PERF,
                    git={'out': 'abc1234'}, clangxx={'linker': 'Ubuntu GNU ld 2.42'},
                    rustc={'linker': 'LLD 21.0', 'linker_on_stderr': True})
        r = self.run_script('--sizes', '2,1', '--runs', '2', env=env)
        self.assertEqual(r.returncode, 0, r.stderr)
        first_log = self.log_lines()
        r = self.run_script('--sizes', '2,1', '--runs', '2', env=env)
        self.assertEqual(r.returncode, 0, r.stderr)
        rows = self.table_rows(r.stdout)
        keys = [tuple(cells[:3]) for cells in rows]
        names = [compile_speed.NAMES[lang] for lang in compile_speed.LANGS]
        wanted = {(row, str(size), name) for row in compile_speed.ROWS for size in (2, 1)
                  for name in names}
        self.assertEqual(len(keys), len(set(keys)), keys)
        self.assertEqual(set(keys), wanted)
        for cells in rows:
            row, language, time, instructions = cells[0], cells[2], cells[5], cells[-1]
            go = language == 'Go'
            if go and row == compile_speed.ROWS[2]:
                self.assertEqual((time, instructions), ('none', '-'), cells)
            elif go and row in compile_speed.ROWS[:2]:
                self.assertEqual((time, instructions), ('29.7 ms', '-'), cells)
                self.assertEqual(cells[7], compile_speed.rate(int(cells[4]), 0.025), cells)
            else:
                self.assertRegex(time, r'^\d+\.\d+ m?s$', cells)
                self.assertRegex(instructions, r'^\d+$', cells)
        log = self.log_lines()
        go_lines = [line for line in log if line.startswith('go ')]
        self.assertTrue(go_lines)
        for line in go_lines:
            self.assertTrue(line.endswith('GOMAXPROCS=1'), line)
        # The check builds keep the plain first line `// run`; the measured runs change it.
        firsts = [line.split(' first=[')[1].split('] ')[0] for line in go_lines
                  if ' first=[// run ' in line]
        self.assertTrue(firsts)
        self.assertEqual(len(firsts), len(set(firsts)), firsts)
        # The empty program is a Go input too, so each of its runs has a stamp.
        empty_go = [line for line in go_lines if '/empty/input.go' in line]
        self.assertTrue(empty_go)
        for line in empty_go:
            self.assertNotIn(' first=[// run] ', line)
        # Every command of a compiler has the same environment.
        for line in log:
            if not line.startswith('ran '):
                self.assertIn(' CCACHE_DISABLE=1 ', line)
        # The linker of rustc is LLD, so its optimized builds get a thread limit. The linker of
        # clang++ is not LLD. The check builds do not use -O2.
        rust = [line for line in log if line.startswith('rustc ') and 'opt-level=2' in line]
        self.assertTrue(rust)
        for line in rust:
            self.assertIn('link-arg=-Wl,--threads=1', line)
        clang = [line for line in log if line.startswith('clang++ ') and ' -O2 ' in line]
        self.assertTrue(clang)
        for line in clang:
            self.assertNotIn('threads=1', line)
        # Samples: `--runs 2` runs of a command, and one more under perf where perf counts. The
        # two type-check rows share their runs. The empty program has two runs of each command
        # once, not once for each size.
        def count(name, *words):
            return len([line for line in first_log if line.startswith(f'{name} ')
                        and all(word in line for word in words)])
        self.assertEqual(count('rustc', '--emit=metadata', '/2/input.rs'), 3)
        self.assertEqual(count('rustc', '--emit=llvm-ir', '/1/input.rs'), 3)
        self.assertEqual(count('go', '-bench=', '/2/input.go'), 2)
        for name, commands in (('sisu', 5), ('rustc', 4), ('clang++', 4), ('go', 3)):
            self.assertEqual(count(name, '/empty/input.'), commands * 2, name)
        header = r.stdout.split('| Row')[0]
        for text in ('commit: abc1234', 'sisu 1.0', 'rustc 1.0', 'clang++ 1.0', 'go 1.0',
                     'rustfmt 1.0', 'clang-format 1.0', 'gofmt: from go 1.0',
                     'linker (Sisu): GNU ld stub 1.0', 'linker (Rust): LLD 21.0',
                     'linker (C++): Ubuntu GNU ld 2.42', 'linker (Go): go link 1.0'):
            self.assertIn(text, header)
        self.assertNotIn('warning', header)

    def test_table_should_print_times_and_rates(self):
        """
        GIVEN fixed measurements
        WHEN `table` prints them
        THEN a time below 1 s is in milliseconds and a longer one in seconds
        AND a rate has the suffix of part A's `rate`
        AND a cell with no perf, no empty time or no time is a dash
        """
        row = compile_speed.ROWS[0]
        Result = compile_speed.Result
        results = [
            Result(row, 10, 'sisu', 100, 1000, 0.25, 0.05, 2_000_000),
            Result(row, 10, 'rust', 300, 999_960, 1.0, None, None),
            Result(row, 10, 'cpp', 300, 3000, 1.5, 1.5, None),
            Result(row, 10, 'go', 100, 1000, note='skipped'),
        ]
        text = compile_speed.table(['commit: abc'], results)
        self.assertIn('commit: abc', text.splitlines()[0])
        # (language, lines, tokens, time, tokens/s, tokens/s past empty, lines/s, instructions/token)
        expected = {
            'Sisu': ['100', '1000', '250.0 ms', '4.0k', '5.0k', '400', '2000'],
            'Rust': ['300', '999960', '1.00 s', '1.0M', '-', '300', '-'],
            'C++': ['300', '3000', '1.50 s', '2.0k', '-', '200', '-'],
            'Go': ['100', '1000', 'skipped', '-', '-', '-', '-'],
        }
        rows = self.table_rows(text)
        self.assertEqual(len(rows), 4, text)
        for cells in rows:
            self.assertEqual(cells[3:], expected[cells[2]], cells)

    def test_measurement_should_keep_the_best_of_the_runs(self):
        """
        GIVEN `--runs 3` and a command whose three runs take 0.03, 0.01 and 0.02 seconds
        WHEN `Measurer.cell` measures it
        THEN it runs the command 3 times and gives 0.01
        AND it runs the command once more under perf when perf counts
        """
        runner = compile_speed.Measurer(mock.Mock(runs=3), None, True)
        calls = []
        times = iter([0.03, 0.01, 0.02])

        def run(cmd, what, lang, src, text, bench, wrap=False):
            calls.append(wrap)
            return (0.0, '7,,instructions:u,1,100.00,,') if wrap else (next(times), '')

        runner.run = run
        self.assertEqual(runner.cell(['c'], 'what', 'rust', None, '', None, True), (0.01, 7))
        self.assertEqual(calls, [False, False, False, True])

    def test_out_with_a_space_should_work_for_every_command(self):
        """
        GIVEN an `--out` directory with a space in its name
        WHEN `scripts/compile_speed.py --sizes 1 --runs 1` runs
        THEN it exits with code 0
        AND the Go type-check cells have the `fe:parse` time
        """
        env = stubs(self.tmp, go={'bench': GO_BENCH})
        r = self.run_script('--sizes', '1', '--runs', '1', '--out', str(self.tmp / 'out dir'),
                            env=env)
        self.assertEqual(r.returncode, 0, r.stderr)
        go = [cells for cells in self.table_rows(r.stdout) if cells[2] == 'Go']
        self.assertEqual([cells[5] for cells in go[:2]], ['29.7 ms'] * 2, go)

    def test_out_with_a_quote_should_stop_only_a_run_that_makes_a_go_bench_command(self):
        """
        GIVEN an `--out` directory with a `'` in its name
        WHEN `scripts/compile_speed.py` runs
        THEN a measurement with Go stops with a message that names the Go flags
        AND `--check-only` and a measurement with no Go exit with code 0
        """
        out = str(self.tmp / "it's")
        # `stubs` leaves the files of an earlier call, so the run with no Go comes first.
        r = self.run_script('--sizes', '1', '--runs', '1', '--out', out,
                            env=stubs(self.tmp, omit=('go',)))
        self.assertEqual(r.returncode, 0, r.stderr)
        r = self.run_script('--sizes', '1', '--check-only', '--out', out, env=stubs(self.tmp))
        self.assertEqual(r.returncode, 0, r.stderr)
        r = self.run_script('--sizes', '1', '--runs', '1', '--out', out,
                            env=stubs(self.tmp, go={'bench': GO_BENCH}))
        self.assertEqual(r.returncode, 1)
        self.assertTrue(r.stderr.startswith(
            "compile_speed: --out may not have a ' in it, because Go cannot read a ' in its "
            "-gcflags: "), r.stderr)

    def with_prefix(self, tool, snippet):
        """A default stub for `tool` that runs the shell `snippet` first."""
        return stub_text(self.tmp, tool).replace('#!/bin/sh\n', '#!/bin/sh\n' + snippet, 1)

    def test_failure_in_a_timed_run_should_stop_the_script_with_a_message(self):
        """
        GIVEN a warning that only a timed build gives, or a `-bench` file that cannot be read
        WHEN `scripts/compile_speed.py` measures
        THEN it exits with code 1
        AND stderr starts with `compile_speed:`, names the input and the cause, and has no traceback
        """
        warn = 'echo "warning: {}" >&2\n'
        # A Go stub that makes the `-bench` file a directory, so a read fails also as root.
        unreadable = self.with_prefix('go', """for a in "$@"; do
    case "$a" in -gcflags=*-bench=*) b=${a#*-bench=}; b=${b%"'"}; /bin/mkdir -p "$b"; exit 0;; esac
done
""")
        cases = (
            ('unreadable bench file', {'go': unreadable},
             ('Go, size 1', 'prog.bench')),
            ('warning only at -O2', {'rustc': self.with_prefix(
                'rustc', 'case "$*" in *opt-level=2*) ' + warn.format('o2 only') + ';; esac\n')},
             ('Rust, size 1, executable, optimized', 'warning: o2 only')),
            ('warning only in the empty program', {'rustc': self.with_prefix(
                'rustc', 'case "$*" in */empty/input.*) ' + warn.format('empty only') + ';; esac\n')},
             ('Rust, empty program', 'warning: empty only')),
            ('warning in the perf run', {'perf': PERF.replace(
                'echo ', warn.format('perf run') + 'echo ', 1)}, ('perf', 'warning: perf run')),
        )
        for label, overrides, named in cases:
            with self.subTest(label):
                env = stubs(self.tmp, **{'go': {'bench': GO_BENCH}, 'perf': PERF, **overrides})
                r = self.run_script('--sizes', '1', '--runs', '1', env=env)
                self.assertEqual(r.returncode, 1, r.stdout)
                self.assertTrue(r.stderr.startswith('compile_speed: '), r.stderr)
                for text in named:
                    self.assertIn(text, r.stderr)
                self.assertNotIn('Traceback', r.stderr)

    def test_optional_tool_that_fails_should_give_unknown_and_not_its_error(self):
        """
        GIVEN version commands, `git`, `perf` and `cc` that exit with code 1 and print an error
        WHEN `scripts/compile_speed.py --sizes 1 --runs 1` runs
        THEN it exits with code 0
        AND the header says `unknown` for each of them, and shows none of their text
        """
        broken = '#!/bin/sh\necho "error: cc broke"\necho "error: cc broke" >&2\nexit 1\n'
        env = stubs(self.tmp, rustc={'version_fail': True}, clang_format={'version_fail': True},
                    go={'version_fail': True, 'bench': GO_BENCH}, cc=broken,
                    git={'fail': True, 'out': 'fatal: not a git repository'},
                    perf={'fail': True, 'out': 'perf: broke'})
        r = self.run_script('--sizes', '1', '--runs', '1', env=env)
        self.assertEqual(r.returncode, 0, r.stderr)
        for text in ('commit: unknown', 'rustc: unknown', 'clang-format: unknown', 'go: unknown',
                     'gofmt: from unknown', 'linker (Sisu): unknown'):
            self.assertIn(text, r.stdout)
        for text in ('version failed', 'fatal', 'cc broke', 'perf: broke'):
            self.assertNotIn(text, r.stdout)

    def test_environment_should_disable_ccache_and_limit_go_to_one_thread(self):
        """
        GIVEN a language
        WHEN `environment` builds the environment of its commands
        THEN ccache is off for every language, so that no compile is a cache hit
        AND only Go has `GOMAXPROCS=1`
        """
        for lang in compile_speed.LANGS:
            with self.subTest(lang):
                env = compile_speed.environment(lang)
                self.assertEqual(env['CCACHE_DISABLE'], '1')
                self.assertEqual(env.get('GOMAXPROCS') == '1', lang == 'go')

    def test_instructions_should_sum_the_lines_of_every_core_type(self):
        """
        GIVEN the stderr of `perf stat -x,` on a CPU with one or two core types
        WHEN `instructions` reads it
        THEN it gives the sum of the lines that count instructions
        AND a line with no number counts as nothing, and no number at all gives None
        """
        hybrid = ('48000,,cpu_core/instructions/u,100,100.00,,\n'
                  '2000,,cpu_atom/instructions/u,50,50.00,,\n')
        cases = (
            ('one line', '1234,,instructions:u,100,100.00,,\n', 1234),
            ('two core types', hybrid, 50000),
            ('a core type that did not count', '48000,,cpu_core/instructions/u,1,1.00,,\n'
             '<not counted>,,cpu_atom/instructions/u,0,0.00,,\n', 48000),
            ('another event', '999,,cycles:u,1,1.00,,\n7,,instructions:u,1,1.00,,\n', 7),
            ('not supported', '<not supported>,,instructions:u,0,100.00,,\n', None),
            ('no line', 'warning: nothing\n', None),
            ('empty', '', None),
        )
        for label, text, expected in cases:
            with self.subTest(label):
                self.assertEqual(compile_speed.instructions(text), expected)

    def test_perf_with_two_core_types_should_give_the_sum_for_each_token(self):
        """
        GIVEN a `perf` that prints one line for each of two core types
        WHEN `scripts/compile_speed.py --sizes 1 --runs 1` runs
        THEN the instruction cells have the sum of both lines divided by the tokens
        """
        two = PERF.replace("echo '1234,,instructions:u,100,100.00,,' >&2",
                           "echo '50000,,cpu_core/instructions/u,1,1.00,,' >&2\n"
                           "echo '30000,,cpu_atom/instructions/u,1,1.00,,' >&2")
        self.assertNotEqual(two, PERF)
        r = self.run_script('--sizes', '1', '--runs', '1', env=stubs(self.tmp, perf=two, go={'bench': GO_BENCH}))
        self.assertEqual(r.returncode, 0, r.stderr)
        cells = self.table_rows(r.stdout)[0]
        self.assertEqual(cells[-1], f'{80000 / int(cells[4]):.0f}', cells)

    def test_linker_line_of_rustc_with_gnu_ld_should_not_have_the_warning_prefix(self):
        """
        GIVEN a rustc that shows the output of GNU ld as `warning: linker stdout: <line>`
        WHEN `scripts/compile_speed.py --sizes 1 --runs 1` runs
        THEN the header has the linker line without the prefix
        AND the Rust builds get no thread limit
        """
        env = stubs(self.tmp, go={'bench': GO_BENCH},
                    rustc={'linker': 'GNU ld stub 2.42', 'linker_on_stderr': True})
        r = self.run_script('--sizes', '1', '--runs', '1', env=env)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn('linker (Rust): GNU ld stub 2.42', r.stdout)
        self.assertNotIn('warning', r.stdout)
        self.assertFalse([line for line in self.log_lines()
                          if line.startswith('rustc ') and 'threads=1' in line])


if __name__ == '__main__':
    unittest.main()
