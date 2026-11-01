#!/usr/bin/env python3
"""Compares the compile speed of Sisu with rustc, clang++ and Go on one program.

Usage: python3 scripts/compile_speed.py [--check-only] [--sizes 10,100,1000] [--runs 5]
       [--sisu target/release/sisu] [--units bench/compile] [--out target/compile-speed]
       [--time-bound SECONDS]

`bench/compile/` holds one unit in each language. The script repeats the unit to make an input of
each size, builds and runs each input, and stops with exit code 1 when a compiler gives a warning
or an input prints something other than the Sisu input prints. `--check-only` does this at the
smallest size and times nothing. The script does not build `sisu`: `make compile-speed` does.

Every command runs through one `Deadline`. The time bound is for the whole run. A command that runs
over it is killed with its processes, and the script exits with code 1 and names the input.
"""

import argparse
import os
import re
import shlex
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

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

        It captures each of stdout and stderr as text, unless `kwargs` gives that stream. On a
        timeout, it kills the whole process group and raises Fail. It also raises Fail when the
        program is not found or its exit code is not 0, unless `check=False` is given.
        """
        check = kwargs.pop('check', True)
        left = self.end - self.clock()
        if left <= 0:
            raise Fail(f'{what}: ran over the time bound')
        kwargs.setdefault('stdout', subprocess.PIPE)
        kwargs.setdefault('stderr', subprocess.PIPE)
        kwargs.setdefault('text', True)
        shown = shlex.join(str(a) for a in cmd)
        try:
            proc = subprocess.Popen(cmd, start_new_session=True, **kwargs)
        except FileNotFoundError:
            raise Fail(f'{what}: {shown}: the program is not found') from None
        try:
            out, err = proc.communicate(timeout=left)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            proc.communicate()
            raise Fail(f'{what}: ran over the time bound') from None
        if check and proc.returncode != 0:
            if isinstance(err, bytes):
                err = err.decode(errors='replace')
            lines = (err or '').splitlines()[:STDERR_LINES]
            raise Fail('\n'.join([f'{what}: {shown} exited with code {proc.returncode}', *lines]))
        return subprocess.CompletedProcess(cmd, proc.returncode, out, err)


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
    return text + '\n' + main_of(lang, copies)


def main_of(lang, copies):
    """The `main` that adds the result of each copy in one statement and prints the sum."""
    calls = [f'total = total + run{i}()' for i in range(copies)]
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
        'go': 'package main\n\nfunc main() {}\n',
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
        return [*base, f'-gcflags=-bench={bench} -c=1', '-o', out, src]
    flags = '-N -l -c=1' if row == ROWS[3] else '-c=1'
    return [*base, f'-gcflags={flags}', '-o', out, src]


def environment(lang):
    """The environment of a command: Go runs on one thread."""
    return {**os.environ, 'GOMAXPROCS': '1'} if lang == 'go' else None


def warnings(stderr):
    """The lines of a build's stderr that have `warning`, in any case."""
    return [line for line in stderr.splitlines() if 'warning' in line.lower()]


def check_no_include(path, text):
    """Stops when the C++ unit has an `#include`: a header would add the library to parse."""
    for number, line in enumerate(text.splitlines(), 1):
        if re.match(r'\s*#\s*include\b', line):
            raise Fail(f'{path}:{number}: the C++ unit may not include a header: {line.strip()}')


def read_units(units_dir):
    units = {}
    for lang in LANGS:
        path = units_dir / f'unit.{EXTENSIONS[lang]}'
        try:
            units[lang] = path.read_text()
        except OSError as e:
            raise Fail(f'{path}: cannot read the unit: {e.strerror}') from None
    return units


def check_style(units_dir, units, deadline):
    """Runs the check mode of each formatter that is on PATH, and says which ones it skips."""
    for lang, cmd in FORMATTERS.items():
        path = units_dir / f'unit.{EXTENSIONS[lang]}'
        if shutil.which(cmd[0]) is None:
            print(f'skipped: the {NAMES[lang]} style check, {cmd[0]} is not on PATH')
            continue
        r = deadline.run([*cmd, str(path)], f'{NAMES[lang]} style check', check=False)
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
            work.mkdir(parents=True, exist_ok=True)
            src, exe = work / f'input.{EXTENSIONS[lang]}', work / 'prog'
            src.write_text(generate(lang, units[lang], size))
            cmd = command(ROWS[3], lang, src, exe, False, sisu=args.sisu)
            built = deadline.run(cmd, what, env=environment(lang))
            found = warnings(built.stderr or '')
            if found:
                raise Fail(f'{what}: the build gave a warning: {found[0].strip()}')
            printed = deadline.run([str(exe)], what, text=False).stdout
            if lang == 'sisu':
                reference = printed
            elif printed != reference:
                raise Fail(f'{what}: the output {printed!r} differs from the Sisu output '
                           f'{reference!r}')
            print(f'checked {what}: prints {printed!r}')


def measure(args, units, sizes, deadline):
    raise Fail('measure: not written yet')


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
    deadline = Deadline(args.time_bound or (600 if args.check_only else 7200))
    try:
        units = read_units(args.units)
        check_no_include(args.units / 'unit.cpp', units['cpp'])
        check_style(args.units, units, deadline)
        sizes = args.sizes[:1] if args.check_only else args.sizes
        check_sizes(args, units, sizes, deadline)
        if not args.check_only:
            measure(args, units, sizes, deadline)
    except Fail as e:
        print(f'compile_speed: {e}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
