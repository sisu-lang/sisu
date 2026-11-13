"""Unit tests for comment_width.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""

import contextlib
import io
import tempfile
import unittest
from pathlib import Path

import comment_width

LONG = "x" * 101


class CommentWidth(unittest.TestCase):
    def check(self, name: str, text: str) -> int:
        """Write `text` to the file `name` in a new directory; return the exit code of the check."""
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / name
            path.write_text(text, encoding="utf-8")
            with contextlib.redirect_stdout(io.StringIO()):
                return comment_width.main([str(path)])

    def test_a_long_rust_comment_should_fail(self) -> None:
        self.assertEqual(self.check("a.rs", f"// {LONG}\n"), 1)

    def test_a_long_toml_comment_should_fail(self) -> None:
        self.assertEqual(self.check("a.toml", f"  # {LONG}\n"), 1)

    def test_a_long_yaml_comment_should_fail(self) -> None:
        self.assertEqual(self.check("a.yml", f"      # {LONG}\n"), 1)

    def test_a_long_toml_value_should_pass(self) -> None:
        self.assertEqual(self.check("a.toml", f'files = "{LONG}"\n'), 0)

    def test_a_hash_line_in_rust_should_pass(self) -> None:
        self.assertEqual(self.check("a.rs", f'#[doc = "{LONG}"]\n'), 0)

    def test_a_comment_of_100_columns_should_pass(self) -> None:
        self.assertEqual(self.check("a.toml", "#" + "x" * 99 + "\n"), 0)


if __name__ == "__main__":
    unittest.main()
