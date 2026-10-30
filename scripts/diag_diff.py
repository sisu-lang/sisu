"""Checks every `.sisu` file under each DIR with two sisu builds and prints each program whose
diagnostics differ: their codes, positions or count, or a crash. A review uses it to show that a
change accepts no program it should reject and adds no cascade.

Usage: python3 diag_diff.py BASE_SISU HEAD_SISU DIR...
Exits 1 when a program differs.
"""
import json
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path


def diagnostics(sisu, path):
    """Each diagnostic sisu reports on `path`: (code, severity, line, column, secondaries)."""
    run = subprocess.run([sisu, 'check', '--format', 'json', str(path)],
                         capture_output=True, text=True)
    if run.returncode not in (0, 1):
        return [('crash', 'error', run.returncode, 0, ())]
    found = []
    for line in run.stderr.splitlines():
        d = json.loads(line)
        if d['kind'] == 'diagnostic':
            p = d['primary']
            secondary = tuple((s['line'], s['column']) for s in d['secondary'])
            found.append((d['code'], d['severity'], p['line'], p['column'], secondary))
    return found


def kind(base, head):
    """How the head's diagnostics differ from the base's, or None when they are the same."""
    if base == head:
        return None
    rejects = [any(d[1] == 'error' for d in ds) for ds in (base, head)]
    return {(True, False): 'newly accepted', (False, True): 'newly rejected'}.get(
        tuple(rejects), 'changed')


def show(ds):
    return ', '.join(f'{c} {line}:{col}' + ''.join(f' +{a}:{b}' for a, b in s)
                     for c, _, line, col, s in ds) or 'none'


def main(base_sisu, head_sisu, *roots):
    paths = sorted({p for root in roots for p in Path(root).rglob('*.sisu')})
    with ThreadPoolExecutor() as pool:
        bases = pool.map(lambda p: diagnostics(base_sisu, p), paths)
        heads = pool.map(lambda p: diagnostics(head_sisu, p), paths)
        rows = list(zip(paths, bases, heads))
    counts = {}
    for path, base, head in rows:
        if k := kind(base, head):
            counts[k] = counts.get(k, 0) + 1
            print(f'{k}: {path}\n  base: {show(base)}\n  head: {show(head)}')
    tally = ', '.join(f'{n} {k}' for k, n in sorted(counts.items())) or 'no differences'
    print(f'diag_diff: {len(paths)} programs, {tally}')
    return 1 if counts else 0


if __name__ == '__main__':
    if len(sys.argv) < 4:
        sys.exit(__doc__)
    sys.exit(main(*sys.argv[1:]))
