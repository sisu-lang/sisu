#!/usr/bin/env python3
"""Finds the shapes of program for which a stage of the Sisu compiler is superlinear.

Usage: python3 scripts/scaling.py [--shapes a,b] [--counts FILE] [--out target/scaling]
       [--sisu target/release/sisu] [--time-bound SECONDS]

A shape is a kind of program that grows in one direction. For each shape, the script makes the
program at size 0 and at the sizes n, 2n, 4n and 8n. It runs `sisu build --emit ir-raw` on each
under callgrind, and reads the instruction count of each stage function. It prints the growth of
each stage (its count less its count at size 0), the ratio at each doubling, and the verdict.

Before callgrind, `sisu check` runs on each shape at size 0 and at 8n. A shape that gives a
diagnostic stops the run: without this check, the failure shows as a missing stage function.

The script exits with code 1 when a superlinear case has no row in `KNOWN`, when a row of `KNOWN`
looks fixed, and when a check or a command fails. `--counts FILE` reads the `counts.json` of an
earlier run and measures nothing. Every measurement command runs through one `Deadline`.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from fractions import Fraction
from pathlib import Path

from compile_speed import Deadline, Fail

# The name of a stage and its function, in the order of the stages.
STAGES = [
    ('lex', 'sisuc::lexer::lex'),
    ('parse', 'sisuc::parser::parse'),
    ('check', 'sisuc::check::check'),
    ('mono', 'sisuc::mono::instantiate'),
    ('mir', 'sisuc::mir::build'),
    ('codegen', 'sisuc::codegen::compile'),
]
# The cases that are superlinear today: (shape, stage, issue). The pull request that fixes a case
# removes its row.
KNOWN = [
    ('array literal depth', 'check', 344),
]
ANNOTATE = ['callgrind_annotate', '--inclusive=yes', '--threshold=100', '--show=Ir', '--auto=no']
DIAGNOSTIC = re.compile(r'^(error|warning)\[([EW]\d{4})\]')
DEFAULT_SISU = 'target/release/sisu'
DEFAULT_OUT = 'target/scaling'
ROOT = Path(__file__).resolve().parent.parent


# The width of a number in a name. The largest size of any shape is 8 * 250 = 2000, so each name
# has the same length at each size, and the text of a program grows with the number of copies.
WIDTH = 4


def tag(i):
    """The number `i` of a copy, with the width of the largest size."""
    return f'{i:0{WIDTH}d}'


def nest(k, wrap, core):
    """`core` inside `k` layers: each layer is `wrap(inner)`."""
    for _ in range(k):
        core = wrap(core)
    return core


def functions(k):
    funs = ['fn step0000() -> i64 {\n    1\n}\n'] if k else []
    funs += [f'fn step{tag(i)}() -> i64 {{\n    step{tag(i - 1)}() + 1\n}}\n' for i in range(1, k)]
    shown = f'"{{step{tag(k - 1)}()}}"' if k else '"none"'
    return '\n'.join(funs) + f'\nfn main() {{\n    print({shown})\n}}\n'


def statements(k):
    lets = ''.join(f'    let a{tag(i)} = a{tag(i - 1)} + 1\n' for i in range(1, k + 1))
    return f'fn main() {{\n    let a0000 = 0\n{lets}    print("{{a{tag(k)}}}")\n}}\n'


def expression_depth(k):
    return f'fn main() {{\n    let x = {nest(k, lambda s: f"1 + ({s})", "1")}\n    print("{{x}}")\n}}\n'


def block_depth(k):
    # No indentation: it would make the text grow with the square of the depth, and so the lexer.
    body = nest(k, lambda s: f'if c {{\n{s}\n}}', 'print("in")')
    return f'fn main() {{\n    let c = true\n    print("{{c}}")\n{body}\n}}\n'


def generic_call_depth(k):
    call = nest(k + 1, lambda s: f'id({s})', '1')
    return ('fn id<T>(x: T) -> T {\n    x\n}\n\n'
            f'fn main() {{\n    let v = {call}\n    print("{{v}}")\n}}\n')


def array_literal_depth(k):
    return (f'fn main() {{\n    let a = {nest(k + 1, lambda s: f"[{s}]", "1")}\n'
            '    print("{a.len()}")\n}\n')


def array_literal_width(k):
    return f'fn main() {{\n    let a: [i64] = [{", ".join(["7"] * k)}]\n    print("{{a.len()}}")\n}}\n'


def match_arms(k):
    names = ['First'] + [f'V{tag(i)}' for i in range(k)]
    variants = ''.join(f'    {name}\n' for name in names)
    arms = ''.join(f'        {name} => 1\n' for name in names)
    built = ', '.join(f'Kind.{name}' for name in names)
    return (f'enum Kind {{\n{variants}}}\n\nfn pick(k: Kind) -> i64 {{\n    match k {{\n{arms}    }}\n}}\n\n'
            f'fn main() {{\n    let mut total = 0\n    for k in [{built}] {{\n        total += pick(k)\n'
            '    }\n    print("{total}")\n}\n')


def fields(k):
    declared = ''.join(f'    f{tag(i)}: i64\n' for i in range(k))
    given = ''.join(f', f{tag(i)}: 1' for i in range(k))
    shown = ''.join(f'    print("{{r.f{tag(i)}}}")\n' for i in range(k))
    return (f'class Row {{\n    id: i64\n{declared}}}\n\n'
            f'fn main() {{\n    let r = Row(id: 0{given})\n    print("{{r.id}}")\n{shown}}}\n')


def conformers(k):
    classes = ''.join(f'class C{tag(i)}: Speak {{\n    fn say(self) -> i64 {{\n        1\n    }}\n}}\n\n'
                      for i in range(k))
    calls = ''.join(f'    total += ask(C{tag(i)}())\n' for i in range(k))
    return ('interface Speak {\n    fn say(self) -> i64\n}\n\n'
            'class Base: Speak {\n    fn say(self) -> i64 {\n        0\n    }\n}\n\n' + classes +
            'fn ask<T: Speak>(x: T) -> i64 {\n    x.say()\n}\n\n'
            f'fn main() {{\n    let mut total = 0\n    total += ask(Base())\n{calls}    print("{{total}}")\n}}\n')


def type_depth(k):
    pair = nest(k + 1, lambda s: f'Pair(a: {s}, b: 1)', '1')
    return ('class Pair<A, B> {\n    a: A\n    b: B\n}\n\n'
            f'fn main() {{\n    let v = {pair}\n    let _first = v.a\n    print("{{v.b}}")\n}}\n')


def interpolations(k):
    return f'fn main() {{\n    let x = 1\n    let s = "{"{x}" * (k + 1)}"\n    print(s)\n}}\n'


def live_locals(k):
    locals_ = ''.join(f'    let live{tag(i)} = "v"\n' for i in range(k))
    uses = ''.join(f'    total += live{tag(i)}.len()\n' for i in range(k))
    return ('fn main() {\n' + locals_ + '    let mut total = 0\n    let mut i = 0\n'
            '    while i < 3 {\n        i += 1\n        total += i\n    }\n' + uses +
            '    print("{total}")\n}\n')


def method_chain(k):
    chain = 'c' + '.next()' * (k + 1)
    return ('class Chain {\n    n: i64\n\n    fn next(self) -> Chain {\n        self\n    }\n}\n\n'
            f'fn main() {{\n    let c = Chain(n: 1)\n    let d = {chain}\n    print("{{d.n}}")\n}}\n')


# The name of a shape, its `n` and the program at a size. The sizes are 0, n, 2n, 4n and 8n.
SHAPES = {
    'functions': (250, functions),
    'statements': (250, statements),
    'expression depth': (12, expression_depth),
    'block depth': (12, block_depth),
    'generic call depth': (24, generic_call_depth),
    'array literal depth': (24, array_literal_depth),
    'array literal width': (250, array_literal_width),
    'match arms': (50, match_arms),
    'fields': (50, fields),
    'conformers': (50, conformers),
    'type depth': (24, type_depth),
    'interpolations': (50, interpolations),
    'live locals': (50, live_locals),
    'method chain': (24, method_chain),
}


# The numbers of the rule. They are exact fractions: a float would turn `2.45 - 0.05` into
# 2.4000000000000004, and a ratio on a boundary would fall on the wrong side.
SUPERLINEAR_R3 = Fraction('2.3')
SUPERLINEAR_R2 = Fraction('2.1')
FALL_ALLOWED = Fraction('0.05')
STEEP = Fraction(3)
KEPT_R3 = Fraction('2.15')
FALL_FIXED = Fraction('0.15')


def judge(growth, total_8n):
    """The verdict for one stage, and its three ratios as exact fractions.

    `growth` is the growth at n, 2n, 4n and 8n. `total_8n` is the sum of the growth at 8n of the
    six stages. A ratio is None when its divisor is 0 or less.

    The verdict is `not judged` when the growth at 8n is not positive, or is under 1% of `total_8n`.
    Else it is `no ratio` when a ratio is None. Else it is `superlinear` or `linear`.
    """
    ratios = [Fraction(b, a) if a > 0 else None for a, b in zip(growth, growth[1:])]
    if growth[3] <= 0 or growth[3] * 100 < total_8n:
        return 'not judged', ratios
    if None in ratios:
        return 'no ratio', ratios
    r1, r2, r3 = ratios
    if r3 > SUPERLINEAR_R3 and r2 >= SUPERLINEAR_R2 and (r3 >= r1 - FALL_ALLOWED or r3 >= STEEP):
        return 'superlinear', ratios
    return 'linear', ratios


def looks_fixed(verdict, ratios):
    """True when the stage of a known row is not superlinear any more, and not in the gap.

    The gap keeps a row from flapping when a change to the linear part moves a ratio a little.
    """
    if verdict in ('not judged', 'no ratio'):
        return True
    r1, _, r3 = ratios
    return r3 < KEPT_R3 or (r3 < STEEP and r3 < r1 - FALL_FIXED)


KNOWN_PREFIX = 'known case: '


def compare(verdicts, known, measured):
    """Compares the verdicts with the known list.

    Returns the lines for known cases, the lines for failures and the exit code. A row of `known`
    whose shape is not in `measured` is skipped.
    """
    rows = {(shape, stage): issue for shape, stage, issue in known}
    lines, failures = [], []
    for (shape, stage), (verdict, _) in verdicts.items():
        if verdict == 'superlinear' and (shape, stage) not in rows:
            failures.append(f'{shape}, {stage} is superlinear: open an issue and add a row')
    for (shape, stage), issue in rows.items():
        if shape not in measured:
            continue
        case = f'{shape}, {stage} (#{issue})'
        if looks_fixed(*verdicts[shape, stage]):
            failures.append(f'#{issue} looks fixed: remove its row ({shape}, {stage})')
        else:
            lines.append(KNOWN_PREFIX + case)
    return lines, failures, 1 if failures else 0


ANNOTATE_LINE = re.compile(r'^\s*([\d,]+) \(\s*[\d.]+%\)\s+(.*)$')
OBJECT_FIELD = re.compile(r' \[[^\]]*\]$')


def stage_counts(annotate):
    """The count of each stage, from the output of `callgrind_annotate`.

    A line is `<count> (<percent>)  <file>:<function> [<object>]`. A line without the object field
    is a row for an inlined file, not a function entry, and is skipped.

    A line counts for a stage when its function is the function of the stage. It also counts when
    the function has `.` and a suffix after it (an LTO clone). A closure of the function has
    another name and does not count. It raises Fail when a stage has no line or more than one.
    """
    rows = []
    for line in annotate.splitlines():
        match = ANNOTATE_LINE.match(line)
        if match is None or ':' not in match[2]:
            continue
        field = OBJECT_FIELD.search(match[2])
        if field is not None:
            where = match[2][:field.start()]
            rows.append((where, where.split(':', 1)[1], int(match[1].replace(',', ''))))
    counts = {}
    for stage, function in STAGES:
        found = [(where, count) for where, name, count in rows
                 if name == function or name.startswith(function + '.')]
        if not found:
            raise Fail(f'{function} is not in the callgrind output')
        if len(found) > 1:
            names = ', '.join(where for where, _ in found)
            raise Fail(f'{function} is in the callgrind output {len(found)} times: {names}')
        counts[stage] = found[0][1]
    return counts


def is_count(value):
    """True for an integer. A boolean is not a count."""
    return type(value) is int


def invalid(data):
    """What is wrong with the structure of a counts file, or None."""
    shapes = data.get('shapes') if isinstance(data, dict) else None
    if not isinstance(shapes, dict):
        return 'it has no "shapes" object'
    for name, shape in shapes.items():
        sizes = shape.get('sizes') if isinstance(shape, dict) else None
        if not (isinstance(sizes, list) and len(sizes) == 5 and all(is_count(s) for s in sizes)
                and sizes[1] > 0 and sizes == [0, sizes[1], 2 * sizes[1], 4 * sizes[1], 8 * sizes[1]]):
            return f'{name}: "sizes" must be 0, n, 2n, 4n and 8n, with n above 0'
        stages = shape.get('counts')
        for stage, _ in STAGES:
            values = stages.get(stage) if isinstance(stages, dict) else None
            if not (isinstance(values, list) and len(values) == 5 and all(is_count(v) for v in values)):
                return f'{name}: "counts" must have a list of 5 integers for {stage}'
    return None


def load(path):
    """The counts of an earlier run (`counts.json`). It raises Fail when the file is not valid."""
    try:
        data = json.loads(path.read_text())
    except (OSError, ValueError) as e:
        raise Fail(f'{path}: {e}') from None
    problem = invalid(data)
    if problem is not None:
        raise Fail(f'{path}: {problem}')
    return data


def verdicts(counts):
    """`judge` for each shape and stage of `counts`: {(shape, stage): (verdict, ratios)}."""
    result = {}
    for shape, data in counts['shapes'].items():
        growth = {stage: [c - data['counts'][stage][0] for c in data['counts'][stage][1:]]
                  for stage, _ in STAGES}
        total = sum(g[3] for g in growth.values())
        for stage, _ in STAGES:
            result[shape, stage] = judge(growth[stage], total)
    return result


def table(counts, verdicts):
    """The markdown table of the run: the growths, the ratios and the verdict of each stage."""
    lines = ['# Scaling sweep', '']
    for key in ('commit', 'cpu', 'valgrind', 'glibc'):
        lines.append(f'- {key}: {counts.get(key, "unknown")}')
    for shape, data in counts['shapes'].items():
        sizes = ', '.join(str(k) for k in data['sizes'])
        lines += ['', f'## {shape}', '', f'Sizes: {sizes}. Growth is the count less the count at size 0.',
                  '', '| stage | g(n) | g(2n) | g(4n) | g(8n) | r1 | r2 | r3 | verdict |',
                  '| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |']
        for stage, _ in STAGES:
            cs = data['counts'][stage]
            verdict, ratios = verdicts[shape, stage]
            growth = [str(c - cs[0]) for c in cs[1:]]
            shown = ['-' if r is None else f'{float(r):.2f}' for r in ratios]
            lines.append(f'| {stage} | ' + ' | '.join(growth + shown) + f' | {verdict} |')
    return '\n'.join(lines) + '\n'


def glibc():
    """The version of the C library, or `unknown`."""
    try:
        return os.confstr('CS_GNU_LIBC_VERSION') or 'unknown'
    except (ValueError, OSError):
        return 'unknown'


def cpu_model():
    """The model name of the first CPU, or `unknown`."""
    try:
        for line in Path('/proc/cpuinfo').read_text().splitlines():
            if line.startswith('model name'):
                return line.split(':', 1)[1].strip()
    except OSError:
        pass
    return 'unknown'


def commit():
    """The short hash of HEAD in this repository, or `unknown`."""
    try:
        done = subprocess.run(['git', '-C', str(ROOT), 'rev-parse', '--short', 'HEAD'],
                              capture_output=True, text=True)
    except OSError:
        return 'unknown'
    return done.stdout.strip() if done.returncode == 0 and done.stdout.strip() else 'unknown'


def check_clean(deadline, sisu, path, what):
    """Runs `sisu check`. A failed command, or a diagnostic on stderr, raises Fail."""
    for line in deadline.run([sisu, 'check', path], what).stderr.splitlines():
        match = DIAGNOSTIC.match(line)
        if match:
            raise Fail(f'{what}: {match[2]}')


def measure(args, counts):
    """Measures the shapes of `args` and puts each into `counts['shapes']` when it is done."""
    if shutil.which('valgrind') is None:
        raise Fail('valgrind is not on PATH')
    deadline = Deadline(args.time_bound)
    counts.update(commit=commit(), cpu=cpu_model(), glibc=glibc())
    counts['valgrind'] = deadline.run(['valgrind', '--version'], 'valgrind --version').stdout.splitlines()[0]
    for name in args.shapes or SHAPES:
        n, make = SHAPES[name]
        sizes = [0, n, 2 * n, 4 * n, 8 * n]
        directory = Path(args.out) / name.replace(' ', '-')
        directory.mkdir(parents=True, exist_ok=True)
        files = [directory / f'size-{k}.sisu' for k in sizes]
        for k, path in zip(sizes, files):
            path.write_text(make(k))
        for k, path in ((sizes[0], files[0]), (sizes[-1], files[-1])):
            check_clean(deadline, args.sisu, path, f'{name}, size {k}')
        per_stage = {stage: [] for stage, _ in STAGES}
        with tempfile.TemporaryDirectory() as tmp:
            profile = os.path.join(tmp, 'callgrind.out')
            for k, path in zip(sizes, files):
                what = f'{name}, size {k}'
                deadline.run(['valgrind', '--tool=callgrind', f'--callgrind-out-file={profile}',
                              args.sisu, 'build', '--emit', 'ir-raw', path], what,
                             stdout=subprocess.DEVNULL)
                found = stage_counts(deadline.run([*ANNOTATE, profile], what).stdout)
                for stage, count in found.items():
                    per_stage[stage].append(count)
        counts['shapes'][name] = {'sizes': sizes, 'counts': per_stage}


def shape_list(text):
    """The shapes of a `--shapes` value."""
    names = text.split(',')
    for name in names:
        if name not in SHAPES:
            raise argparse.ArgumentTypeError(f'{name!r} is not a shape: {", ".join(SHAPES)}')
    return names


def main(argv):
    parser = argparse.ArgumentParser(description='Find the shapes for which a stage is superlinear.')
    parser.add_argument('--shapes', type=shape_list, help='the shapes to measure, separated by commas')
    parser.add_argument('--counts', help='read the counts.json of an earlier run, and measure nothing')
    parser.add_argument('--out', default=DEFAULT_OUT, help='the directory for table.md and counts.json')
    parser.add_argument('--sisu', default=DEFAULT_SISU, help='the compiler')
    parser.add_argument('--time-bound', type=float, default=1800, help='seconds for the whole run')
    args = parser.parse_args(argv)
    counts = {'commit': 'unknown', 'cpu': 'unknown', 'valgrind': 'unknown', 'glibc': 'unknown',
              'shapes': {}}
    failure = None
    try:
        if args.counts:
            counts = load(Path(args.counts))
        else:
            measure(args, counts)
    except Fail as e:
        failure = str(e)
        if args.counts:
            # Write nothing: the output directory may hold the file that was read.
            print(f'scaling: {failure}', file=sys.stderr)
            return 1
    results = verdicts(counts)
    text = table(counts, results)
    print(text)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / 'counts.json').write_text(json.dumps(counts, indent=2) + '\n')
    if failure:
        (out / 'table.md').write_text(f'{text}\nscaling: {failure}\n')
        print(f'scaling: {failure}', file=sys.stderr)
        return 1
    (out / 'table.md').write_text(text)
    lines, failures, code = compare(results, KNOWN, set(counts['shapes']))
    for line in lines:
        print(f'scaling: {line}')
        if os.environ.get('GITHUB_ACTIONS') == 'true':
            print(f'::warning title=Known superlinear case::{line.removeprefix(KNOWN_PREFIX)}')
    for line in failures:
        print(f'scaling: {line}', file=sys.stderr)
    return code


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
