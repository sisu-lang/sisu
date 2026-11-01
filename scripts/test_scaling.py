"""Unit tests for scaling.py: python3 -m unittest discover -s scripts -p 'test_scaling.py'.

A test that runs `main` on counts gives it a `counts.json` that the test writes. A test that runs
`main` on shapes puts stub `sisu`, `valgrind` and `callgrind_annotate` scripts in a directory, and
makes that directory the whole PATH. Each stub logs its arguments to `log`. No test starts a real
compiler or valgrind.
"""
import contextlib
import io
import json
import os
import stat
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import scaling as sc
from compile_speed import Fail

# The lines of a real `callgrind_annotate --inclusive=yes --threshold=100 --show=Ir --auto=no`
# run of the release `sisu build --emit ir-raw` on a program with 3 functions (valgrind 3.22.0).
# It keeps the header, the total, one line for each of the six stage functions, one closure of
# `check::check`, and two lines of other functions.
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
"""
MONO_LINE = '   20,561 ( 0.27%)  ???:sisuc::mono::instantiate [/src/target/release/sisu]\n'
EXPECTED_COUNTS = {'lex': 183335, 'parse': 118740, 'check': 306067, 'mono': 20561, 'mir': 61582,
                   'codegen': 355831}
KNOWN_344 = [('array literal depth', 'check', 344)]
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
# check stage as the row and the codegen stage as the rest of the 100000.
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

    `check` is shell text that runs first for each call of the stub `sisu`.
    """
    tmp = Path(tmp)
    bin_dir = tmp / 'bin'
    bin_dir.mkdir()
    log = tmp / 'log'
    (tmp / 'annotate.txt').write_text(annotate)
    script(bin_dir / 'sisu', f'echo "sisu $*" >> {log}\n{check}\nexit 0\n')
    if valgrind:
        script(bin_dir / 'valgrind', f'echo "valgrind $*" >> {log}\n'
               'case "$1" in --version) echo valgrind-3.22.0; exit 0;; esac\n'
               'for a in "$@"; do case "$a" in --callgrind-out-file=*) : > "${a#*=}";; esac; done\n'
               'exit 0\n')
    script(bin_dir / 'callgrind_annotate', f'echo "callgrind_annotate $*" >> {log}\n'
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

    def counts_run(self, shapes):
        """Runs `main --counts` on `shapes`, as `counts_file` makes them."""
        return run_main('--counts', str(counts_file(self.tmp, shapes)), '--out', str(self.out))

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
                self.assertEqual([None if r is None else round(r, 2) for r in got_ratios], ratios)
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
                for text in ('array literal depth', 'check', '#344'):
                    self.assertIn(text, out)
                self.assertEqual(err, '')
        with self.subTest('annotation'), mock.patch.object(sc, 'KNOWN', KNOWN_344):
            shapes = {'array literal depth': {'check': growths(2.29, 2.45, 2.77)}}
            counts = str(counts_file(self.tmp, shapes))
            _, out, _ = run_main('--counts', counts, '--out', str(self.out), GITHUB_ACTIONS='true')
            self.assertIn('::warning title=Known superlinear case::array literal depth, check (#344)\n', out)
            _, out, _ = run_main('--counts', counts, '--out', str(self.out), GITHUB_ACTIONS='false')
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
        WHEN `stage_counts` reads it
        THEN it raises Fail and names the function
        """
        clone = '   306,067 ( 4.09%)  ???:sisuc::check::check.llvm.123 [/src/target/release/sisu]\n'
        with self.assertRaisesRegex(Fail, 'sisuc::check::check .*2 times'):
            sc.stage_counts(ANNOTATE + clone)

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

    def test_each_shape_should_have_its_size_in_its_structure(self):
        """
        GIVEN the 14 shapes
        WHEN each makes its program at size k
        THEN the repeated part of the program has a count of base + k
        AND the base is what the program at size 0 needs to check clean
        """
        sizes = [('functions', 250), ('statements', 250), ('expression depth', 12),
                 ('block depth', 12), ('generic call depth', 24), ('array literal depth', 24),
                 ('array literal width', 250), ('match arms', 50), ('fields', 50),
                 ('conformers', 50), ('type depth', 24), ('interpolations', 50),
                 ('live locals', 50), ('method chain', 24)]
        self.assertEqual([(name, n) for name, (n, _) in sc.SHAPES.items()], sizes)

        def bracket_depth(text):
            depth = deepest = 0
            for char in text:
                depth += (char == '[') - (char == ']')
                deepest = max(deepest, depth)
            return deepest

        # shape: (what to count in the program, the count at size 0)
        parts = {
            'functions': (lambda t: t.count('fn '), 1),
            'statements': (lambda t: t.count('let '), 1),
            'expression depth': (lambda t: t.count('+ ('), 0),
            'block depth': (lambda t: t.count('if c'), 0),
            'generic call depth': (lambda t: t.count('id('), 1),
            'array literal depth': (bracket_depth, 1),
            'array literal width': (lambda t: t.count('7'), 0),
            'match arms': (lambda t: t.count('=>'), 1),
            'fields': (lambda t: t.count(': i64'), 1),
            'conformers': (lambda t: t.count('class '), 1),
            'type depth': (lambda t: t.count('Box<'), 1),
            'interpolations': (lambda t: t.count('{x}'), 1),
            'live locals': (lambda t: t.count('let live'), 0),
            'method chain': (lambda t: t.count('.next()'), 1),
        }
        self.assertEqual(list(parts), list(sc.SHAPES))
        for name, (_, make) in sc.SHAPES.items():
            count, base = parts[name]
            for k in (0, 1, 3):
                with self.subTest(shape=name, k=k):
                    self.assertEqual(count(make(k)), base + k)

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
                self.assertGreater(issue, 0)
        self.assertEqual(sc.KNOWN[0], ('array literal depth', 'check', 344))


if __name__ == '__main__':
    unittest.main()
