"""Unit tests for solve-issues' ledger.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""

import importlib.util
import tempfile
import unittest
from datetime import UTC, datetime
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

    def test_a_state_with_a_bar_should_be_an_error(self) -> None:
        with self.assertRaises(ValueError):
            ledger.set_group(GROUPS, "G1", "a | b")

    def test_main_should_write_the_new_state_to_the_file(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "progress.md"
            path.write_text(GROUPS, encoding="utf-8")
            self.assertEqual(ledger.main([str(path), "group", "G2", "final review"]), 0)
            self.assertIn("| G2 | #2 | final review |", path.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
