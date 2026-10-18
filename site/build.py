# /// script
# requires-python = ">=3.12"
# dependencies = ["markdown-it-py==4.2.0"]
# ///
"""Builds the site into site/_site and checks it.

Usage: uv run --locked --script site/build.py [--out DIR] [--forbid-tk] [--test]

Each site/src/<page>.html gives <page>.html: line 1 is its title, line 2 its meta description,
the rest its body, filled in between _head.html and _foot.html. Whole-line `<!-- -->` comments
are notes for editors and are left out. `{{name arg…}}` is replaced once, from the page's
context or from DIRECTIVES. The build then checks the output and prints each problem.
"""
import argparse
import html
import importlib.util
import inspect
import re
import shutil
import subprocess
import sys
import unicodedata
import unittest
from bisect import bisect_left
from collections.abc import Callable
from html.parser import HTMLParser
from itertools import accumulate, groupby
from pathlib import Path

SITE = Path(__file__).resolve().parent
ROOT = SITE.parent

_spec = importlib.util.spec_from_file_location('check_links', ROOT / '.agents/skills/update-docs/scripts/check_links.py')
check_links = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(check_links)


class BuildError(Exception):
    """A page that cannot be built."""


def escape(text: str) -> str:
    return html.escape(text, quote=False)


def read(path: str) -> str:
    """The file at SITE/<path>."""
    try:
        return (SITE / path).read_text(encoding='utf-8')
    except OSError as e:
        raise BuildError(f'cannot read {path}: {e.strerror}') from e


def commit() -> str:
    return subprocess.run(['git', '-C', str(ROOT), 'rev-parse', '--short=7', 'HEAD'],
                          capture_output=True, text=True, check=True).stdout.strip()


def help_line(path: str) -> str:
    """The text after `help: ` on the file's first line that starts with it."""
    for line in read(path).splitlines():
        if line.startswith('help: '):
            return escape(line.removeprefix('help: '))
    raise BuildError(f'{path} has no line that starts with "help: "')


# The gutter of a diagnostic line: `N |`, `|`, `=`, `-->`, `...`, and the `N +`, `N -`, `N ~` of
# suggestion lines.
GUTTER = re.compile(r' *(\d+ [|+~-]|\||=|-->|\.\.\.)(?= |$)')
# A label run under a source line: carets for the primary label, dashes for a secondary one,
# pluses for an addition.
MARK = re.compile(r' +(\^+|-+|\++)(?= |$)')
PLUS_LINE = re.compile(r' *\|( +\++)+')


def colours(line: str, below: str) -> list[str]:
    """The class of each character of `line`; `below` is the line after it."""
    cls = [''] * len(line)

    def paint(start, end, c):
        end = min(end, len(line))
        cls[start:end] = [c] * max(end - start, 0)

    if m := re.match(r'error(\[\w+\])?(?=:)', line):
        paint(0, m.end(), 'e')
    elif m := re.match(r'(warning\[\w+\]|help)(?=:)', line):
        paint(0, m.end(), 'h')
    elif m := GUTTER.match(line):
        paint(m.start(1), m.end(1), 'g')
        if m[1] == '=' and (h := re.compile(r' (help)(?=:)').match(line, m.end())):
            paint(h.start(1), h.end(1), 'h')
        elif m[1] == '|':
            pos = m.end()
            while mark := MARK.match(line, pos):
                paint(mark.start(1), mark.end(1), {'^': 'e', '-': 'g', '+': 'add'}[mark[1][0]])
                pos = mark.end()
        elif m[1].endswith('|') and PLUS_LINE.fullmatch(below):
            # The markers count display columns, as annotate-snippets does: a wide character takes two.
            # shortcut: zero-width characters count as one column; upgrade when an output holds one.
            starts = list(accumulate((2 if unicodedata.east_asian_width(ch) in ('W', 'F') else 1 for ch in line), initial=0))
            for run in re.finditer(r'\++', below):
                paint(bisect_left(starts, run.start()), bisect_left(starts, run.end()), 'add')
    return cls


def diag_html(text: str) -> str:
    """sisuc's diagnostic output as coloured lines, one `<span class="ln">` each."""
    rows = re.findall(r'[^\n]*\n|[^\n]+\Z', text)
    lines = [row.removesuffix('\n') for row in rows]
    out = []
    for i, (row, line) in enumerate(zip(rows, lines)):
        below = lines[i + 1] if i + 1 < len(lines) else ''
        parts = []
        for c, chars in groupby(zip(line, colours(line, below)), key=lambda pair: pair[1]):
            run = escape(''.join(ch for ch, _ in chars))
            parts.append(f'<span class="{c}">{run}</span>' if c else run)
        out.append(f'<span class="ln">{"".join(parts)}{row[len(line):]}</span>')
    return ''.join(out)


DIRECTIVES: dict[str, Callable[..., str]] = {
    'commit': commit,
    'text': lambda path: escape(read(path)),
    'diag': lambda path: diag_html(read(path)),
    'help': help_line,
}
DIRECTIVE = re.compile(r'\{\{(\w+)((?: [^\s{}]+)*)\}\}')


def expand(text: str, context: dict[str, str]) -> str:
    """Replaces each `{{name arg…}}` once; what it substitutes is not expanded again."""
    def one(m):
        name = m[1]
        if name in context:
            return context[name]
        if name in DIRECTIVES:
            directive, args = DIRECTIVES[name], m[2].split()
            params = inspect.signature(directive).parameters
            if len(args) != len(params):
                raise BuildError(f'expected {{{{{" ".join([name, *params])}}}}}, found {m[0]}')
            return directive(*args)
        raise BuildError(f'unknown {{{{{name}}}}}')
    return DIRECTIVE.sub(one, text)


def assemble(body: str, *, title: str, description: str, page: str, depth: int, context: dict[str, str]) -> str:
    """Head, body and foot, with `page`'s nav link marked current."""
    # 404.html is served at any depth, so its links start at the site root.
    root = '/sisu/' if page == '404.html' else '../' * depth
    link = f'<a href="{{{{root}}}}{page}">'
    head = read('_head.html').replace(link, link[:-1] + ' aria-current="page">')
    names = context | {'root': root, 'title': html.escape(title), 'description': html.escape(description)}
    return expand(head + body + read('_foot.html'), names)


COMMENT_LINE = re.compile(r' *<!--.*-->')
# The file each build writes into its output, so that the next one knows the directory is its own.
MARKER = '.built-by-build-py'


def build(out: Path) -> None:
    dest = out.resolve()
    if SITE.is_relative_to(dest) or (dest.is_relative_to(SITE) and not dest.is_relative_to(SITE / '_site')):
        raise BuildError(f'{out} is a build input; choose another --out')
    if out.exists():
        if any(out.iterdir()) and not (out / MARKER).is_file():
            raise BuildError(f'{out} holds files not built by site/build.py; remove it or choose another --out')
        shutil.rmtree(out)
    shutil.copytree(SITE / 'assets', out / 'assets')
    (out / MARKER).write_text('site/build.py built this directory and replaces it on the next build.\n', encoding='utf-8')
    for src in sorted((SITE / 'src').glob('*.html')):
        text = src.read_text(encoding='utf-8')
        try:
            if text.count('\n') < 2:
                raise BuildError('needs a title line, a description line and the body')
            title, description, body = text.split('\n', 2)
            body = '\n'.join(line for line in body.split('\n') if not COMMENT_LINE.fullmatch(line))
            page = assemble(body, title=title, description=description, page=src.name, depth=0, context={})
        except BuildError as e:
            raise BuildError(f'{src.relative_to(ROOT)}: {e}') from e
        (out / src.name).write_text(page, encoding='utf-8')


SCHEME = re.compile(r'[a-z][a-z0-9+.-]*:|//')
BLOB = 'https://github.com/sisu-lang/sisu/blob/main/'
FOREIGN = re.compile(r'(?:https?:)?//', re.I)
CSS_URL = re.compile(r'url\(\s*([\'"]?)(.*?)\1\s*\)|@import\s+([\'"])(.*?)\3')
HEADING = re.compile(r'<h([1-6])\b')
# A <pre> with no data-lang holds diag_html output, which shows what sisuc prints.
UNTAGGED_PRE = re.compile(r'<pre\b(?![^>]*\bdata-lang=)[^>]*>.*?</pre>', re.S)


class Tags(HTMLParser):
    """A page's `href` and `src` values with their tags, its ids, and its inline CSS."""

    def __init__(self, text: str):
        super().__init__()
        self.links: list[tuple[str, str]] = []
        self.ids: set[str] = set()
        self.styles: list[str] = []
        self.in_style = False
        self.feed(text)
        self.close()

    def handle_starttag(self, tag, attrs):
        for name, value in attrs:
            if name in ('href', 'src') and value is not None:
                self.links.append((tag, value))
            elif name == 'id' and value is not None:
                self.ids.add(value)
            elif name == 'style' and value:
                self.styles.append(value)
        self.in_style = tag == 'style'

    def handle_endtag(self, tag):
        self.in_style = False

    def handle_data(self, data):
        if self.in_style:
            self.styles.append(data)


def link_problems(out: Path, page: Path, links: list[tuple[str, str]], ids: dict[Path, set[str]]):
    for _, target in links:
        if target.startswith(BLOB):
            path, _, anchor = target.removeprefix(BLOB).partition('#')
            if path not in check_links.TRACKED:
                yield f'{target} names no tracked file'
            elif anchor and path.endswith('.md') and anchor.lower() not in set(check_links.anchors(ROOT / path)):
                yield f'missing anchor {target}'
            continue
        if SCHEME.match(target):
            continue
        file, hash_, anchor = target.partition('#')
        file = file.partition('?')[0]
        if file.startswith('/'):
            if not file.startswith('/sisu/'):
                yield f'{target} is outside /sisu/'
                continue
            dest = out / file.removeprefix('/sisu/')
        else:
            dest = page.parent / file if file else page
        dest = dest.resolve()
        if not dest.is_file() or not dest.is_relative_to(out.resolve()):
            yield f'missing {target}'
        elif hash_ and dest.suffix == '.html' and anchor not in ids[dest]:
            yield f'missing anchor {target}'


def page_problems(out: Path, page: Path, text: str, tags: Tags, ids: dict[Path, set[str]], forbid_tk: bool):
    yield from link_problems(out, page, tags.links, ids)
    for tag, url in tags.links:
        if tag in ('link', 'script', 'img') and FOREIGN.match(url):
            yield f'loads {url} from another host'
    for style in tags.styles:
        yield from css_problems(out, page.parent, style)
    levels = [int(level) for level in HEADING.findall(text)]
    if levels.count(1) != 1:
        yield f'has {levels.count(1)} <h1>, not one'
    for before, level in zip([0, *levels], levels):
        if level > before + 1:
            yield f'<h{level}> after <h{before}> skips a level' if before else f'starts with <h{level}>, not <h1>'
    if '<!--' in text:
        yield 'holds a <!-- comment'
    if re.search('milestone', UNTAGGED_PRE.sub('', text), re.I):
        yield 'names a milestone'
    if re.search('kotlin', text, re.I):
        yield 'names Kotlin'
    if forbid_tk and '[TK' in text:
        yield 'holds a [TK] marker'


def css_problems(out: Path, base: Path, css: str):
    """The problems of the CSS text `css`, whose relative URLs resolve from the directory `base`."""
    for m in CSS_URL.finditer(css):
        target = m[2] if m[2] is not None else m[4]
        if target.startswith('data:'):
            continue
        if SCHEME.match(target):
            yield f'loads {target} from another host'
            continue
        dest = (base / re.split('[?#]', target)[0]).resolve()
        if not dest.is_file() or not dest.is_relative_to(out.resolve()):
            yield f'missing {target}'


def check_site(out: Path, *, forbid_tk: bool = False) -> list[str]:
    """Every problem in the built site, one string each naming the file."""
    texts = {page: page.read_text(encoding='utf-8') for page in sorted(out.rglob('*.html'))}
    tags = {page: Tags(text) for page, text in texts.items()}
    ids = {page.resolve(): tags[page].ids for page in texts}
    found = []
    for page, text in texts.items():
        found += [f'{page.relative_to(out)}: {p}' for p in page_problems(out, page, text, tags[page], ids, forbid_tk)]
    for css in sorted(out.rglob('*.css')):
        found += [f'{css.relative_to(out)}: {p}' for p in css_problems(out, css.parent, css.read_text(encoding='utf-8'))]
    return found


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description='Build the site and check it.')
    parser.add_argument('--out', type=Path, default=SITE / '_site', help='where to build (default: site/_site)')
    parser.add_argument('--forbid-tk', action='store_true', help='also fail on any [TK marker')
    parser.add_argument('--test', action='store_true', help='run build_test.py instead')
    args = parser.parse_args(argv)
    if args.test:
        sys.path.insert(0, str(SITE))
        sys.modules.setdefault('build', sys.modules[__name__])
        suite = unittest.defaultTestLoader.loadTestsFromName('build_test')
        return 0 if unittest.TextTestRunner().run(suite).wasSuccessful() else 1
    try:
        build(args.out)
    except BuildError as e:
        print(e)
        return 1
    found = check_site(args.out, forbid_tk=args.forbid_tk)
    for problem in found:
        print(problem)
    return 1 if found else 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
