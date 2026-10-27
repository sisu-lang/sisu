"""Warns about the tests in a nextest JUnit report that ran over their time budget
(CODING_STANDARDS.md, Tests). A warning, never a failure: wall-clock time depends on the machine
and its load.

Usage: python3 slow_tests.py JUNIT.xml [--diff REF | --fail-changed REF]
Prints nothing when the report is missing or every test is within budget, and one line when the
report cannot be read. Under GitHub Actions, each slow test is also a workflow warning. It exits 0,
except with --fail-changed. With --diff, it lists only the slow tests whose name `git diff -U0 REF`
mentions, the tests a change added or edited, and counts the rest: on a loaded machine the rest
are noise. With --fail-changed, it exits 1 when a test that the diff to REF adds or edits takes more
than FAIL_FACTOR times its budget. It then lists every slow test, as a warning.
"""
import argparse
import os
import re
import subprocess
import sys
import time
import tomllib
import xml.etree.ElementTree as ET

import changed_tests

# Seconds on a GitHub-hosted runner, which takes about twice as long as a developer machine. A
# unit test runs in-process; an end-to-end test spawns sisu, links, and may run under Valgrind,
# and a `tests/run` case, with three builds and two Valgrind runs, takes about 2 s.
UNIT_BUDGET = 1.0
END_TO_END_BUDGET = 4.0
# A changed test fails the check above this multiple of its budget. Wall-clock time varies, so a
# test over its budget by less is only a warning.
FAIL_FACTOR = 2
# The datatest-stable harness: one test per case file. Its name is `<function>::<path under root>`.
CASES = 'crates/sisuc/tests/cases.rs'
CASES_BINARY = 'sisuc::cases'
CASE_ENTRY = re.compile(r'\{\s*test\s*=\s*(\w+)\s*,\s*root\s*=\s*"([^"]+)"')


def timed(junit: str) -> list[tuple[float, float, str]]:
    """(seconds, budget, name) of each test in the report, setup scripts left out."""
    tests = []
    for case in ET.fromstring(junit).iter('testcase'):
        binary = case.get('classname', '')
        if binary.startswith('@'):  # a setup script, not a test
            continue
        # nextest names an integration test binary `<crate>::<binary>`, a crate's own tests `<crate>`.
        budget = END_TO_END_BUDGET if '::' in binary else UNIT_BUDGET
        tests.append((float(case.get('time', '0')), budget, f"{binary} {case.get('name')}"))
    return tests


def over_budget(tests: list[tuple[float, float, str]]) -> list[tuple[float, float, str]]:
    """The tests over their budget, slowest first."""
    return sorted((test for test in tests if test[0] > test[1]), reverse=True)


def slow_tests(junit: str) -> list[tuple[float, float, str]]:
    """(seconds, budget, name) of each test over its budget, slowest first."""
    return over_budget(timed(junit))


def in_diff(name: str, diff: str) -> bool:
    """Whether the diff mentions the test: its function name, which a `fn` line or a hunk header
    carries (`.gitattributes` gives Rust files git's `rust` hunk headers), or the stem of its
    `tests/` case file, such as `fib` for `run::fib.sisu`."""
    key = name.split(' ', 1)[-1].rsplit('::', 1)[-1].removesuffix('.sisu')
    return re.search(rf'(?<!\w){re.escape(key)}(?!\w)', diff) is not None


def locate(path: str, repo: str) -> tuple[str, str, str] | None:
    """(nextest binary, module prefix, directory of child modules) of a changed `.rs` file, or None
    when the file is not in a target. A test in the file is named `<prefix><inline modules>::<fn>`
    in that binary.

    shortcut: only `src/lib.rs`, `src/main.rs`, its modules and `tests/` files are known; a file
    under `src/bin/` gives None, so a slow test there only warns. Upgrade if such a target is added."""
    m = re.fullmatch(r'(.*?)/(src|tests)/(.*)\.rs', path)
    if m is None:
        return None
    crate, kind, rest = m.groups()
    try:
        with open(f'{repo}/{crate}/Cargo.toml', 'rb') as file:
            manifest = tomllib.load(file)
    except FileNotFoundError:
        return None
    package = manifest['package']['name']
    parts = rest.split('/')
    if kind == 'src':
        binary, root, modules = package, f'{crate}/src', parts
        if parts == ['main']:
            name = next((b['name'] for b in manifest.get('bin', []) if b.get('path') == 'src/main.rs'), package)
            binary = f'{package}::bin/{name}'
    else:  # `tests/<bin>.rs`, or a file below `tests/<bin>/`, whose root file is `main.rs`
        binary, root, modules = f'{package}::{parts[0]}', f'{crate}/tests/{parts[0]}', parts[1:]
    if modules[-1:] == ['mod'] or modules == ['main'] or (kind == 'src' and modules in (['lib'], ['main'])):
        modules = modules[:-1]
    prefix = ''.join(f'{module}::' for module in modules)
    return binary, prefix, '/'.join([root, *modules])


def maps_to(test: str, name: str, located: tuple[str, str, str], repo: str) -> bool:
    """Whether the report entry `test` ("binary name") is the test `name` of the located file. The
    name between the prefix and the function holds only inline modules: a module with a file of its
    own below the file is another file's test."""
    binary, prefix, children = located
    suite, _, junit_name = test.partition(' ')
    rest = junit_name.removeprefix(prefix)
    if suite != binary or not junit_name.startswith(prefix) or not (rest == name or rest.endswith(f'::{name}')):
        return False
    directory = children
    for module in rest.removesuffix(name).split('::')[:-1]:
        if any(os.path.exists(f'{repo}/{directory}/{module}{tail}') for tail in ('.rs', '/mod.rs')):
            return False
        directory += f'/{module}'
    return True


def case_files(test: str, repo: str) -> list[str]:
    """The files of the case that the report entry `test` runs: the case file in each root of its
    function in the harness, if it exists. Empty for a test that is not a case."""
    binary, _, junit_name = test.partition(' ')
    function, _, path = junit_name.partition('::')
    if binary != CASES_BINARY:
        return []
    with open(f'{repo}/{CASES}', encoding='utf-8') as file:
        harness = file.read().split('datatest_stable::harness!', 1)[-1]
    files = (os.path.normpath(f'{os.path.dirname(os.path.dirname(CASES))}/{root}/{path}')
             for fn, root in CASE_ENTRY.findall(harness) if fn == function)
    return [f for f in files if os.path.exists(f'{repo}/{f}')]


def is_changed(test: str, files: set[str], pairs: set[tuple[str, str]], repo: str) -> bool:
    """Whether the diff changes the report entry `test`: its case file or a file next to it with
    the same stem, or the body of the test it maps to by its full name."""
    for case in case_files(test, repo):
        stem = os.path.splitext(case)[0]
        if any(os.path.splitext(f)[0] == stem for f in files):
            return True
    for path, name in pairs:
        located = locate(path, repo)
        if located and maps_to(test, name, located, repo):
            return True
    return False


def too_slow(tests: list[tuple[float, float, str]], ref: str, repo: str = '.') -> list[tuple[float, float, str]]:
    """The tests over FAIL_FACTOR times their budget that the diff to REF adds or edits."""
    limit = [test for test in tests if test[0] > FAIL_FACTOR * test[1]]
    if not limit:
        return []
    files = set(changed_tests.git(repo, '-c', 'core.quotepath=off', 'diff', '--name-only', '-M', '--no-color',
                                  '--no-ext-diff', '--no-textconv', ref).splitlines())
    pairs = changed_tests.changed(ref, repo)
    return [test for test in limit if is_changed(test[2], files, pairs, repo)]


def main(path: str, ref: str | None = None, fail_ref: str | None = None) -> int:
    try:
        with open(path, encoding='utf-8') as file:
            junit = file.read()
        tests = timed(junit)
        minutes = (time.time() - os.path.getmtime(path)) / 60
    except FileNotFoundError:
        return 0
    except (OSError, ET.ParseError, ValueError) as error:
        # An interrupted run can leave the report half written; that is still no reason to fail.
        print(f"slow_tests: cannot read {path}: {error}")
        return 0
    if fail_ref is not None and not tests:
        # Nextest ran no test, so it has already failed the job. Nothing here is a slow test.
        print("slow_tests: the report holds no test, so no changed test was checked")
        return 0
    slow = over_budget(tests)
    others = 0
    if ref is not None and slow:
        diff = subprocess.run(['git', 'diff', '-U0', '--no-color', '--no-ext-diff', '--no-textconv', ref],
                              capture_output=True, text=True, errors='surrogateescape').stdout
        mentioned = [test for test in slow if in_diff(test[2], diff)]
        others = len(slow) - len(mentioned)
        slow = mentioned
    if others:
        print(f"slow_tests: {others} other tests over budget are outside `git diff {ref}`")
    if slow:
        print(f"Tests over their time budget (CODING_STANDARDS.md, Tests), in {path} from {minutes:.0f} min ago:")
    for seconds, budget, name in slow:
        print(f"  {seconds:6.2f} s (budget {budget:g} s)  {name}")
        if os.environ.get('GITHUB_ACTIONS') == 'true':
            print(f"::warning title=Slow test::{name} took {seconds:.2f} s; its budget is {budget:g} s")
    failures = too_slow(tests, fail_ref) if fail_ref is not None else []
    for seconds, budget, name in failures:
        line = (f"{name} took {seconds:.2f} s; its budget is {budget:g} s and the limit for a changed test is "
                f"{FAIL_FACTOR * budget:g} s: shrink its input or split it (CODING_STANDARDS.md, Tests)")
        print(f"::error title=Slow changed test::{line}" if os.environ.get('GITHUB_ACTIONS') == 'true' else line)
    return int(bool(failures))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('junit')
    group = parser.add_mutually_exclusive_group()
    group.add_argument('--diff', metavar='REF')
    group.add_argument('--fail-changed', metavar='REF')
    args = parser.parse_args()
    sys.exit(main(args.junit, args.diff, args.fail_changed))
