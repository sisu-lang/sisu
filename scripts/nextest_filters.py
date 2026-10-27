"""Checks that each `test(...)` leaf of a nextest filter matches a test.
A filter that matches no test does nothing and says nothing: a renamed test then loses its override.

Usage: python3 nextest_filters.py CONFIG LIST_JSON
CONFIG is .config/nextest.toml. LIST_JSON is the output of
`cargo nextest list --workspace --message-format json`. It prints one line for each leaf that
matches no test, or that is not `=name` or a substring (nextest also has /regex/, ~x and #glob,
which this check does not read), and exits 1. It prints nothing and exits 0 when every leaf is fine.
"""
import json
import re
import sys
import tomllib

# nextest allows whitespace, a newline too, before `(`. A leaf runs to the first `)`; a regex leaf
# runs to its closing `/`, so that the message shows all of it.
LEAF = re.compile(r'\btest\s*\((/.*?/|[^)]*)\)')


def leaves(config: dict) -> list[str]:
    """The argument of each `test(...)` leaf of each override and script filter, once each. nextest
    trims the space before an argument, not the space after it."""
    found = []
    for profile in config.get('profile', {}).values():
        for table in ('overrides', 'scripts'):
            for entry in profile.get(table, []):
                found += [leaf.lstrip() for leaf in LEAF.findall(entry.get('filter', ''))]
    return list(dict.fromkeys(found))


def line_of(text: str, leaf: str) -> int:
    """The line of the first leaf with this argument, in the text without its comments."""
    code = re.sub(r'(?m)^[ \t]*#.*', '', text)
    # shortcut: a leaf inside an escaped basic string may not be found, so it reports line 1.
    found = re.search(r'\btest\s*\(\s*' + re.escape(leaf) + r'\)', code)
    return code.count('\n', 0, found.start()) + 1 if found else 1


def problem(leaf: str, names: list[str]) -> str | None:
    """What is wrong with a leaf, or None."""
    # shortcut: a leaf is matched against every binary, not the one its filter names, so a rename
    # in another binary can hide behind a leaf of the same text; scope by `binary(...)` then.
    if leaf.startswith(('/', '~', '#')) or '\\' in leaf:
        return 'is not a name leaf; use test(=name) or test(substring)'
    found = leaf[1:] in names if leaf.startswith('=') else any(leaf in name for name in names)
    return None if found else 'matches no test; the test was renamed or deleted'


def check(config_path: str, list_path: str) -> list[str]:
    """One line for each bad leaf of the config."""
    with open(config_path, 'rb') as file:
        raw = file.read()
    with open(list_path, encoding='utf-8') as file:
        suites = json.load(file)['rust-suites']
    names = [name for suite in suites.values() for name in suite.get('testcases', {})]
    text = raw.decode('utf-8')
    found = []
    for leaf in leaves(tomllib.loads(text)):
        wrong = problem(leaf, names)
        if wrong:
            found.append((line_of(text, leaf), leaf, wrong))
    return [f"{config_path}:{line}: filter leaf `test({leaf})` {wrong}" for line, leaf, wrong in sorted(found)]


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(__doc__)
        return 2
    lines = check(*argv)
    if lines:
        print('\n'.join(lines))
    return 1 if lines else 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
