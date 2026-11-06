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

    def test_a_table_in_a_comment_should_not_count(self):
        row = "| " + " ".join(["a"] * 9) + " |"
        for path, lead in [("a.rs", "///"), ("a.rs", "//"), ("a.py", "#"), ("a.sh", "#")]:
            with self.subTest(path=path, lead=lead):
                self.assertEqual(found(path, f"{lead} {row}\n" * 3), [])

    def test_a_heading_in_a_rust_comment_should_not_count(self):
        self.assertEqual(found("a.rs", f"/// # Panics\n/// {FITS}\n"), [])

    def test_a_sentence_should_end_at_a_table(self):
        half = " ".join(["word"] * 13)
        self.assertEqual(found("a.md", f"{half}\n| a |\n{half}.\n"), [])

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

    def test_the_width_message_should_give_the_columns_it_measured(self):
        self.assertRegex(found("a.sh", f"x\n# {'w' * 98}  \n")[0], "has 102 columns")


class MainTests(unittest.TestCase):
    NEW = "a.md:1: a sentence has 26 words; the limit is 25: split it\n"

    def setUp(self):
        # A hook runs these tests with Git's variables set, and they must not reach the real index.
        clean = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        # The Git settings of the machine must not change a test: a test sets its own.
        clean |= {"GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1"}
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

    def commit(self):
        self.git("add", ".")
        self.git("commit", "-qm", "next")

    def run_main(self, path="a.md"):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            status = text_limits.main(["--diff", "HEAD", path])
        return status, out.getvalue()

    def test_a_change_beside_an_old_long_sentence_should_pass(self):
        self.write("a.md", f"New.\n\n{LONG}\n")
        self.assertEqual(self.run_main(), (0, ""))

    def test_a_new_long_sentence_should_fail_and_name_its_line(self):
        self.write("a.md", f"{LONG}\n\n{LONG}\n")
        status, out = self.run_main()
        self.assertEqual(status, 1)
        self.assertEqual(out, self.NEW)

    def test_a_removed_line_should_pass(self):
        self.write("a.md", f"{LONG}\n")
        self.assertEqual(self.run_main(), (0, ""))

    def test_a_rename_should_check_only_the_lines_it_changes(self):
        self.git("config", "diff.renames", "false")
        self.git("mv", "a.md", "b.md")
        self.write("b.md", f"New.\n\n{LONG}\n")
        self.git("add", ".")
        self.assertEqual(self.run_main("b.md"), (0, ""))

    def test_a_new_long_sentence_in_a_renamed_file_should_fail(self):
        self.write("a.md", f"{LONG}\n" * 5)
        self.commit()
        self.git("mv", "a.md", "b.md")
        self.write("b.md", f"{LONG}\n" * 6)
        self.git("add", ".")
        self.assertEqual(self.run_main("b.md"), (1, "b.md:6: a sentence has 26 words; the limit is 25: split it\n"))

    def test_a_git_setting_should_not_hide_a_new_long_sentence(self):
        self.write("a.md", f"{LONG}\n\n{LONG}\n")
        for key, value in [("color.ui", "always"), ("color.diff", "always"), ("diff.external", "true"),
                           ("diff.noprefix", "true"), ("diff.mnemonicPrefix", "true")]:
            with self.subTest(key):
                self.git("config", key, value)
                result = self.run_main()
                self.git("config", "--unset", key)
                self.assertEqual(result, (1, self.NEW))

    def test_a_git_setting_should_not_mark_a_line_between_two_changes(self):
        self.git("config", "diff.interHunkContext", "5")
        self.write("a.md", f"Old.\n\n{LONG}\n\nOld.\n")
        self.commit()
        self.write("a.md", f"New.\n\n{LONG}\n\nNew.\n")
        self.assertEqual(self.run_main(), (0, ""))

    def test_a_path_that_git_writes_in_another_form_should_be_read(self):
        for name in ("a b.md", "ä.md"):
            with self.subTest(name):
                self.write(name, f"{LONG}\n")
                self.git("add", name)
                self.assertEqual(self.run_main(name)[0], 1)
        self.write("a.md", f"{LONG}\n\n{LONG}\n")
        self.assertEqual(self.run_main("./a.md")[0], 1)

    def test_a_run_from_a_subdirectory_should_read_the_file(self):
        os.mkdir("sub")
        self.write("sub/c.md", f"{LONG}\n")
        self.git("add", ".")
        os.chdir("sub")
        self.assertEqual(self.run_main("c.md")[0], 1)

    def test_an_absolute_path_or_a_path_above_this_directory_should_be_read(self):
        os.mkdir("sub")
        self.write("a.md", f"{LONG}\n\n{LONG}\n")
        self.git("add", ".")
        self.assertEqual(self.run_main(os.path.abspath("a.md"))[0], 1)
        os.chdir("sub")
        self.assertEqual(self.run_main("../a.md")[0], 1)
        self.git("config", "diff.relative", "true")
        self.assertEqual(self.run_main("../a.md")[0], 1)

    def test_a_git_attribute_should_not_hide_a_new_long_sentence(self):
        self.write("a.md", f"Old.\n\n{LONG}\n\n{LONG}\n")
        self.git("config", "diff.cut.textconv", "sed 1d")
        for attribute in ("*.md diff=cut", "*.md -diff"):
            with self.subTest(attribute):
                self.write(".git/info/attributes", f"{attribute}\n")
                self.assertEqual(self.run_main(), (1, "a.md:5: a sentence has 26 words; the limit is 25: split it\n"))

    def test_added_text_like_a_diff_header_should_not_start_a_file(self):
        self.write("z.md", "Old.\n")
        self.commit()
        self.write("a.md", f"{LONG}\n\n{LONG}\n")
        self.write("z.md", "Old.\n++ b/a.md\n")
        self.assertEqual(self.run_main(), (1, self.NEW))

    def test_a_file_that_is_not_utf8_should_not_stop_the_check(self):
        with open("z.txt", "wb") as f:
            f.write(b"\xff\n")
        self.git("add", ".")
        self.write("a.md", f"{LONG}\n\n{LONG}\n")
        self.assertEqual(self.run_main(), (1, self.NEW))

    def test_a_merge_should_pass(self):
        self.write("a.md", f"{LONG}\n\n{LONG}\n")
        head = subprocess.run(["git", "rev-parse", "HEAD"], check=True, capture_output=True, text=True).stdout.strip()
        self.git("update-ref", "MERGE_HEAD", head)
        self.assertEqual(self.run_main(), (0, ""))


if __name__ == "__main__":
    unittest.main()
