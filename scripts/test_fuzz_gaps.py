"""Tests for fuzz_gaps.py: python3 -m unittest discover -s scripts -p 'test_fuzz*.py'.

Each case runs the script as a subprocess on small `llvm-cov export` files. A fixture name is
already demangled, in the form `c++filt -s rust` prints: `c++filt` passes such text through
unchanged. Every case keeps the three reached ANCHORS in both exports, one under check/, one under
mir/ and one under codegen/, so that a case that expects exit 0 cannot pass by exit 2 with a short
export.
"""
import itertools
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


def record(file, name, count, line=1, region_count=None):
    region_count = count if region_count is None else region_count
    return {'name': name, 'count': count, 'filenames': [SRC + file],
            'regions': [[line, 1, line + 1, 2, region_count, 0, 0, 0]]}


def export(*records):
    return {'data': [{'type': 'llvm.coverage.json.export', 'functions': list(records)}]}


STAGES = ('check/', 'mir/', 'codegen/')
ANCHORS = (record('check/mod.rs', METHOD, 1), record('mir/mod.rs', 'sisuc[abc]::mir::anchor', 1),
           record('codegen/mod.rs', 'sisuc[abc]::codegen::anchor', 1))
LEGACY = '_ZN5sisuc1f17h0123456789abcdefE'


def lacking(rec, field):
    return {k: v for k, v in rec.items() if k != field}


def altered(rec, **fields):
    return {**rec, **fields}


def c_plus_plus_filt(text):
    """What `c++filt -s rust` prints for the text, or the text when the tool cannot run."""
    try:
        return subprocess.run(['c++filt', '-s', 'rust'], input=text + '\n', capture_output=True,
                              text=True, check=False).stdout.strip()
    except OSError:
        return text


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
        return Run(self, export(*ANCHORS, *seeds), export(*ANCHORS, *valid), known, **kw)

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
        self.closure_rows()
        self.guard_rows()
        self.reached_rows()
        self.known_rows()
        self.quoted_rows()

    def closure_rows(self):
        outer = 'sisuc[a]::emit::outer::{closure#0}'
        listed = [
            (f'{outer}::helper', 'sisuc::emit::outer::{closure#0}::helper'),
            (f'<{outer}::L as core[b]::fmt::Debug>::fmt',
             '<sisuc::emit::outer::{closure#0}::L as core::fmt::Debug>::fmt'),
            (f'<{outer} as sisuc[a]::Tr>::m', '<sisuc::emit::outer::{closure#0} as sisuc::Tr>::m'),
            ('sisuc[a]::emit::f::{constant#0}', 'sisuc::emit::f::{constant#0}'),
        ]
        for name, key in listed:
            with self.subTest(f'valid reaches {name}'):
                run = self.reached([], [record('emit.rs', name, 1)])
                self.assertEqual(run.code, 1, run.out)
                self.assertIn(f'emit.rs\t{key}', run.lines)
        for name in ('sisuc[a]::emit::f::{closure#0}::{closure#1}', 'sisuc[a]::emit::f::<i64>::{closure#0}',
                     'sisuc[a]::emit::f<i64>::{closure#0}', 'sisuc[a]::emit::f::{closure#0}::<i64>'):
            with self.subTest(f'{name} ends in a closure'):
                self.no_gap(self.reached([], [record('emit.rs', name, 1)]), 'emit::f')
        with self.subTest('generic arguments after a closure merge'):
            seeds = [record('emit.rs', '<sisuc[a]::f::{closure#0}<i32> as sisuc[a]::Tr>::m', 1, 7)]
            valid = [record('emit.rs', '<sisuc[a]::f::{closure#0}<i64> as sisuc[a]::Tr>::m', 1, 7)]
            self.no_gap(self.reached(seeds, valid), 'Tr')

    def guard_rows(self):
        rows = [
            ('LLVMFuzzerInitialize', 'LLVMFuzzerInitialize'),
            ('sisuc[a]::m::hash', 'sisuc::m::hash'),
            ('sisuc[a]::m::foo_Rbar', 'sisuc::m::foo_Rbar'),
            ('sisuc[a]::m::head', 'sisuc::m::head'),
            ('sisuc[a]::m::h0123456789abcdef::x', 'sisuc::m::h0123456789abcdef::x'),
        ]
        for name, key in rows:
            with self.subTest(f'{name} is no mangled name'):
                run = self.reached([], [record('check/mod.rs', name, 1)])
                self.assertEqual(run.code, 1, run.out)
                self.assertIn(f'check/mod.rs\t{key}', run.lines)
        with self.subTest('the guard reads only names in scope'):
            self.no_gap(self.reached([], [record('generate/item.rs', '_Rbogus', 1)]), '_Rbogus')

    def reached_rows(self):
        a, b, c = (f'sisuc[a]::m::g::<{t}>' for t in ('i32', 'bool', 'u8'))
        key = 'm.rs\tsisuc::m::g'
        rows = [
            ([(a, 1), (b, 0)], [(c, 1)], 0), ([(b, 0), (a, 1)], [(c, 1)], 0),
            ([], [(a, 1), (b, 0)], 1), ([], [(b, 0), (a, 1)], 1), ([], [(a, 0), (b, 0)], 0),
        ]
        for seeds, valid, code in rows:
            with self.subTest(f'seeds {seeds}, valid {valid}'):
                run = self.reached([record('m.rs', n, k) for n, k in seeds],
                                   [record('m.rs', n, k) for n, k in valid])
                self.assertEqual(run.code, code, run.out)
                if code:
                    self.assertIn(key, run.lines)
                else:
                    self.assertNotIn('m::g', run.out)
        name = 'sisuc[a]::m::g'
        with self.subTest('the function count decides: count 1, region count 0'):
            run = self.reached([], [record('m.rs', name, 1, region_count=0)])
            self.assertEqual(run.code, 1, run.out)
            self.assertIn(key, run.lines)
        with self.subTest('the function count decides: count 0, region count 1'):
            self.no_gap(self.reached([], [record('m.rs', name, 0, region_count=1)]), 'm::g')
        with self.subTest('seeds count 1, region count 0'):
            self.no_gap(self.reached([record('m.rs', name, 1, region_count=0)], [record('m.rs', name, 1)]),
                        'm::g')

    def known_rows(self):
        m_f, m_g = 'sisuc[a]::m::f', 'm.rs\tsisuc::m::g'
        with self.subTest('a stale key is listed under a new gap'):
            run = self.reached([], [record('m.rs', m_f, 1)], f'## a reason\n{m_g}\n')
            self.assertEqual(run.code, 1, run.out)
            self.assertIn('m.rs\tsisuc::m::f', run.lines)
            warning = next(i for i, line in enumerate(run.lines) if line.startswith('Warning'))
            self.assertGreater(run.lines.index(m_g), warning)
        with self.subTest('a key holds its file'):
            run = self.reached([], [record('b.rs', m_f, 1)], '## a reason\na.rs\tsisuc::m::f\n')
            self.assertEqual(run.code, 1, run.out)
            self.assertIn('b.rs\tsisuc::m::f', run.lines)
        for line in (10, 12):
            with self.subTest(f'an edit above a function, now on line {line}'):
                run = self.reached([record('m.rs', m_f, 0, line)], [record('m.rs', m_f, 1, line)],
                                   '## a reason\nm.rs\tsisuc::m::f\n')
                self.assertEqual(run.code, 0, run.out)
                self.assertNotIn('Warning', run.out)
        with self.subTest('no gap, one stale key'):
            run = self.reached([], [], f'## a reason\n{m_g}\n')
            self.assertEqual(run.code, 0, run.out)
            self.assertIn('measured 3 functions', run.out)
            self.assertIn('Known keys: 1', run.out)

    def quoted_rows(self):
        token = 'sisuc[a]::m::token::<{}>'
        for quoted in ("'>'", "'<'"):
            with self.subTest(f'a generic argument {quoted} merges'):
                run = self.reached([record('m.rs', token.format("'a'"), 1)],
                                   [record('m.rs', token.format(quoted), 1)])
                self.no_gap(run, 'token')
        with self.subTest('a quoted bracket inside a type'):
            run = self.reached([], [record('m.rs', "<sisuc[a]::T<'<'>>::v", 1)])
            self.assertEqual(run.code, 1, run.out)
            self.assertIn('m.rs\t<sisuc::T>::v', run.lines)
        with self.subTest('a lifetime is no quoted character'):
            run = self.reached([], [record('m.rs', "sisuc[a]::m::g::<for<'a> fn(&'a i64) -> i64>", 1)])
            self.assertEqual(run.code, 1, run.out)
            self.assertIn('m.rs\tsisuc::m::g', run.lines)

    def test_definitions_should_keep_their_identity(self):
        i32_value = '<sisuc[abc]::Tag<i32>>::value'
        bool_value = '<sisuc[abc]::Tag<bool>>::value'
        key = 'tag.rs\t<sisuc::Tag>::value'
        known = f'## a reason\n{key}\n'
        seeds = [record('tag.rs', i32_value, 1, 10), record('tag.rs', bool_value, 0, 20)]
        valid = [record('tag.rs', i32_value, 1, 10), record('tag.rs', bool_value, 1, 20)]
        with self.subTest('the second definition is listed'):
            run = self.reached(seeds, valid)
            self.assertEqual(run.code, 1, run.out)
            self.assertEqual(run.lines.count(key), 1, run.out)
        rows = [
            ('a key that matches both definitions exits 2', seeds, valid, ''),
            ('the lines are in valid only', [], [record('tag.rs', i32_value, 1, 10),
                                                 record('tag.rs', bool_value, 1, 20)], ''),
            ('one line in each export', [record('tag.rs', i32_value, 1, 10)],
             [record('tag.rs', bool_value, 1, 20)], 'lines 10 and 20'),
            ('both lines in seeds only', [record('tag.rs', i32_value, 1, 10), record('tag.rs', bool_value, 1, 20)],
             [record('tag.rs', i32_value, 1, 10)], ''),
            ('unreached lines count', [record('tag.rs', i32_value, 0, 10), record('tag.rs', bool_value, 0, 20)],
             [record('tag.rs', i32_value, 1, 10)], 'lines 10 and 20'),
        ]
        for label, in_seeds, in_valid, lines in rows:
            with self.subTest(label):
                run = self.reached(in_seeds, in_valid, known)
                self.assertEqual(run.code, 2, run.out)
                self.assertIn('two definitions', run.out)
                self.assertIn(lines, run.out)
                self.assertIn('generator rule.', run.out)
                self.assertNotIn('rule..', run.out)

    def test_broken_measurement_should_exit_2(self):
        good = export(*ANCHORS)
        known_key = f'{FN_KEY}\n'
        floor = 'valid-program export reaches no function under '
        valid_with = lambda *extra: Run(self, good, export(*ANCHORS, *extra))  # noqa: E731
        cases = [
            ('a missing valid export', lambda: Run(self, good, None), 'cannot read the export'),
            ('a missing seeds export', lambda: Run(self, None, good), 'cannot read the export'),
            ('an export that is not JSON', lambda: Run(self, good, '{"data": '), 'is not valid JSON'),
            ('an export without data[0].functions', lambda: Run(self, good, {'data': [{}]}),
             'has no data[0].functions'),
            ('no valid-program functions', lambda: Run(self, good, export()), f'{floor}check/'),
            ('only unreached valid-program functions',
             lambda: Run(self, good, export(record('check/mod.rs', METHOD, 0))), f'{floor}check/'),
            ('only out-of-scope valid-program functions',
             lambda: Run(self, good, export(record('generate/item.rs', METHOD, 1))), f'{floor}check/'),
            ('no seed functions', lambda: Run(self, export(), good), 'seeds export reaches no function under check/'),
            ('seeds hold only unreached records',
             lambda: Run(self, export(*(record(f'{s}x.rs', f'sisuc[a]::{s[:-1]}::x', 0) for s in STAGES)), good),
             'seeds export reaches no function under check/'),
            ('a codegen/ record with count 0',
             lambda: Run(self, good, export(*ANCHORS[:2], record('codegen/mod.rs', 'sisuc[a]::codegen::x', 0))),
             f'{floor}codegen/'),
            ('a file that starts with the letters of a stage',
             lambda: Run(self, good, export(record('checker.rs', METHOD, 1), record('codegenerate.rs', 'sisuc[a]::cg', 1),
                                            ANCHORS[1])),
             f'{floor}check/'),
            ('a gap, and no codegen/ in the export',
             lambda: Run(self, good, export(*ANCHORS[:2], record('conformance.rs', FN, 1))), f'{floor}codegen/'),
            ('a closure is no identity',
             lambda: Run(self, good, export(*ANCHORS[:2], record('codegen/mod.rs', 'sisuc[a]::codegen::f::{closure#0}', 1))),
             f'{floor}codegen/'),
            ('a file that holds the stage inside its path',
             lambda: Run(self, good, export(record('x/check/a.rs', METHOD, 1), *ANCHORS[1:])), f'{floor}check/'),
            ('a name that stays _R', lambda: valid_with(record('check/mod.rs', '_Rbogus', 1)), 'still mangled'),
            ('a name that stays _ZN', lambda: valid_with(record('check/mod.rs', '_ZNbroken', 1)), 'still mangled'),
            ('a name that stays _ZN, and its key is known',
             lambda: Run(self, good, export(*ANCHORS, record('check/mod.rs', '_ZNbroken', 1)),
                         '## a reason\ncheck/mod.rs\t_ZNbroken\n'), 'still mangled'),
            ('a mangled name after a file name', lambda: valid_with(record('check/mod.rs', 'file.rs:_Rbad', 1)),
             'still mangled'),
            ('an unreached mangled name', lambda: valid_with(record('check/mod.rs', '_ZNbroken', 0)), 'still mangled'),
            ('a record without a name',
             lambda: valid_with({'count': 1, 'filenames': [SRC + 'a.rs'], 'regions': [[1]]}), 'unexpected KeyError'),
            ('a record without a count', lambda: valid_with(lacking(record('a.rs', FN, 1), 'count')), 'unexpected'),
            ('a record with a count that is a string', lambda: valid_with(altered(record('a.rs', FN, 1), count='1')),
             'unexpected'),
            ('a record with no regions', lambda: valid_with(altered(record('a.rs', FN, 1), regions=[])), 'unexpected'),
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
        if c_plus_plus_filt(LEGACY) == 'sisuc::f::h0123456789abcdef':
            cases.append(('a legacy Rust name', lambda: valid_with(record('check/mod.rs', LEGACY, 1)), 'legacy'))
        for label, anchor in itertools.product(('seeds', 'valid-program'), zip(STAGES, ANCHORS)):
            stage, missing = anchor
            short = export(*(a for a in ANCHORS if a is not missing))
            cases.append((f'{label} export misses {stage}',
                          lambda label=label, short=short: Run(self, *((short, good) if label == 'seeds' else (good, short))),
                          f'the {label} export reaches no function under {stage}'))
        for label, value in (('absent', None), ('empty', []), ('a string', 'a.rs'), ('an integer item', [1])):
            gap = record('a.rs', 'sisuc[a]::real_gap', 1)
            gap = lacking(gap, 'filenames') if value is None else altered(gap, filenames=value)
            cases.append((f'a record whose filenames is {label}', lambda gap=gap: valid_with(gap), 'has no file name'))
        cases.append(('an unreached record with no file, in seeds',
                      lambda: Run(self, export(*ANCHORS, altered(record('a.rs', FN, 0), filenames=[])), good),
                      'has no file name'))
        for label, make, reason in cases:
            with self.subTest(label):
                run = make()
                self.assertEqual(run.code, 2, run.out)
                self.assertIn(reason, run.out)
                self.assertIn('did not finish', run.out)
                if label.startswith('an unreached record with no file'):
                    self.assertIn('seeds.json', run.out)

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
        run = Run(self, export(*ANCHORS), export(*ANCHORS, *valid))
        self.assertEqual(run.code, 1, run.out)
        listed = [line for line in run.lines if '\t' in line]
        self.assertEqual(listed, [f'file{n}.rs\t{want}' for n, (_, want) in enumerate(names)])


if __name__ == '__main__':
    unittest.main()
