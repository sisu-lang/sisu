"""Tests for fuzz_gaps.py: python3 -m unittest discover -s scripts -p 'test_fuzz*.py'.

Each case runs the script as a subprocess on small `llvm-cov export` files. A fixture name is
already demangled, in the form `c++filt -s rust` prints: `c++filt` passes such text through
unchanged. Every case keeps one reached ANCHOR in both exports, so that a case that expects exit 0
cannot pass by exit 2 with an empty export.
"""
import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, 'fuzz_gaps.py')
SRC = '/work/crates/sisuc/src/'
HEADING = '## Generator gaps'
NO_REASON = '## no reason yet\n'

# Real forms, as `c++filt -s rust` prints them for names of the compiler.
METHOD = '<sisuc[31a72b24980b05b4]::check::Checker>::want_name'
CLOSURE = '<sisuc[31a72b24980b05b4]::check::Checker>::compound_value::{closure#0}'
METHOD_OF_CLOSURE = '<sisuc[31a72b24980b05b4]::check::Checker>::compound_value'
GENERIC = 'sisuc[31a72b24980b05b4]::each_ir_unit::<sisuc[31a72b24980b05b4]::emit_ir_raw::{closure#1}>'
GENERIC_OTHER = 'sisuc[31a72b24980b05b4]::each_ir_unit::<core[a9090a07a7fa7517]::option::Option<i64>>'
FN = 'sisuc[abc]::conformance::f'
FN_KEY = 'conformance.rs\tsisuc::conformance::f'


def record(file, name, count, line=1):
    return {'name': name, 'count': count, 'filenames': [SRC + file],
            'regions': [[line, 1, line + 1, 2, count, 0, 0, 0]]}


def export(*records):
    return {'data': [{'type': 'llvm.coverage.json.export', 'functions': list(records)}]}


ANCHOR = record('check/mod.rs', METHOD, 1)


def fake_tool(directory, script):
    path = os.path.join(directory, 'c++filt')
    with open(path, 'w', encoding='utf-8') as f:
        f.write('#!/bin/sh\ncat >/dev/null\n' + script)
    os.chmod(path, os.stat(path).st_mode | stat.S_IXUSR)


class Run:
    """Writes the three inputs to a temporary directory, runs the script and keeps its result."""

    def __init__(self, test, seeds, valid, known=NO_REASON, path=None):
        with tempfile.TemporaryDirectory() as directory:
            names = {}
            for label, content in (('seeds.json', seeds), ('valid.json', valid)):
                names[label] = os.path.join(directory, label)
                if content is None:
                    continue
                with open(names[label], 'w', encoding='utf-8') as f:
                    f.write(content if isinstance(content, str) else json.dumps(content))
            names['known'] = os.path.join(directory, 'known_gaps.txt')
            if known is not None:
                with open(names['known'], 'w', encoding='utf-8') as f:
                    f.write(known)
            env = dict(os.environ)
            if path is not None:
                tools = os.path.join(directory, 'tools')
                os.mkdir(tools)
                path(tools)
                env['PATH'] = tools
            done = subprocess.run(
                [sys.executable, SCRIPT, names['seeds.json'], names['valid.json'], names['known']],
                capture_output=True, text=True, check=False, env=env)
        self.code, self.out, self.err = done.returncode, done.stdout, done.stderr
        test.assertTrue(self.out.startswith(HEADING), f'stdout starts the section: {self.out!r}')
        test.assertEqual(self.err, '', 'no text on stderr')
        self.lines = self.out.splitlines()


class Gaps(unittest.TestCase):
    def reached(self, seeds, valid, known=NO_REASON, **kw):
        return Run(self, export(ANCHOR, *seeds), export(ANCHOR, *valid), known, **kw)

    def no_gap(self, run, name):
        self.assertEqual(run.code, 0, run.out)
        self.assertNotIn(name, run.out)

    def test_only_an_unknown_function_valid_programs_reach_should_fail(self):
        with self.subTest('valid reaches f, seeds do not'):
            run = self.reached([record('conformance.rs', FN, 0)], [record('conformance.rs', FN, 1)])
            self.assertEqual(run.code, 1, run.out)
            self.assertIn(FN_KEY, run.lines)
            self.assertEqual(run.lines.count('```text'), 1)
        with self.subTest('the same, f known'):
            known = f'## a reason\n{FN_KEY}\n'
            run = self.reached([record('conformance.rs', FN, 0)], [record('conformance.rs', FN, 1)],
                               known)
            self.no_gap(run, 'conformance::f')
            self.assertNotIn('Warning', run.out)
        with self.subTest('no valid program reaches f'):
            run = self.reached([], [record('conformance.rs', FN, 0)])
            self.no_gap(run, 'conformance::f')
        with self.subTest('seeds reach one generic instance of f'):
            run = self.reached([record('emit.rs', GENERIC, 1)], [record('emit.rs', GENERIC_OTHER, 1)])
            self.no_gap(run, 'each_ir_unit')
        with self.subTest('the > of an arrow inside generic arguments does not close them'):
            arrow = 'sisuc[abc]::emit::g::<fn(i64) -> i64>'
            run = self.reached([record('emit.rs', 'sisuc[abc]::emit::g', 1)], [record('emit.rs', arrow, 1)])
            self.no_gap(run, 'emit::g')
        with self.subTest('a slice type that looks like a crate hash stays'):
            run = self.reached([], [record('slice.rs', '<[f32]>::first', 1)])
            self.assertEqual(run.code, 1, run.out)
            self.assertIn('slice.rs\t<[f32]>::first', run.lines)
        with self.subTest('a generic argument that holds a closure is no closure record'):
            holder = 'sisuc[abc]::emit::h::<sisuc[abc]::emit::k::{closure#0}>'
            run = self.reached([], [record('emit.rs', holder, 1)])
            self.assertEqual(run.code, 1, run.out)
            self.assertIn('emit.rs\tsisuc::emit::h', run.lines)
        with self.subTest('seeds miss only a closure of f'):
            run = self.reached(
                [record('check/place.rs', METHOD_OF_CLOSURE, 1, 5), record('check/place.rs', CLOSURE, 0, 6)],
                [record('check/place.rs', METHOD_OF_CLOSURE, 1, 5), record('check/place.rs', CLOSURE, 1, 6)])
            self.no_gap(run, 'compound_value')
        with self.subTest('seeds reach f, f known'):
            known = f'## a reason\n{FN_KEY}\n'
            run = self.reached([record('conformance.rs', FN, 1)], [record('conformance.rs', FN, 1)],
                               known)
            self.assertEqual(run.code, 0, run.out)
            self.assertIn('Warning', run.out)
            self.assertIn(FN_KEY, run.lines)
        for file in ('generate/item.rs', 'generate/deep/more.rs', 'fuzz.rs', 'fuzz_program.rs',
                     'report.rs', 'diagnostic/json.rs', 'diagnostic/render.rs'):
            with self.subTest(f'f in {file} is out of scope'):
                run = self.reached([], [record(file, FN, 1)])
                self.no_gap(run, 'conformance::f')
        for file in ('check/fuzz.rs', 'check/report.rs', 'diagnostic/mod.rs'):
            with self.subTest(f'f in {file} is in scope'):
                run = self.reached([], [record(file, FN, 1)])
                self.assertEqual(run.code, 1, run.out)
                self.assertIn(f'{file}\tsisuc::conformance::f', run.lines)
        with self.subTest('f outside crates/sisuc/src is out of scope'):
            outside = record('conformance.rs', FN, 1)
            outside['filenames'] = ['/work/crates/runtime/src/conformance.rs']
            self.no_gap(self.reached([], [outside]), 'conformance::f')

    def test_definitions_should_keep_their_identity(self):
        i32_value = '<sisuc[abc]::Tag<i32>>::value'
        bool_value = '<sisuc[abc]::Tag<bool>>::value'
        key = 'tag.rs\t<sisuc::Tag>::value'
        seeds = [record('tag.rs', i32_value, 1, 10), record('tag.rs', bool_value, 0, 20)]
        valid = [record('tag.rs', i32_value, 1, 10), record('tag.rs', bool_value, 1, 20)]
        with self.subTest('the second definition is listed'):
            run = self.reached(seeds, valid)
            self.assertEqual(run.code, 1, run.out)
            self.assertEqual(run.lines.count(key), 1, run.out)
        with self.subTest('a key that matches both definitions exits 2'):
            run = self.reached(seeds, valid, f'## a reason\n{key}\n')
            self.assertEqual(run.code, 2, run.out)
            self.assertIn('two definitions', run.out)
        with self.subTest('the same, when only the seeds export holds both'):
            only_seeds = [record('tag.rs', i32_value, 1, 10), record('tag.rs', bool_value, 1, 20)]
            run = self.reached(only_seeds, [record('tag.rs', i32_value, 1, 10)], f'## a reason\n{key}\n')
            self.assertEqual(run.code, 2, run.out)
            self.assertIn('two definitions', run.out)

    def test_broken_measurement_should_exit_2(self):
        good = export(ANCHOR)
        known_key = f'{FN_KEY}\n'
        cases = [
            ('a missing valid export', lambda: Run(self, good, None), 'cannot read the export'),
            ('a missing seeds export', lambda: Run(self, None, good), 'cannot read the export'),
            ('an export that is not JSON', lambda: Run(self, good, '{"data": '), 'is not valid JSON'),
            ('an export without data[0].functions', lambda: Run(self, good, {'data': [{}]}),
             'has no data[0].functions'),
            ('no valid-program functions', lambda: Run(self, good, export()), 'no function that valid programs reach'),
            ('only unreached valid-program functions',
             lambda: Run(self, good, export(record('check/mod.rs', METHOD, 0))),
             'no function that valid programs reach'),
            ('only out-of-scope valid-program functions',
             lambda: Run(self, good, export(record('generate/item.rs', METHOD, 1))),
             'no function that valid programs reach'),
            ('no seed functions', lambda: Run(self, export(), good), 'no function that the seeds reach'),
            ('a name that stays _R', lambda: Run(self, good, export(ANCHOR, record('check/mod.rs', '_Rbogus', 1))),
             'still mangled'),
            ('a record without a name',
             lambda: Run(self, good, export(ANCHOR, {'count': 1, 'filenames': [SRC + 'a.rs'], 'regions': [[1]]})),
             'unexpected KeyError'),
            ('a known key before a header', lambda: Run(self, good, good, known_key), 'before any section header'),
            ('a section with an empty reason', lambda: Run(self, good, good, f'##  \n{known_key}'),
             'empty reason'),
            ('a key line without a TAB', lambda: Run(self, good, good, '## a reason\nconformance.rs sisuc::f\n'),
             'needs a TAB'),
            ('a missing known list', lambda: Run(self, good, good, None), 'cannot read the known list'),
            ('no c++filt', lambda: Run(self, good, good, path=lambda tools: None), 'c++filt did not run'),
            ('a failing c++filt', lambda: Run(self, good, good, path=lambda tools: fake_tool(tools, 'exit 1\n')),
             'c++filt failed'),
            ('a c++filt that returns a wrong number of lines',
             lambda: Run(self, good, good, path=lambda tools: fake_tool(tools, 'echo one\necho two\n')),
             'wrong number of lines'),
        ]
        for label, make, reason in cases:
            with self.subTest(label):
                run = make()
                self.assertEqual(run.code, 2, run.out)
                self.assertIn(reason, run.out)
                self.assertIn('did not finish', run.out)

    def test_real_names_should_normalize_to_their_keys(self):
        names = [
            ('_RNvMs_NtNtCs4giHZ0QWn4K_5sisuc5check5aliasNtB6_7Checker9want_name',
             '<sisuc::check::Checker>::want_name'),
            ('_RINvCs4giHZ0QWn4K_5sisuc12each_ir_unitNCNvB2_11emit_ir_raws_0EB2_',
             'sisuc::each_ir_unit'),
            ('_RINvMs0_NtCs4giHZ0QWn4K_5sisuc7codegenNtB6_7Codegen10panic_withNCNCNvMNtB6_5arrayBA_5slices1_00EB8_',  # spellchecker:disable-line
             '<sisuc::codegen::Codegen>::panic_with'),
            ('_RINvXNtNtCs4giHZ0QWn4K_5sisuc5check8mut_argsNtB3_7RootKeyNtNtCsevLNFiNqfJP_4core4hash4Hash4hashNtCs2otsBLhtD3r_10rustc_hash8FxHasherEB7_',
             '<sisuc::check::mut_args::RootKey as core::hash::Hash>::hash'),
        ]
        closure = '_RNCNvMs0_NtNtCs4giHZ0QWn4K_5sisuc5check5placeNtB9_7Checker14compound_value0Bb_'
        try:
            read = subprocess.run(['c++filt', '-s', 'rust'], input=names[0][0] + '\n',
                                  capture_output=True, text=True, check=False).stdout.strip()
        except OSError:
            read = names[0][0]
        if read == names[0][0]:
            self.skipTest('c++filt cannot read v0 names of Rust here')
        valid = [record(f'file{n}.rs', mangled, 1) for n, (mangled, _) in enumerate(names)]
        valid.append(record('closure.rs', closure, 1))
        run = Run(self, export(ANCHOR), export(ANCHOR, *valid))
        self.assertEqual(run.code, 1, run.out)
        listed = [line for line in run.lines if '\t' in line]
        self.assertEqual(listed, [f'file{n}.rs\t{want}' for n, (_, want) in enumerate(names)])


if __name__ == '__main__':
    unittest.main()
