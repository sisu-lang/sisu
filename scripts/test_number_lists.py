"""Unit tests for audit-sisu's number_lists.py.

Run: python3 -m unittest discover -s scripts -p 'test_*.py'.
"""

import importlib.util
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / ".agents/skills/audit-sisu/scripts/number_lists.py"
SPEC = importlib.util.spec_from_file_location("number_lists", SCRIPT)
assert SPEC is not None
assert SPEC.loader is not None
number_lists = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(number_lists)


class NumberLists(unittest.TestCase):
    def test_a_list_over_several_lines_should_count_once_at_its_first_line(self) -> None:
        text = "const SEEDS: &[u8] = &[\n1,\n];\nfn f() {\n    Type::I64\n    | Type::U8 => 1,\n}\n"
        self.assertEqual(list(number_lists.lists(text)), [(5, "Type::I64 | Type::U8")])

    def test_a_list_of_every_leaf_kind_should_not_count(self) -> None:
        text = "Type::I64 | Type::U8 | Type::F64 | Type::Unit => true,\n"
        self.assertEqual(list(number_lists.lists(text)), [])

    def test_a_list_in_a_comment_or_a_test_module_should_not_count(self) -> None:
        text = "// Type::I64 | Type::U8\n#[cfg(test)]\nmod tests {\n    Type::I64 | Type::F64\n}\n"
        self.assertEqual(list(number_lists.lists(text)), [])

    def test_a_list_with_one_number_type_should_not_count(self) -> None:
        text = "Type::I64 | Type::Bool => true,\n"
        self.assertEqual(list(number_lists.lists(text)), [])


if __name__ == "__main__":
    unittest.main()
