"""Unit tests for scaling.py: python3 -m unittest discover -s scripts -p 'test_scaling.py'.

A test that runs `main` on counts gives it a `counts.json` that the test writes, and a PATH with
no tool. A test that runs `main` on shapes puts stub `sisu`, `valgrind` and `callgrind_annotate`
scripts in a directory, and makes that directory the whole PATH. Each stub logs its arguments to
`log`, and fails when its arguments are not the ones that the script must give. No test starts a
real compiler or valgrind.
"""
import contextlib
import io
import json
import os
import re
import stat
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import scaling as sc
from compile_speed import Fail

# These are lines of a real `callgrind_annotate --inclusive=yes --threshold=100 --show=Ir
# --auto=no` run. The run was on the release `sisu build --emit ir-raw` of a program with 3
# functions (valgrind 3.22.0). The text keeps the header, the total and one line for each of the
# six stage functions. It keeps one closure of `check::check` and two lines of other functions.
# The last line is a row of `lexer::lex` from the inlined file `core/src/convert/num.rs`. It has
# no object field (80 instructions).
ANNOTATE = """\
--------------------------------------------------------------------------------
Ir
--------------------------------------------------------------------------------
7,489,418 (100.0%)  PROGRAM TOTALS

--------------------------------------------------------------------------------
Ir                  file:function
--------------------------------------------------------------------------------
4,817,152 (64.32%)  ???:0x000000000001f540 [/usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2]
  355,831 ( 4.75%)  ???:sisuc::codegen::compile [/src/target/release/sisu]
  306,067 ( 4.09%)  ???:sisuc::check::check [/src/target/release/sisu]
  183,335 ( 2.45%)  ???:sisuc::lexer::lex [/src/target/release/sisu]
  118,740 ( 1.59%)  ???:sisuc::parser::parse [/src/target/release/sisu]
   61,582 ( 0.82%)  ???:sisuc::mir::build [/src/target/release/sisu]
   47,035 ( 0.63%)  ???:<clap_builder::parser::parser::Parser>::get_matches_with [/src/target/release/sisu]
   20,561 ( 0.27%)  ???:sisuc::mono::instantiate [/src/target/release/sisu]
    7,069 ( 0.09%)  ???:<core::iter::adapters::filter_map::FilterMap<core::iter::adapters::enumerate::Enumerate<core::slice::iter::Iter<core::option::Option<sisuc::tir::Function>>>, sisuc::check::check::{closure#4}> as core::iter::traits::iterator::Iterator>::next [/src/target/release/sisu]
       80 ( 0.00%)  /home/mikko/.rustup/toolchains/1.99.0-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/core/src/convert/num.rs:sisuc::lexer::lex
"""
MONO_LINE = '   20,561 ( 0.27%)  ???:sisuc::mono::instantiate [/src/target/release/sisu]\n'
CHECK_LINE = '  306,067 ( 4.09%)  ???:sisuc::check::check [/src/target/release/sisu]\n'
EXPECTED_COUNTS = {'lex': 183335, 'parse': 118740, 'check': 306067, 'mono': 20561, 'mir': 61582,
                   'codegen': 355831}
KNOWN_344 = [('array literal depth', 'check', 344)]
# The callgrind counts of commit 85c95105, where issue #344 is open.
SAVED = Path(__file__).resolve().parent.parent / 'bench' / 'compile' / 'scaling-85c95105.json'
# The 12 rows of the table in scenario S10: check growths at n, 2n, 4n and 8n, the ratios, and
# the verdict.
S10 = [
    ([1000, 2290, 5610, 15540], [2.29, 2.45, 2.77], 'superlinear'),
    ([1000, 2430, 5808, 15449], [2.43, 2.39, 2.66], 'superlinear'),
    ([1000, 2420, 5760, 13766], [2.42, 2.38, 2.39], 'superlinear'),
    ([1000, 4000, 16000, 64000], [4.00, 4.00, 4.00], 'superlinear'),
    ([66, 276, 1128, 4560], [4.18, 4.09, 4.04], 'superlinear'),
    ([1000, 1980, 3960, 7880], [1.98, 2.00, 1.99], 'linear'),
    ([1000, 2560, 6246, 14741], [2.56, 2.44, 2.36], 'linear'),
    ([1000, 2100, 4620, 10395], [2.10, 2.20, 2.25], 'linear'),
    ([1000, 1990, 3920, 9680], [1.99, 1.97, 2.47], 'linear'),
    ([0, 500, 2000, 8000], [None, 4.00, 4.00], 'no ratio'),
    ([170, 146, 18, 146], [0.86, 0.12, 8.11], 'not judged'),
    ([-65, -160, -103, -109], [None, None, None], 'not judged'),
]
# The table that `main --counts` writes for the 12 rows of S10, each on its own shape, with the
# check stage as the row and the lex stage as the rest of the 100000.
S10_TABLE = """\
# Scaling sweep

- commit: abc1234
- cpu: Test CPU
- valgrind: valgrind-3.0
- glibc: glibc 2.0

## functions

Sizes: 0, 250, 500, 1000, 2000. Growth is the count less the count at size 0.

| stage | g(n) | g(2n) | g(4n) | g(8n) | r1 | r2 | r3 | verdict |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| lex | 10557 | 21115 | 42230 | 84460 | 2.00 | 2.00 | 2.00 | linear |
| parse | 0 | 0 | 0 | 0 | - | - | - | not judged |
| check | 1000 | 2290 | 5610 | 15540 | 2.29 | 2.45 | 2.77 | superlinear |
| mono | 0 | 0 | 0 | 0 | - | - | - | not judged |
| mir | 0 | 0 | 0 | 0 | - | - | - | not judged |
| codegen | 0 | 0 | 0 | 0 | - | - | - | not judged |

## statements

Sizes: 0, 250, 500, 1000, 2000. Growth is the count less the count at size 0.

| stage | g(n) | g(2n) | g(4n) | g(8n) | r1 | r2 | r3 | verdict |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| lex | 10568 | 21137 | 42275 | 84551 | 2.00 | 2.00 | 2.00 | linear |
| parse | 0 | 0 | 0 | 0 | - | - | - | not judged |
| check | 1000 | 2430 | 5808 | 15449 | 2.43 | 2.39 | 2.66 | superlinear |
| mono | 0 | 0 | 0 | 0 | - | - | - | not judged |
| mir | 0 | 0 | 0 | 0 | - | - | - | not judged |
| codegen | 0 | 0 | 0 | 0 | - | - | - | not judged |

## expression depth

Sizes: 0, 12, 24, 48, 96. Growth is the count less the count at size 0.

| stage | g(n) | g(2n) | g(4n) | g(8n) | r1 | r2 | r3 | verdict |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| lex | 10779 | 21558 | 43117 | 86234 | 2.00 | 2.00 | 2.00 | linear |
| parse | 0 | 0 | 0 | 0 | - | - | - | not judged |
| check | 1000 | 2420 | 5760 | 13766 | 2.42 | 2.38 | 2.39 | superlinear |
| mono | 0 | 0 | 0 | 0 | - | - | - | not judged |
| mir | 0 | 0 | 0 | 0 | - | - | - | not judged |
| codegen | 0 | 0 | 0 | 0 | - | - | - | not judged |

## block depth

Sizes: 0, 12, 24, 48, 96. Growth is the count less the count at size 0.

| stage | g(n) | g(2n) | g(4n) | g(8n) | r1 | r2 | r3 | verdict |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| lex | 4500 | 9000 | 18000 | 36000 | 2.00 | 2.00 | 2.00 | linear |
| parse | 0 | 0 | 0 | 0 | - | - | - | not judged |
| check | 1000 | 4000 | 16000 | 64000 | 4.00 | 4.00 | 4.00 | superlinear |
| mono | 0 | 0 | 0 | 0 | - | - | - | not judged |
| mir | 0 | 0 | 0 | 0 | - | - | - | not judged |
| codegen | 0 | 0 | 0 | 0 | - | - | - | not judged |

## generic call depth

Sizes: 0, 24, 48, 96, 192. Growth is the count less the count at size 0.

| stage | g(n) | g(2n) | g(4n) | g(8n) | r1 | r2 | r3 | verdict |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| lex | 11930 | 23860 | 47720 | 95440 | 2.00 | 2.00 | 2.00 | linear |
| parse | 0 | 0 | 0 | 0 | - | - | - | not judged |
| check | 66 | 276 | 1128 | 4560 | 4.18 | 4.09 | 4.04 | superlinear |
| mono | 0 | 0 | 0 | 0 | - | - | - | not judged |
| mir | 0 | 0 | 0 | 0 | - | - | - | not judged |
| codegen | 0 | 0 | 0 | 0 | - | - | - | not judged |

## array literal depth

Sizes: 0, 24, 48, 96, 192. Growth is the count less the count at size 0.

| stage | g(n) | g(2n) | g(4n) | g(8n) | r1 | r2 | r3 | verdict |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| lex | 11515 | 23030 | 46060 | 92120 | 2.00 | 2.00 | 2.00 | linear |
| parse | 0 | 0 | 0 | 0 | - | - | - | not judged |
| check | 1000 | 1980 | 3960 | 7880 | 1.98 | 2.00 | 1.99 | linear |
| mono | 0 | 0 | 0 | 0 | - | - | - | not judged |
| mir | 0 | 0 | 0 | 0 | - | - | - | not judged |
| codegen | 0 | 0 | 0 | 0 | - | - | - | not judged |

## array literal width

Sizes: 0, 250, 500, 1000, 2000. Growth is the count less the count at size 0.

| stage | g(n) | g(2n) | g(4n) | g(8n) | r1 | r2 | r3 | verdict |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| lex | 10657 | 21314 | 42629 | 85259 | 2.00 | 2.00 | 2.00 | linear |
| parse | 0 | 0 | 0 | 0 | - | - | - | not judged |
| check | 1000 | 2560 | 6246 | 14741 | 2.56 | 2.44 | 2.36 | linear |
| mono | 0 | 0 | 0 | 0 | - | - | - | not judged |
| mir | 0 | 0 | 0 | 0 | - | - | - | not judged |
| codegen | 0 | 0 | 0 | 0 | - | - | - | not judged |

## match arms

Sizes: 0, 50, 100, 200, 400. Growth is the count less the count at size 0.

| stage | g(n) | g(2n) | g(4n) | g(8n) | r1 | r2 | r3 | verdict |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| lex | 11200 | 22401 | 44802 | 89605 | 2.00 | 2.00 | 2.00 | linear |
| parse | 0 | 0 | 0 | 0 | - | - | - | not judged |
| check | 1000 | 2100 | 4620 | 10395 | 2.10 | 2.20 | 2.25 | linear |
| mono | 0 | 0 | 0 | 0 | - | - | - | not judged |
| mir | 0 | 0 | 0 | 0 | - | - | - | not judged |
| codegen | 0 | 0 | 0 | 0 | - | - | - | not judged |

## fields

Sizes: 0, 50, 100, 200, 400. Growth is the count less the count at size 0.

| stage | g(n) | g(2n) | g(4n) | g(8n) | r1 | r2 | r3 | verdict |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| lex | 11290 | 22580 | 45160 | 90320 | 2.00 | 2.00 | 2.00 | linear |
| parse | 0 | 0 | 0 | 0 | - | - | - | not judged |
| check | 1000 | 1990 | 3920 | 9680 | 1.99 | 1.97 | 2.47 | linear |
| mono | 0 | 0 | 0 | 0 | - | - | - | not judged |
| mir | 0 | 0 | 0 | 0 | - | - | - | not judged |
| codegen | 0 | 0 | 0 | 0 | - | - | - | not judged |

## conformers

Sizes: 0, 50, 100, 200, 400. Growth is the count less the count at size 0.

| stage | g(n) | g(2n) | g(4n) | g(8n) | r1 | r2 | r3 | verdict |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| lex | 11500 | 23000 | 46000 | 92000 | 2.00 | 2.00 | 2.00 | linear |
| parse | 0 | 0 | 0 | 0 | - | - | - | not judged |
| check | 0 | 500 | 2000 | 8000 | - | 4.00 | 4.00 | no ratio |
| mono | 0 | 0 | 0 | 0 | - | - | - | not judged |
| mir | 0 | 0 | 0 | 0 | - | - | - | not judged |
| codegen | 0 | 0 | 0 | 0 | - | - | - | not judged |

## type depth

Sizes: 0, 24, 48, 96, 192. Growth is the count less the count at size 0.

| stage | g(n) | g(2n) | g(4n) | g(8n) | r1 | r2 | r3 | verdict |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| lex | 12481 | 24963 | 49927 | 99854 | 2.00 | 2.00 | 2.00 | linear |
| parse | 0 | 0 | 0 | 0 | - | - | - | not judged |
| check | 170 | 146 | 18 | 146 | 0.86 | 0.12 | 8.11 | not judged |
| mono | 0 | 0 | 0 | 0 | - | - | - | not judged |
| mir | 0 | 0 | 0 | 0 | - | - | - | not judged |
| codegen | 0 | 0 | 0 | 0 | - | - | - | not judged |

## interpolations

Sizes: 0, 50, 100, 200, 400. Growth is the count less the count at size 0.

| stage | g(n) | g(2n) | g(4n) | g(8n) | r1 | r2 | r3 | verdict |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| lex | 12513 | 25027 | 50054 | 100109 | 2.00 | 2.00 | 2.00 | linear |
| parse | 0 | 0 | 0 | 0 | - | - | - | not judged |
| check | -65 | -160 | -103 | -109 | - | - | - | not judged |
| mono | 0 | 0 | 0 | 0 | - | - | - | not judged |
| mir | 0 | 0 | 0 | 0 | - | - | - | not judged |
| codegen | 0 | 0 | 0 | 0 | - | - | - | not judged |
"""


def growths(r1, r2, r3):
    """The growths at n, 2n, 4n and 8n for a stage with these ratios."""
    g2 = round(1000 * r1)
    g4 = round(g2 * r2)
    return [1000, g2, g4, round(g4 * r3)]


def counts_file(tmp, shapes):
    """Writes a counts.json for `shapes`: {shape: {stage: growths}}, and returns its path.

    The first stage without growths is linear and takes the rest of 100000 for the growth at 8n.
    Each other stage has no growth.
    """
    data = {'commit': 'abc1234', 'cpu': 'Test CPU', 'valgrind': 'valgrind-3.0',
            'glibc': 'glibc 2.0', 'shapes': {}}
    for name, given in shapes.items():
        n = sc.SHAPES[name][0]
        rest = 100000 - sum(g[3] for g in given.values())
        filler = next(stage for stage, _ in sc.STAGES if stage not in given)
        growth = {stage: given.get(stage, [0, 0, 0, 0]) for stage, _ in sc.STAGES}
        growth[filler] = [rest // 8, rest // 4, rest // 2, rest]
        base = 1_000_000
        data['shapes'][name] = {
            'sizes': [0, n, 2 * n, 4 * n, 8 * n],
            'counts': {stage: [base] + [base + g for g in growth[stage]] for stage, _ in sc.STAGES},
        }
    path = Path(tmp) / 'counts-in.json'
    path.write_text(json.dumps(data))
    return path


def script(path, text):
    path.write_text('#!/bin/sh\n' + text)
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


def stubs(tmp, check='', annotate=ANNOTATE, valgrind=True):
    """Writes stub `sisu`, `valgrind` and `callgrind_annotate` in `tmp/bin`; returns that PATH.

    `check` is shell text that runs first for each call of the stub `sisu`. A stub exits with
    code 2 when its arguments are not the ones that the script must give:
    `sisu check FILE`, `sisu build --emit ir-raw FILE`, `valgrind --tool=callgrind
    --callgrind-out-file=FILE SISU build --emit ir-raw FILE` and `callgrind_annotate
    --inclusive=yes --threshold=100 --show=Ir --auto=no FILE`.
    """
    tmp = Path(tmp)
    bin_dir = tmp / 'bin'
    bin_dir.mkdir()
    log = tmp / 'log'
    (tmp / 'annotate.txt').write_text(annotate)
    bad = 'echo "stub $0: wrong arguments: $*" >&2; exit 2'
    script(bin_dir / 'sisu', f'echo "sisu $*" >> {log}\n'
           f'case "$1" in\n'
           f'  check) [ $# -eq 2 ] || {{ {bad}; }};;\n'
           f'  build) [ $# -eq 4 ] && [ "$2 $3" = "--emit ir-raw" ] || {{ {bad}; }};;\n'
           f'  *) {bad};;\n'
           f'esac\n{check}\nexit 0\n')
    if valgrind:
        script(bin_dir / 'valgrind', f'echo "valgrind $*" >> {log}\n'
               'case "$1" in --version) echo valgrind-3.22.0; exit 0;; esac\n'
               '[ $# -eq 7 ] && [ "$1" = --tool=callgrind ] && [ "$4 $5 $6" = "build --emit ir-raw" ] '
               f'|| {{ {bad}; }}\n'
               'case "$2" in --callgrind-out-file=*) : > "${2#*=}";; *) ' + bad + ';; esac\n'
               'exit 0\n')
    script(bin_dir / 'callgrind_annotate', f'echo "callgrind_annotate $*" >> {log}\n'
           '[ $# -eq 5 ] && [ "$1 $2 $3 $4" = "--inclusive=yes --threshold=100 --show=Ir --auto=no" ] '
           f'&& [ -f "$5" ] || {{ {bad}; }}\n'
           f'/bin/cat {tmp / "annotate.txt"}\n')
    return str(bin_dir)


def run_main(*argv, path=None, **env):
    """Runs `main`; returns the exit code, standard output and standard error."""
    out, err = io.StringIO(), io.StringIO()
    if path is not None:
        env['PATH'] = path
    with (mock.patch.dict(os.environ, env), contextlib.redirect_stdout(out),
          contextlib.redirect_stderr(err)):
        code = sc.main(list(argv))
    return code, out.getvalue(), err.getvalue()


class ScalingTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(tmp.name)
        self.out = self.tmp / 'out'
        self.no_tools = self.tmp / 'no-tools'
        self.no_tools.mkdir()

    def counts_run(self, shapes, **env):
        """Runs `main --counts` on `shapes`, as `counts_file` makes them, with no tool on PATH."""
        return run_main('--counts', str(counts_file(self.tmp, shapes)), '--out', str(self.out),
                        path=str(self.no_tools), **env)

    def stub_run(self, *shapes, **kwargs):
        """Runs `main --shapes ...` with stubs for the tools; returns (code, stdout, stderr)."""
        path = stubs(self.tmp, **kwargs)
        return run_main('--shapes', ','.join(shapes), '--sisu', f'{path}/sisu', '--out',
                        str(self.out), path=path)

    def log(self):
        log = self.tmp / 'log'
        return log.read_text() if log.exists() else ''

    def test_judge_should_mark_a_superlinear_stage(self):
        """
        GIVEN a shape whose total growth at `8n` is 100000
        AND check growths at `n`, `2n`, `4n` and `8n` of the rows of the table of S10
        WHEN `scripts/scaling.py` reads them
        THEN the check row is marked `superlinear` for the rows that the table marks
        """
        for growth, ratios, verdict in S10:
            with self.subTest(growth=growth):
                got, got_ratios = sc.judge(growth, 100000)
                self.assertEqual(got, verdict)
                self.assertEqual([None if r is None else round(float(r), 2) for r in got_ratios], ratios)
        names = list(sc.SHAPES)
        shapes = {names[i]: {'check': growth} for i, (growth, _, _) in enumerate(S10)}
        with mock.patch.object(sc, 'KNOWN', []):
            self.counts_run(shapes)
        self.assertEqual((self.out / 'table.md').read_text(), S10_TABLE)

    def test_stage_function_missing_should_stop_the_sweep(self):
        """
        GIVEN callgrind output with no `mono::instantiate`
        WHEN `scripts/scaling.py` reads it
        THEN it exits with code 1 and names `mono::instantiate`
        """
        code, _, err = self.stub_run('functions', annotate=ANNOTATE.replace(MONO_LINE, ''))
        self.assertEqual(code, 1)
        self.assertEqual(len(err.splitlines()), 1)
        self.assertTrue(err.startswith('scaling: '))
        self.assertIn('mono::instantiate', err)

    def test_superlinear_case_with_no_known_row_should_fail_the_sweep(self):
        """
        GIVEN counts that make the match arms shape superlinear in the check stage
        AND a known list with no row for that shape and stage
        WHEN `scripts/scaling.py` reads them
        THEN the check row is marked `superlinear`
        AND it exits with code 1 and says to open an issue and add a row
        """
        with mock.patch.object(sc, 'KNOWN', KNOWN_344):
            code, out, err = self.counts_run({'match arms': {'check': growths(2.29, 2.45, 2.77)}})
        self.assertEqual(code, 1)
        for text in ('match arms', 'check', 'open an issue and add a row'):
            self.assertIn(text, err)
        self.assertIn('| check | 1000 | 2290 | 5610 | 15540 | 2.29 | 2.45 | 2.77 | superlinear |', out)

    def test_known_case_no_longer_superlinear_should_fail_the_sweep(self):
        """
        GIVEN a known list with the row (array literal depth, check, #344)
        AND counts that give the check stage of that shape the ratios <ratios>
        WHEN `scripts/scaling.py` reads them
        THEN it exits with code 1
        AND it says that #344 looks fixed and that its row must go
        """
        cases = {
            'a linear stage': growths(2.00, 2.01, 2.02),
            'r3 just under 2.15': growths(2.10, 2.12, 2.14),
            'an n log n stage': growths(2.56, 2.44, 2.36),
            'a no ratio stage': [0, 500, 2000, 8000],
        }
        for name, growth in cases.items():
            with self.subTest(name), mock.patch.object(sc, 'KNOWN', KNOWN_344):
                code, _, err = self.counts_run({'array literal depth': {'check': growth}})
                self.assertEqual(code, 1)
                self.assertIn('#344 looks fixed: remove its row', err)

    def test_known_case_should_pass_the_sweep(self):
        """
        GIVEN a known list with the row (array literal depth, check, #344)
        AND counts that give the check stage of that shape the ratios <ratios>
        WHEN `scripts/scaling.py` reads them
        THEN it prints the case and `#344`
        AND the exit code is 0
        """
        cases = {
            'a superlinear known case': growths(2.29, 2.45, 2.77),
            'a known case in the gap': growths(2.20, 2.24, 2.25),
            'r3 just over 2.15': growths(2.12, 2.14, 2.16),
        }
        for name, growth in cases.items():
            with self.subTest(name), mock.patch.object(sc, 'KNOWN', KNOWN_344):
                code, out, err = self.counts_run({'array literal depth': {'check': growth}})
                self.assertEqual(code, 0)
                self.assertIn('\nscaling: known case: array literal depth, check (#344)\n', out)
                self.assertEqual(err, '')
        with self.subTest('annotation'), mock.patch.object(sc, 'KNOWN', KNOWN_344):
            shapes = {'array literal depth': {'check': growths(2.29, 2.45, 2.77)}}
            _, out, _ = self.counts_run(shapes, GITHUB_ACTIONS='true')
            self.assertIn('::warning title=Known case::array literal depth, check (#344)\n', out)
            _, out, _ = self.counts_run(shapes, GITHUB_ACTIONS='false')
            self.assertNotIn('::warning', out)

    def test_shape_that_does_not_build_should_stop_the_sweep(self):
        """
        GIVEN a shape whose program at `8n` gives E0071
        WHEN `scripts/scaling.py` runs
        THEN it exits with code 1 before it runs callgrind
        AND it names the shape, the size and E0071
        """
        at_8n = 'case "$*" in *size-2000.sisu*) {} ;; esac'
        error = at_8n.format("echo 'error[E0071]: nesting is too deep' >&2; exit 1")
        warning = at_8n.format("echo 'warning[W0014]: variable `x` is never read' >&2")
        with self.subTest('error'):
            code, _, err = self.stub_run('functions', check=error)
            self.assertEqual(code, 1)
            first = err.splitlines()[0]
            self.assertTrue(first.startswith('scaling: '))
            self.assertIn('functions', first)
            self.assertIn('size 2000', first)
            self.assertIn('E0071', err)
            self.assertNotIn('--tool=callgrind', self.log())
        with self.subTest('warning'):
            self.tmp = Path(tempfile.mkdtemp(dir=self.tmp))
            code, _, err = self.stub_run('functions', check=warning)
            self.assertEqual(code, 1)
            self.assertEqual(err, 'scaling: functions, size 2000: W0014\n')
            self.assertNotIn('--tool=callgrind', self.log())

    def test_stage_function_shown_twice_should_stop_the_sweep(self):
        """
        GIVEN callgrind output with `sisuc::check::check` and `sisuc::check::check.llvm.123`
        AND output with the function from two source files, both with an object field
        WHEN `stage_counts` reads it
        THEN it raises Fail and names the function
        AND it names each match by its file and function
        """
        clone = '   306,067 ( 4.09%)  ???:sisuc::check::check.llvm.123 [/src/target/release/sisu]\n'
        with self.assertRaisesRegex(Fail, 'sisuc::check::check .*2 times'):
            sc.stage_counts(ANNOTATE + clone)
        # The same function from two source files, as a build with debug information gives it.
        one = '  100 ( 1.00%)  /src/a.rs:sisuc::check::check [/src/target/debug/sisu]\n'
        two = '   50 ( 0.50%)  /src/b.rs:sisuc::check::check [/src/target/debug/sisu]\n'
        with self.assertRaises(Fail) as raised:
            sc.stage_counts(ANNOTATE.replace(CHECK_LINE, one + two))
        self.assertIn('/src/a.rs:sisuc::check::check', str(raised.exception))
        self.assertIn('/src/b.rs:sisuc::check::check', str(raised.exception))

    def test_known_row_of_a_shape_not_measured_should_be_skipped(self):
        """
        GIVEN a known row for the type depth shape
        AND a run that measured only the functions shape
        WHEN `compare` reads the verdicts
        THEN it gives exit code 0 and no line for the row
        """
        verdicts = {('functions', 'check'): ('linear', [2.0, 2.0, 2.0])}
        known = [('type depth', 'check', 99)]
        self.assertEqual(sc.compare(verdicts, known, {'functions'}), ([], [], 0))

    def test_missing_valgrind_should_stop_the_sweep_first(self):
        """
        GIVEN a `PATH` that has `sisu` and no `valgrind`
        WHEN `scripts/scaling.py` runs
        THEN it exits with code 1 and says that valgrind is not on PATH
        AND it runs no command
        """
        code, _, err = self.stub_run('functions', valgrind=False)
        self.assertEqual(code, 1)
        self.assertIn('valgrind is not on PATH', err)
        self.assertEqual(self.log(), '')

    def test_stopped_run_should_write_the_table_so_far(self):
        """
        GIVEN a run of the functions and match arms shapes
        AND a `sisu check` that fails for the match arms shape
        WHEN `scripts/scaling.py` runs
        THEN it exits with code 1
        AND `table.md` has the rows of the functions shape and the `scaling:` line
        """
        check = 'case "$*" in *match-arms*) echo "boom" >&2; exit 1;; esac'
        code, _, err = self.stub_run('functions', 'match arms', check=check)
        self.assertEqual(code, 1)
        text = (self.out / 'table.md').read_text()
        self.assertIn('## functions', text)
        self.assertIn('| codegen | 0 | 0 | 0 | 0 |', text)
        self.assertNotIn('## match arms', text)
        self.assertIn('\nscaling: match arms, size 0', text)
        counts = json.loads((self.out / 'counts.json').read_text())
        self.assertEqual(list(counts['shapes']), ['functions'])
        self.assertEqual(counts['shapes']['functions']['counts']['check'], [306067] * 5)
        self.assertEqual(counts['valgrind'], 'valgrind-3.22.0')

    def test_file_error_while_measuring_should_write_the_table_so_far(self):
        """
        GIVEN a run of the functions and match arms shapes
        AND a regular file where the directory of the match arms shape belongs
        WHEN `scripts/scaling.py` runs
        THEN it exits with code 1
        AND `counts.json` holds the functions shape only
        AND `table.md` has the rows of the functions shape and the `scaling:` line
        """
        self.out.mkdir()
        (self.out / 'match-arms').write_text('not a directory')
        code, _, err = self.stub_run('functions', 'match arms')
        self.assertEqual(code, 1)
        self.assertIn('scaling: ', err)
        counts = json.loads((self.out / 'counts.json').read_text())
        self.assertEqual(list(counts['shapes']), ['functions'])
        text = (self.out / 'table.md').read_text()
        self.assertIn('## functions', text)
        self.assertIn('\nscaling: ', text)

    def test_report_that_cannot_be_written_should_exit_with_code_1(self):
        """
        GIVEN a run of the functions shape that measures with no error
        AND a directory where `counts.json` belongs
        WHEN `scripts/scaling.py` runs
        THEN it exits with code 1
        AND standard error has one `scaling:` line that says the report cannot be written
        AND it raises no exception
        """
        self.out.mkdir()
        (self.out / 'counts.json').mkdir()
        code, _, err = self.stub_run('functions')
        self.assertEqual(code, 1)
        self.assertIn('scaling: cannot write the report', err)

    def test_each_shape_should_have_its_size_in_its_structure(self):
        """
        GIVEN the 14 shapes
        WHEN each makes its program at size k
        THEN the repeated part of the program has a size of base + k
        AND the base is what the program at size 0 needs to check clean
        AND a depth shape has that nesting, and the method chain is one chain
        """
        sizes = [('functions', 250), ('statements', 250), ('expression depth', 12),
                 ('block depth', 12), ('generic call depth', 24), ('array literal depth', 24),
                 ('array literal width', 250), ('match arms', 50), ('fields', 50),
                 ('conformers', 50), ('type depth', 24), ('interpolations', 50),
                 ('live locals', 50), ('method chain', 24)]
        self.assertEqual([(name, n) for name, (n, _) in sc.SHAPES.items()], sizes)

        def depth(text, opening, closing):
            """The deepest nesting of one kind of bracket."""
            now = deepest = 0
            for char in text:
                now += (char == opening) - (char == closing)
                deepest = max(deepest, now)
            return deepest

        def blocks(text):
            """The deepest nesting of `{ ... }` blocks, from the lines that open and close one."""
            now = deepest = 0
            for row in text.splitlines():
                now += row.rstrip().endswith('{') - (row.strip() == '}')
                deepest = max(deepest, now)
            return deepest

        def line(text, start):
            return next(row.strip() for row in text.splitlines() if row.strip().startswith(start))

        def chain(text):
            """The length of the one receiver chain `c.next().next()...`; -1 when it is not one."""
            found = re.fullmatch(r'let d = c((?:\.next\(\))*)', line(text, 'let d ='))
            if found is None or text.count('.next()') != len(found[1]) // 7:
                return -1
            return len(found[1]) // 7

        # shape: (the size of the repeated part of the program, its size at size 0)
        parts = {
            'functions': (lambda t: t.count('fn '), 1),
            'statements': (lambda t: t.count('let '), 1),
            'expression depth': (lambda t: depth(line(t, 'let x'), '(', ')'), 0),
            'block depth': (blocks, 1),
            'generic call depth': (lambda t: depth(line(t, 'let v'), '(', ')'), 1),
            'array literal depth': (lambda t: depth(t, '[', ']'), 1),
            'array literal width': (lambda t: t.count('7'), 0),
            'match arms': (lambda t: t.count('=>'), 1),
            'fields': (lambda t: t.count(': i64'), 1),
            'conformers': (lambda t: t.count('class '), 1),
            'type depth': (lambda t: depth(line(t, 'let v'), '(', ')'), 1),
            'interpolations': (lambda t: t.count('{x}'), 1),
            'live locals': (lambda t: t.count('let live'), 0),
            'method chain': (chain, 1),
        }
        self.assertEqual(list(parts), list(sc.SHAPES))
        for name, (_, make) in sc.SHAPES.items():
            measure, base = parts[name]
            for k in (0, 1, 3):
                with self.subTest(shape=name, k=k):
                    self.assertEqual(measure(make(k)), base + k)

    def test_text_of_each_shape_should_grow_linearly_with_its_size(self):
        """
        GIVEN the 14 shapes at their sizes 0, n, 2n, 4n and 8n
        WHEN the bytes of each program are counted
        THEN the growth in bytes at each size is below 2.01 times the growth at half that size
        """
        # Linear text gives 2.00. The commas of the array literal width and the text that differs at
        # size 0 give at most 2.003. A name with more digits at a larger size gives up to 2.12.
        # A nested block that indents its inner lines again gives about 4. The lexer would then
        # look quadratic. The names have a fixed width, so the bound 2.01 is tight.
        for name, (n, make) in sc.SHAPES.items():
            sizes = [0, n, 2 * n, 4 * n, 8 * n]
            growth = [len(make(k)) - len(make(0)) for k in sizes]
            for i in (2, 3, 4):
                with self.subTest(shape=name, size=sizes[i]):
                    self.assertLess(growth[i] / growth[i - 1], 2.01)

    def test_stage_counts_should_read_each_stage_from_its_own_line(self):
        """
        GIVEN callgrind output with a line for each stage function and a closure of `check::check`
        WHEN `stage_counts` reads it
        THEN each stage has the count of its own line
        AND the closure is not added to `check`
        """
        self.assertEqual(sc.stage_counts(ANNOTATE), EXPECTED_COUNTS)
        # A function with a longer name is no clone, and an LTO clone counts alone.
        longer = '    1,000 ( 0.01%)  ???:sisuc::check::check_item [/src/target/release/sisu]\n'
        self.assertEqual(sc.stage_counts(ANNOTATE + longer), EXPECTED_COUNTS)
        clone = ANNOTATE.replace('sisuc::check::check ', 'sisuc::check::check.llvm.123 ')
        self.assertEqual(sc.stage_counts(clone), EXPECTED_COUNTS)

    def test_rule_should_find_issue_344_in_the_saved_counts(self):
        """
        GIVEN the callgrind counts of 85c95105, where #344 is open
        WHEN scaling.py reads them
        THEN array literal depth is superlinear in check only, and functions in no stage
        """
        verdicts = sc.verdicts(sc.load(SAVED))
        for stage, _ in sc.STAGES:
            array_superlinear = verdicts[('array literal depth', stage)][0] == 'superlinear'
            self.assertEqual(array_superlinear, stage == 'check', stage)
            self.assertNotEqual(verdicts[('functions', stage)][0], 'superlinear', stage)
        with mock.patch.object(sc, 'KNOWN', KNOWN_344):
            code, _, _ = run_main('--counts', str(SAVED), '--out', str(self.out),
                                  path=str(self.no_tools))
        self.assertEqual(code, 0)

    def test_known_rows_should_name_a_shape_and_a_stage(self):
        """
        GIVEN the known list
        WHEN each row is read
        THEN its shape is a shape, its stage is a stage and its issue is a positive integer
        """
        for shape, stage, issue in sc.KNOWN:
            with self.subTest(shape=shape, stage=stage):
                self.assertIn(shape, sc.SHAPES)
                self.assertIn(stage, [name for name, _ in sc.STAGES])
                self.assertIsInstance(issue, int)
                self.assertGreater(issue, 0)


    def test_judge_should_keep_each_boundary_of_the_rule(self):
        """
        GIVEN check growths on each side of each number of the rule
        WHEN `judge` reads them
        THEN it takes the number as its text says, with no round-off
        """
        rows = {
            'r3 = 2.30 is not above 2.3': ([2000, 4600, 11500, 26450], 100000, 'linear'),
            'r3 = 2.31 is above 2.3': ([2000, 4600, 11500, 26565], 100000, 'superlinear'),
            'r2 = 2.10 is enough': ([1000, 2000, 4200, 10080], 100000, 'superlinear'),
            'r2 = 2.09 is not enough': ([1000, 2000, 4180, 10032], 100000, 'linear'),
            'r3 = r1 - 0.05 is not a fall': ([4000, 9800, 22540, 54096], 100000, 'superlinear'),
            'r3 just under r1 - 0.05 is a fall': ([4000, 9800, 22540, 54095], 100000, 'linear'),
            'r3 = 3.0 is kept in a fall': ([1000, 3600, 11880, 35640], 100000, 'superlinear'),
            'r3 = 2.9 in a fall is linear': ([1000, 3600, 11880, 34452], 100000, 'linear'),
            'the growth of 1% is judged': ([24, 96, 384, 1536], 153600, 'superlinear'),
            'under 1% is not judged': ([24, 96, 384, 1536], 153601, 'not judged'),
        }
        for name, (growth, total, verdict) in rows.items():
            with self.subTest(name):
                self.assertEqual(sc.judge(growth, total)[0], verdict)

    def test_sweep_should_decide_each_boundary_of_the_rule_exactly(self):
        """
        GIVEN counts whose ratios are on a number of the rule, or just beside it
        WHEN `scripts/scaling.py` reads them through `main --counts`
        THEN the exit code is the one that the rule gives
        """
        shape = 'match arms'
        known_shape = 'array literal depth'
        # shape, growth of check, known list, exit code, text in stderr
        rows = {
            'r3 = r1 - 0.05 is superlinear and has no row':
                (shape, [4000, 9800, 22540, 54096], [], 1, 'match arms, check is superlinear'),
            'r3 just under r1 - 0.05 is linear': (shape, [4000, 9800, 22540, 54095], [], 0, ''),
            'r3 = r1 - 0.15 stays known':
                (known_shape, [4000, 9800, 22540, 51842], KNOWN_344, 0, ''),
            'r3 just under r1 - 0.15 looks fixed':
                (known_shape, [4000, 9800, 22540, 51841], KNOWN_344, 1, '#344 looks fixed'),
            'a known row that is not judged looks fixed':
                (known_shape, [10, 40, 160, 640], KNOWN_344, 1, '#344 looks fixed'),
            'r3 = 3.8 in a fall stays known':
                (known_shape, [1000, 4500, 18900, 71820], KNOWN_344, 0, ''),
            'r3 = 2.95 in a fall looks fixed':
                (known_shape, [1000, 3400, 10200, 30090], KNOWN_344, 1, '#344 looks fixed'),
            'r3 = 2.15 stays known':
                (known_shape, [1000, 2200, 5000, 10750], KNOWN_344, 0, ''),
            'r3 = 2.14 looks fixed':
                (known_shape, [1000, 2200, 5000, 10700], KNOWN_344, 1, '#344 looks fixed'),
            'r3 = 3.0 in a fall stays known':
                (known_shape, [1000, 3500, 10500, 31500], KNOWN_344, 0, ''),
            'r3 just under 3.0 in a fall looks fixed':
                (known_shape, [1000, 3500, 10500, 31499], KNOWN_344, 1, '#344 looks fixed'),
        }
        for name, (name_of_shape, growth, known, code, text) in rows.items():
            with self.subTest(name), mock.patch.object(sc, 'KNOWN', known):
                got, _, err = self.counts_run({name_of_shape: {'check': growth}})
                self.assertEqual(got, code)
                self.assertIn(text, err)

    def test_saved_counts_that_are_not_valid_should_stop_the_sweep(self):
        """
        GIVEN a counts file that is missing, is not JSON, or does not have the structure of a run
        WHEN `scripts/scaling.py` reads it
        THEN it exits with code 1 and one line `scaling: ...` that names the file
        AND it writes no file into the output directory
        """
        good = json.loads(counts_file(self.tmp, {'functions': {'check': [1, 2, 4, 8]}}).read_text())
        stage_counts = good['shapes']['functions']['counts']

        def changed(edit):
            data = json.loads(json.dumps(good))
            edit(data['shapes']['functions'])
            return json.dumps(data)

        cases = {
            'not JSON': '{',
            'a list': '[]',
            'no shapes': json.dumps({'commit': 'x'}),
            'shapes that is a list': json.dumps({'shapes': []}),
            'sizes that do not double': changed(lambda s: s.update(sizes=[0, 250, 251, 252, 253])),
            'size n of 0': changed(lambda s: s.update(sizes=[0, 0, 0, 0, 0])),
            'four sizes': changed(lambda s: s.update(sizes=[0, 250, 500, 1000])),
            'a missing stage': changed(lambda s: s['counts'].pop('mir')),
            'four counts': changed(lambda s: s['counts'].update(mir=stage_counts['mir'][:4])),
            'a fraction': changed(lambda s: s['counts'].update(mir=[1, 2, 3, 4, 1.5])),
            'a boolean': changed(lambda s: s['counts'].update(mir=[1, 2, 3, 4, True])),
            'a string': changed(lambda s: s['counts'].update(mir=[1, 2, 3, 4, '5'])),
        }
        for name, text in cases.items():
            with self.subTest(name):
                path = self.tmp / 'bad.json'
                path.write_text(text)
                code, _, err = run_main('--counts', str(path), '--out', str(self.out),
                                        path=str(self.no_tools))
                self.assertEqual(code, 1)
                self.assertEqual(len(err.splitlines()), 1)
                self.assertTrue(err.startswith('scaling: '))
                self.assertIn('bad.json', err)
                self.assertFalse(self.out.exists())
        with self.subTest('a missing file'):
            code, _, err = run_main('--counts', str(self.tmp / 'none.json'), '--out', str(self.out),
                                    path=str(self.no_tools))
            self.assertEqual(code, 1)
            self.assertTrue(err.startswith('scaling: '))
            self.assertIn('none.json', err)
            self.assertFalse(self.out.exists())

    def test_header_should_be_read_only_when_the_sweep_measures(self):
        """
        GIVEN a libc that cannot give its version
        WHEN `scripts/scaling.py` reads saved counts
        THEN it works, and the header is the one of the file
        AND when it measures, the glibc of the header is `unknown`
        """
        for error in (ValueError('unrecognized configuration name'), OSError('no libc')):
            with self.subTest(error=type(error).__name__), mock.patch.object(os, 'confstr',
                                                                             side_effect=error):
                code, _, _ = self.counts_run({'functions': {'check': [1000, 2000, 4000, 8000]}})
                self.assertEqual(code, 0)
                self.assertEqual(json.loads((self.out / 'counts.json').read_text())['glibc'],
                                 'glibc 2.0')
                self.tmp = Path(tempfile.mkdtemp(dir=self.tmp))
                self.stub_run('functions')
                self.assertEqual(json.loads((self.out / 'counts.json').read_text())['glibc'],
                                 'unknown')

    def test_sweep_should_run_the_documented_commands_at_five_sizes(self):
        """
        GIVEN a run of the functions shape with stubs that accept only the documented arguments
        WHEN `scripts/scaling.py` runs
        THEN it checks the sizes 0 and 2000, and measures the sizes 0, 250, 500, 1000 and 2000
        AND each measurement is `sisu build --emit ir-raw` under callgrind, read by `callgrind_annotate`
        """
        code, _, err = self.stub_run('functions')
        self.assertEqual((code, err), (0, ''))
        rows = self.log().splitlines()
        size = re.compile(r'size-(\d+)\.sisu$')
        checks = [size.search(row)[1] for row in rows if row.startswith('sisu check ')]
        self.assertEqual(checks, ['0', '2000'])
        profiled = [row for row in rows if row.startswith('valgrind --tool=callgrind ')]
        self.assertEqual([size.search(row)[1] for row in profiled], ['0', '250', '500', '1000', '2000'])
        for row in profiled:
            self.assertIn(' build --emit ir-raw ', row)
        annotated = [row for row in rows if row.startswith('callgrind_annotate ')]
        self.assertEqual(len(annotated), 5)

    def test_unknown_shape_should_exit_with_code_2(self):
        """
        GIVEN a `--shapes` value with a name that is not a shape
        WHEN `scripts/scaling.py` runs
        THEN it exits with code 2
        """
        with self.assertRaises(SystemExit) as raised:
            run_main('--shapes', 'functions,nope')
        self.assertEqual(raised.exception.code, 2)


if __name__ == '__main__':
    unittest.main()
