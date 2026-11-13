"""Unit tests for coverage_report.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""

import contextlib
import io
import json
import tempfile
import unittest
from collections.abc import Callable
from pathlib import Path
from typing import Any

import coverage_report as cr

ROOT = "/r"
TOTALS = {"lines": {"percent": 87.34}, "regions": {"percent": 85.06}}


def func(
    name: str,
    regions: list[tuple[int, ...]],
    file: str = "/r/a.rs",
    count: int | None = None,
    files: list[str] | None = None,
) -> dict[str, Any]:
    """A function record; each region is (l1, c1, l2, c2, count[, file_id[, kind]])."""
    return {
        "name": name,
        "filenames": files or [file],
        "count": regions[0][4] if count is None else count,
        "regions": [
            [*r[:5], r[5] if len(r) > 5 else 0, 0, r[6] if len(r) > 6 else 0] for r in regions
        ],
    }


def export(*functions: dict[str, Any]) -> dict[str, Any]:
    return {"data": [{"totals": TOTALS, "functions": list(functions)}]}


def diff(path: str = "a.rs", start: int = 1, count: int = 1) -> str:
    body = "".join("+x\n" for _ in range(count))
    return f"diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -0,0 +{start},{count} @@\n{body}"


def lcov(**lines: dict[int, int]) -> str:
    """lcov({"a.rs": {1: 1, 2: 0}}) as text, rooted at /r."""
    return "".join(
        f"SF:/r/{p}\n" + "".join(f"DA:{n},{h}\n" for n, h in da.items()) + "end_of_record\n"
        for p, da in lines.items()
    )


def branch_lcov(path: str, **lines: list[int | None]) -> str:
    """branch_lcov("a.rs", n3=[1, 0]) gives BRDA lines for line 3; None is a `-` count."""
    out = f"SF:/r/{path}\n"
    for key, counts in lines.items():
        for i, c in enumerate(counts):
            out += f"BRDA:{key.removeprefix('n')},0,{i},{'-' if c is None else c}\n"
    return out + "end_of_record\n"


def branch_export(count: int = 10, percent: float = 94.26) -> dict[str, Any]:
    totals = {**TOTALS, "branches": {"count": count, "percent": percent}}
    return {"data": [{"totals": totals, "functions": []}]}


def run(
    exp: dict[str, Any],
    lc: str = "",
    df: str = "",
    read: Callable[[str, int], str] = lambda _p, n: f"line {n}",
) -> str:
    return cr.report(exp, lc, df, ROOT, read)


def sections(text: str) -> tuple[str, list[str], list[str]]:
    head, touching, other, _tail = text.split("\n\n")
    return head, touching.splitlines()[1:], other.splitlines()[1:]


class ChangedLines(unittest.TestCase):
    def test_changed_lines_takes_added_lines_of_the_new_side(self) -> None:
        d = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -3,2 +3,3 @@\n+a\n+b\n+c\n"
        self.assertEqual(cr.changed_lines(d), {"a.rs": {3, 4, 5}})

    def test_a_plus_plus_plus_line_inside_a_hunk_is_added_text(self) -> None:
        d = (
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,0 +1,3 @@\n"
            "+x\n+++ b/fake.rs\n+y\n"
        )
        self.assertEqual(cr.changed_lines(d), {"a.rs": {1, 2, 3}})

    def test_changed_lines_follows_u0_hunks_across_files(self) -> None:
        d = (
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -2 +1,0 @@\n-x\n@@ -5,0 +5 @@\n+y\n"
            "diff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-p\n+q\n"
        )
        self.assertEqual(cr.changed_lines(d), {"a.rs": {5}, "b.rs": {1}})

    def test_changed_lines_skips_deleted_test_and_non_rust_files(self) -> None:
        d = (
            "diff --git a/b.rs b/b.rs\ndeleted file mode 100644\n--- a/b.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n"
            "diff --git a/crates/sisuc/tests/x.rs b/crates/sisuc/tests/x.rs\n--- a/crates/sisuc/tests/x.rs\n+++ b/crates/sisuc/tests/x.rs\n@@ -0,0 +1 @@\n+x\n"
            "diff --git a/docs/x.md b/docs/x.md\n--- a/docs/x.md\n+++ b/docs/x.md\n@@ -0,0 +1 @@\n+x\n"
            "diff --git a/c.rs b/d.rs\nsimilarity index 100%\nrename from c.rs\nrename to d.rs\n"
        )
        self.assertEqual(cr.changed_lines(d), {})

    def test_changed_lines_skips_unit_test_modules_in_their_own_file(self) -> None:
        def added(path: str) -> str:
            return f"diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -0,0 +1 @@\n+x\n"

        d = "".join(
            added(p)
            for p in (
                "crates/sisuc/src/codegen/tests.rs",
                "crates/sisuc/src/mir/tests/certify.rs",
                "crates/sisuc/src/mir/certify.rs",
                "crates/sisuc/src/mir/tests_helper.rs",
            )
        )
        self.assertEqual(
            cr.changed_lines(d),
            {"crates/sisuc/src/mir/certify.rs": {1}, "crates/sisuc/src/mir/tests_helper.rs": {1}},
        )


class Report(unittest.TestCase):
    def test_a_multiline_region_touching_one_changed_line_is_listed_first(self) -> None:
        exp = export(func("f", [(1, 1, 30, 1, 1), (10, 5, 14, 6, 0), (20, 1, 21, 2, 0)]))
        _, touching, other = sections(run(exp, df=diff(start=12)))
        self.assertEqual(len(touching), 1)
        self.assertTrue(touching[0].startswith("a.rs:10:5-14:6: "))
        self.assertEqual(len(other), 1)
        self.assertTrue(other[0].startswith("a.rs:20:1-21:2: "))

    def test_identical_regions_take_the_highest_count(self) -> None:
        a = func("_RA", [(1, 1, 9, 1, 3), (5, 1, 6, 1, 0)])
        b = func("_RB", [(1, 1, 9, 1, 3), (5, 1, 6, 1, 3)])
        for order in ((a, b), (b, a)):
            _, touching, other = sections(run(export(*order), df=diff(start=5)))
            self.assertEqual((touching, other), (["(none)"], ["(none)"]))

    def test_zero_count_regions_of_other_kinds_are_not_listed(self) -> None:
        exp = export(func("f", [(1, 1, 9, 1, 1), (3, 1, 4, 1, 0, 0, 3)]))
        self.assertEqual(cr.uncovered(exp, ROOT), {})

    def test_a_never_run_function_is_one_line(self) -> None:
        exp = export(func("f", [(10, 1, 14, 2, 0), (11, 1, 11, 5, 0), (12, 1, 12, 5, 0)]))
        out = run(exp, df=diff(start=11))
        self.assertEqual(sections(out)[1], ["a.rs:10: never run: line 10"])

    def test_a_never_run_function_touches_changes_anywhere_in_its_body(self) -> None:
        # The head region is one line; the change is in a later region of the body.
        exp = export(func("f", [(10, 1, 10, 20, 0), (11, 1, 14, 2, 0)]))
        self.assertEqual(sections(run(exp, df=diff(start=13)))[1], ["a.rs:10: never run: line 10"])
        _, touching, other = sections(run(exp, df=diff(start=15)))
        self.assertEqual((touching, other), (["(none)"], ["a.rs:10: never run: line 10"]))

    def test_never_run_uses_the_function_count_not_the_first_region(self) -> None:
        exp = export(func("f", [(10, 1, 14, 2, 0), (11, 1, 11, 5, 0)], count=2))
        out = run(exp, df=diff(start=11))
        self.assertNotIn("never run:", out)
        self.assertEqual(sections(out)[1], ["a.rs:10:1-14:2: line 10", "a.rs:11:1-11:5: line 11"])

    def test_the_function_count_is_the_highest_across_instantiations(self) -> None:
        a = func("_RA", [(10, 1, 14, 2, 0), (11, 1, 11, 5, 0)], count=0)
        b = func("_RB", [(10, 1, 14, 2, 0), (11, 1, 11, 5, 0)], count=3)
        for order in ((a, b), (b, a)):
            self.assertNotIn("never run:", run(export(*order), df=diff(start=11)))

    def test_regions_resolve_their_own_file(self) -> None:
        exp = export(
            func(
                "f",
                [(1, 1, 9, 1, 1), (3, 1, 4, 1, 0, 1), (5, 1, 6, 1, 0, 2)],
                files=["/r/a.rs", "/r/b.rs", "/rustc/x/lib.rs"],
            )
        )
        self.assertEqual(
            cr.uncovered(exp, ROOT), {"b.rs": [cr.Region(3, 1, 4, 1, never_run=False)]}
        )

    def test_zero_regions_in_unchanged_files_are_not_listed(self) -> None:
        exp = export(func("f", [(1, 1, 9, 1, 1), (3, 1, 4, 1, 0)], file="/r/b.rs"))
        _, touching, other = sections(run(exp, df=diff(path="a.rs", start=3)))
        self.assertEqual((touching, other), (["(none)"], ["(none)"]))

    def test_an_invariant_panic_touching_changes_is_advisory(self) -> None:
        exp = export(
            func("f", [(1, 1, 9, 1, 1), (3, 5, 3, 17, 0), (4, 5, 4, 11, 0), (5, 5, 5, 9, 0)])
        )
        lines = {
            3: '    unreachable!("x should be y")',
            4: '    _ => panic!("z should hold"),',
            5: "    0",
        }
        _, touching, other = sections(
            run(exp, df=diff(start=3, count=3), read=lambda _p, n: lines.get(n, ""))
        )
        self.assertEqual([t.split(": ")[0] for t in touching], ["a.rs:5:5-5:9"])
        self.assertEqual([t.split(": ")[0] for t in other], ["a.rs:3:5-3:17", "a.rs:4:5-4:11"])

    def test_the_message_of_an_assert_that_ran_is_advisory(self) -> None:
        exp = export(
            func("f", [(1, 1, 9, 1, 1), (3, 30, 3, 36, 0), (4, 30, 4, 36, 0), (5, 5, 5, 9, 0)])
        )
        lines = {
            3: '    assert!(n <= 4, "{} steps", n.get());',
            4: '    debug_assert_eq!(a, b, "{}", c);',
            5: "    0",
        }
        # Line 4 never ran, so its assert did not run, and its region still counts.
        lc = lcov(**{"a.rs": {3: 1, 4: 0, 5: 0}})
        _, touching, other = sections(
            run(exp, lc, diff(start=3, count=3), read=lambda _p, n: lines.get(n, ""))
        )
        self.assertEqual([t.split(": ")[0] for t in touching], ["a.rs:4:30-4:36", "a.rs:5:5-5:9"])
        self.assertEqual([t.split(": ")[0] for t in other], ["a.rs:3:30-3:36"])

    def test_a_panic_or_an_assert_in_a_string_should_not_move_a_region_out_of_the_list(
        self,
    ) -> None:
        exp = export(func("f", [(1, 1, 9, 1, 1), (3, 5, 3, 30, 0), (4, 5, 4, 30, 0)]))
        lines = {3: '    if a { f("panic!(x)") }', 4: '    if a { f("assert!(x)") }'}
        lc = lcov(**{"a.rs": {3: 1, 4: 1}})
        _, touching, other = sections(
            run(exp, lc, diff(start=3, count=2), read=lambda _p, n: lines[n])
        )
        self.assertEqual([t.split(": ")[0] for t in touching], ["a.rs:3:5-3:30", "a.rs:4:5-4:30"])
        self.assertEqual(other, ["(none)"])

    def test_changed_lines_coverage_counts_only_instrumented_lines(self) -> None:
        out = run(export(), lcov(**{"a.rs": {1: 1, 2: 1, 3: 0}}), diff(start=1, count=4))
        self.assertIn("changed lines 66.7% (2 of 3).", out.splitlines()[0])

    def test_the_report_should_end_with_its_figures(self) -> None:
        out = run(export(), lcov(**{"a.rs": {1: 1, 2: 1, 3: 0}}), diff(start=1, count=4))
        self.assertEqual(out.splitlines()[-1], out.splitlines()[0])
        self.assertIn("changed lines 66.7% (2 of 3).", out.splitlines()[-1])

    def test_no_changed_rust_line_says_none(self) -> None:
        head, touching, other = sections(run(export(func("f", [(1, 1, 2, 1, 1)]))))
        self.assertEqual((touching, other), (["(none)"], ["(none)"]))
        self.assertEqual(head, "Coverage: lines 87.3%, regions 85.1%, changed lines: none.")

    def test_paths_outside_the_root_are_dropped(self) -> None:
        exp = export(
            func("f", [(1, 1, 5, 1, 1), (3, 1, 4, 1, 0)], file="/rustc/abc/lib.rs"),
            func("g", [(1, 1, 5, 1, 0)], file="/rx/lib.rs"),
        )
        self.assertEqual(cr.uncovered(exp, ROOT), {})

    def test_instantiations_with_different_names_are_one_function(self) -> None:
        spans = [(1, 1, 9, 1), (3, 1, 4, 1), (6, 1, 7, 1)]
        a = func("_RNvA", [(*s, 0) for s in spans])
        b = func("_RNvB", [(*spans[0], 4), (*spans[1], 4), (*spans[2], 0)])
        out = run(export(a, b), df=diff(start=6))
        self.assertNotIn("never run:", out)
        _, touching, other = sections(out)
        self.assertEqual(len(touching), 1)
        self.assertTrue(touching[0].startswith("a.rs:6:1-7:1: "))
        self.assertEqual(other, ["(none)"])

    def test_long_source_lines_are_cut_at_80_characters(self) -> None:
        exp = export(func("f", [(1, 1, 9, 1, 1), (3, 1, 4, 1, 0)]))
        out = run(exp, df=diff(start=3), read=lambda _p, _n: "  " + "x" * 100)
        self.assertIn(": " + "x" * 80 + "\n", out + "\n")


class Branches(unittest.TestCase):
    def test_the_figures_should_name_branches_when_branches_were_measured(self) -> None:
        out = run(branch_export())
        self.assertEqual(
            out.splitlines()[0],
            "Coverage: lines 87.3%, regions 85.1%, branches 94.3%, changed lines: none.",
        )

    def test_the_figures_should_leave_out_branches_when_none_were_measured(self) -> None:
        out = run(branch_export(count=0, percent=0.0))
        self.assertNotIn("branches", out.splitlines()[0])

    def test_a_changed_line_with_a_missed_branch_should_be_listed(self) -> None:
        lc = branch_lcov("a.rs", n3=[1, 0], n4=[None, 2, 0], n5=[1, 1], n9=[0, 0])
        out = run(branch_export(), lc, diff(start=3, count=3))
        self.assertIn(
            "\n\nMissed branches on changed lines:\na.rs:3: missed 1 of 2: line 3\n"
            "a.rs:4: missed 2 of 3: line 4\n\n",
            out,
        )
        self.assertNotIn("a.rs:5:", out)  # all branches ran
        self.assertNotIn("a.rs:9:", out)  # the line did not change

    def test_the_branch_list_should_say_none_when_no_changed_line_misses_a_branch(self) -> None:
        out = run(branch_export(), branch_lcov("a.rs", n3=[1, 1], n9=[0, 1]), diff(start=3))
        self.assertIn("Missed branches on changed lines:\n(none)\n\n", out)

    def test_the_branch_list_should_leave_out_assert_and_invariant_lines(self) -> None:
        lines = {
            3: '    assert!(n <= 4, "{}", n.get());',
            4: '    _ => unreachable!("x should be y"),',
            5: "    if a {",
        }
        lc = branch_lcov("a.rs", n3=[1, 0], n4=[1, 0], n5=[1, 0])
        out = run(branch_export(), lc, diff(start=3, count=3), read=lambda _p, n: lines.get(n, ""))
        self.assertIn("Missed branches on changed lines:\na.rs:5: missed 1 of 2: if a {\n\n", out)

    def test_the_branch_list_should_cut_long_source_lines_at_80_characters(self) -> None:
        out = run(
            branch_export(),
            branch_lcov("a.rs", n3=[1, 0]),
            diff(start=3),
            read=lambda _p, _n: "  " + "x" * 100,
        )
        self.assertIn(": " + "x" * 80 + "\n", out)

    def test_a_line_only_run_should_have_no_branch_list(self) -> None:
        out = run(export(), branch_lcov("a.rs", n3=[1, 0]), diff(start=3))
        self.assertNotIn("Missed branches", out)

    def test_the_report_should_end_with_its_figures_after_the_branch_list(self) -> None:
        out = run(branch_export(), branch_lcov("a.rs", n3=[1, 0]), diff(start=3))
        lines = out.splitlines()
        self.assertEqual(lines[-1], lines[0])
        self.assertLess(out.index("Missed branches"), out.rindex("Coverage:"))


def source(*lines: str) -> Callable[[str, int], str]:
    """A line reader over `lines`, numbered from 1; a line outside them is empty."""
    return lambda _path, n: lines[n - 1] if 0 < n <= len(lines) else ""


def listed(*lines: str, branches: list[int]) -> list[int]:
    """The lines of `lines` that the branch list names, when each line of `branches` misses one."""
    missed: list[int | None] = [1, 0]
    lc = branch_lcov("a.rs", **{f"n{n}": missed for n in branches})
    out = run(branch_export(), lc, diff(start=1, count=len(lines)), read=source(*lines))
    section = out.split("Missed branches on changed lines:\n")[1].split("\n\n")[0]
    return [int(x.split(":")[1]) for x in section.splitlines() if x != "(none)"]


class BranchNoise(unittest.TestCase):
    """Lines whose branches no test should take, or that no test of the product measures."""

    def test_a_let_else_before_an_invariant_panic_should_be_left_out(self) -> None:
        lines = (
            "let Ok(x) = y else {",
            '    unreachable!("y should be ok")',
            "};",
            "let Some(z) = y else {",
            "    return None;",
            "};",
            'let Ok(x) = y else { panic!("y should be ok") };',
        )
        self.assertEqual(listed(*lines, branches=[1, 4, 7]), [4])

    def test_the_lines_of_a_multiline_assert_should_be_left_out(self) -> None:
        lines = (
            "let a = 1;",
            "assert!(",
            "    a.map(|x| if x { 1 } else { 2 }) == b,",
            '    "message {}",',
            "    c.map(|y| if y { 1 } else { 2 })",
            ");",
            "foo(",
            "    if a { 1 } else { 2 },",
            ");",
            "if a {",
            "    debug_assert_eq!(",
            "        a,",
            "        if b { 1 } else { 2 },",
            "    );",
            "}",
        )
        self.assertEqual(listed(*lines, branches=[3, 5, 8, 10, 13]), [8, 10])

    def test_a_one_line_assert_after_a_closed_statement_should_not_hide_the_next_line(self) -> None:
        lines = ("assert!(a);", "if b {", "}")
        self.assertEqual(listed(*lines, branches=[2]), [2])

    def test_an_inline_test_module_should_be_left_out(self) -> None:
        lines = (
            "fn f() { if a {} }",
            "#[cfg(test)]",
            "#[allow(clippy::x)]",
            "mod tests {",
            '    const S: &str = "{";',
            "    mod inner {",
            "        fn g() { if b {} }",
            "    }",
            "    fn h() { if c {} }",
            "}",
            "fn k() { if d {} }",
        )
        self.assertEqual(listed(*lines, branches=[1, 7, 9, 11]), [1, 11])

    def test_a_pub_crate_inline_test_module_should_be_left_out(self) -> None:
        lines = (
            "#[cfg(test)]",
            "pub(crate) mod tests {",
            "    fn h() { if c {} }",
            "}",
            "fn k() { if d {} }",
        )
        self.assertEqual(listed(*lines, branches=[3, 5]), [5])

    def test_braces_in_raw_strings_and_char_literals_should_not_end_a_test_module_wrongly(
        self,
    ) -> None:
        lines = (
            "#[cfg(test)]",
            "mod tests {",
            '    const S: &str = r#"{ "a": {"#;',
            '    const R: &str = r#""}""#;',
            "    const C: char = '{';",
            "    const Q: char = '\\'';",
            "    fn f<'a>(x: &'a str) { if b {} }",
            '    const T: &str = r"}";',
            "}",
            "fn k() { if d {} }",
        )
        self.assertEqual(listed(*lines, branches=[7, 10]), [10])

    def test_a_comment_after_an_assert_should_not_hide_the_next_line(self) -> None:
        lines = ("debug_assert!(n > 0); // why", "if n > 1 {", "}")
        self.assertEqual(listed(*lines, branches=[2]), [2])

    def test_a_comment_that_names_an_assert_should_not_hide_the_next_line(self) -> None:
        lines = ("// see assert!(x)", "if a && b {", "}")
        self.assertEqual(listed(*lines, branches=[2]), [2])

    def test_a_comment_that_names_an_invariant_should_not_hide_its_own_line(self) -> None:
        lines = (
            "if a { // not unreachable!(x)",
            "}",
            "let Some(z) = y else { // x",
            "    return;",
            "let Some(x) = y else {",
            "    return; // not panic!(x)",
        )
        self.assertEqual(listed(*lines, branches=[1, 3, 5]), [1, 3, 5])

    def test_comments_inside_a_multiline_assert_should_not_end_the_walk(self) -> None:
        lines = (
            "assert!( // why",
            "    a, // and why",
            "    // why",
            "    if b { 1 } else { 2 }, // more",
            ");",
        )
        self.assertEqual(listed(*lines, branches=[4]), [])

    def test_an_assert_that_closed_on_an_earlier_line_should_not_hide_the_next_line(self) -> None:
        lines = (
            "match x {",
            "    A => assert!(ready),",
            "    B if left && right => run(),",
            "}",
            "foo(",
            "    xs.iter().for_each(|x| assert!(f(x))),",
            "    if a { 1 } else { 2 },",
            ");",
        )
        self.assertEqual(listed(*lines, branches=[3, 7]), [3, 7])

    def test_a_closed_assert_inside_an_open_assert_should_not_end_the_walk(self) -> None:
        lines = (
            "assert!(",
            "    { assert!(ready); ready },",
            '    "{}",',
            "    if verbose { 1 } else { 2 },",
            ");",
        )
        self.assertEqual(listed(*lines, branches=[4]), [])

    def test_a_block_comment_that_names_an_invariant_should_not_hide_its_own_line(self) -> None:
        lines = ("if ready /* panic!( */ {", "}", "let x = a /* b */ && c;")
        self.assertEqual(listed(*lines, branches=[1, 3]), [1, 3])

    def test_a_blank_line_should_end_the_walk_back_from_an_assert(self) -> None:
        lines = ("assert!(a,", "", "if b {", "}")
        self.assertEqual(listed(*lines, branches=[3]), [3])

    def test_a_cfg_test_item_that_is_not_a_module_should_stay(self) -> None:
        lines = (
            "#[cfg(test)]",
            "fn t() { if a {} }",
            "#[cfg(test)]",
            "mod tests;",
            "fn k() { if d {} }",
            "#[cfg(not(test))]",
            "mod real {",
            "    fn m() { if e {} }",
            "}",
        )
        self.assertEqual(listed(*lines, branches=[2, 5, 8]), [2, 5, 8])


class Main(unittest.TestCase):
    def call(self, exp_text: str, lcov_text: str = "", diff_text: str = "") -> tuple[int, str]:
        with tempfile.TemporaryDirectory() as d:
            paths: list[str] = []
            for name, text in (("c.json", exp_text), ("l.info", lcov_text), ("d.diff", diff_text)):
                file = Path(d, name)
                paths.append(str(file))
                with file.open("w") as f:
                    f.write(text)
            return self.run_main(paths)

    def run_main(self, paths: list[str]) -> tuple[int, str]:
        err = io.StringIO()
        with contextlib.redirect_stderr(err), contextlib.redirect_stdout(io.StringIO()):
            code = cr.main(paths)
        return code, err.getvalue()

    def test_a_missing_input_exits_1(self) -> None:
        code, err = self.run_main(["/nonexistent/c.json", "/nonexistent/l", "/nonexistent/d"])
        self.assertEqual(code, 1)
        self.assertTrue(err.startswith("coverage_report:"))

    def test_malformed_inputs_exit_1_without_a_traceback(self) -> None:
        good = json.dumps(export(func("f", [(1, 1, 2, 1, 1)])))
        bad_func = json.dumps({"data": [{"totals": TOTALS, "functions": [{"count": 1}]}]})
        bad_pct = json.dumps(
            {
                "data": [
                    {
                        "totals": {"lines": {"percent": "x"}, "regions": {"percent": 1}},
                        "functions": [],
                    }
                ]
            }
        )
        for args in (
            (bad_func,),
            (bad_pct,),
            (json.dumps({"data": [{}]}),),
            (good, "SF:/r/a.rs\nDA:1,x\n", diff()),
            ("{", "", ""),
        ):
            code, err = self.call(*args)
            self.assertEqual(code, 1, args)
            self.assertRegex(err, r"^coverage_report: [^\n]+\n$")

    def test_a_wrong_argument_count_exits_1(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            path = Path(d, "f")
            with path.open("w") as f:
                f.write(json.dumps(export()))
            for count in (2, 4):
                code, err = self.run_main([str(path)] * count)
                self.assertEqual(code, 1)
                self.assertIn("usage: coverage_report.py", err)

    def test_a_valid_export_exits_0(self) -> None:
        code, _ = self.call(json.dumps(export()))
        self.assertEqual(code, 0)


if __name__ == "__main__":
    unittest.main()
