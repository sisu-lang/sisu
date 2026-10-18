"""Tests for build.py. Run them with `uv run --locked --script site/build.py --test`."""
import html
import re
import tempfile
import unittest
from html.parser import HTMLParser
from pathlib import Path
from unittest import mock

import build

# The E0032 output quoted in the mockup's site/src/diagnostics.html, unescaped.
E0032 = """error[E0032]: expected `f64`, found `i64`
 --> main.sisu:4:11
  |
4 |     print(n * x)
  |           ^
  |
help: if the types are right, convert it with `f64(...)`
  |
4 |     print(f64(n) * x)
  |           ++++ +

sisuc: 1 error
sisuc: for more on a code, run `sisuc --explain <code>`: E0032
"""

# The mockup's spans for those lines, each with its newline moved inside, before `</span>`.
E0032_SPANS = [
    '<span class="ln"><span class="e">error[E0032]</span>: expected `f64`, found `i64`\n</span>',
    '<span class="ln"> <span class="g">--&gt;</span> main.sisu:4:11\n</span>',
    '<span class="ln">  <span class="g">|</span>           <span class="e">^</span>\n</span>',
    '<span class="ln"><span class="h">help</span>: if the types are right, convert it with `f64(...)`\n</span>',
    '<span class="ln"><span class="g">4 |</span>     print(<span class="add">f64(</span>n<span class="add">)</span> * x)\n</span>',
]

# Lines that colour other ways: each input line, then the span diag_html must give it.
COLOURED = [
    # The header of site/programs/explanations/W0002.err.
    ('warning[W0002]: `count` is never reassigned\n',
     '<span class="ln"><span class="h">warning[W0002]</span>: `count` is never reassigned\n</span>'),
    # Line 7 of crates/sisuc/tests/diagnostics/W0004-branch.err.
    ('  = help: use it, or write `let _ =` to drop it on purpose\n',
     '<span class="ln">  <span class="g">=</span> <span class="h">help</span>: use it, or write `let _ =` to drop it on purpose\n</span>'),
    # From `sisuc --check` on a program whose `let n` follows two wide characters: the `+` markers
    # count display columns, where 界 takes two.
    ('2 |     if "界" == "界" { let mut n = 1\n  |                           +++\n',
     '<span class="ln"><span class="g">2 |</span>     if "界" == "界" { let <span class="add">mut</span> n = 1\n</span>'),
]

PAGE = '<!doctype html><html><head></head><body><h1>Page</h1>{}</body></html>'

# Two areas: the first has a note, a counted directive, and two links, one with an anchor.
STATUS = """
[[area]]
id = "one"
name = "One"
intro = "Intro <code>x</code>"
note = "Careful."
[[area.feature]]
text = "Alpha for {{diagnostic-count}} codes"
state = "works"
notes = [["ADR 1", "docs/adr/0001-first.md"], ["Roadmap: here", "docs/roadmap.md#what-works-today"]]
[[area.feature]]
text = "Beta"
state = "works"
notes = []
[[area.feature]]
text = "Gamma"
state = "planned"
notes = [["Roadmap", "docs/roadmap.md"]]

[[area]]
id = "two"
name = "Two"
intro = "Second."
note = ""
[[area.feature]]
text = "Delta"
state = "works"
notes = [["Roadmap", "docs/roadmap.md"]]
[[area.feature]]
text = "Epsilon"
state = "planned"
notes = [["Roadmap", "docs/roadmap.md"]]
"""


BENCH_HEADER = '| Language | Wall (s) | CPU (s) | Peak RSS (MiB) | Binary (KiB) | Build (s) |\n| --- | ---: | ---: | ---: | ---: | ---: |\n'


def md_table(program, walls, builds=None):
    """A `### <program>` table: `walls` maps each language to its Wall cell, `builds` to its Build cell.
    By default every build takes 0.05 and Python has none."""
    builds = builds or {lang: '-' if lang == 'Python' else '0.05' for lang in walls}
    rows = []
    for lang, wall in walls.items():
        cells = [lang, wall, '0.1', '1.0', '9', builds[lang]]
        rows.append('| ' + ' | '.join(f'**{c}**' for c in cells) + ' |' if lang == 'Sisu' else '| ' + ' | '.join(cells) + ' |')
    return f'### {program}\n\n{BENCH_HEADER}' + '\n'.join(rows) + '\n\n'


# The mockup's fib values, then a hello whose Wall cells agree, in the order the Markdown has them.
FIB = {'Rust': '0.17', 'C': '0.18', 'Sisu': '0.26', 'Java': '0.27', 'Go': '0.28', 'Python': '5.18'}
HELLO = {lang: '0.00' for lang in FIB}
HELLO_BUILDS = {'Go': '0.037', 'C': '0.053', 'Sisu': '0.058', 'Rust': '0.073', 'Java': '0.196', 'Python': '-'}
BENCH_LI = r'<li( class="\w+")?><span>(\w+)</span><span class="track">(.*?)</span><span class="val">(.*?)</span></li>'


def bench_md(*tables, intro='Commit abc1234, on 2026-10-12.\n\nOn a machine.\n\n'):
    return f'# Benchmarks\n\nAbout.\n\n## Results\n\n{intro}' + ''.join(tables) + '## The programs\n\nText.\n'


def temp_file(test, text):
    """A file holding `text`, removed when the test ends."""
    tmp = tempfile.TemporaryDirectory()
    test.addCleanup(tmp.cleanup)
    path = Path(tmp.name) / 'f'
    path.write_text(text, encoding='utf-8')
    return path


def fake_site(test, pages, repo=None):
    """A repository whose site/src holds `pages`, standing in for build.ROOT and build.SITE.
    `repo` maps other repository paths to their text."""
    tmp = tempfile.TemporaryDirectory()
    test.addCleanup(tmp.cleanup)
    site = Path(tmp.name).resolve() / 'site'
    (site / 'assets').mkdir(parents=True)
    (site / 'src').mkdir()
    for name in ('_head.html', '_foot.html'):
        (site / name).write_text('', encoding='utf-8')
    (site / 'data').mkdir()
    (site / 'data/categories.toml').write_text('category = []\n', encoding='utf-8')
    for name, text in pages.items():
        (site / 'src' / name).write_text(text, encoding='utf-8')
    for name, text in (repo or {}).items():
        (site.parent / name).parent.mkdir(parents=True, exist_ok=True)
        (site.parent / name).write_text(text, encoding='utf-8')
    test.enterContext(mock.patch.object(build, 'ROOT', site.parent))
    test.enterContext(mock.patch.object(build, 'SITE', site))
    return site


def adr_row(num, slug, title, shown=None):
    """A Decisions row linking docs/adr/{num}-{slug}.md and showing the number `shown` (default `num`)."""
    return f'<li><a href="{build.BLOB}docs/adr/{num}-{slug}.md"><span class="num">{shown or num}</span><b>{title}</b><span>x</span></a></li>'


def problems(test, files, **kwargs):
    """check_site over a site holding `files`: {relative path: text}."""
    tmp = tempfile.TemporaryDirectory()
    test.addCleanup(tmp.cleanup)
    out = Path(tmp.name)
    for name, text in files.items():
        (out / name).parent.mkdir(parents=True, exist_ok=True)
        (out / name).write_text(text, encoding='utf-8')
    return build.check_site(out, **kwargs)


class Build(unittest.TestCase):
    def assert_fails(self, page, found):
        """`found` is exactly one problem, and it names `page`."""
        self.assertEqual(len(found), 1, found)
        self.assertIn(page, found[0])

    def test_text_and_diag_escape_markup(self):
        path = temp_file(self, 'a < b && c > d')
        self.assertEqual(build.expand(f'{{{{text {path}}}}}', {}), 'a &lt; b &amp;&amp; c &gt; d')
        shown = build.diag_html((build.SITE / 'programs/diagnostics/rounds.err').read_text(encoding='utf-8'))
        self.assertIn('--explain &lt;code&gt;', shown)
        self.assertNotIn('<code>', shown)

    def test_diag_html_matches_the_mockup_colouring(self):
        shown = build.diag_html(E0032)
        for span in E0032_SPANS:
            self.assertIn(span, shown)

    def test_diag_html_colours_warnings_help_and_wide_text(self):
        for text, span in COLOURED:
            with self.subTest(text):
                self.assertIn(span, build.diag_html(text))

    def test_unknown_directive_fails(self):
        with self.assertRaisesRegex(build.BuildError, 'nope'):
            build.expand('{{nope}}', {})

    def test_directive_arguments_are_counted(self):
        for text in ('{{text}}', '{{text a b}}'):
            with self.subTest(text), self.assertRaisesRegex(build.BuildError, re.escape(f'expected {{{{text path}}}}, found {text}')):
                build.expand(text, {})

    def test_substitution_is_not_expanded_again(self):
        path = temp_file(self, '{{root}}')
        self.assertEqual(build.expand(f'{{{{text {path}}}}}', {'root': '../'}), '{{root}}')

    def test_head_escapes_title_and_description(self):
        page = build.assemble('', title='A <go> & B', description='Say "hi" & <go>', page='tour.html', depth=0, context={})
        self.assertIn('<title>A &lt;go&gt; &amp; B', page)
        self.assertIn('content="Say &quot;hi&quot; &amp; &lt;go&gt;"', page)

    def test_assemble_marks_the_page_and_its_depth(self):
        page = build.assemble('', title='t', description='d', page='diagnostics.html', depth=1, context={})
        self.assertIn('<a href="../diagnostics.html" aria-current="page">', page)
        self.assertEqual(page.count('aria-current'), 1)
        self.assertIn('href="../assets/site.css"', page)
        missing = build.assemble('', title='t', description='d', page='404.html', depth=0, context={})
        self.assertIn('href="/sisu/assets/site.css"', missing)

    def test_help_quotes_the_file(self):
        path = temp_file(self, '  = help: x\nhelp: a `b` <c>\n')
        self.assertEqual(build.expand(f'{{{{help {path}}}}}', {}), 'a `b` &lt;c&gt;')
        bare = temp_file(self, '  = help: x\n')
        with self.assertRaisesRegex(build.BuildError, re.escape(str(bare))):
            build.expand(f'{{{{help {bare}}}}}', {})

    def test_links_resolve_from_the_page_directory(self):
        base = {
            'index.html': PAGE.format('<p id="top">top</p>'),
            'assets/site.css': '',
        }
        cases = [
            ('<link rel="stylesheet" href="../assets/site.css">', True),
            ('<a href="../index.html#top">top</a>', True),
            ('<a href="/sisu/index.html">home</a>', True),
            ('<a href="https://github.com/sisu-lang/sisu/blob/main/docs/development.md">dev</a>', True),
            ('<link rel="stylesheet" href="assets/site.css">', False),
            ('<a href="#missing">x</a>', False),
            ('<a href="#">x</a>', False),
            ('<a href="/sisu/nope.html">x</a>', False),
            ('<a href="https://github.com/sisu-lang/sisu/blob/main/docs/nope.md">x</a>', False),
            ('<a href="https://github.com/sisu-lang/sisu/blob/main/docs/development.md#nope">x</a>', False),
            ('<script src="https://cdn.example/x.js"></script>', False),
            ("<a href='missing.html'>x</a>", False),
            ("<script src='https://cdn.example/x.js'></script>", False),
            ('<pre>href="missing.html"</pre>', True),
        ]
        for body, ok in cases:
            with self.subTest(body):
                found = problems(self, base | {'diagnostics/X.html': PAGE.format(body)})
                if ok:
                    self.assertEqual(found, [])
                else:
                    self.assert_fails('diagnostics/X.html', found)

    def test_diag_html_keeps_the_text(self):
        errs = sorted((build.SITE / 'programs').rglob('*.err'))
        self.assertGreater(len(errs), 100)
        for err in errs:
            with self.subTest(err.name):
                text = err.read_text(encoding='utf-8')
                self.assertEqual(html.unescape(re.sub(r'<[^>]*>', '', build.diag_html(text))), text)

    def test_stylesheet_urls(self):
        page = {'index.html': PAGE.format('')}
        cases = [
            ('@font-face { src: url(https://fonts.example/f.woff2); }', False),
            ('@import "https://fonts.example/f.css";', False),
            ('@font-face { src: url(fonts/missing.woff2); }', False),
            ('@font-face { src: url("fonts/f.woff2") format("woff2"); }', True),
        ]
        for css, ok in cases:
            with self.subTest(css):
                found = problems(self, page | {'assets/site.css': css, 'assets/fonts/f.woff2': ''})
                if ok:
                    self.assertEqual(found, [])
                else:
                    self.assert_fails('assets/site.css', found)
        # A page's own <style> and style="" are checked too, resolved from the page's directory.
        styles = [
            ('<style>body { background: url(https://cdn.example/bg.png); }</style>', False),
            ('<p style="background: url(https://cdn.example/bg.png)">x</p>', False),
            ('<style>@font-face { src: url(assets/fonts/f.woff2); }</style>', False),
            ('<style>@font-face { src: url(../assets/fonts/f.woff2); }</style>', True),
        ]
        for body, ok in styles:
            with self.subTest(body):
                found = problems(self, page | {'diagnostics/X.html': PAGE.format(body), 'assets/fonts/f.woff2': ''})
                if ok:
                    self.assertEqual(found, [])
                else:
                    self.assert_fails('diagnostics/X.html', found)

    def test_copy_words(self):
        cases = [
            ('<p>Milestone 9</p>', False),
            ('<p>Kotlin</p>', False),
            ('<!-- x -->', False),
            ('<pre>labels come in milestone 10</pre>', True),
            ('<pre data-lang="out">labels come in milestone 10</pre>', False),
        ]
        for body, ok in cases:
            with self.subTest(body):
                found = problems(self, {'tour.html': PAGE.format(body)})
                if ok:
                    self.assertEqual(found, [])
                else:
                    self.assert_fails('tour.html', found)

    def test_headings(self):
        cases = [
            ('<h2>a</h2><h3>b</h3><h2>c</h2>', True),
            ('<h1>again</h1>', False),
            ('<h3>skips h2</h3>', False),
        ]
        for body, ok in cases:
            with self.subTest(body):
                found = problems(self, {'tour.html': PAGE.format(body)})
                if ok:
                    self.assertEqual(found, [])
                else:
                    self.assert_fails('tour.html', found)

    def test_build_replaces_only_its_own_output(self):
        site = fake_site(self, {'index.html': 'Home\n\n<h1>Home</h1>\n'})
        out = site / '_site'
        build.build(out)
        build.build(out)  # the first build marked the directory as its own
        self.assertTrue((out / 'index.html').is_file())
        notes = site.parent / 'notes'
        notes.mkdir()
        (notes / 'keep.txt').write_text('x', encoding='utf-8')
        for dest, refusal in [(site / 'src', 'is a build input'), (site.parent, 'is a build input'), (notes, 'not built by')]:
            with self.subTest(dest), self.assertRaisesRegex(build.BuildError, refusal):
                build.build(dest)
        self.assertTrue((site / 'src' / 'index.html').is_file())
        self.assertTrue((notes / 'keep.txt').is_file())

    def test_page_needs_its_title_and_description_lines(self):
        fake_site(self, {'short.html': 'Title\n'})
        with self.assertRaisesRegex(build.BuildError, re.escape('site/src/short.html')):
            build.build(build.SITE / '_site')

    def test_every_page_has_a_description(self):
        for src in sorted((build.SITE / 'src').glob('*.html')):
            description = src.read_text(encoding='utf-8').split('\n', 2)[1]
            with self.subTest(src.name):
                self.assertTrue(description.strip(), 'description is empty')
                self.assertLess(len(description), 160)

    def test_forbid_tk(self):
        files = {'contribute.html': PAGE.format('<p>[TK: x]</p>')}
        self.assertEqual(problems(self, files), [])
        self.assert_fails('contribute.html', problems(self, files, forbid_tk=True))

    def test_status_board(self):
        fake_site(self, {}, {'site/data/status.toml': STATUS, 'docs/diagnostics/E0001.md': ''})
        board = build.expand('{{status-board}}', {})
        counts = [re.sub('<[^>]*>', '', c) for c in re.findall(r'<span class="counts">(.*?)</span>', board)]
        self.assertEqual(counts, ['2 work today · 1 planned', '1 work today · 1 planned'])
        self.assertEqual(re.findall(r'style="flex:(\d+)"', board), ['2', '1', '1', '1'])
        self.assertEqual(re.findall(r'<details class="area"( open)?>', board), [' open', ''])
        self.assertEqual(re.findall(r'<span class="area-name">(.*?)</span>', board), ['One', 'Two'])
        self.assertEqual(re.findall(r'<p class="note">(.*?)</p>', board), ['Careful.'])
        self.assertIn('<li>Alpha for 1 codes</li>', board)

    def test_status_tables(self):
        fake_site(self, {}, {'site/data/status.toml': STATUS, 'docs/diagnostics/E0001.md': ''})
        tables = build.expand('{{status-tables}}', {})
        self.assertEqual(re.findall(r'<h2 id="(\w+)">(\w+)</h2>\s*<p>(.*?)</p>', tables),
                         [('one', 'One', 'Intro <code>x</code>'), ('two', 'Two', 'Second.')])
        rows = re.findall(r'<tr><td>(.*?)</td><td><span class="st (\w+)">(.*?)</span></td><td>(.*?)</td></tr>', tables)
        self.assertEqual([row[:3] for row in rows], [
            ('Alpha for 1 codes', 'ok', '✓ Works today'), ('Beta', 'ok', '✓ Works today'), ('Gamma', 'plan', '○ Planned'),
            ('Delta', 'ok', '✓ Works today'), ('Epsilon', 'plan', '○ Planned')])
        self.assertEqual([re.findall(r'<a href="(.*?)">(.*?)</a>', row[3]) for row in rows[:3]], [
            [(build.BLOB + 'docs/adr/0001-first.md', 'ADR 1'), (build.BLOB + 'docs/roadmap.md#what-works-today', 'Roadmap: here')],
            [], [(build.BLOB + 'docs/roadmap.md', 'Roadmap')]])

    def test_status_state_must_be_known(self):
        fake_site(self, {}, {'site/data/status.toml': STATUS.replace('"planned"', '"soon"', 1)})
        with self.assertRaisesRegex(build.BuildError, 'soon'):
            build.expand('{{status-tables}}', {})

    def test_status_data_must_be_complete(self):
        cases = [('area id', 'id = "one"\n', 'area 1'), ('area name', 'name = "One"\n', "'one'"), ('area intro', 'intro = "Second."\n', "'two'"),
                 ('area note', 'note = "Careful."\n', "'one'"), ('feature text', 'text = "Beta"\n', "'one'"),
                 ('feature state', 'state = "planned"\n', "'one'"), ('feature notes', 'notes = []\n', "'Beta'")]
        for name, line, where in cases:
            with self.subTest(name):
                fake_site(self, {}, {'site/data/status.toml': STATUS.replace(line, '', 1)})
                with self.assertRaisesRegex(build.BuildError, rf'status\.toml.*{where}'):
                    build.expand('{{status-tables}}', {})
        fake_site(self, {}, {'site/data/status.toml': '[[area]]\nid = "one"\nname = "One"\nintro = ""\nnote = ""\n'})
        with self.assertRaisesRegex(build.BuildError, r"status\.toml.*'one'.*feature"):
            build.expand('{{status-board}}', {})

    def test_status_note_paths_are_checked(self):
        page = 'Status\n\n<h1>Status</h1>\n{{status-tables}}\n'
        cases = [('docs/roadmap.md#what-works-today', True), ('docs/nope.md', False), ('docs/roadmap.md#nope', False)]
        for path, ok in cases:
            with self.subTest(path):
                toml = STATUS.replace('docs/roadmap.md#what-works-today', path)
                site = fake_site(self, {'status.html': page}, {'site/data/status.toml': toml, 'docs/adr/0001-first.md': '',
                                                                'docs/roadmap.md': '# Roadmap\n\n## What works today\n'})
                self.enterContext(mock.patch.object(build.check_links, 'TRACKED', build.check_links.TRACKED | {'docs/adr/0001-first.md'}))
                build.build(site / '_site')
                found = build.check_site(site / '_site')
                if ok:
                    self.assertEqual(found, [])
                else:
                    self.assertTrue(all(path in problem for problem in found) and found, found)

    def test_counts(self):
        fake_site(self, {}, {f'docs/adr/{name}.md': '# T' for name in ('0001-a', '0002-b')}
                  | {f'docs/diagnostics/{name}.md': '' for name in ('E0001', 'W0002', 'README')})
        self.assertEqual(build.expand('{{adr-count}} {{diagnostic-count}}', {}), '2 2')

    def test_adr_list(self):
        fake_site(self, {}, {'docs/adr/0001-first.md': '# First `code` record\n\nBody.\n', 'docs/adr/0002-second.md': '# Second\n'})
        self.enterContext(mock.patch.object(build.check_links, 'TRACKED', {'docs/adr/0001-first.md', 'docs/adr/0002-second.md'}))
        cases = [
            ('both', adr_row('0001', 'first', 'First <code>code</code> record') + adr_row('0002', 'second', 'Second'), True),
            ('small note', adr_row('0001', 'first', 'First <code>code</code> record<small>proposed</small>') + adr_row('0002', 'second', 'Second'), True),
            ('soon note', adr_row('0001', 'first', 'First <code>code</code> record') + adr_row('0002', 'second', 'Second <span class="soon">planned</span>'), True),
            ('missing', adr_row('0001', 'first', 'First <code>code</code> record'), False),
            ('twice', adr_row('0001', 'first', 'First <code>code</code> record') * 2 + adr_row('0002', 'second', 'Second'), False),
            ('different title', adr_row('0001', 'first', 'First code record') + adr_row('0002', 'second', 'Second'), False),
            ('longer title', adr_row('0001', 'first', 'First <code>code</code> record') + adr_row('0002', 'second', 'Second one'), False),
            ('visible tags', adr_row('0001', 'first', 'First &lt;code&gt;code&lt;/code&gt; record') + adr_row('0002', 'second', 'Second'), False),
            ('wrong number', adr_row('0001', 'first', 'First <code>code</code> record', shown='0012') + adr_row('0002', 'second', 'Second'), False),
        ]
        for name, rows, ok in cases:
            with self.subTest(name):
                found = problems(self, {'decisions.html': PAGE.format(f'<ul>{rows}</ul>')})
                if ok:
                    self.assertEqual(found, [])
                else:
                    self.assert_fails('decisions.html', found)

    def test_adr_needs_a_heading(self):
        fake_site(self, {}, {'docs/adr/0001-first.md': 'No heading.\n'})
        self.enterContext(mock.patch.object(build.check_links, 'TRACKED', {'docs/adr/0001-first.md'}))
        found = problems(self, {'decisions.html': PAGE.format('<ul>' + adr_row('0001', 'first', 'First') + '</ul>')})
        self.assert_fails('decisions.html', found)
        self.assertIn('no "# " heading', found[0])

    def test_status_board_headings(self):
        fake_site(self, {}, {'site/data/status.toml': STATUS})
        board = build.expand('{{status-board}}', {})
        self.assertNotRegex(board, r'<summary>(?:(?!</summary>).)*<h\d')
        self.assertEqual(re.findall(r'<h(\d)>(.*?)</h', board), [('3', 'Works today'), ('3', 'Planned')] * 2)
        page = f'<h1>Home</h1><svg><symbol id="chev"></symbol></svg><section><h2>Where Sisu stands</h2>{board}</section>'
        self.assertEqual(problems(self, {'index.html': PAGE.replace('<h1>Page</h1>{}', '{}').format(page)}), [])

    def test_explanation_links(self):
        adr = '../adr/0019-enums-are-values-named-through-their-enum.md'
        cases = [('#considered-options', True), ('#nope', False)]
        for anchor, ok in cases:
            with self.subTest(anchor):
                text = f'# X0001: t\n\nSee [W0002](W0002.md) and [ADR 19]({adr}{anchor}).\n\n## Fixed\n'
                shown = build.explanation_html('X0001', text, '')
                self.assertIn('<a href="W0002.html">W0002</a>', shown)
                url = f'{build.BLOB}docs/adr/0019-enums-are-values-named-through-their-enum.md{anchor}'
                self.assertIn(f'<a href="{url}">ADR 19</a>', shown)
                found = problems(self, {'diagnostics/X0001.html': PAGE.format(shown), 'diagnostics/W0002.html': PAGE.format('')})
                if ok:
                    self.assertEqual(found, [])
                else:
                    self.assert_fails('diagnostics/X0001.html', found)
                    self.assertIn(url, found[0])

    def test_explanation_html_is_text(self):
        # `sisuc --explain` prints the Markdown as it is, so a `<` outside code shows, and markup cannot slip past the checks.
        text = '# X0001: t\n\nA value of type Option<T> is <iframe src="https://evil.example/x"></iframe> here.\n\n    indented milestone 9\n\n## Fixed\n'
        shown = build.explanation_html('X0001', text, '')
        self.assertIn('<p>A value of type Option&lt;T&gt; is &lt;iframe src=&quot;https://evil.example/x&quot;&gt;&lt;/iframe&gt; here.</p>', shown)
        self.assertNotIn('<iframe', shown)
        self.assertIn('<pre data-lang="">indented milestone 9\n</pre>', shown)
        self.assert_fails('diagnostics/X0001.html', problems(self, {'diagnostics/X0001.html': PAGE.format(shown)}))

    def test_categories_complete(self):
        explained = {f'docs/diagnostics/{name}.md': '' for name in ('E0001', 'W0002', 'README')}
        cases = [
            ('complete', '["E0001"]', '["W0002"]', None),
            ('missing', '["E0001"]', '[]', 'W0002'),
            ('no explanation', '["E0001", "E0003"]', '["W0002"]', 'E0003'),
            ('twice', '["E0001"]', '["W0002", "E0001"]', 'E0001'),
        ]
        for name, one, two, code in cases:
            with self.subTest(name):
                toml = f'[[category]]\nid = "a"\nname = "A"\ncodes = {one}\n\n[[category]]\nid = "b"\nname = "B"\ncodes = {two}\n'
                fake_site(self, {}, explained | {'site/data/categories.toml': toml})
                found = problems(self, {})
                if code is None:
                    self.assertEqual(found, [])
                else:
                    self.assert_fails('categories.toml', found)
                    self.assertIn(code, found[0])

    def test_benchmark_table_shape(self):
        tables = build.benchmark_tables(bench_md(md_table('fib', FIB)))
        self.assertEqual(tables['fib']['Sisu'], {'Wall (s)': '0.26', 'CPU (s)': '0.1', 'Peak RSS (MiB)': '1.0',
                                                 'Binary (KiB)': '9', 'Build (s)': '0.05'})
        self.assertEqual(list(tables['fib']), list(FIB))
        fib = md_table('fib', FIB)
        cases = [
            ('a renamed column', fib.replace('CPU (s)', 'CPU'), 'fib'),
            ('a new language', md_table('fib', FIB | {'Zig': '0.2'}), 'fib'),
            ('a missing language', md_table('fib', {k: v for k, v in FIB.items() if k != 'Go'}), 'fib'),
            ('a program without a table', fib + '### sieve\n\nNo table yet.\n\n', 'sieve'),
        ]
        for name, table, program in cases:
            with self.subTest(name):
                with self.assertRaisesRegex(build.BuildError, rf'docs/benchmarks\.md.*{program}'):
                    build.benchmark_tables(bench_md(table))
        expected = ['fib', 'sieve']
        with self.assertRaisesRegex(build.BuildError, r"docs/benchmarks\.md.*missing \['sieve'\], extra \[\]"):
            build.benchmark_tables(bench_md(fib), expected)
        with self.assertRaisesRegex(build.BuildError, r"docs/benchmarks\.md.*missing \[\], extra \['zig'\]"):
            build.benchmark_tables(bench_md(fib, md_table('sieve', FIB), md_table('zig', FIB)), expected)
        self.assertEqual(build.benchmark_tables(bench_md(md_table('sieve', FIB), fib), expected).keys(), {'fib', 'sieve'})
        other = f'## The programs\n\n{BENCH_HEADER}| Zig | 1 | 1 | 1 | 1 | 1 |\n\n### Extra\n\nNo table.\n'
        self.assertEqual(build.benchmark_tables(bench_md(fib) + other).keys(), {'fib'})

    def test_chart_cells_must_be_drawable(self):
        hello = md_table('hello', HELLO, HELLO_BUILDS)
        cases = [
            ('a Wall cell', md_table('fib', FIB | {'Go': '-'}), hello, 'fib'),
            ("Python's Wall cell", md_table('fib', FIB | {'Python': '-'}), hello, 'fib'),
            ('a hello Wall cell', md_table('fib', FIB), md_table('hello', HELLO | {'Go': '-'}, HELLO_BUILDS), 'hello'),
            ('a hello Build cell', md_table('fib', FIB), md_table('hello', HELLO, HELLO_BUILDS | {'Go': '-'}), 'hello'),
            ('a hello Python Build cell', md_table('fib', FIB), md_table('hello', HELLO, HELLO_BUILDS | {'Python': '0.5'}), 'hello'),
            ('no hello', md_table('fib', FIB), '', 'hello'),
            ('only zeros', md_table('fib', {lang: '0.00' for lang in FIB}), hello, 'fib'),
            # Python is drawn off the scale, which is only true when it is the slowest.
            ('Python faster than Go', md_table('fib', FIB | {'Python': '0.20'}), hello, 'fib'),
            ('Python level with Go', md_table('fib', FIB | {'Python': '0.28'}), hello, 'fib'),
        ]
        for name, fib, hello, program in cases:
            with self.subTest(name):
                with self.assertRaisesRegex(build.BuildError, rf'docs/benchmarks\.md.*{program}'):
                    build.bench_charts(bench_md(fib, hello))

    def test_chart_widths(self):
        charts = build.bench_charts(bench_md(md_table('hello', HELLO, HELLO_BUILDS), md_table('fib', FIB)))
        self.assertEqual(re.findall(r'<h3>(.*?)</h3><p class="unit">(.*?)</p>', charts),
                         [('fib', 'wall time, seconds'), ('hello', 'build time, seconds: compile one file and link it')])
        fib, hello = charts.split('<figure class="chart">')[1:]
        self.assertEqual(re.findall(BENCH_LI, fib), [
            ('', 'Rust', '<i style="width:60.7%"></i>', '0.17 s'), ('', 'C', '<i style="width:64.3%"></i>', '0.18 s'),
            (' class="sisu"', 'Sisu', '<i style="width:92.9%"></i>', '0.26 s'), ('', 'Java', '<i style="width:96.4%"></i>', '0.27 s'),
            ('', 'Go', '<i style="width:100%"></i>', '0.28 s'), (' class="off"', 'Python', 'off the scale', '5.18 s')])
        self.assertEqual(re.findall(BENCH_LI, hello), [
            ('', 'Go', '<i style="width:18.9%"></i>', '0.037 s'), ('', 'C', '<i style="width:27%"></i>', '0.053 s'),
            (' class="sisu"', 'Sisu', '<i style="width:29.6%"></i>', '0.058 s'), ('', 'Rust', '<i style="width:37.2%"></i>', '0.073 s'),
            ('', 'Java', '<i style="width:100%"></i>', '0.196 s'), (' class="off"', 'Python', 'no build step', '-')])

    def test_chart_ties_keep_the_document_order(self):
        walls = {'C': '0.14', 'Sisu': '0.14', 'Go': '0.23', 'Java': '0.20', 'Rust': '0.12', 'Python': '12.16'}
        charts = build.bench_charts(bench_md(md_table('nbody', walls), md_table('hello', HELLO, HELLO_BUILDS)))
        self.assertEqual(re.findall(r'<span>(\w+)</span>', charts.split('<figure class="chart">')[1]),
                         ['Rust', 'C', 'Sisu', 'Java', 'Go', 'Python'])

    def test_hello_foot_note(self):
        cases = [
            ('one differs', HELLO | {'Java': '0.01'}, "Every hello program runs in 0.00 s of wall time except Java's, 0.01 s. The build is what differs."),
            ('spelled differently', HELLO | {'Java': '0.0'}, 'Every hello program runs in 0.00 s of wall time. The build is what differs.'),
            ('all equal', HELLO, 'Every hello program runs in 0.00 s of wall time. The build is what differs.'),
        ]
        for name, walls, note in cases:
            with self.subTest(name):
                charts = build.bench_charts(bench_md(md_table('fib', FIB), md_table('hello', walls, HELLO_BUILDS)))
                self.assertEqual(re.findall(r'<p class="foot">(.*?)</p>', charts), [note])
        for name, walls in [('two differ', HELLO | {'Java': '0.01', 'Go': '0.01'}), ('a split', {k: '0.01' if k in ('C', 'Go', 'Java') else '0.00' for k in HELLO})]:
            with self.subTest(name):
                with self.assertRaisesRegex(build.BuildError, r'docs/benchmarks\.md.*hello'):
                    build.bench_charts(bench_md(md_table('hello', walls, HELLO_BUILDS)))

    def test_bench_table(self):
        table = build.bench_table(bench_md(md_table('fib', FIB), md_table('hello', HELLO | {'Java': '0.01'}, HELLO_BUILDS),
                                           md_table('sieve', FIB)))
        self.assertEqual(re.findall(r'<th(?: [^>]*)?>(.*?)</th>', table), ['Benchmark', 'Sisu', 'C', 'Rust', 'Go', 'Java', 'Python'])
        rows = [re.findall(r'<td[^>]*>(.*?)</td>', row) for row in re.findall(r'<tr><td>.*?</tr>', table)]
        self.assertEqual(rows, [
            ['<code>fib</code>', '<b>0.26</b>', '0.18', '0.17', '0.28', '0.27', '5.18'],
            ['<code>hello</code>', '<b>0.00</b>', '0.00', '0.00', '0.00', '0.01', '0.00'],
            ['<code>hello</code>, build', '<b>0.058</b>', '0.053', '0.073', '0.037', '0.196', '-'],
            ['<code>sieve</code>', '<b>0.26</b>', '0.18', '0.17', '0.28', '0.27', '5.18']])

    def test_results_note(self):
        md = bench_md(intro='Commit `abc1234`, on\n2026-10-12.\n\nOn a machine.\n\n')
        self.assertEqual(build.results_note(md), '<p class="after">Commit <code>abc1234</code>, on\n2026-10-12.</p>\n')
        for name, text in [('a heading first', bench_md(intro='### fib\n\nOn a machine.\n\n')),
                           ('no paragraph', bench_md(intro='')),
                           ('a table first', bench_md(intro=BENCH_HEADER + '| C | 1 | 1 | 1 | 1 | 1 |\n\n')),
                           ('no Results', '# Benchmarks\n\nAbout.\n\n## Other\n\nText.\n')]:
            with self.subTest(name):
                with self.assertRaisesRegex(build.BuildError, r'docs/benchmarks\.md'):
                    build.results_note(text)

    def test_bench_directives_read_the_document(self):
        fake_site(self, {}, {'docs/benchmarks.md': bench_md(md_table('fib', FIB), md_table('hello', HELLO, HELLO_BUILDS)),
                             'scripts/bench.sh': '[[ $# -gt 0 ]] || benchmarks=(hello fib sieve)\n',
                             'bench/features/strings/main.sisu': ''})
        for directive in ('bench-charts', 'bench-table'):
            with self.assertRaisesRegex(build.BuildError, r"docs/benchmarks\.md.*missing \['sieve'\]"):
                build.expand('{{%s}}' % directive, {})
        (build.ROOT / 'scripts/bench.sh').write_text('[[ $# -gt 0 ]] || benchmarks=(hello fib)\n')
        self.assertEqual(build.expand('{{bench-charts}}', {}).count('<figure class="chart">'), 2)
        self.assertIn('<td><code>fib</code></td>', build.expand('{{bench-table}}', {}))
        self.assertIn('Commit abc1234', build.expand('{{bench-results-note}}', {}))


class Pages(unittest.TestCase):
    """The real site, built once."""

    @classmethod
    def setUpClass(cls):
        tmp = tempfile.TemporaryDirectory()
        cls.addClassCleanup(tmp.cleanup)
        cls.out = Path(tmp.name) / 'site'
        build.build(cls.out)

    def page(self, name):
        return (self.out / name).read_text(encoding='utf-8')

    def test_diagnostic_pages(self):
        built = {p.stem for p in (self.out / 'diagnostics').glob('*.html')}
        explained = {p.stem for p in (build.ROOT / 'docs/diagnostics').glob('*.md')} - {'README'}
        self.assertTrue(explained)
        self.assertEqual(built, explained)
        self.assertTrue((self.out / 'diagnostics.html').is_file())
        self.assertIn('href="/sisu/diagnostics.html"', self.page('404.html'))
        # The first paragraph of E0130 quotes the error, so the description escapes a `"`.
        rule = (build.ROOT / 'docs/diagnostics/E0130.md').read_text(encoding='utf-8').split('\n\n')[1].replace('`', '')
        self.assertIn('"', rule)
        metas = Metas(self.page('diagnostics/E0130.html')).descriptions
        self.assertEqual(metas, [rule])

    def test_explanation_order(self):
        page = self.page('diagnostics/E0032.html')
        parts = ['A value must have the type its position expects', '<h2>Why</h2>', '<h2>Broken</h2>',
                 '<h2>What sisuc says</h2>', 'error[E0032]', '<h2>Fixed</h2>']
        places = [page.find(part) for part in parts]
        self.assertNotIn(-1, places, parts)
        self.assertEqual(places, sorted(places))
        self.assertNotIn('Your code', page)
        self.assertIn('action="../diagnostics.html"', page)
        self.assertNotIn('After the fix', page)

    def test_category_and_severity(self):
        cases = [('E0072', 'Syntax', 'Error'), ('W0002', 'Variables and change', 'Warning')]
        for code, category, severity in cases:
            with self.subTest(code):
                page = self.page(f'diagnostics/{code}.html')
                self.assertEqual(re.findall(r'<a href="[^"]*#[^"]*" aria-current="page"[^>]*>(.*?)</a>', page), [category])
                self.assertIn(f'<span class="sev">{severity}</span>', page)

    def test_fix_badges(self):
        cases = [('E0130', ['Unsafe fix']), ('E0001', ['Safe fix']), ('E0032', [])]
        for code, badges in cases:
            with self.subTest(code):
                page = self.page(f'diagnostics/{code}.html')
                self.assertIn(f'<span class="id">{code}</span>', page)
                self.assertEqual(re.findall(r'<span class="badge">(.*?)</span>', page), badges)

    def test_search_terms(self):
        self.assertIn('<li data-code="E0032" data-words="value of the wrong type"><a href="diagnostics/E0032.html">'
                      '<span class="num">E0032</span><b>Value of the wrong type</b></a></li>', self.page('diagnostics.html'))

    def test_titles(self):
        index = self.page('diagnostics.html')
        cases = [('E0032', 'Value of the wrong type', 'Value of the wrong type'),
                 ('E0006', '<code>else</code> on a line of its own', 'else on a line of its own')]
        for code, heading, plain in cases:
            with self.subTest(code):
                page = self.page(f'diagnostics/{code}.html')
                self.assertIn(f'<h1>{heading}</h1>', page)
                self.assertIn(f'<b>{heading}</b>', index)
                title = re.search('<title>(.*?)</title>', page)[1]
                self.assertEqual(title, f'{code}: {plain}')
                self.assertNotIn('`', title + heading)


class Metas(HTMLParser):
    """The `content` of each `<meta name="description">` of a page."""

    def __init__(self, text):
        super().__init__()
        self.descriptions = []
        self.feed(text)
        self.close()

    def handle_starttag(self, tag, attrs):
        if tag == 'meta' and dict(attrs).get('name') == 'description':
            self.descriptions.append(dict(attrs).get('content'))


if __name__ == '__main__':
    unittest.main()
