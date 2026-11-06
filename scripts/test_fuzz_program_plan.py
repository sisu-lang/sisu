"""Unit tests for fuzz_program_plan.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""

import json
import os
import re
import subprocess
import tempfile
import unittest
from unittest import mock

import fuzz_program_plan as fp


WORKFLOW = os.path.join(
    os.path.dirname(os.path.abspath(__file__)), "..", ".github", "workflows", "fuzz-program.yml"
)


def steps():
    """The workflow's steps as lists of lines. A step starts at a line of six spaces and `- `, and
    ends before the next one or at the first line with less indent (the next job)."""
    with open(WORKFLOW, encoding="utf-8") as f:
        lines = f.read().splitlines()
    found = []
    for line in lines:
        if line.startswith("      - "):
            found.append([])
        elif line.strip() and not line.startswith("      ") and found:
            found.append(None)
        if found and found[-1] is not None:
            found[-1].append(line)
    return [step for step in found if step is not None]


def step_named(name):
    """The step named `name` and the step after it."""
    found = steps()
    for i, step in enumerate(found):
        if any(re.fullmatch(rf"\s*(- )?name: {re.escape(name)}", line) for line in step):
            return step, found[i + 1]
    raise AssertionError(f"no step named {name}")


def field(step, key):
    """The value of `key:` on a line of the step that is not a comment."""
    values = [m[1] for line in step if (m := re.fullmatch(rf"\s*(?:- )?{key}: (.*)", line))]
    assert len(values) == 1, (key, values)
    return values[0]


def run_block(step):
    """The text of the step's `run: |` block without its indent."""
    start = step.index("        run: |") + 1
    block = []
    for line in step[start:]:
        if line.strip() and not line.startswith(" " * 10):
            break
        block.append(line[10:])
    return "\n".join(block) + "\n"


def pull_request(labels, action="synchronize", added=None):
    payload = {"action": action, "pull_request": {"labels": [{"name": n} for n in labels]}}
    if added is not None:
        payload["label"] = {"name": added}
    return payload


class Labels(unittest.TestCase):
    def plan(self, labels, **kw):
        return fp.plan("pull_request", pull_request(labels, **kw), {})

    def test_minutes_and_hours(self):
        self.assertEqual(self.plan(["fuzz-program 45m"]), (2700, True))
        self.assertEqual(self.plan(["bug", "fuzz-program 2h"]), (7200, True))
        # The bounds themselves are allowed.
        self.assertEqual(self.plan(["fuzz-program 1m"]), (60, True))
        self.assertEqual(self.plan(["fuzz-program 4h"]), (14400, True))

    def test_a_bare_label_fuzzes_two_hours(self):
        self.assertEqual(self.plan(["fuzz-program"]), (7200, True))

    def test_out_of_range_fails(self):
        for label in ("fuzz-program 0m", "fuzz-program 6h", "fuzz-program 241m"):
            with self.subTest(label), self.assertRaisesRegex(ValueError, "outside"):
                self.plan([label])

    def test_two_labels_fail_naming_both(self):
        with self.assertRaisesRegex(ValueError, "fuzz-program 45m, fuzz-program$"):
            self.plan(["fuzz-program 45m", "fuzz-program"])

    def test_unrelated_labels_run_nothing(self):
        for labels in (
            [],
            ["bug"],
            ["fuzz-program 45"],
            ["fuzz-program 2d"],
            ["xfuzz-program"],
            ["fuzz-program  2h"],
            ["fuzz-program 2h "],
        ):
            with self.subTest(labels):
                self.assertIsNone(self.plan(labels))

    def test_adding_another_label_runs_nothing(self):
        self.assertIsNone(self.plan(["fuzz-program", "bug"], action="labeled", added="bug"))
        self.assertEqual(
            self.plan(["fuzz-program 45m"], action="labeled", added="fuzz-program 45m"),
            (2700, True),
        )


class Events(unittest.TestCase):
    def test_nightly_run_should_get_65_minutes_of_fuzz_step_in_150(self):
        self.assertEqual(fp.fuzz_minutes(1800), 65)
        self.assertEqual(fp.minutes(1800, True), 150)

    def test_schedule_should_measure_coverage(self):
        self.assertEqual(fp.plan("schedule", {}, {}), (1800, True))

    def test_fuzz_step_should_have_time_for_the_corpus_pass_of_a_short_run(self):
        # 15 of build, 30 of corpus pass, 17 of crash replays and 3 to stop.
        self.assertEqual(fp.fuzz_minutes(60), 65)

    def test_the_longest_run_should_leave_time_for_gaps(self):
        self.assertLessEqual(fp.minutes(fp.MOST, True), 360)
        self.assertEqual(
            fp.minutes(fp.MOST, True) - fp.minutes(fp.MOST, False),
            fp.COVERAGE_MINUTES + fp.GAPS_MINUTES,
        )
        coverage, _ = step_named("Coverage")
        gaps, upload = step_named("Generator gaps")
        self.assertEqual(field(coverage, "timeout-minutes"), str(fp.COVERAGE_MINUTES))
        self.assertEqual(field(gaps, "timeout-minutes"), str(fp.GAPS_MINUTES))
        self.assertEqual(field(gaps, "continue-on-error"), "${{ github.event_name != 'schedule' }}")
        self.assertEqual(
            field(gaps, "if"),
            "${{ !cancelled() && steps.fuzz.outcome == 'success' && needs.plan.outputs.coverage == 'true' }}",
        )
        self.assertRegex(
            run_block(gaps),
            rf"timeout -k 10 {(fp.GAPS_MINUTES - 1) * 60} scripts/fuzz.sh "
            r"--target program --gaps",
        )
        self.assertIn("steps.gaps.outcome == 'failure'", field(upload, "if"))
        self.assertEqual(field(upload, "name"), "fuzz-program-failure")
        self.assertEqual(field(upload, "path"), "fuzz/artifacts/summary.md")

    def test_the_gaps_step_should_report_a_check_that_did_not_finish(self):
        gaps, _ = step_named("Generator gaps")
        section = "## Generator gaps\n\nNo new generator gap.\n"
        for label, fake, code, text in (
            ("exit 124, no section", "exit 124", 124, "did not finish: exit status 124"),
            ("exit 137, no section", "exit 137", 137, "did not finish: exit status 137"),
            (
                "exit 1, with section",
                f"printf '\\n{section}' >>fuzz/artifacts/summary.md; exit 1",
                1,
                None,
            ),
            ("exit 0, with section", f"printf '\\n{section}' >>fuzz/artifacts/summary.md", 0, None),
        ):
            with self.subTest(label), tempfile.TemporaryDirectory() as tmp:
                os.makedirs(os.path.join(tmp, "scripts"))
                os.makedirs(os.path.join(tmp, "fuzz", "artifacts"))
                fake_path = os.path.join(tmp, "scripts", "fuzz.sh")
                with open(fake_path, "w", encoding="utf-8") as f:
                    f.write(f"#!/bin/bash\n{fake}\n")
                os.chmod(fake_path, 0o755)
                summary = os.path.join(tmp, "fuzz", "artifacts", "summary.md")
                with open(summary, "w", encoding="utf-8") as f:
                    f.write("# Fuzzing: program\n\nNone.\n")
                step_summary = os.path.join(tmp, "step-summary.md")
                env = dict(os.environ, GITHUB_STEP_SUMMARY=step_summary)
                done = subprocess.run(
                    ["bash", "-e", "-c", run_block(gaps)],
                    cwd=tmp,
                    env=env,
                    capture_output=True,
                    text=True,
                    check=False,
                )
                with open(summary, encoding="utf-8") as f:
                    in_summary = f.read()
                with open(step_summary, encoding="utf-8") as f:
                    in_step = f.read()
                self.assertEqual(done.returncode, code, done.stderr)
                self.assertEqual(in_summary.count("## Generator gaps"), 1, in_summary)
                self.assertEqual(in_step.count("## Generator gaps"), 1, in_step)
                self.assertNotIn("None.", in_step)
                if text is None:
                    self.assertNotIn("did not finish", in_summary + in_step)
                else:
                    self.assertIn(text, in_summary)
                    self.assertIn(text, in_step)

    def test_dispatch_takes_its_inputs(self):
        env = {"FUZZ_SECONDS": "600", "FUZZ_COVERAGE": "true"}
        self.assertEqual(fp.plan("workflow_dispatch", {}, env), (600, True))
        with self.assertRaisesRegex(ValueError, "outside"):
            fp.plan("workflow_dispatch", {}, {"FUZZ_SECONDS": "30", "FUZZ_COVERAGE": "false"})

    def main(self, labels):
        """Runs main() on a pull_request event with `labels`; returns its status and output."""
        with tempfile.TemporaryDirectory() as tmp:
            event = os.path.join(tmp, "event.json")
            output = os.path.join(tmp, "output")
            with open(event, "w", encoding="utf-8") as f:
                json.dump(pull_request(labels), f)
            open(output, "w", encoding="utf-8").close()
            env = {
                "GITHUB_EVENT_NAME": "pull_request",
                "GITHUB_EVENT_PATH": event,
                "GITHUB_OUTPUT": output,
            }
            with mock.patch.dict(os.environ, env), mock.patch("sys.stdout"):
                status = fp.main()
            with open(output, encoding="utf-8") as f:
                return status, f.read()

    def test_main_writes_the_outputs(self):
        self.assertEqual(
            self.main(["fuzz-program 45m"]),
            (0, "seconds=2700\ncoverage=true\nfuzz_minutes=80\nminutes=165\n"),
        )

    def test_main_plans_no_run_for_an_unlabeled_pull_request(self):
        self.assertEqual(self.main(["bug"]), (0, ""))


if __name__ == "__main__":
    unittest.main()
