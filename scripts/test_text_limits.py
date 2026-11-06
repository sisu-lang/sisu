"""Unit tests for text_limits.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""

import contextlib
import io
import os
import subprocess
import tempfile
import unittest
from unittest import mock

import text_limits

LONG = " ".join(["a"] * 26) + "."
FITS = " ".join(["a"] * 25) + "."


def found(path, text, changed=None):
    lines = text.splitlines(True)
    return text_limits.findings(path, lines, set(range(1, len(lines) + 1)) if changed is None else changed)


class SentenceTests(unittest.TestCase):
    def test_a_changed_sentence_over_the_limit_should_fail(self):
        for path, text in [("a.md", LONG), ("a.rs", f"// {LONG}"), ("a.rs", f"/// {LONG}"),
                           ("a.py", f'"""{LONG}"""'), ("a.py", f"x = 1\n# {LONG}"), ("a.sh", f"# {LONG}")]:
            with self.subTest(path=path, text=text[:6]):
                self.assertEqual(len(found(path, text + "\n")), 1)

    def test_a_sentence_at_the_limit_should_pass(self):
        self.assertEqual(found("a.md", FITS + "\n"), [])

    def test_a_sentence_should_count_over_wrapped_lines(self):
        half = " ".join(["word"] * 13)
        self.assertEqual(len(found("a.rs", f"// {half}\n// {half}.\n")), 1)

    def test_a_sentence_with_no_changed_line_should_pass(self):
        self.assertEqual(found("a.md", f"Other.\n\n{LONG}\n", changed={1}), [])

    def test_a_change_to_one_line_should_count_the_whole_sentence(self):
        half = " ".join(["word"] * 13)
        self.assertEqual(len(found("a.md", f"{half}\n{half}.\n", changed={2})), 1)

    def test_a_code_span_should_count_as_one_word(self):
        code = "`" + " ".join(["x"] * 30) + "`"
        self.assertEqual(found("a.md", f"Run {code} now.\n"), [])

    def test_a_link_target_should_not_count(self):
        self.assertEqual(found("a.md", f"Read [the guide]({'a b ' * 30}) now.\n"), [])

    def test_text_that_is_no_prose_should_pass(self):
        for name, text in [("fence", f"```\n{LONG}\n```\n"), ("table", f"| {LONG} |\n"), ("heading", f"# {LONG}\n"),
                           ("frontmatter", f"---\ndescription: {LONG}\n---\n"), ]:
            with self.subTest(name):
                self.assertEqual(found("a.md", text), [])

    def test_each_given_when_then_line_should_be_its_own_sentence(self):
        half = " ".join(["word"] * 13)
        self.assertEqual(found("a.rs", f"/// GIVEN {half}\n/// WHEN {half}\n/// THEN {half}\n"), [])

    def test_each_list_item_should_be_its_own_sentence(self):
        half = " ".join(["word"] * 13)
        self.assertEqual(found("a.md", f"- {half}\n- {half}\n"), [])

    def test_a_python_string_that_is_no_docstring_should_pass(self):
        self.assertEqual(found("a.py", f'x = 1\nstub = """\n# {LONG}\n"""\n'), [])

    def test_python_that_does_not_parse_should_pass(self):
        self.assertEqual(found("a.py", f"def (\n# {LONG}\n"), [])

    def test_another_kind_of_file_should_pass(self):
        self.assertEqual(found("a.toml", f"# {LONG}\n"), [])


class WidthTests(unittest.TestCase):
    WIDE = "# " + "w" * 99

    def test_a_changed_python_or_shell_comment_over_the_width_should_fail(self):
        for path in ("a.py", "a.sh"):
            with self.subTest(path):
                self.assertRegex(found(path, f"x\n{self.WIDE}\n" if path == "a.sh" else f"x = 1\n{self.WIDE}\n")[0],
                                 r":2: a comment line has 101 columns")

    def test_a_comment_at_the_width_should_pass(self):
        self.assertEqual(found("a.sh", self.WIDE[:-1] + "\n"), [])

    def test_a_python_code_line_over_the_width_should_pass(self):
        self.assertEqual(found("a.py", f"x = '{'w' * 100}'\n"), [])

    def test_an_unchanged_comment_over_the_width_should_pass(self):
        self.assertEqual(found("a.sh", f"x\n{self.WIDE}\n", changed={1}), [])


class MainTests(unittest.TestCase):
    def setUp(self):
        # A hook runs these tests with Git's variables set, and they must not reach the real index.
        clean = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        patcher = mock.patch.dict(os.environ, clean, clear=True)
        patcher.start()
        self.addCleanup(patcher.stop)
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.addCleanup(os.chdir, os.getcwd())
        os.chdir(tmp.name)
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.email", "test@example.invalid")
        self.git("config", "user.name", "test")
        self.write("a.md", f"Old.\n\n{LONG}\n")
        self.git("add", ".")
        self.git("commit", "-qm", "first")

    @staticmethod
    def git(*args):
        subprocess.run(["git", *args], check=True, capture_output=True)

    @staticmethod
    def write(path, text):
        with open(path, "w", encoding="utf-8") as f:
            f.write(text)

    def run_main(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            status = text_limits.main(["--diff", "HEAD", "a.md"])
        return status, out.getvalue()

    def test_a_change_beside_an_old_long_sentence_should_pass(self):
        self.write("a.md", f"New.\n\n{LONG}\n")
        self.assertEqual(self.run_main(), (0, ""))

    def test_a_new_long_sentence_should_fail_and_name_its_line(self):
        self.write("a.md", f"{LONG}\n\n{LONG}\n")
        status, out = self.run_main()
        self.assertEqual(status, 1)
        self.assertEqual(out, "a.md:1: a sentence has 26 words; the limit is 25: split it\n")

    def test_a_merge_should_pass(self):
        self.write("a.md", f"{LONG}\n\n{LONG}\n")
        head = subprocess.run(["git", "rev-parse", "HEAD"], check=True, capture_output=True, text=True).stdout.strip()
        self.git("update-ref", "MERGE_HEAD", head)
        self.assertEqual(self.run_main(), (0, ""))


if __name__ == "__main__":
    unittest.main()
