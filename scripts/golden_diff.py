"""Checks that a re-bless changed the expected files only by the given text substitutions, so a
review can sample a mechanical golden diff instead of reading it: every changed `.err`, `.out`
and `.json` of a test case between BASE and HEAD keeps its line count, and each changed line
equals its old line with every OLD replaced by NEW.

Usage: python3 golden_diff.py BASE..HEAD OLD NEW [OLD NEW]...
Exits 1 when a file differs in any other way.
"""
import subprocess
import sys

SUFFIXES = ('.err', '.out', '.json')
# Where the test cases and their expected files live.
CASES = (':(glob)crates/*/tests/**', ':(glob)site/programs/**')


def git(*args):
    # An `.out` file holds the program's raw bytes, so a byte that is not UTF-8 is kept as a surrogate.
    return subprocess.run(['git', *args], capture_output=True, text=True, encoding='utf-8',
                          errors='surrogateescape', check=True).stdout


def main(span, subs):
    base, head = span.split('..')
    changed = git('diff', '--name-only', '--diff-filter=M', base, head, '--', *CASES)
    files = [f for f in changed.split() if f.endswith(SUFFIXES)]
    lines = problems = 0
    for f in files:
        old = git('show', f'{base}:{f}').split('\n')
        new = git('show', f'{head}:{f}').split('\n')
        if len(old) != len(new):
            print(f'{f}: {len(old)} lines became {len(new)}')
            problems += 1
            continue
        for a, b in zip(old, new):
            if a == b:
                continue
            lines += 1
            want = a
            for o, n in subs:
                want = want.replace(o, n)
            if want != b:
                print(f'{f}: {a!r} became {b!r}')
                problems += 1
    print(f'golden_diff: {len(files)} files, {lines} changed lines, {problems} problems')
    return 1 if problems else 0


if __name__ == '__main__':
    if len(sys.argv) < 4 or len(sys.argv) % 2:
        sys.exit(__doc__)
    pairs = list(zip(sys.argv[2::2], sys.argv[3::2]))
    sys.exit(main(sys.argv[1], pairs))
