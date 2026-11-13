"""Text tests for the Test job of the CI workflow.

Run them with `python3 -m unittest discover -s scripts -p 'test_*.py'`. They reuse the
indentation parser of test_nightly_workflow.py and use no YAML library.
"""

import unittest

from test_nightly_workflow import GATE, ROOT, flow_list, job_map, jobs, part, run_lines, steps

CI = ROOT / ".github" / "workflows" / "ci.yml"


class CiTestJobTests(unittest.TestCase):
    def job(self, name: str) -> list[str]:
        found = jobs(self, CI)
        if name not in found:
            self.fail(f"no {name} job in {CI}")
        return found[name]

    def test_the_parts_should_cover_each_slice_once(self) -> None:
        job = self.job("test")
        strategy = job_map(job).get("strategy", {})
        parts = flow_list(str(strategy.get("matrix", {}).get("part", "")))
        self.assertEqual(strategy.get("fail-fast"), "false")
        self.assertEqual(parts, [str(n) for n in range(1, len(parts) + 1)])
        self.assertGreater(len(parts), 1)
        partitions = [
            line for step in steps(job) for line in run_lines(step) if "--partition" in line
        ]
        self.assertEqual(len(partitions), 1)
        self.assertIn(f"--partition slice:${{{{ matrix.part }}}}/{len(parts)}", partitions[0])

    def test_the_test_check_should_fail_unless_every_part_passes(self) -> None:
        job = self.job("test-result")
        keys = job_map(job)
        self.assertEqual(keys.get("name"), "Test")
        self.assertEqual(keys.get("needs"), "test")
        self.assertEqual(keys.get("if"), "always()")
        result_steps = steps(job)
        self.assertEqual(len(result_steps), 1)
        step = result_steps[0]
        self.assertEqual(part(step, "env").get("RESULT"), "${{ needs.test.result }}")
        self.assertNotIn("if", step)
        self.assertEqual(run_lines(step), ['test "$RESULT" = success'])
        self.assertIsNone(GATE.search("\n".join(job)))

    def test_whole_list_checks_should_run_in_part_one_only(self) -> None:
        ran = "(success() || failure()) && steps.nextest.outcome"
        expected = {
            "scripts/test_names.py": f"{ran} != 'skipped' && github.event_name == 'pull_request'"
            " && matrix.part == 1",
            "scripts/nextest_filters.py": f"{ran} == 'success' && matrix.part == 1",
        }
        for script, condition in expected.items():
            found = [
                step
                for step in steps(self.job("test"))
                if any(script in line for line in run_lines(step))
            ]
            self.assertEqual(len(found), 1, script)
            self.assertEqual(found[0].get("if"), condition, script)


if __name__ == "__main__":
    unittest.main()
