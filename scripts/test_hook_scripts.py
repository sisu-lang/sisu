"""Unit tests for the prek hook scripts: python3 -m unittest discover -s scripts -p 'test_*.py'."""
import contextlib
import io
import os
import tempfile
import unittest

import comment_width
import registry_debug_format
import session_url
import test_names


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


class SessionUrl(unittest.TestCase):
    def test_a_session_line_or_url_should_fail_at_its_line(self):
        for line in ('Claude-Session: https://claude.ai/code/session_01ab',
                     'See https://claude.ai/code/session_01ab'):
            code, out = run(session_url.main, f'Fix the lexer\n\n{line}\n')
            self.assertEqual(code, 1, line)
            self.assertIn(':3: remove the session line', out)

    def test_a_message_without_one_should_pass(self):
        text = 'Fix the lexer\n\nThe session of a `let` ends at its line.\n# Claude-Session: a comment\n'
        self.assertEqual(run(session_url.main, text), (0, ''))


def run_test_names(text, path='crates/sisuc/src/f.rs', baseline=''):
    """test_names' exit code and output on one file at `path` holding `text`."""
    with tempfile.TemporaryDirectory() as d:
        full = os.path.join(d, path)
        os.makedirs(os.path.dirname(full))
        with open(full, 'w', encoding='utf-8') as f:
            f.write(text)
        old = os.path.join(d, 'baseline.txt')
        with open(old, 'w', encoding='utf-8') as f:
            f.write(baseline.replace('PATH', full))
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = test_names.main([full], old)
    return code, out.getvalue()


GWT = '/// GIVEN a file\n/// WHEN sisu builds it\n/// AND it runs\n/// THEN it prints 1\n'


class TestNames(unittest.TestCase):
    def test_a_name_without_should_should_fail_at_its_fn_line(self):
        code, out = run_test_names('#[test]\n#[should_panic]\nfn emit_ir() {}\n')
        self.assertEqual(code, 1)
        self.assertIn(':3: test name `emit_ir`', out)

    def test_a_name_with_should_should_pass(self):
        self.assertEqual(run_test_names('#[test]\nfn ir_should_verify() {}\n'), (0, ''))

    def test_fn_in_an_attribute_should_not_be_the_name(self):
        text = '#[test]\n#[should_panic(expected = "fn f_should_x")]\nfn emit_ir() {}\n'
        code, out = run_test_names(text)
        self.assertEqual(code, 1)
        self.assertIn(':3: test name `emit_ir`', out)

    def test_a_reused_baseline_name_should_fail_at_its_second_use(self):
        text = '#[test]\nfn emit_ir() {}\nmod m {\n#[test]\nfn emit_ir() {}\n}\n'
        code, out = run_test_names(text, baseline='PATH::emit_ir\n')
        self.assertEqual(code, 1)
        self.assertIn(':5: test name `emit_ir`', out)

    def test_an_end_to_end_test_should_fail_with_plain_comments(self):
        text = ('// GIVEN a file\n// WHEN sisu builds it\n// THEN it prints 1\n'
                '#[test]\nfn ir_should_verify() {}\n')
        self.assertEqual(run_test_names(text, path='crates/sisuc/tests/cli.rs')[0], 1)

    def test_a_macro_test_should_not_take_the_name_of_the_next_function(self):
        text = ('macro_rules! m {\n    () => {\n        #[test]\n        fn $name() {}\n    };\n}\n'
                'fn setup() {}\n')
        self.assertEqual(run_test_names(text), (0, ''))

    def test_doc_lines_above_a_long_attribute_should_count(self):
        text = (GWT + '#[cfg_attr(\n    any(),\n    ignore = "slow"\n)]\n'
                '#[test]\nfn ir_should_verify() {}\n')
        self.assertEqual(run_test_names(text, path='crates/sisuc/tests/cli.rs'), (0, ''))

    def test_a_name_in_the_baseline_should_pass(self):
        text = '#[test]\nfn emit_ir() {}\n'
        self.assertEqual(run_test_names(text, baseline='# old\nPATH::emit_ir\n'), (0, ''))

    def test_an_end_to_end_test_should_fail_without_then(self):
        text = '/// GIVEN a file\n/// WHEN sisu builds it\n#[test]\nfn ir_should_verify() {}\n'
        code, out = run_test_names(text, path='crates/sisuc/tests/cli.rs')
        self.assertEqual(code, 1)
        self.assertIn(':4: end-to-end test `ir_should_verify` needs GIVEN', out)

    def test_an_end_to_end_test_should_fail_when_its_lines_are_out_of_order(self):
        text = ('/// WHEN sisu builds it\n/// GIVEN a file\n/// THEN it prints 1\n'
                '#[test]\nfn ir_should_verify() {}\n')
        self.assertEqual(run_test_names(text, path='crates/sisuc/tests/cli.rs')[0], 1)

    def test_an_end_to_end_test_should_pass_with_given_when_and_then_above_its_attributes(self):
        text = GWT + '#[test]\n#[ignore]\nfn ir_should_verify() {}\n'
        self.assertEqual(run_test_names(text, path='crates/sisuc/tests/cli.rs'), (0, ''))

    def test_a_unit_test_should_pass_without_given_when_and_then(self):
        text = '#[test]\nfn ir_should_verify() {}\n'
        self.assertEqual(run_test_names(text, path='crates/sisuc/src/check/tests/u8.rs'), (0, ''))


if __name__ == '__main__':
    unittest.main()
