# /// script
# requires-python = ">=3.12"
# dependencies = ["markdown-it-py==4.2.0"]
# ///
"""Builds the site into site/_site and checks it.

Usage: uv run --locked --script site/build.py [--out DIR] [--forbid-tk] [--test]

Each site/src/<page>.html gives <page>.html: line 1 is its title, line 2 its meta description,
the rest its body, filled in between _head.html and _foot.html. Each docs/diagnostics/<code>.md
gives diagnostics/<code>.html, filled into _code.html. Whole-line `<!-- -->` comments
are notes for editors and are left out. `{{name arg…}}` is replaced once, from the page's
context or from DIRECTIVES. The build then checks the output and prints each problem.
"""
import argparse
import html
import importlib.util
import inspect
import json
import posixpath
import re
import shutil
import subprocess
import sys
import tomllib
import unicodedata
import unittest
from bisect import bisect_left
from collections import Counter
from collections.abc import Callable
from decimal import ROUND_HALF_UP, Decimal
from functools import cache
from html.parser import HTMLParser
from itertools import accumulate, groupby
from pathlib import Path

from markdown_it import MarkdownIt

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


@cache
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


BLOB = 'https://github.com/sisu-lang/sisu/blob/main/'
STATES = {'works': ('ok', '✓ Works today'), 'planned': ('plan', '○ Planned')}


def adr_files() -> list[Path]:
    return sorted((ROOT / 'docs/adr').glob('[0-9][0-9][0-9][0-9]-*.md'))


def explanation_codes() -> list[str]:
    """The codes that docs/diagnostics/ explains."""
    return sorted(path.stem for path in (ROOT / 'docs/diagnostics').glob('[EW][0-9][0-9][0-9][0-9].md'))


def diagnostic_count() -> str:
    return str(len(explanation_codes()))


def status_areas() -> list[dict]:
    """The areas of data/status.toml, each with its features."""
    try:
        areas = tomllib.loads(read('data/status.toml'))['area']
    except (tomllib.TOMLDecodeError, KeyError) as e:
        raise BuildError(f'data/status.toml: {e!r}') from e
    for i, area in enumerate(areas, 1):
        where = f'data/status.toml: area {area.get("id", i)!r}'
        if missing := {'id', 'name', 'intro', 'note', 'feature'} - area.keys():
            raise BuildError(f'{where} lacks {sorted(missing)}')
        for feature in area['feature']:
            where_feature = f'{where}, feature {feature.get("text", "without text")!r}'
            if missing := {'text', 'state', 'notes'} - feature.keys():
                raise BuildError(f'{where_feature} lacks {sorted(missing)}')
            if feature['state'] not in STATES:
                raise BuildError(f'{where_feature} has state {feature["state"]!r}, not works or planned')
    return areas


def status_board() -> str:
    """The Home page's board: a bar, counts and the two lists for each area, the first open."""
    def items(area, state):
        return [expand(f['text'], {}) for f in area['feature'] if f['state'] == state]

    def li(texts):
        return ''.join(f'<li>{t}</li>' for t in texts)

    out = []
    for i, area in enumerate(status_areas()):
        works, planned = items(area, 'works'), items(area, 'planned')
        note = f'\n    <p class="note">{expand(area["note"], {})}</p>' if area['note'] else ''
        out.append(f'''<details class="area"{' open' if i == 0 else ''}><summary>
  <span class="area-name">{escape(area['name'])}</span>
  <div class="bar" aria-hidden="true"><i class="ok" style="flex:{len(works)}"></i><i class="plan" style="flex:{len(planned)}"></i></div>
  <span class="counts"><b class="ok">{len(works)}</b> work today · <b class="plan">{len(planned)}</b> planned</span>
  <svg class="chev" aria-hidden="true"><use href="#chev"/></svg></summary>
  <div class="detail">
    <div class="wl"><h3>Works today</h3><ul>{li(works)}</ul></div>
    <div><h3>Planned</h3><ul class="pl">{li(planned)}</ul></div>{note}
  </div></details>''')
    return '\n'.join(out)


def status_tables() -> str:
    """The Status page's sections: a heading, an introduction and a table for each area."""
    out = []
    for area in status_areas():
        rows = []
        for f in area['feature']:
            cls, label = STATES[f['state']]
            links = ', '.join(f'<a href="{BLOB}{html.escape(path)}">{escape(text)}</a>' for text, path in f['notes'])
            rows.append(f'<tr><td>{expand(f["text"], {})}</td><td><span class="st {cls}">{label}</span></td><td>{links}</td></tr>')
        body = '\n          '.join(rows)
        out.append(f'''<h2 id="{area['id']}">{escape(area['name'])}</h2>
<p>{expand(area['intro'], {})}</p>
<div class="tbl-wrap"><table class="tbl">
  <thead><tr><th>Feature</th><th>State</th><th>Design notes</th></tr></thead>
  <tbody>
    {body}
  </tbody>
</table></div>''')
    return '\n\n'.join(out)


def code_fence(renderer, tokens, idx, options, env) -> str:
    token = tokens[idx]
    return f'<figure class="code"><pre data-lang="{html.escape(token.info.strip())}">{escape(token.content)}</pre></figure>\n'


# No raw HTML: `sisuc --explain` prints `Option<T>` as written, and the page must show the same.
MD = MarkdownIt('commonmark', {'html': False}).enable('table')
MD.add_render_rule('fence', code_fence)
# An indented block gets data-lang too, so the page checks see it as shown code.
MD.add_render_rule('code_block', code_fence)
CODE_LINK = re.compile(r'([EW]\d{4})\.md(#.*)?')


def plain(inline) -> str:
    """The text of an `inline` token, without its markup."""
    return ''.join(' ' if child.type in ('softbreak', 'hardbreak') else child.content for child in inline.children)


def explanation_link(href: str) -> str:
    """Where a link in docs/diagnostics/ goes on the site: another code's page, or the file on GitHub."""
    if m := CODE_LINK.fullmatch(href):
        return f'{m[1]}.html{m[2] or ""}'
    if SCHEME.match(href) or href.startswith(('#', '/')):
        return href
    path, hash_, anchor = href.partition('#')
    return BLOB + posixpath.normpath(f'docs/diagnostics/{path}') + hash_ + anchor


def explanation_html(code: str, markdown: str, err: str) -> str:
    """The explanation without its heading, with what sisuc says about Broken placed before Fixed."""
    tokens = MD.parse(markdown)
    if tokens and tokens[0].tag == 'h1':
        tokens = tokens[3:]
    for token in tokens:
        for child in token.children or []:
            if child.type == 'link_open':
                child.attrSet('href', explanation_link(str(child.attrGet('href'))))
    before, fixed, after = MD.renderer.render(tokens, MD.options, {}).partition('<h2>Fixed</h2>')
    if not fixed:
        raise BuildError(f'{code} has no "## Fixed"')
    return f'{before}<h2>What sisuc says</h2><figure class="code"><pre>{diag_html(err)}</pre></figure>\n{fixed}{after}'


def diagnostics_of(json_text: str, code: str) -> list[dict]:
    """The diagnostics with code `code` in sisuc's JSON output, one object per line."""
    records = [json.loads(line) for line in json_text.splitlines() if line.strip()]
    return [r for r in records if r.get('kind') == 'diagnostic' and r.get('code') == code]


def fixes(json_text: str, code: str) -> list[bool]:
    """Whether each fix of each `code` diagnostic is safe, in order."""
    return [fix['safe'] for d in diagnostics_of(json_text, code) for fix in d['fixes']]


def explanation(code: str) -> str:
    return (ROOT / f'docs/diagnostics/{code}.md').read_text(encoding='utf-8')


def title(code: str, markdown: str) -> str:
    """The Markdown of the explanation's title, the heading after `CODE: `, its first letter upper-cased."""
    heading = markdown.partition('\n')[0]
    if not heading.startswith(f'# {code}: '):
        raise BuildError(f'{code} does not start with "# {code}: "')
    text = heading.removeprefix(f'# {code}: ')
    return text[0].upper() + text[1:] if text[:1].isalpha() else text


def categories() -> list[dict]:
    """The categories of data/categories.toml, in order."""
    try:
        found = tomllib.loads(read('data/categories.toml'))['category']
    except (tomllib.TOMLDecodeError, KeyError) as e:
        raise BuildError(f'data/categories.toml: {e!r}') from e
    for i, category in enumerate(found, 1):
        if missing := {'id', 'name', 'codes'} - category.keys():
            raise BuildError(f'data/categories.toml: category {category.get("id", i)!r} lacks {sorted(missing)}')
    return found


def category_problems():
    """Each explained code is in exactly one category, and each listed code is explained."""
    explained = set(explanation_codes())
    listed = Counter(code for category in categories() for code in category['codes'])
    for code in sorted(explained | listed.keys()):
        if code not in explained:
            yield f'{code} has no explanation in docs/diagnostics/'
        elif listed[code] != 1:
            yield f'{code} is in {listed[code]} categories, not one'


def category_links(page: str, code: str | None = None) -> str:
    """The aside's category list, linking `page#<id>`, with `code`'s category marked."""
    out = []
    for category in categories():
        current = ' aria-current="page"' if code in category['codes'] else ''
        out.append(f'<li><a href="{page}#{category["id"]}"{current}>{escape(category["name"])}</a></li>')
    return ''.join(out)


def diagnostics_index() -> str:
    """For each category, its heading and a row per explained code it lists."""
    explained = set(explanation_codes())
    out = []
    for category in categories():
        rows = []
        for code in category['codes']:
            if code in explained:  # category_problems reports the others
                name = title(code, explanation(code))
                words = html.escape(plain(MD.parseInline(name)[0]).lower())
                rows.append(f'<li data-code="{code}" data-words="{words}"><a href="diagnostics/{code}.html">'
                            f'<span class="num">{code}</span><b>{MD.renderInline(name)}</b></a></li>')
        out.append(f'<h2 id="{category["id"]}">{escape(category["name"])}</h2>\n<ul class="rows">{"".join(rows)}</ul>')
    return '\n'.join(out)


def code_page(code: str) -> str:
    """The page diagnostics/<code>.html."""
    markdown = explanation(code)
    name = title(code, markdown)
    json_text = read(f'programs/explanations/{code}.json')
    found = diagnostics_of(json_text, code)
    if not found:
        raise BuildError(f'programs/explanations/{code}.json reports no {code}')
    tokens = MD.parse(markdown)
    rule = next((tokens[i + 1] for i, t in enumerate(tokens) if t.type == 'paragraph_open'), None)
    if rule is None:
        raise BuildError(f'{code} has no paragraph')
    context = {
        'code': code,
        'heading': MD.renderInline(name),
        'severity': escape(found[0]['severity'].capitalize()),
        'badges': ''.join(f'<span class="badge">{"Safe" if safe else "Unsafe"} fix</span>' for safe in fixes(json_text, code)),
        'categories': category_links('../diagnostics.html', code),
        'explanation': explanation_html(code, markdown, read(f'programs/explanations/{code}.err')),
    }
    return assemble(read('_code.html'), title=f'{code}: {plain(MD.parseInline(name)[0])}', description=plain(rule),
                    page='diagnostics.html', depth=1, context=context)


BENCHMARKS = 'docs/benchmarks.md'
BENCH_HEADER = ['Language', 'Wall (s)', 'CPU (s)', 'Peak RSS (MiB)', 'Binary (KiB)', 'Build (s)']
# The table's columns, which is not the order of the Markdown's rows: that runs from fastest to slowest.
BENCH_LANGUAGES = ['Sisu', 'C', 'Rust', 'Go', 'Java', 'Python']


def benchmarks_md() -> str:
    return read(f'../{BENCHMARKS}')


def bench_programs() -> list[str]:
    """The programs scripts/bench.sh runs: a directory under bench/ each."""
    return sorted(p.name for p in (ROOT / 'bench').iterdir() if p.is_dir())


def benchmark_tables(markdown: str, expected: list[str] | None = None) -> dict[str, dict[str, dict[str, str]]]:
    """Program -> language -> column -> cell text, from each `### <program>` table under `## Results`.
    With `expected`, the programs must be exactly those."""
    tables, programs = {}, []
    section = tag = ''
    rows = None
    for token in MD.parse(markdown):
        if token.type == 'heading_open':
            tag = token.tag
        elif token.type == 'inline' and tag:
            if tag == 'h2':
                section = token.content
            elif tag == 'h3' and section == 'Results':
                programs.append(token.content)
            tag = ''
        elif token.type == 'table_open':
            rows = []
        elif token.type == 'tr_open' and rows is not None:
            rows.append([])
        elif token.type == 'inline' and rows is not None:
            rows[-1].append(plain(token))
        elif token.type == 'table_close':
            if section == 'Results' and programs:
                tables[programs[-1]] = benchmark_rows(programs[-1], rows)
            rows = None
    for program in programs:
        if program not in tables:
            raise BuildError(f'{BENCHMARKS}: table {program!r} is missing')
    if expected is not None and sorted(tables) != sorted(expected):
        missing, extra = sorted(set(expected) - set(tables)), sorted(set(tables) - set(expected))
        raise BuildError(f'{BENCHMARKS}: the programs differ from the directories under bench/: missing {missing}, extra {extra}')
    return tables


def benchmark_rows(program: str, rows: list[list[str]]) -> dict[str, dict[str, str]]:
    where = f'{BENCHMARKS}: table {program!r}'
    if rows[0] != BENCH_HEADER:
        raise BuildError(f'{where} has the header {rows[0]}, not {BENCH_HEADER}')
    languages = [row[0] for row in rows[1:]]
    if sorted(languages) != sorted(BENCH_LANGUAGES):
        raise BuildError(f'{where} has the languages {languages}, not exactly {sorted(BENCH_LANGUAGES)}')
    return {row[0]: dict(zip(BENCH_HEADER[1:], row[1:])) for row in rows[1:]}


def seconds(program: str, language: str, column: str, cell: str) -> Decimal:
    """A cell that a chart draws, which must be a number."""
    if not re.fullmatch(r'\d+(\.\d+)?', cell):
        raise BuildError(f"{BENCHMARKS}: table {program!r}: {language}'s {column} is {cell!r}, not a number")
    return Decimal(cell)


def hello_note(table: dict[str, dict[str, str]]) -> str:
    """The foot of the hello chart: the one Wall time all its programs share, or all but one."""
    cells = {language: row['Wall (s)'] for language, row in table.items()}
    # Decimals, so that 0.0 and 0.00 are one time; the note shows the first spelling.
    times = {language: seconds('hello', language, 'Wall (s)', cell) for language, cell in cells.items()}
    (common, _), *rest = Counter(times.values()).most_common()
    common = next(cells[l] for l, t in times.items() if t == common)
    if not rest:
        odd = ''
    elif len(rest) == 1 and rest[0][1] == 1:
        language = next(l for l, t in times.items() if t == rest[0][0])
        odd = f" except {language}'s, {cells[language]} s"
    else:
        raise BuildError(f"{BENCHMARKS}: table 'hello': the Wall cells {cells} do not agree but for one language")
    return f'Every hello program runs in {common} s of wall time{odd}. The build is what differs.'


def chart(program: str, table: dict[str, dict[str, str]]) -> str:
    """One bar chart: the program's Wall time, or its Build time for hello. Python is text, off the scale."""
    column = 'Build (s)' if program == 'hello' else 'Wall (s)'
    values = {l: seconds(program, l, column, row[column]) for l, row in table.items() if l != 'Python'}
    largest = max(values.values())
    if not largest:
        raise BuildError(f'{BENCHMARKS}: table {program!r}: every {column} is 0')
    if program != 'hello' and seconds(program, 'Python', column, table['Python'][column]) <= largest:
        raise BuildError(f"{BENCHMARKS}: table {program!r}: Python's {column} is not above every other, so it is not off the scale")

    def li(language):
        width = (values[language] / largest * 100).quantize(Decimal('0.1'), ROUND_HALF_UP)
        cls = ' class="sisu"' if language == 'Sisu' else ''
        return (f'<li{cls}><span>{language}</span><span class="track"><i style="width:{str(width).removesuffix(".0")}%"></i></span>'
                f'<span class="val">{table[language][column]} s</span></li>')

    python = table['Python'][column]
    if program == 'hello' and python != '-':
        raise BuildError(f"{BENCHMARKS}: table 'hello': Python's {column} is {python!r}, not -")
    track, val = ('no build step', '-') if python == '-' and program == 'hello' else ('off the scale', f'{seconds(program, "Python", column, python)} s')
    items = [li(language) for language in sorted(values, key=values.__getitem__)]
    items.append(f'<li class="off"><span>Python</span><span class="track">{track}</span><span class="val">{val}</span></li>')
    unit = 'build time, seconds: compile one file and link it' if program == 'hello' else 'wall time, seconds'
    foot = f'<p class="foot">{hello_note(table)}</p>' if program == 'hello' else ''
    body = '\n        '.join(items)
    return f'<figure class="chart"><h3>{escape(program)}</h3><p class="unit">{unit}</p><ol>\n        {body}\n      </ol>{foot}</figure>'


def bench_charts(markdown: str, expected: list[str] | None = None) -> str:
    """A chart for each program, in the document's order with hello last."""
    tables = benchmark_tables(markdown, expected)
    if 'hello' not in tables:
        raise BuildError(f"{BENCHMARKS}: no table for 'hello'")
    return '\n      '.join(chart(p, tables[p]) for p in [*(p for p in tables if p != 'hello'), 'hello'])


def bench_table(markdown: str, expected: list[str] | None = None) -> str:
    """The results table: a row for each program's Wall time, and for hello's Build time after hello's."""
    def row(label, table, column):
        cells = [f'<b>{escape(table[l][column])}</b>' if l == 'Sisu' else escape(table[l][column]) for l in BENCH_LANGUAGES]
        return f'<tr><td>{label}</td>' + ''.join(f'<td class="n">{c}</td>' for c in cells) + '</tr>'

    rows = []
    for program, table in benchmark_tables(markdown, expected).items():
        rows.append(row(f'<code>{escape(program)}</code>', table, 'Wall (s)'))
        if program == 'hello':
            rows.append(row('<code>hello</code>, build', table, 'Build (s)'))
    head = ''.join(f'<th class="n">{l}</th>' for l in BENCH_LANGUAGES)
    body = '\n        '.join(rows)
    return f'''<div class="tbl-wrap"><table class="tbl">
      <thead><tr><th>Benchmark</th>{head}</tr></thead>
      <tbody>
        {body}
      </tbody>
    </table></div>'''


def results_note(markdown: str) -> str:
    """The first paragraph between `## Results` and the next heading, rendered."""
    tokens = MD.parse(markdown)
    in_results = False
    for i, token in enumerate(tokens):
        if token.type == 'heading_open':
            in_results = token.tag == 'h2' and tokens[i + 1].content == 'Results'
        elif token.type == 'paragraph_open' and in_results:
            token.attrSet('class', 'after')
            return MD.renderer.render(tokens[i:i + 3], MD.options, {})
    raise BuildError(f'{BENCHMARKS} has no paragraph between "## Results" and the first "###"')


DIRECTIVES: dict[str, Callable[..., str]] = {
    'commit': commit,
    'text': lambda path: escape(read(path)),
    'diag': lambda path: diag_html(read(path)),
    'help': help_line,
    'status-board': status_board,
    'status-tables': status_tables,
    'adr-count': lambda: str(len(adr_files())),
    'diagnostic-count': diagnostic_count,
    'diagnostics-index': diagnostics_index,
    'diagnostic-categories': lambda: category_links(''),
    'bench-charts': lambda: bench_charts(benchmarks_md(), bench_programs()),
    'bench-table': lambda: bench_table(benchmarks_md(), bench_programs()),
    'bench-results-note': lambda: results_note(benchmarks_md()),
}
DIRECTIVE = re.compile(r'\{\{([\w-]+)((?: [^\s{}]+)*)\}\}')


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
            name, description, rest = text.split('\n', 2)
            body = '\n'.join(line for line in rest.split('\n') if not COMMENT_LINE.fullmatch(line))
            page = assemble(body, title=name, description=description, page=src.name, depth=0, context={})
        except BuildError as e:
            raise BuildError(f'{src.relative_to(ROOT)}: {e}') from e
        (out / src.name).write_text(page, encoding='utf-8')
    (out / 'diagnostics').mkdir()
    for code in explanation_codes():
        try:
            page = code_page(code)
        except BuildError as e:
            raise BuildError(f'docs/diagnostics/{code}.md: {e}') from e
        (out / 'diagnostics' / f'{code}.html').write_text(page, encoding='utf-8')


SCHEME = re.compile(r'[a-z][a-z0-9+.-]*:|//')
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


# A row of the Decisions page: the ADR's GitHub link around its number and title.
ADR_ROW = re.compile(re.escape(BLOB) + r'(docs/adr/[^"#]+\.md)"[^>]*>\s*<span class="num">(\d+)</span>\s*<b>(.*?)</b>', re.S)
# What follows a row's title in its <b>: a status note the ADR's heading does not hold.
ADR_NOTE = re.compile(r'<(?:small>|span class="soon">)')


def adr_problems(text: str):
    """Each ADR is listed once, by its GitHub link, under its heading rendered inline."""
    rows: dict[str, list[tuple[str, str]]] = {}
    for m in ADR_ROW.finditer(text):
        rows.setdefault(m[1], []).append((m[2], ADR_NOTE.split(m[3])[0].strip()))
    for file in adr_files():
        path = file.relative_to(ROOT).as_posix()
        if len(rows.get(path, [])) != 1:
            yield f'lists {path} {len(rows.get(path, []))} times, not once'
            continue
        heading = next((line for line in file.read_text(encoding='utf-8').splitlines() if line.startswith('# ')), None)
        if heading is None:
            yield f'{path} has no "# " heading'
            continue
        num, title = rows[path][0]
        if num != file.name[:4]:
            yield f'numbers {path} {num!r}, not {file.name[:4]!r}'
        want = MarkdownIt().renderInline(heading.removeprefix('# '))
        if title != want:
            yield f'titles {path} {title!r}, not its heading {want!r}'


def check_site(out: Path, *, forbid_tk: bool = False) -> list[str]:
    """Every problem in the built site, one string each naming the file."""
    texts = {page: page.read_text(encoding='utf-8') for page in sorted(out.rglob('*.html'))}
    tags = {page: Tags(text) for page, text in texts.items()}
    ids = {page.resolve(): tags[page].ids for page in texts}
    found = []
    for page, text in texts.items():
        found += [f'{page.relative_to(out)}: {p}' for p in page_problems(out, page, text, tags[page], ids, forbid_tk)]
        if page == out / 'decisions.html':
            found += [f'decisions.html: {p}' for p in adr_problems(text)]
    found += [f'data/categories.toml: {p}' for p in category_problems()]
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
