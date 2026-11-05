"""Finds generator gaps: functions that a valid program of tests/run or tests/run-fail reaches and
no fixed fuzz seed reaches. It compares two `llvm-cov export` files with a list of known gaps.

A function is (file, first line, name). The file is the part of its path after `crates/sisuc/src/`.
The name is the `c++filt -s rust` name without crate hashes and generic arguments, so the
instances of a generic function merge. A closure is not a function: its enclosing function counts.
`generate/`, `fuzz.rs`, `fuzz_program.rs`, `report.rs`, `diagnostic/json.rs` and
`diagnostic/render.rs` are out of scope. A function is reached when one of its records has a count
above 0.

The known list has sections. A section is a line `## <reason>`, then key lines `<file><TAB><name>`.

Usage: python3 fuzz_gaps.py <seeds.json> <valid.json> <known_gaps.txt>
Prints one Markdown section that starts with `## Generator gaps`. Exit codes:
  0  no new gap (a known key that matches no gap is a warning),
  1  a new gap,
  2  the check did not finish: a broken measurement is never a pass.
"""
import collections
import json
import re
import subprocess
import sys

MARKER = '/crates/sisuc/src/'
OUT_OF_SCOPE_FILES = {'fuzz.rs', 'fuzz_program.rs', 'report.rs', 'diagnostic/json.rs',
                      'diagnostic/render.rs'}
CRATE_HASH = re.compile(r'(?<=\w)\[[0-9a-f]+\]')
HEADING = '## Generator gaps'


class Broken(Exception):
    """The measurement cannot be trusted. The message is the reason, without a final period."""


def matching_close(name: str, start: int) -> int:
    """The index of the `>` that closes the `<` at `start`. The `>` of `->` does not count."""
    depth = 0
    for i in range(start, len(name)):
        if name[i] == '<':
            depth += 1
        elif name[i] == '>' and name[i - 1] != '-':
            depth -= 1
            if depth == 0:
                return i
    raise Broken(f'the name {name!r} has angle brackets that do not balance')


def strip_generics(name: str) -> str:
    """Removes each `<...>` that follows an identifier character or `::`, with that `::`. A `<`
    at the start or after another `<` opens `<Type>` or `<T as Trait>`, and stays."""
    out: list[str] = []
    i = 0
    while i < len(name):
        follows_name = bool(out) and (out[-1].isalnum() or out[-1] == '_' or out[-2:] == [':', ':'])
        if name[i] == '<' and follows_name:
            if out[-2:] == [':', ':']:
                del out[-2:]
            i = matching_close(name, i) + 1
        else:
            out.append(name[i])
            i += 1
    return ''.join(out)


def normalize(name: str) -> str | None:
    """The name without crate hashes and generic arguments, or None for a closure. The closure
    test comes last: a generic argument can hold a closure, and the function is not one."""
    name = strip_generics(CRATE_HASH.sub('', name))
    return None if '{closure' in name else name


def demangle(names: list[str]) -> list[str]:
    """One `c++filt -s rust` call for all names. It must return one line for each name."""
    try:
        done = subprocess.run(['c++filt', '-s', 'rust'], input='\n'.join(names) + '\n',
                              capture_output=True, text=True, check=False)
    except OSError as e:
        raise Broken(f'c++filt did not run ({e.strerror})') from e
    if done.returncode != 0:
        raise Broken(f'c++filt failed with exit code {done.returncode}')
    lines = done.stdout.split('\n')[:-1]
    if len(lines) != len(names):
        raise Broken(f'c++filt returned a wrong number of lines: {len(lines)} for {len(names)} names')
    return lines


def load(path: str) -> list[dict]:
    """The `data[0].functions` records of an export."""
    try:
        with open(path, encoding='utf-8') as f:
            document = json.load(f)
    except OSError as e:
        raise Broken(f'cannot read the export {path} ({e.strerror})') from e
    except ValueError as e:
        raise Broken(f'the export {path} is not valid JSON ({e})') from e
    try:
        functions = document['data'][0]['functions']
    except (KeyError, IndexError, TypeError):
        functions = None
    if not isinstance(functions, list):
        raise Broken(f'the export {path} has no data[0].functions')
    return functions


def in_scope(functions: list[dict]) -> list[tuple[str, int, str, bool]]:
    """(file, first line, mangled name, reached) of each record in scope."""
    found = []
    for record in functions:
        files = record.get('filenames')
        if not files or MARKER not in files[0]:
            continue
        file = files[0].rsplit(MARKER, 1)[1]
        if file.startswith('generate/') or file in OUT_OF_SCOPE_FILES:
            continue
        found.append((file, record['regions'][0][0], record['name'], record['count'] > 0))
    return found


def identities(rows: list[tuple[str, int, str, bool]], names: dict[str, str | None]) -> dict:
    """Maps (file, first line, name) to whether any record of that function was reached."""
    seen: dict[tuple[str, int, str], bool] = {}
    for file, line, mangled, reached in rows:
        name = names[mangled]
        if name is not None:
            key = (file, line, name)
            seen[key] = seen.get(key, False) or reached
    return seen


def read_known(path: str) -> set[tuple[str, str]]:
    """The (file, name) keys of the known list."""
    try:
        with open(path, encoding='utf-8') as f:
            text = f.read()
    except OSError as e:
        raise Broken(f'cannot read the known list {path} ({e.strerror})') from e
    keys: set[tuple[str, str]] = set()
    reason = None
    for number, line in enumerate(text.splitlines(), 1):
        if not line.strip():
            continue
        if line.startswith('##'):
            reason = line[2:].strip()
            if not reason:
                raise Broken(f'line {number} of the known list is a section with an empty reason')
        elif reason is None:
            raise Broken(f'line {number} of the known list is a key before any section header')
        else:
            file, tab, name = line.partition('\t')
            if not (tab and file and name):
                raise Broken(f'line {number} of the known list needs a TAB between file and name')
            keys.add((file, name))
    return keys


def check_keys_are_unique(known: set[tuple[str, str]], seen: dict, label: str) -> None:
    """A known key must name one definition. Two first lines under one key mean that the key
    would hide the second definition too."""
    lines = collections.defaultdict(set)
    for file, line, name in seen:
        lines[(file, name)].add(line)
    for key in sorted(known):
        if len(lines[key]) > 1:
            raise Broken(f'the known key {key[0]} {key[1]} matches two definitions, on lines '
                         + ' and '.join(map(str, sorted(lines[key]))) + f', in the {label} export')


def fence(lines: list[str]) -> str:
    return '```text\n' + '\n'.join(lines) + '\n```'


def section(*parts: str) -> str:
    return '\n\n'.join((HEADING, *parts)) + '\n'


def compare(seeds_path: str, valid_path: str, known_path: str) -> tuple[int, str]:
    exports = {'seeds': in_scope(load(seeds_path)), 'valid-program': in_scope(load(valid_path))}
    known = read_known(known_path)
    mangled = sorted({row[2] for rows in exports.values() for row in rows})
    names: dict[str, str | None] = {}
    for original, demangled in zip(mangled, demangle(mangled) if mangled else []):
        if demangled.startswith('_R'):
            raise Broken(f'the name {original} is still mangled after c++filt')
        names[original] = normalize(demangled)
    seeds, valid = (identities(exports[label], names) for label in ('seeds', 'valid-program'))
    if not any(valid.values()):
        raise Broken('the export has no function that valid programs reach')
    if not any(seeds.values()):
        raise Broken('the export has no function that the seeds reach')
    for label, seen in (('seeds', seeds), ('valid-program', valid)):
        check_keys_are_unique(known, seen, label)

    gaps = {key for key, reached in valid.items() if reached} - {k for k, r in seeds.items() if r}
    gap_keys = {(file, name) for file, _, name in gaps}
    new = sorted(gap_keys - known)
    stale = sorted(known - gap_keys)
    stale_text = [] if not stale else [
        'Warning: these known keys match no gap. Remove them from `fuzz/known_gaps.txt`:',
        fence([f'{file}\t{name}' for file, name in stale])]
    if new:
        return 1, section(
            'Valid programs reach these functions, and no fixed seed reaches them:',
            fence([f'{file}\t{name}' for file, name in new]),
            'Add a generator rule for each function, or add its line to `fuzz/known_gaps.txt` '
            'under a section with a reason.', *stale_text)
    return 0, section(
        f'No new generator gap. The check measured {sum(valid.values())} functions that valid '
        f'programs reach. Known keys: {len(known)}.', *stale_text)


def main(argv: list[str]) -> int:
    try:
        if len(argv) != 4:
            raise Broken('usage: fuzz_gaps.py <seeds.json> <valid.json> <known_gaps.txt>')
        code, text = compare(*argv[1:])
    except Broken as e:
        code, text = 2, f'{e}'
    # shortcut: any other failure also exits 2; an uncaught one exits 1, which reads as a new gap.
    except Exception as e:
        code, text = 2, f'an unexpected {type(e).__name__}: {e}'
    if code == 2:
        text = section(f'The gap check did not finish, so it proves nothing about gaps. Reason: {text}.')
    sys.stdout.write(text)
    return code


if __name__ == '__main__':
    sys.exit(main(sys.argv))
