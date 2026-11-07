"""Unit tests for fuzz_program_plan.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""

import json
import os
import re
import subprocess
import tempfile
import unittest
from pathlib import Path
from typing import Any
from unittest import mock

import fuzz_program_plan as fp

WORKFLOW = Path(__file__).absolute().parent / ".." / ".github" / "workflows" / "fuzz-program.yml"


def steps() -> list[list[str]]:
    """The workflow's steps as lists of lines. A step starts at a line of six spaces and `- `, and
    ends before the next one or at the first line with less indent (the next job).
    """
    with WORKFLOW.open(encoding="utf-8") as f:
        lines = f.read().splitlines()
    found: list[list[str] | None] = []
    for line in lines:
        if line.startswith("      - "):
            found.append([])
        elif line.strip() and not line.startswith("      ") and found:
            found.append(None)
        if found and found[-1] is not None:
            found[-1].append(line)
    return [step for step in found if step is not None]


def step_named(name: str) -> tuple[list[str], list[str]]:
    """The step named `name` and the step after it."""
    found = steps()
    for i, step in enumerate(found):
        if any(re.fullmatch(rf"\s*(- )?name: {re.escape(name)}", line) for line in step):
            return step, found[i + 1]
    raise AssertionError(f"no step named {name}")


def field(step: list[str], key: str) -> str:
    """The value of `key:` on a line of the step that is not a comment."""
    values = [m[1] for line in step if (m := re.fullmatch(rf"\s*(?:- )?{key}: (.*)", line))]
    assert len(values) == 1, (key, values)
    return values[0]


def run_block(step: list[str]) -> str:
    """The text of the step's `run: |` block without its indent."""
    start = step.index("        run: |") + 1
    block: list[str] = []
    for line in step[start:]:
        if line.strip() and not line.startswith(" " * 10):
            break
        block.append(line[10:])
    return "\n".join(block) + "\n"


def pull_request(
    labels: list[str], action: str = "synchronize", added: str | None = None
) -> dict[str, Any]:
    payload: dict[str, Any] = {
        "action": action,
        "pull_request": {"labels": [{"name": n} for n in labels]},
    }
    if added is not None:
        payload["label"] = {"name": added}
    return payload


class Labels(unittest.TestCase):
    def plan(self, labels: list[str], **kw: str) -> tuple[int, bool] | None:
        return fp.plan("pull_request", pull_request(labels, **kw), {})

    def test_minutes_and_hours(self) -> None:
        self.assertEqual(self.plan(["fuzz-program 45m"]), (2700, True))
        self.assertEqual(self.plan(["bug", "fuzz-program 2h"]), (7200, True))
        # The bounds themselves are allowed.
        self.assertEqual(self.plan(["fuzz-program 1m"]), (60, True))
        self.assertEqual(self.plan(["fuzz-program 220m"]), (13200, True))

    def test_a_bare_label_fuzzes_two_hours(self) -> None:
        self.assertEqual(self.plan(["fuzz-program"]), (7200, True))

    def test_out_of_range_fails(self) -> None:
        for label in ("fuzz-program 0m", "fuzz-program 6h", "fuzz-program 4h", "fuzz-program 221m"):
            with self.subTest(label), self.assertRaisesRegex(ValueError, "outside"):
                self.plan([label])

    def test_two_labels_fail_naming_both(self) -> None:
        with self.assertRaisesRegex(ValueError, "fuzz-program 45m, fuzz-program$"):
            self.plan(["fuzz-program 45m", "fuzz-program"])

    def test_unrelated_labels_run_nothing(self) -> None:
        labels: list[str]
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

    def test_adding_another_label_runs_nothing(self) -> None:
        self.assertIsNone(self.plan(["fuzz-program", "bug"], action="labeled", added="bug"))
        self.assertEqual(
            self.plan(["fuzz-program 45m"], action="labeled", added="fuzz-program 45m"),
            (2700, True),
        )


class Events(unittest.TestCase):
    def test_nightly_run_should_get_95_minutes_of_fuzz_step_in_200(self) -> None:
        self.assertEqual(fp.fuzz_minutes(fp.NIGHTLY), 95)
        self.assertEqual(fp.minutes(fp.NIGHTLY, coverage=True), 200)

    def test_schedule_should_measure_coverage(self) -> None:
        self.assertEqual(fp.plan("schedule", {}, {}), (3600, True))

    def test_fuzz_step_should_have_time_for_the_corpus_pass_of_a_short_run(self) -> None:
        # 15 of build, 30 of corpus pass, 17 of crash replays and 3 to stop.
        self.assertEqual(fp.fuzz_minutes(60), 65)

    def test_cmin_should_stop_at_the_planned_bound_and_warn_when_it_does_not_finish(self) -> None:
        cmin, _ = step_named("Minimize the corpus")
        block = run_block(cmin)
        self.assertIn(f"bound={fp.CMIN_MINUTES * 60}\n", block)
        self.assertRegex(block, r'timeout -k 10 "\$bound" scripts/fuzz\.sh --target program --cmin')
        self.assertRegex(block, r'if \[ "\$outcome" != finished \]; then\n\s*echo "::warning ')

    def test_the_longest_run_should_leave_time_for_gaps(self) -> None:
        self.assertLessEqual(fp.minutes(fp.MOST, coverage=True), 360)
        self.assertEqual(
            fp.minutes(fp.MOST, coverage=True) - fp.minutes(fp.MOST, coverage=False),
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

    def test_the_gaps_step_should_report_a_check_that_did_not_finish(self) -> None:
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
                Path(tmp, "scripts").mkdir(parents=True)
                Path(tmp, "fuzz", "artifacts").mkdir(parents=True)
                fake_path = Path(tmp, "scripts", "fuzz.sh")
                with fake_path.open("w", encoding="utf-8") as f:
                    f.write(f"#!/bin/bash\n{fake}\n")
                fake_path.chmod(0o755)
                summary = Path(tmp, "fuzz", "artifacts", "summary.md")
                with summary.open("w", encoding="utf-8") as f:
                    f.write("# Fuzzing: program\n\nNone.\n")
                step_summary = Path(tmp, "step-summary.md")
                env = dict(os.environ, GITHUB_STEP_SUMMARY=str(step_summary))
                done = subprocess.run(
                    ["bash", "-e", "-c", run_block(gaps)],
                    cwd=tmp,
                    env=env,
                    capture_output=True,
                    text=True,
                    check=False,
                )
                with summary.open(encoding="utf-8") as f:
                    in_summary = f.read()
                with step_summary.open(encoding="utf-8") as f:
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

    def test_dispatch_takes_its_inputs(self) -> None:
        env = {"FUZZ_SECONDS": "600", "FUZZ_COVERAGE": "true"}
        self.assertEqual(fp.plan("workflow_dispatch", {}, env), (600, True))
        with self.assertRaisesRegex(ValueError, "outside"):
            fp.plan("workflow_dispatch", {}, {"FUZZ_SECONDS": "30", "FUZZ_COVERAGE": "false"})

    def main(self, labels: list[str]) -> tuple[int, str]:
        """Runs main() on a pull_request event with `labels`; returns its status and output."""
        with tempfile.TemporaryDirectory() as tmp:
            event = Path(tmp, "event.json")
            output = Path(tmp, "output")
            with event.open("w", encoding="utf-8") as f:
                json.dump(pull_request(labels), f)
            output.touch()
            env = {
                "GITHUB_EVENT_NAME": "pull_request",
                "GITHUB_EVENT_PATH": str(event),
                "GITHUB_OUTPUT": str(output),
            }
            with mock.patch.dict(os.environ, env), mock.patch("sys.stdout"):
                status = fp.main()
            with output.open(encoding="utf-8") as f:
                return status, f.read()

    def test_main_writes_the_outputs(self) -> None:
        self.assertEqual(
            self.main(["fuzz-program 45m"]),
            (0, "seconds=2700\ncoverage=true\nfuzz_minutes=80\nminutes=185\n"),
        )

    def test_main_plans_no_run_for_an_unlabeled_pull_request(self) -> None:
        self.assertEqual(self.main(["bug"]), (0, ""))


if __name__ == "__main__":
    unittest.main()
