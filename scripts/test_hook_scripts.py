"""Unit tests for the prek hook scripts: python3 -m unittest discover -s scripts -p 'test_*.py'."""
import contextlib
import io
import os
import tempfile
import unittest

import comment_width
import registry_debug_format


def run(main, text):
    """`main`'s exit code and output on one file holding `text`."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, 'f.rs')
        with open(path, 'w', encoding='utf-8') as f:
            f.write(text)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = main([path])
    return code, out.getvalue()


class CommentWidth(unittest.TestCase):
    def test_a_comment_over_100_columns_fails_at_its_line(self):
        code, out = run(comment_width.main, 'fn f() {}\n    // ' + 'x' * 94 + '\n')
        self.assertEqual(code, 1)
        self.assertIn(':2: comment is 101 columns', out)

    def test_a_comment_of_100_columns_and_long_code_pass(self):
        text = '    // ' + 'x' * 93 + '\nlet s = "' + 'x' * 100 + '";\n'
        self.assertEqual(run(comment_width.main, text), (0, ''))


class RegistryDebugFormat(unittest.TestCase):
    def test_debug_format_before_the_tests_fails(self):
        code, out = run(registry_debug_format.main, 'use a;\nformat!("{x:?}")\n#[cfg(test)]\n')
        self.assertEqual(code, 1)
        self.assertIn(':2: diagnostic text uses {:?}', out)

    def test_debug_format_in_the_tests_passes(self):
        text = 'use a;\n#[cfg(test)]\nformat!("{x:?}")\n'
        self.assertEqual(run(registry_debug_format.main, text), (0, ''))


if __name__ == '__main__':
    unittest.main()
