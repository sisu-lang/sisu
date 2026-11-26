"""Unit tests for solve-issues' ledger.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""

import importlib.util
import tempfile
import unittest
import unittest.mock
from contextlib import redirect_stderr
from datetime import UTC, datetime
from io import StringIO
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / ".agents/skills/solve-issues/scripts/ledger.py"
spec = importlib.util.spec_from_file_location("ledger", SCRIPT)
# pyrefly: ignore[bad-argument-type]  the script is in the repository, so the spec is found
ledger = importlib.util.module_from_spec(spec)
# pyrefly: ignore[missing-attribute]  as above, and a file spec has a loader
spec.loader.exec_module(ledger)

NOW = datetime(2026, 11, 3, 14, 5, tzinfo=UTC)
LEDGER = """# Batch b

## After the merge

- old: repro.

## Events

- 2026-11-02 10:00 started.
"""


class Add(unittest.TestCase):
    def test_event_should_go_at_the_end_of_events(self) -> None:
        out = ledger.add(LEDGER, "event", "merged #1.", NOW)
        self.assertTrue(
            out.endswith("- 2026-11-02 10:00 started.\n- 2026-11-03 14:05 merged #1.\n")
        )

    def test_after_merge_should_go_after_the_last_line_of_its_section(self) -> None:
        out = ledger.add(LEDGER, "after-merge", "new: repro.", NOW)
        self.assertIn("- old: repro.\n- new: repro.\n\n## Events", out)

    def test_after_merge_should_fill_an_empty_section(self) -> None:
        out = ledger.add("## After the merge\n\n## Events\n", "after-merge", "x.", NOW)
        self.assertEqual(out, "## After the merge\n- x.\n\n## Events\n")

    def test_a_missing_section_should_be_an_error(self) -> None:
        with self.assertRaises(ValueError):
            ledger.add("# Batch b\n", "event", "x.", NOW)

    def test_an_unknown_kind_should_be_an_error(self) -> None:
        with self.assertRaises(ValueError):
            ledger.add(LEDGER, "note", "x.", NOW)


GROUPS = """## Groups

| Group | Issues | State |
| --- | --- | --- |
| G1 | #1 | task review |
| G2 | #2 | waiting |

## Events
"""


class SetGroup(unittest.TestCase):
    def test_set_group_should_change_only_the_state_of_its_row(self) -> None:
        out = ledger.set_group(GROUPS, "G1", "PR #9 open")
        self.assertIn("| G1 | #1 | PR #9 open |\n| G2 | #2 | waiting |\n", out)

    def test_set_group_should_match_the_whole_group_cell(self) -> None:
        with self.assertRaises(ValueError):
            ledger.set_group(GROUPS, "G", "x")

    def test_the_header_and_the_delimiter_should_not_match_a_group(self) -> None:
        for group in ("Group", "---"):
            with self.subTest(group=group), self.assertRaises(ValueError):
                ledger.set_group(GROUPS, group, "x")

    def test_a_line_after_the_table_should_not_match_a_group(self) -> None:
        text = GROUPS.replace("\n\n## Events", "\nNote for | G3 | see #3\n\n## Events")
        with self.assertRaises(ValueError):
            ledger.set_group(text, "G3", "x")

    def test_a_row_with_no_last_bar_should_get_its_state_in_the_last_cell(self) -> None:
        text = GROUPS.replace("| G2 | #2 | waiting |", "| G2 | #2 | waiting")
        self.assertIn("| G2 | #2 | done |\n", ledger.set_group(text, "G2", "done"))

    def test_a_groups_section_with_no_table_should_not_read_the_next_section(self) -> None:
        text = "## Groups\n\nNone yet.\n\n## Other\n\n| A | B |\n| --- | --- |\n| G1 | x |\n"
        with self.assertRaises(ValueError):
            ledger.set_group(text, "G1", "done")

    def test_a_row_with_fewer_cells_than_the_header_should_be_an_error(self) -> None:
        text = GROUPS.replace("| G2 | #2 | waiting |", "| G2 | #2 |")
        with self.assertRaises(ValueError):
            ledger.set_group(text, "G2", "done")

    def test_a_state_with_a_bar_should_be_an_error(self) -> None:
        with self.assertRaises(ValueError):
            ledger.set_group(GROUPS, "G1", "a | b")

    def test_a_state_with_a_line_break_should_be_an_error(self) -> None:
        with self.assertRaises(ValueError):
            ledger.set_group(GROUPS, "G1", "a\nb")

    def test_a_state_with_any_break_of_splitlines_should_be_an_error(self) -> None:
        for state in ("a\rb", "a\u2028b"):
            with self.subTest(state=state), self.assertRaises(ValueError):
                ledger.set_group(GROUPS, "G1", state)

    def test_a_group_line_with_no_state_should_print_the_usage(self) -> None:
        with tempfile.TemporaryDirectory() as d, redirect_stderr(StringIO()) as err:
            path = Path(d) / "progress.md"
            path.write_text(GROUPS, encoding="utf-8")
            self.assertEqual(ledger.main([str(path), "group", "G2"]), 2)
            self.assertEqual(path.read_text(encoding="utf-8"), GROUPS)
        self.assertIn("group <group>", err.getvalue())

    def test_main_should_write_the_new_state_to_the_file(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "progress.md"
            path.write_text(GROUPS, encoding="utf-8")
            self.assertEqual(ledger.main([str(path), "group", "G2", "final review"]), 0)
            self.assertIn("| G2 | #2 | final review |", path.read_text(encoding="utf-8"))

    def test_subcommand_first_order_should_edit_the_ledger(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "progress.md"
            path.write_text(LEDGER + "\n" + GROUPS, encoding="utf-8")
            self.assertEqual(ledger.main(["event", str(path), "pushed branch"]), 0)
            self.assertEqual(ledger.main(["after-merge", str(path), "deferred item"]), 0)
            self.assertEqual(ledger.main(["group", str(path), "G1", "done"]), 0)
            text = path.read_text(encoding="utf-8")
        self.assertIn("pushed branch\n", text)
        self.assertIn("- deferred item\n", text)
        self.assertIn("| G1 | #1 | done |", text)

    def test_a_missing_ledger_file_should_return_exit_2_with_an_error(self) -> None:
        with tempfile.TemporaryDirectory() as d, redirect_stderr(StringIO()) as err:
            missing = Path(d) / "missing.md"
            self.assertEqual(ledger.main(["event", str(missing), "note"]), 2)
        self.assertIn("ledger.py:", err.getvalue())

    def test_an_event_on_a_read_only_ledger_should_return_exit_2(self) -> None:
        with tempfile.TemporaryDirectory() as d, redirect_stderr(StringIO()) as err:
            path = ledger.init_batch(Path(d), "issues-x", NOW)
            path.chmod(0o444)
            try:
                self.assertEqual(ledger.main([str(path), "event", "started"]), 2)
            finally:
                path.chmod(0o644)
        self.assertIn("ledger.py:", err.getvalue())


class InitBatch(unittest.TestCase):
    def test_init_should_reserve_unique_folders_and_write_skeleton(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            scratch = Path(d)
            first = ledger.init_batch(scratch, "issues-2026-11-26", NOW)
            second = ledger.init_batch(scratch, "issues-2026-11-26", NOW)
            self.assertEqual(first, scratch / "issues-2026-11-26" / "progress.md")
            self.assertEqual(second, scratch / "issues-2026-11-26b" / "progress.md")
            text = second.read_text(encoding="utf-8")
            self.assertIn("# Batch issues-2026-11-26b", text)
            self.assertIn("## Groups", text)
            self.assertIn("## Events", text)
            self.assertEqual(ledger.main([str(second), "event", "started"]), 0)

    def test_init_with_invalid_prefix_should_fail(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            for prefix in ("bad/prefix", " ", "\t", ".."):
                with self.subTest(prefix=prefix), redirect_stderr(StringIO()) as err:
                    self.assertEqual(ledger.main(["init", d, prefix]), 2)
                self.assertIn("invalid batch prefix", err.getvalue())

    def test_init_on_an_existing_file_should_return_exit_2(self) -> None:
        with tempfile.TemporaryDirectory() as d, redirect_stderr(StringIO()) as err:
            blocker = Path(d) / "not-a-dir"
            blocker.write_text("x", encoding="utf-8")
            self.assertEqual(ledger.main(["init", str(blocker), "issues-2026-11-26"]), 2)
        self.assertIn("ledger.py:", err.getvalue())

    def test_init_template_should_match_ledger_md_headings_and_tables(self) -> None:
        doc = (SCRIPT.parents[1] / "ledger.md").read_text(encoding="utf-8")
        doc_lines = [
            line for line in doc.splitlines() if line.startswith(("## ", "| Group |", "| Issue |"))
        ]
        tpl_lines = [
            line
            for line in ledger.TEMPLATE.splitlines()
            if line.startswith(("## ", "| Group |", "| Issue |"))
        ]
        self.assertEqual(tpl_lines, doc_lines)
        self.assertIn("Started {date} on origin/main <sha>. Build:", ledger.TEMPLATE)

    def test_main_init_should_print_the_created_path(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            out = StringIO()
            with unittest.mock.patch("sys.stdout", out):
                code = ledger.main(["init", d, "issues-2026-11-26"])
            self.assertEqual(code, 0)
            created = Path(out.getvalue().strip())
            self.assertTrue(created.is_file())


if __name__ == "__main__":
    unittest.main()
