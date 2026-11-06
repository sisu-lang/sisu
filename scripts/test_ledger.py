"""Unit tests for solve-issues' ledger.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""

import importlib.util
import unittest
from datetime import datetime
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / ".agents/skills/solve-issues/scripts/ledger.py"
spec = importlib.util.spec_from_file_location("ledger", SCRIPT)
ledger = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ledger)

NOW = datetime(2026, 11, 3, 14, 5)
LEDGER = """# Batch b

## After the merge

- old: repro.

## Events

- 2026-11-02 10:00 started.
"""


class Add(unittest.TestCase):
    def test_event_should_go_at_the_end_of_events(self):
        out = ledger.add(LEDGER, "event", "merged #1.", NOW)
        self.assertTrue(
            out.endswith("- 2026-11-02 10:00 started.\n- 2026-11-03 14:05 merged #1.\n")
        )

    def test_after_merge_should_go_after_the_last_line_of_its_section(self):
        out = ledger.add(LEDGER, "after-merge", "new: repro.", NOW)
        self.assertIn("- old: repro.\n- new: repro.\n\n## Events", out)

    def test_after_merge_should_fill_an_empty_section(self):
        out = ledger.add("## After the merge\n\n## Events\n", "after-merge", "x.", NOW)
        self.assertEqual(out, "## After the merge\n- x.\n\n## Events\n")

    def test_a_missing_section_should_be_an_error(self):
        with self.assertRaises(ValueError):
            ledger.add("# Batch b\n", "event", "x.", NOW)

    def test_an_unknown_kind_should_be_an_error(self):
        with self.assertRaises(ValueError):
            ledger.add(LEDGER, "note", "x.", NOW)


if __name__ == "__main__":
    unittest.main()
