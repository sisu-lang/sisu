"""Tests for build.py. Run them with `uv run --locked --script site/build.py --test`."""
import html
import re
import tempfile
import unittest
from pathlib import Path

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

PAGE = '<!doctype html><html><head></head><body><h1>Page</h1>{}</body></html>'


def temp_file(test, text):
    """A file holding `text`, removed when the test ends."""
    tmp = tempfile.TemporaryDirectory()
    test.addCleanup(tmp.cleanup)
    path = Path(tmp.name) / 'f'
    path.write_text(text, encoding='utf-8')
    return path


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

    def test_unknown_directive_fails(self):
        with self.assertRaisesRegex(build.BuildError, 'nope'):
            build.expand('{{nope}}', {})

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

    def test_forbid_tk(self):
        files = {'contribute.html': PAGE.format('<p>[TK: x]</p>')}
        self.assertEqual(problems(self, files), [])
        self.assert_fails('contribute.html', problems(self, files, forbid_tk=True))


if __name__ == '__main__':
    unittest.main()
