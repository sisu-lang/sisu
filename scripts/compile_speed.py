#!/usr/bin/env python3
"""Compares the compile speed of Sisu with rustc, clang++ and Go on one program.

Usage: python3 scripts/compile_speed.py [--check-only] [--sizes 10,100,1000] [--runs 5]
       [--sisu target/release/sisu] [--units bench/compile] [--out target/compile-speed]
       [--time-bound SECONDS]

`bench/compile/` holds one unit in each language. The script repeats the unit to make an input of
each size. It builds and runs each input. It stops with exit code 1 when a compiler gives a
warning. It also stops when an input prints other output than the Sisu input prints.
`--check-only` does this at the smallest size and times nothing. Without it, the script then times
each row for each size and language, and prints a markdown table. The script does not build
`sisu`: `make compile-speed` does.

Every command runs through one `Deadline`. The time bound is for the whole run. A command that runs
over it is killed with its processes, and the script exits with code 1 and names the input.
"""

import argparse
import itertools
import os
import platform
import re
import shlex
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path
from typing import NamedTuple

LANGS = ('sisu', 'rust', 'cpp', 'go')
NAMES = {'sisu': 'Sisu', 'rust': 'Rust', 'cpp': 'C++', 'go': 'Go'}
ROWS = ('type check', 'type check and mir', 'LLVM IR, no optimization',
        'executable, no optimization', 'executable, optimized')
DEFAULT_SISU = 'target/release/sisu'
EXTENSIONS = {'sisu': 'sisu', 'rust': 'rs', 'cpp': 'cpp', 'go': 'go'}
COMPILERS = {'sisu': None, 'rust': 'rustc', 'cpp': 'clang++', 'go': 'go'}
# The check mode of each formatter. gofmt prints the files that it would change.
FORMATTERS = {
    'rust': ['rustfmt', '--check', '--edition', '2024'],
    'go': ['gofmt', '-l'],
    'cpp': ['clang-format', '--dry-run', '--Werror', '--style=LLVM'],
}
STDERR_LINES = 20


class Fail(Exception):
    """A check or a command failed: `main` prints the message and exits with code 1."""


class Deadline:
    """A total time bound that every command of the run shares."""

    def __init__(self, seconds, clock=time.monotonic):
        self.clock = clock
        self.end = clock() + seconds

    def run(self, cmd, what, **kwargs):
        """Runs `cmd` with the time that is left as its timeout.

        It captures each of stdout and stderr as text, unless `kwargs` gives that stream. After
        every command it kills the process group, and waits for the command and its pipes.
        It raises Fail on a timeout, when the command cannot start, and when the exit code is not
        0 (unless `check=False` is given).
        """
        check = kwargs.pop('check', True)
        left = self.end - self.clock()
        if left <= 0:
            raise Fail(f'{what}: ran over the time bound')
        kwargs.setdefault('stdout', subprocess.PIPE)
        kwargs.setdefault('stderr', subprocess.PIPE)
        kwargs.setdefault('text', True)
        if kwargs['text']:
            kwargs.setdefault('errors', 'replace')
        shown = shlex.join(str(a) for a in cmd)
        try:
            proc = subprocess.Popen(cmd, start_new_session=True, **kwargs)
        except FileNotFoundError:
            raise Fail(f'{what}: {shown}: the program is not found') from None
        except OSError as e:
            raise Fail(f'{what}: {shown}: {e.strerror}') from None
        try:
            out, err = proc.communicate(timeout=left)
        except subprocess.TimeoutExpired:
            raise Fail(f'{what}: ran over the time bound') from None
        finally:
            kill_group(proc)
        if check and proc.returncode != 0:
            if isinstance(err, bytes):
                err = err.decode(errors='replace')
            lines = (err or '').splitlines()[:STDERR_LINES]
            raise Fail('\n'.join([f'{what}: {shown} {exit_text(proc.returncode)}', *lines]))
        return subprocess.CompletedProcess(cmd, proc.returncode, out, err)


def exit_text(code):
    """Says how a command ended: its exit code, or the signal that stopped it."""
    if code >= 0:
        return f'exited with code {code}'
    try:
        return f'was stopped by {signal.Signals(-code).name}'
    except ValueError:
        return f'was stopped by signal {-code}'


def kill_group(proc):
    """Kills the process group of `proc`, and waits for `proc` and its pipes."""
    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    proc.communicate()


def generate(lang, unit, copies):
    """The input for `lang`: the header of the unit once, `copies` copies, and a `main`.

    The header is every line before the first line that has `XX`. Each copy has its number in
    place of every `XX`.
    """
    lines = unit.splitlines(keepends=True)
    first = next((i for i, line in enumerate(lines) if 'XX' in line), len(lines))
    header, body = ''.join(lines[:first]), ''.join(lines[first:])
    parts = [header] + [body.replace('XX', str(i)) for i in range(copies)]
    text = '\n'.join(part for part in parts if part)
    # The first line of a Go input is `// run`. The measurement changes it, so no build is cached.
    return ('// run\n' if lang == 'go' else '') + text + '\n' + main_of(lang, copies)


def main_of(lang, copies):
    """The `main` that adds the result of each copy in one statement and prints the sum.

    Each statement calls its copy twice. In Sisu and Rust, `runN` has internal linkage. LLVM gives
    the last call of an internal function a large inline bonus. With one call, every copy would go
    into `main` at -O2, and the time of the optimizer on one large function grows faster than the
    program. Two calls remove the bonus. The C++ `runN` has external linkage and never had the
    bonus, and the inliner of Go has no such bonus. They use the same form, so that the programs
    are the same.

    The copies must also differ in more than their names. Otherwise rustc merges their `run`
    functions into one, because it runs the LLVM pass MergeFunctions at -O2. So the label
    string of each unit has the number of the copy, and its length goes into the sum.
    """
    calls = [f'total = total + run{i}() + run{i}()' for i in range(copies)]
    if lang == 'sisu':
        return 'fn main() {\n    let mut total = 0\n' + ''.join(
            f'    {c}\n' for c in calls) + '    print(total)\n}\n'
    if lang == 'rust':
        return 'fn main() {\n    let mut total: i64 = 0;\n' + ''.join(
            f'    {c};\n' for c in calls) + '    println!("{}", total);\n}\n'
    if lang == 'cpp':
        return 'int main() {\n  long long total = 0;\n' + ''.join(
            f'  {c};\n' for c in calls) + '  printf("%lld\\n", total);\n  return 0;\n}\n'
    return 'func main() {\n\tvar total int64\n' + ''.join(
        f'\t{c}\n' for c in calls) + '\tfmt.Println(total)\n}\n'


def empty_program(lang):
    """A program with a `main` that returns."""
    return {
        'sisu': 'fn main() {}\n',
        'rust': 'fn main() {}\n',
        'cpp': 'int main() { return 0; }\n',
        'go': '// run\npackage main\n\nfunc main() {}\n',
    }[lang]


def command(row, lang, src, out, lld, sisu=DEFAULT_SISU):
    """The command of `row` for `lang`, or None when the language has no such row.

    Every command runs on one thread. `lld` says that the linker is LLD, which gets one thread.
    """
    src, out = str(src), str(out)
    here = str(Path(out).parent)
    if row not in ROWS:
        raise ValueError(f'unknown row: {row}')
    if lang == 'sisu':
        return {
            ROWS[0]: [sisu, 'check', src],
            ROWS[1]: [sisu, 'build', '--emit', 'mir', src],
            ROWS[2]: [sisu, 'build', '--emit', 'ir-raw', src],
            ROWS[3]: [sisu, 'build', src, '-o', out],
            ROWS[4]: [sisu, 'build', '-O2', src, '-o', out],
        }[row]
    if lang == 'rust':
        base = ['rustc', '--edition', '2024', '-C', 'codegen-units=1']
        if row in ROWS[:2]:
            return [*base, '--emit=metadata', '--out-dir', here, src]
        if row == ROWS[2]:
            return [*base, '--emit=llvm-ir', '-C', 'opt-level=0', '--out-dir', here, src]
        link = ['-C', 'link-arg=-Wl,--threads=1'] if lld else []
        return [*base, '-C', f'opt-level={0 if row == ROWS[3] else 2}', *link, '-o', out, src]
    if lang == 'cpp':
        base = ['clang++', '-std=c++20']
        if row in ROWS[:2]:
            return [*base, '-fsyntax-only', src]
        if row == ROWS[2]:
            return [*base, '-S', '-emit-llvm', '-O0', '-o', out, src]
        link = ['-Wl,--threads=1'] if lld else []
        return [*base, '-O0' if row == ROWS[3] else '-O2', *link, '-o', out, src]
    if row == ROWS[2]:
        return None
    base = ['go', 'build', '-p=1']
    if row in ROWS[:2]:
        bench = out + '.bench'
        # Go splits the flags on spaces, and it reads a quoted word as one.
        return [*base, f"-gcflags=-c=1 '-bench={bench}'", '-o', out, src]
    flags = '-N -l -c=1' if row == ROWS[3] else '-c=1'
    return [*base, f'-gcflags={flags}', '-o', out, src]


def environment(lang):
    """The environment of a command: Go runs on one thread, and ccache is off.

    A ccache in front of clang++ would make each run after the first a cache hit.
    """
    return {**os.environ, 'CCACHE_DISABLE': '1', **({'GOMAXPROCS': '1'} if lang == 'go' else {})}


def warnings(stderr):
    """The lines of a build's stderr that have `warning`, in any case."""
    return [line for line in stderr.splitlines() if 'warning' in line.lower()]


def build(deadline, cmd, what, **kwargs):
    """Runs a compile command, and stops when it gives a warning.

    A warning adds the time to print it. Every compile of the script goes through here, except the
    linker discovery: the check builds, the timed runs, the empty program and the perf runs.
    """
    result = deadline.run(cmd, what, **kwargs)
    found = warnings(result.stderr or '')
    if found:
        raise Fail(f'{what}: the build gave a warning: {found[0].strip()}')
    return result


def check_no_include(path, text):
    """Stops when the C++ unit has an `#include`, `#include_next` or `#import` directive.

    A `%:` digraph in place of `#` counts too.

    A header adds the library to parse. The match runs on the text without block comments, and
    with each line that ends in a backslash joined to the next line. The message has the first
    physical line of the directive.
    """
    original = text.splitlines()
    # A block comment keeps its line breaks, so the line numbers stay the same.
    text = re.sub(r'//[^\n]*|/\*.*?\*/', lambda m: m.group() if m.group().startswith('//')
                  else ' ' + '\n' * m.group().count('\n'), text, flags=re.S)
    first, joined = 0, ''
    for number, line in enumerate(text.splitlines(), 1):
        first = first or number
        if line.endswith('\\'):
            joined += line[:-1]
            continue
        if re.match(r'\s*(#|%:)\s*(include|include_next|import)\b', joined + line):
            raise Fail(f'{path}:{first}: the C++ unit may not include a header: '
                       f'{original[first - 1].strip()}')
        first, joined = 0, ''


def read_units(units_dir):
    units = {}
    for lang in LANGS:
        path = units_dir / f'unit.{EXTENSIONS[lang]}'
        try:
            units[lang] = path.read_text()
        except OSError as e:
            raise Fail(f'{path}: cannot read the unit: {e.strerror}') from None
    return units


def check_style(units_dir, deadline):
    """Runs the check mode of each formatter that is on PATH, and says which ones it skips."""
    for lang, cmd in FORMATTERS.items():
        path = units_dir / f'unit.{EXTENSIONS[lang]}'
        if shutil.which(cmd[0]) is None:
            print(f'skipped: the {NAMES[lang]} style check, {cmd[0]} is not on PATH')
            continue
        r = deadline.run([*cmd, str(path)], f'{NAMES[lang]} style check', check=False)
        if r.returncode < 0:
            raise Fail(f'{NAMES[lang]} style check: {cmd[0]} {exit_text(r.returncode)}')
        if r.returncode != 0 or (lang == 'go' and r.stdout.strip()):
            raise Fail(f'{NAMES[lang]}: {path} is not in the style of {cmd[0]}')


def check_sizes(args, units, sizes, deadline):
    """Builds and runs each input, and compares its output with the output of Sisu."""
    for size in sizes:
        reference = None
        for lang in LANGS:
            tool = COMPILERS[lang]
            if tool is not None and shutil.which(tool) is None:
                print(f'skipped: {NAMES[lang]}, size {size}, {tool} is not on PATH')
                continue
            what = f'{NAMES[lang]}, size {size}'
            work = args.out / lang / str(size)
            src, exe = work / f'input.{EXTENSIONS[lang]}', work / 'prog'
            try:
                work.mkdir(parents=True, exist_ok=True)
                src.write_text(generate(lang, units[lang], size))
            except OSError as e:
                raise Fail(f'{what}: {e.filename or work}: {e.strerror}') from None
            cmd = command(ROWS[3], lang, src, exe, False, sisu=args.sisu)
            build(deadline, cmd, what, env=environment(lang))
            printed = deadline.run([str(exe)], what, text=False).stdout
            if lang == 'sisu':
                reference = printed
            elif printed != reference:
                raise Fail(f'{what}: the output {printed!r} differs from the Sisu output '
                           f'{reference!r}')
            print(f'checked {what}: prints {printed!r}')


class Result(NamedTuple):
    """The measurement of one row, size and language.

    `time` and `empty` are in seconds, `empty` for the empty program. `note` replaces the time:
    `none` for a row that the language has no command for, `skipped` for a missing compiler.
    """
    row: str
    size: int
    lang: str
    lines: int
    tokens: int
    time: float | None = None
    empty: float | None = None
    instructions: int | None = None
    note: str = ''


# A character literal on one line, with escapes.
CHAR = re.compile(r"'(?:\\.|[^'\\\n])*'")

# Words, numbers with a suffix, character literals and comments. A `"` starts a string, and
# `skip_string` reads it.
TOKEN = re.compile(rf"""
    (?P<skip>\s+|//[^\n]*|/\*.*?(?:\*/|\Z))
  | (?P<word>[^\W\d]\w*)
  | (?P<number>[0-9][0-9_]*(?:\.[0-9][0-9_]*)?(?:[eE][+-]?[0-9]+)?(?:[A-Za-z][A-Za-z0-9]*)?)
  | (?P<char>{CHAR.pattern})
  | (?P<bad>')
  | (?P<quote>")
  | (?P<other>.)
""", re.X | re.S)


def tokens(text, path):
    """The number of tokens in `text`, the same way for every language.

    A word or a number is one token. A string literal is one token with its interpolations, and
    only a `.sisu` file has interpolations. A character literal is one token on one line. Each
    other character that is not white space is one token. A comment is none.
    """
    count, pos = 0, 0
    while pos < len(text):
        m = TOKEN.match(text, pos)
        kind = m.lastgroup
        if kind == 'bad':
            raise Fail(f'{path}:{line_of(text, pos)}: a character literal has no close on its line')
        if kind == 'quote':
            pos = skip_string(text, pos + 1, path, path.endswith('.sisu'))
            count += 1
            continue
        count += kind != 'skip'
        pos = m.end()
    return count


def line_of(text, pos):
    return text.count('\n', 0, pos) + 1


def skip_string(text, pos, path, interpolate):
    """The position after the string whose text starts at `pos`, after its opening quote."""
    i = pos
    while i < len(text):
        c = text[i]
        if c == '\\':
            i += 2
        elif c == '"':
            return i + 1
        elif c == '{' and interpolate:
            i = skip_braces(text, i + 1, path)
        else:
            i += 1
    raise Fail(f'{path}:{line_of(text, pos)}: a string has no close')


def skip_braces(text, pos, path):
    """The position after the `}` that closes the `{` before `pos`, over braces and strings."""
    i, depth = pos, 1
    while i < len(text):
        c = text[i]
        if c == '"':
            i = skip_string(text, i + 1, path, True)
            continue
        if c == "'" and (m := CHAR.match(text, i)):
            # The braces and quotes of a character literal are not syntax.
            i = m.end()
            continue
        depth += (c == '{') - (c == '}')
        i += 1
        if depth == 0:
            return i
    raise Fail(f'{path}:{line_of(text, pos)}: an interpolation has no close')


def go_fe_parse(text, what='Go'):
    """The `fe:parse` time, in seconds, of the `-bench` file of one Go compile.

    The file must have one `commit:` line and one `fe:parse` line. A cached build writes none.
    """
    headers = len(re.findall(r'^commit:', text, re.M))
    times = re.findall(r'^BenchmarkCompile:\S*:fe:parse\s+\d+\s+(\d+(?:\.\d+)?)\s+ns/op', text,
                       re.M)
    if headers != 1 or len(times) != 1:
        raise Fail(f'{what}: the -bench file has {headers} commit lines and {len(times)} fe:parse '
                   'lines, and it needs one of each (a cached build writes none)')
    return float(times[0]) / 1e9


def rate(count, seconds):
    """`count` per second as part A prints it: no suffix below 1000, then k, M and G."""
    if count == 0 or seconds <= 0:
        return '-'
    per_second = count / seconds
    whole = int(per_second + 0.5)
    if whole < 1000:
        return str(whole)
    for divisor, suffix in ((1e3, 'k'), (1e6, 'M')):
        text = f'{per_second / divisor:.1f}'
        # The text decides: "999.9" has 5 characters, and "1000.0" has 6.
        if len(text) < 6:
            return text + suffix
    return f'{per_second / 1e9:.1f}G'


def past_empty(tokens, time, empty):
    """The tokens per second after the time of the empty program, or a dash if nothing is left."""
    return rate(tokens, time - empty)


def instructions(stderr):
    """The instructions that `perf stat -x,` counted in `stderr`, or None when it gave no number.

    A hybrid CPU has one line for each core type, such as `cpu_core/instructions/u`. The result
    is their sum. A line with no number, such as `<not counted>`, counts as nothing.
    """
    counts = []
    for line in (stderr or '').splitlines():
        fields = line.split(',')
        if len(fields) > 2 and 'instructions' in fields[2] and fields[0].isdigit():
            counts.append(int(fields[0]))
    return sum(counts) if counts else None


def first_line(text):
    return next((line.strip() for line in (text or '').splitlines() if line.strip()), '')


def try_run(deadline, cmd, what, lang=None):
    """Runs an optional tool. A program that is not on PATH or fails gives None."""
    if shutil.which(cmd[0]) is None:
        return None
    r = deadline.run(cmd, what, check=False, env=environment(lang))
    return r if r.returncode == 0 else None


def find_linker(lang, args, deadline):
    """The linker line of `lang` and whether the linker is LLD.

    The linker prints its version, because the compiler passes it `--version`.
    """
    what = f'{NAMES[lang]} linker'
    if lang == 'go':
        r = try_run(deadline, ['go', 'tool', 'link', '-V'], what, lang)
        return (first_line(r.stdout) if r else '') or 'unknown', False
    work = args.out / lang / 'linker'
    empty = work / f'empty.{EXTENSIONS[lang]}'
    try:
        work.mkdir(parents=True, exist_ok=True)
        empty.write_text(empty_program(lang))
    except OSError as e:
        raise Fail(f'{what}: {e.filename or work}: {e.strerror}') from None
    prog = str(work / 'prog')
    cmd = {
        'sisu': ['cc', '-Wl,--version'],
        'rust': ['rustc', '--edition', '2024', '-C', 'link-arg=-Wl,--version', '-o', prog,
                 str(empty)],
        'cpp': ['clang++', '-std=c++20', '-Wl,--version', '-o', prog, str(empty)],
    }[lang]
    # The build can fail, as `ld --version` makes no file. The LLD line is in the output also then,
    # and the script must find it, because LLD gets a thread limit. Any other line needs exit 0.
    r = deadline.run(cmd, what, check=False, env=environment(lang)) if shutil.which(cmd[0]) \
        else None
    lines = ((r.stdout or '') + '\n' + (r.stderr or '')).splitlines() if r else []
    lld = next((line.strip() for line in lines if 'LLD' in line), None)
    # rustc shows the output of its linker as a warning on stderr.
    shown = 'warning: linker stdout: '
    if lld:
        return lld.removeprefix(shown), True
    warned = next((line.removeprefix(shown) for line in lines if line.startswith(shown)), None)
    return warned or (r and r.returncode == 0 and first_line(r.stdout)) or 'unknown', False


def available(lang):
    return COMPILERS[lang] is None or shutil.which(COMPILERS[lang]) is not None


def tool_line(deadline, cmd, lang=None):
    """The version line of a tool."""
    if shutil.which(cmd[0]) is None:
        return 'not on PATH'
    r = deadline.run(cmd, f'{cmd[0]} version', check=False, env=environment(lang))
    ok = r.returncode == 0
    return (ok and (first_line(r.stdout) or first_line(r.stderr))) or 'unknown'


def cpu_model():
    try:
        for line in Path('/proc/cpuinfo').read_text().splitlines():
            if line.startswith('model name'):
                return line.split(':', 1)[1].strip()
    except OSError:
        pass
    return platform.machine() or 'unknown'


def header_lines(args, deadline, linkers):
    """The lines that go with the table: the machine, the tools and the linkers."""
    git = try_run(deadline, ['git', 'rev-parse', '--short', 'HEAD'], 'git')
    go_version = tool_line(deadline, ['go', 'version'], 'go')
    lines = [
        f'commit: {(first_line(git.stdout) if git else "") or "unknown"}',
        f'date: {time.strftime("%Y-%m-%d")}',
        f'cpu: {cpu_model()}',
        f'kernel: {platform.release()}',
        f'sisu: {tool_line(deadline, [args.sisu, "--version"])}',
        f'rustc: {tool_line(deadline, ["rustc", "--version"])}',
        f'clang++: {tool_line(deadline, ["clang++", "--version"])}',
        f'go: {go_version}',
        f'rustfmt: {tool_line(deadline, ["rustfmt", "--version"])}',
        f'clang-format: {tool_line(deadline, ["clang-format", "--version"])}',
        f'gofmt: from {go_version}',
    ]
    return lines + [f'linker ({NAMES[lang]}): {line}' for lang, line in linkers.items()]


class Measurer:
    """Runs a command several times and keeps the best time."""

    def __init__(self, args, deadline, perf):
        self.args, self.deadline, self.perf = args, deadline, perf
        self.serial = itertools.count()

    def run(self, cmd, what, lang, src, text, bench, wrap=False):
        """One run of `cmd`: its time in seconds (the `fe:parse` time when `bench` is a file),
        and its stderr. Before a Go command, the first line of the input changes."""
        if lang == 'go':
            stamp = f'// run {time.time_ns()} {next(self.serial)}'
            try:
                src.write_text(text.replace('// run', stamp, 1))
                if bench:
                    bench.unlink(missing_ok=True)
            except OSError as e:
                raise Fail(f'{what}: {e.filename or src}: {e.strerror}') from None
        full = ['perf', 'stat', '-x,', '-e', 'instructions:u', '--', *cmd] if wrap else cmd
        # An `--emit` row prints its stage. It goes to /dev/null, so the script does not read it.
        stdout = subprocess.DEVNULL if lang == 'sisu' and '--emit' in cmd else subprocess.PIPE
        start = time.perf_counter()
        r = build(self.deadline, full, what, env=environment(lang), stdout=stdout)
        elapsed = time.perf_counter() - start
        if bench:
            try:
                content = bench.read_text(errors='replace') if bench.exists() else ''
            except OSError as e:
                raise Fail(f'{what}: {bench}: {e.strerror}') from None
            elapsed = go_fe_parse(content, what)
        return elapsed, r.stderr

    def cell(self, cmd, what, lang, src, text, bench, count_instructions):
        """The best time of `--runs` runs, and the instructions of one more run under perf."""
        best = min(self.run(cmd, what, lang, src, text, bench)[0] for _ in range(self.args.runs))
        counted = None
        if count_instructions and self.perf and not bench:
            counted = instructions(self.run(cmd, what, lang, src, text, bench, wrap=True)[1])
        return best, counted


def measure(args, units, sizes, deadline):
    """Times each row, size and language. Returns the header lines and the list of Result."""
    if available('go') and "'" in str(args.out):
        # The Go `-bench` flag has the path, and Go reads a quote in its flags as a quote.
        raise Fail(f"--out may not have a ' in it, because Go cannot read a ' in its -gcflags: "
                   f'{args.out}')
    linkers = {lang: find_linker(lang, args, deadline) for lang in LANGS if available(lang)}
    lld = {lang: found[1] for lang, found in linkers.items()}
    # The perf test is the one of the spec: it fails on a machine that denies the counters.
    perf = try_run(deadline, ['perf', 'stat', '-e', 'instructions:u', 'true'], 'perf') is not None
    runner = Measurer(args, deadline, perf)
    done, empties, results = {}, {}, []
    for size in sizes:
        for lang in LANGS:
            ext = EXTENSIONS[lang]
            work = args.out / lang / str(size)
            src, exe = work / f'input.{ext}', work / 'prog'
            text = generate(lang, units[lang], size)
            counts = (text.count('\n'), tokens(text, str(src)))
            if lang not in linkers:
                print(f'skipped: {NAMES[lang]}, size {size}, {COMPILERS[lang]} is not on PATH')
                results += [Result(row, size, lang, *counts, note='skipped') for row in ROWS]
                continue
            empty_dir = args.out / lang / 'empty'
            empty_src, empty_exe = empty_dir / f'input.{ext}', empty_dir / 'prog'
            try:
                work.mkdir(parents=True, exist_ok=True)
                empty_dir.mkdir(parents=True, exist_ok=True)
                src.write_text(text)
                empty_src.write_text(empty_program(lang))
            except OSError as e:
                raise Fail(f'{NAMES[lang]}, size {size}: {e.filename or work}: '
                           f'{e.strerror}') from None
            for row in ROWS:
                cmd = command(row, lang, src, exe, lld[lang], sisu=args.sisu)
                if cmd is None:
                    results.append(Result(row, size, lang, *counts, note='none'))
                    continue
                what = f'{NAMES[lang]}, size {size}, {row}'
                bench = Path(f'{exe}.bench') if lang == 'go' and row in ROWS[:2] else None
                # Two rows with the same command have the same measurement.
                key = (lang, tuple(cmd))
                if key not in done:
                    done[key] = runner.cell(cmd, what, lang, src, text, bench, True)
                empty_cmd = command(row, lang, empty_src, empty_exe, lld[lang], sisu=args.sisu)
                empty_key = (lang, tuple(empty_cmd))
                if empty_key not in empties:
                    empty_bench = Path(f'{empty_exe}.bench') if bench else None
                    empties[empty_key] = runner.cell(
                        empty_cmd, f'{NAMES[lang]}, empty program, {row}', lang, empty_src,
                        empty_program(lang), empty_bench, False)[0]
                best, counted = done[key]
                results.append(Result(row, size, lang, *counts, best, empties[empty_key], counted))
    return header_lines(args, deadline, {lang: found[0] for lang, found in linkers.items()}), \
        results


TABLE_HEADERS = ('Row', 'Size', 'Language', 'Lines', 'Tokens', 'Time', 'Tokens/s',
                 'Tokens/s past empty', 'Lines/s', 'Instructions/token')


def duration(seconds):
    return f'{seconds * 1000:.1f} ms' if seconds < 1 else f'{seconds:.2f} s'


def table(header, results):
    """The header lines as a list, then a markdown table with a line for each Result."""
    lines = [f'- {line}' for line in header] + ['']
    lines.append('| ' + ' | '.join(TABLE_HEADERS) + ' |')
    lines.append('| ' + ' | '.join(['---'] * len(TABLE_HEADERS)) + ' |')
    for r in sorted(results, key=lambda r: (ROWS.index(r.row), r.size, LANGS.index(r.lang))):
        if r.time is None:
            rest = [r.note, '-', '-', '-', '-']
        else:
            per_token = f'{r.instructions / r.tokens:.0f}' if r.instructions and r.tokens else '-'
            rest = [duration(r.time), rate(r.tokens, r.time),
                    past_empty(r.tokens, r.time, r.empty) if r.empty is not None else '-',
                    rate(r.lines, r.time), per_token]
        cells = [r.row, r.size, NAMES[r.lang], r.lines, r.tokens, *rest]
        lines.append('| ' + ' | '.join(str(c) for c in cells) + ' |')
    return '\n'.join(lines)


def parse_sizes(text):
    try:
        sizes = sorted({int(part) for part in text.split(',')})
    except ValueError:
        raise argparse.ArgumentTypeError(f'sizes are whole numbers with commas: {text}') from None
    if not sizes or sizes[0] < 1:
        raise argparse.ArgumentTypeError(f'a size is 1 or more: {text}')
    return sizes


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('--sisu', default=DEFAULT_SISU, help='the sisu executable')
    parser.add_argument('--units', type=Path, default=Path('bench/compile'))
    parser.add_argument('--out', type=Path, default=Path('target/compile-speed'))
    parser.add_argument('--sizes', type=parse_sizes, default=[10, 100, 1000],
                        help='copies of the unit, with commas (default 10,100,1000)')
    parser.add_argument('--runs', type=int, default=5, help='runs of each command (default 5)')
    parser.add_argument('--check-only', action='store_true',
                        help='check the smallest size and time nothing')
    parser.add_argument('--time-bound', type=float, help='total seconds (default 7200, 600 with '
                        '--check-only)')
    args = parser.parse_args(argv)
    if args.runs < 1:
        parser.error('--runs is 1 or more')
    deadline = Deadline(args.time_bound or (600 if args.check_only else 7200))
    try:
        units = read_units(args.units)
        check_no_include(args.units / 'unit.cpp', units['cpp'])
        check_style(args.units, deadline)
        sizes = args.sizes[:1] if args.check_only else args.sizes
        check_sizes(args, units, sizes, deadline)
        if not args.check_only:
            header, results = measure(args, units, sizes, deadline)
            print(table(header, results))
    except Fail as e:
        print(f'compile_speed: {e}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
