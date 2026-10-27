"""Checks that each `test(...)` leaf of a nextest filter matches a test.
A filter that matches no test does nothing and says nothing: a renamed test then loses its override.

Usage: python3 nextest_filters.py CONFIG LIST_JSON
CONFIG is .config/nextest.toml. LIST_JSON is the output of
`cargo nextest list --workspace --message-format json`. It prints one line for each leaf that
matches no test, or that is not `=name` or a substring (nextest also has /regex/, ~x and #glob,
which this check does not read), and exits 1. It prints each line where a stale leaf is. It skips
an override with a `platform` key. It prints nothing and exits 0 when every leaf is fine.
"""
import json
import re
import sys
import tomllib

# nextest allows whitespace, a newline too, before `(`. A leaf runs to the first `)`; a regex leaf
# runs to its closing `/`, so that the message shows all of it.
LEAF = re.compile(r'\btest\s*\(\s*(/.*?/|[^)]*)\)')
# A TOML string or comment. Read in order, so a quote in a comment and a `#` in a string are no token.
TOKEN = re.compile(r"""'''.*?'''|\"\"\".*?\"\"\"|'[^'\n]*'|"(?:[^"\\\n]|\\.)*"|#[^\n]*""", re.S)
ASSIGN = re.compile(r'(?<![\w-])filter\s*=\s*$')


def checked_filters(config: dict) -> set[str]:
    """The filter of each override and script that has no `platform` key.

    shortcut: the check runs on the Linux list only, so an override for a platform is not checked.
    It is found by its filter text: the same text in an override with no platform is checked, on
    both lines. Upgrade when a platform override is added, to one list for each platform."""
    return {entry['filter'] for profile in config.get('profile', {}).values()
            for table in ('overrides', 'scripts') for entry in profile.get(table, [])
            if 'filter' in entry and 'platform' not in entry}


def leaves(text: str, filters: set[str]) -> set[tuple[int, str]]:
    """(line, argument) of each `test(...)` leaf in each `filter = ...` value of the config text
    that is one of `filters`. A TOML comment holds no leaf. nextest trims the space before an
    argument, not the space after it."""
    found = set()
    for token in TOKEN.finditer(text):
        before = text[text.rfind('\n', 0, token.start()) + 1:token.start()]
        if not ASSIGN.search(before):
            continue
        try:
            value = tomllib.loads('v = ' + token.group())['v']
        except tomllib.TOMLDecodeError:  # a comment
            continue
        if value in filters:
            found |= {(text.count('\n', 0, token.start() + leaf.start()) + 1, leaf.group(1))
                      for leaf in LEAF.finditer(token.group())}
    return found


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
    for line, leaf in sorted(leaves(text, checked_filters(tomllib.loads(text)))):
        wrong = problem(leaf, names)
        if wrong:
            found.append(f"{config_path}:{line}: filter leaf `test({leaf})` {wrong}")
    return found


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
