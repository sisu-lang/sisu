"""Tests for build.py. Run them with `uv run --locked --script site/build.py --test`."""
import html
import re
import tempfile
import unittest
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

REAL_ROOT = build.ROOT

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
                site = fake_site(self, {'status.html': page}, {'site/data/status.toml': toml, 'docs/adr/0001-first.md': ''})
                self.enterContext(mock.patch.object(build, 'ROOT', REAL_ROOT))  # the anchors are read from a real file
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


if __name__ == '__main__':
    unittest.main()
