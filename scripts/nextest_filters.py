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


def leaves(config: dict) -> list[str]:
    """The argument of each `test(...)` leaf of each override and script filter, once each, as the
    parsed TOML holds it. nextest trims the space before an argument, not the space after it.

    shortcut: the check runs on the Linux list only, so an override with a `platform` key is
    skipped. Upgrade when a platform override is added, to one list for each platform."""
    found = []
    for profile in config.get('profile', {}).values():
        for table in ('overrides', 'scripts'):
            for entry in profile.get(table, []):
                if 'platform' not in entry:
                    found += LEAF.findall(entry.get('filter', ''))
    return list(dict.fromkeys(found))


def lines_of(text: str, wanted: set[str]) -> dict[str, set[int]]:
    """The lines of the config text, outside its comments, where each argument of `wanted` stands
    in a `test(...)` leaf. A leaf is on the line of its `test`. One pass: the lines are counted
    from one leaf to the next.

    shortcut: the text is not the parsed value, so a leaf that an escape or a line continuation
    spells is not found, and the caller reports line 1. A leaf of a platform override, or in a key
    other than `filter`, is reported on its line too."""
    code = TOKEN.sub(lambda token: '' if token.group().startswith('#') else token.group(), text)
    found: dict[str, set[int]] = {}
    line = start = 0
    for leaf in LEAF.finditer(code):
        line += code.count('\n', start, leaf.start())
        start = leaf.start()
        if leaf.group(1) in wanted:
            found.setdefault(leaf.group(1), set()).add(line + 1)
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
    wrong = {leaf: why for leaf in leaves(tomllib.loads(text)) if (why := problem(leaf, names))}
    where = lines_of(text, set(wrong))
    return [f"{config_path}:{line}: filter leaf `test({leaf})` {wrong[leaf]}"
            for line, leaf in sorted((line, leaf) for leaf in wrong for line in where.get(leaf, {1}))]


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
