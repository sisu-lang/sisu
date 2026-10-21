"""Turns a cargo-llvm-cov export, its LCOV file and a git diff into the uncovered regions the
change touches.

Usage: python3 coverage_report.py COVERAGE.json LCOV.info CHANGES.diff
Exits 0 for any valid report, however low the coverage, and 1 when an input is unusable.
"""
import json
import re
import subprocess
import sys
from typing import Callable, NamedTuple

# Integration tests, and unit-test modules in their own file (src/**/tests.rs, src/**/tests/).
TEST_DIR = re.compile(r'^crates/[^/]+/(?:tests/|src/(?:.+/)?tests(?:\.rs$|/))')
# A panic that states an invariant, which no test should reach.
INVARIANT = re.compile(r'\b(?:unreachable|panic)!\(')
HUNK = re.compile(r'^@@ -\d+(?:,(\d+))? \+(\d+)(?:,(\d+))? @@')


class Region(NamedTuple):
    start: int
    start_col: int
    end: int
    end_col: int
    never_run: bool


def changed_lines(diff: str) -> dict[str, set[int]]:
    """The new-side line numbers of added lines, per Rust file that is not test code (`TEST_DIR`)."""
    result: dict[str, set[int]] = {}
    path = None
    line = old_left = new_left = 0
    for text in diff.splitlines():
        if old_left > 0 or new_left > 0:  # inside a hunk, "+++ " is an added line
            if text.startswith('+'):
                if path:
                    result.setdefault(path, set()).add(line)
                line += 1
                new_left -= 1
            elif text.startswith('-'):
                old_left -= 1
            elif text.startswith(' '):
                line += 1
                old_left -= 1
                new_left -= 1
        elif text.startswith('+++ '):
            name = text[4:].removeprefix('b/')
            ok = name.endswith('.rs') and not TEST_DIR.match(name)
            path = name if ok else None
        elif text.startswith('diff --git '):
            path = None
        elif m := HUNK.match(text):
            old_left = int(1 if m[1] is None else m[1])
            line = int(m[2])
            new_left = int(1 if m[3] is None else m[3])
    return result


def relative(path: str, root: str) -> str | None:
    return path[len(root) + 1:] if path.startswith(root + '/') else None


def uncovered(export: dict, root: str) -> dict[str, list[Region]]:
    """Zero-count regions per file; a function that never ran is one never_run region."""
    # A source function is its file plus the span of its first region; its instantiations
    # have different mangled names, so they are grouped by that key and merged span by span.
    functions: dict[tuple, tuple[int, dict[tuple, int]]] = {}
    for record in export['data'][0]['functions']:
        regions = [r for r in record['regions'] if r[7] == 0]
        if not regions:
            continue
        names = record['filenames']
        first = regions[0]
        count, spans = functions.setdefault((names[first[5]], tuple(first[:4])), (0, {}))
        functions[(names[first[5]], tuple(first[:4]))] = (max(count, record['count']), spans)
        for r in regions:
            key = (names[r[5]], *r[:4])
            spans[key] = max(spans.get(key, 0), r[4])
    result: dict[str, list[Region]] = {}
    for (name, first), (count, spans) in functions.items():
        if count == 0:
            path = relative(name, root)
            if path is None:
                continue
            # The head region alone often misses the body, so the function spans all its regions.
            own = [k for k in spans if k[0] == name]
            top = min(own, key=lambda k: k[1:])
            result.setdefault(path, []).append(Region(top[1], top[2], max(k[3] for k in own), 0, True))
            continue
        for (file, *span), n in spans.items():
            if n == 0 and (path := relative(file, root)) is not None:
                result.setdefault(path, []).append(Region(*span, False))
    return {p: sorted(rs) for p, rs in result.items()}


def line_hits(lcov: str, root: str) -> dict[str, dict[int, int]]:
    result: dict[str, dict[int, int]] = {}
    hits: dict[int, int] = {}
    for text in lcov.splitlines():
        if text.startswith('SF:'):
            path = relative(text[3:], root)
            hits = result.setdefault(path, {}) if path else {}
        elif text.startswith('DA:'):
            number, count = text[3:].split(',')[:2]
            hits[int(number)] = hits.get(int(number), 0) + int(count)
    return result


def report(export: dict, lcov: str, diff: str, root: str,
           read_line: Callable[[str, int], str]) -> str:
    totals = export['data'][0]['totals']
    changed = changed_lines(diff)
    hits = line_hits(lcov, root)
    instrumented = [h[n] for p, ns in changed.items() for n in ns if n in (h := hits.get(p, {}))]
    if instrumented:
        covered = sum(1 for h in instrumented if h > 0)
        pct = f'changed lines {100 * covered / len(instrumented):.1f}% ({covered} of {len(instrumented)})'
    else:
        pct = 'changed lines: none'
    head = f"Coverage: lines {totals['lines']['percent']:.1f}%, regions {totals['regions']['percent']:.1f}%, {pct}."

    touching, other = [], []
    for path, regions in sorted(uncovered(export, root).items()):
        if path not in changed:
            continue
        for r in regions:
            line = read_line(path, r.start)
            source = line.strip()[:80]
            if r.never_run:
                text = f'{path}:{r.start}: never run: {source}'
            else:
                text = f'{path}:{r.start}:{r.start_col}-{r.end}:{r.end_col}: {source}'
            hit = any(n in changed[path] for n in range(r.start, r.end + 1)) and not INVARIANT.search(line)
            (touching if hit else other).append(text)
    return (f'{head}\n\nUncovered regions touching changed code:\n' + '\n'.join(touching or ['(none)'])
            + '\n\nOther uncovered regions in changed files (advisory):\n' + '\n'.join(other or ['(none)']))


def main(argv: list[str]) -> int:
    cache: dict[str, list[str]] = {}

    def read_line(path: str, number: int) -> str:
        if path not in cache:
            try:
                with open(f'{root}/{path}', errors='replace') as f:
                    cache[path] = f.read().splitlines()
            except OSError:
                cache[path] = []
        lines = cache[path]
        return lines[number - 1] if 0 < number <= len(lines) else ''

    try:
        if len(argv) != 3:
            raise ValueError('usage: coverage_report.py COVERAGE.json LCOV.info CHANGES.diff')
        texts = []
        for path in argv:
            with open(path, errors='replace') as f:
                texts.append(f.read())
        export = json.loads(texts[0])
        data = export['data'][0]
        for kind in ('lines', 'regions'):
            if not isinstance(data['totals'][kind]['percent'], (int, float)):
                raise ValueError(f'totals.{kind}.percent is not a number')
        if not isinstance(data['functions'], list):
            raise ValueError('functions is not a list')
        root = subprocess.run(['git', 'rev-parse', '--show-toplevel'], capture_output=True,
                              text=True, check=True).stdout.strip()
        text = report(export, texts[1], texts[2], root, read_line)
    except (OSError, ValueError, LookupError, TypeError, AttributeError,
            subprocess.CalledProcessError) as e:
        reason = ' '.join(f'{type(e).__name__}: {e}'.split())
        print(f'coverage_report: {reason}', file=sys.stderr)
        return 1
    print(text)
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
