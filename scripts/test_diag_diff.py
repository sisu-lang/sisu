"""Unit tests for diag_diff.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""

import contextlib
import io
import json
import shlex
import subprocess
import sys
import tempfile
import unittest
from collections.abc import Callable, Mapping
from pathlib import Path
from typing import Any

import diag_diff as dd

E0032 = (
    '{"kind":"diagnostic","code":"E0032","severity":"error",'
    '"primary":{"line":2,"column":5},"secondary":[{"line":2,"column":9}]}'
)
W0001 = (
    '{"kind":"diagnostic","code":"W0001","severity":"warning",'
    '"primary":{"line":1,"column":1},"secondary":[]}'
)


def fake_sisu(tmp: str, name: str, outputs: Mapping[str, str]) -> str:
    """A sisu stand-in that prints the JSON lines `outputs` gives a file's stem, or crashes."""
    path = Path(tmp, name)
    cases = "".join(f"  {stem}) printf '%s\\n' '{out}' >&2;;\n" for stem, out in outputs.items())
    with path.open("w", encoding="utf-8") as f:
        f.write(f'#!/bin/sh\ncase $(basename "$4" .sisu) in\n{cases}  *) exit 101;;\nesac\n')
    path.chmod(0o755)
    return str(path)


class DiagDiff(unittest.TestCase):
    def test_reports_each_program_whose_diagnostics_differ(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            for stem in ["same", "accepted", "rejected", "moved", "crash"]:
                Path(tmp, f"{stem}.sisu").touch()
            moved = E0032.replace('"column":9', '"column":10')
            base = fake_sisu(
                tmp,
                "base",
                {
                    "same": E0032,
                    "accepted": E0032,
                    "rejected": W0001,
                    "moved": E0032,
                    "crash": E0032,
                },
            )
            head = fake_sisu(
                tmp, "head", {"same": E0032, "accepted": W0001, "rejected": E0032, "moved": moved}
            )
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                status = dd.main(base, head, tmp, full=False)
        self.assertEqual(status, 1)
        lines = out.getvalue().splitlines()
        self.assertEqual(
            [line.split(":")[0] for line in lines if not line.startswith(" ")][:-1],
            ["newly accepted", "crash", "changed", "newly rejected"],
        )
        self.assertIn("  head: crash 101:0", lines)
        self.assertIn("  head: E0032 2:5 +2:10", lines)
        self.assertEqual(
            lines[-1],
            "diag_diff: 5 programs, 1 changed, 1 crash, 1 newly accepted, 1 newly rejected",
        )

    def test_reports_the_programs_of_every_directory_once(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            for sub in ["a", "b", "b/c"]:
                Path(tmp, sub).mkdir(parents=True, exist_ok=True)
                Path(tmp, sub, "same.sisu").touch()
            sisu = fake_sisu(tmp, "sisu", {"same": E0032})
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                status = dd.main(
                    sisu,
                    sisu,
                    str(Path(tmp, "a")),
                    str(Path(tmp, "b")),
                    str(Path(tmp, "b", "c")),
                )
        self.assertEqual(status, 0)
        self.assertEqual(out.getvalue(), "diag_diff: 3 programs, no differences\n")

    def test_reports_a_program_that_crashes_both_builds(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, "crash.sisu").touch()
            sisu = fake_sisu(tmp, "sisu", {})
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                status = dd.main(sisu, sisu, tmp)
        self.assertEqual(status, 1)
        lines = out.getvalue().splitlines()
        self.assertTrue(lines[0].startswith("crash: "), lines)
        self.assertEqual(lines[-1], "diag_diff: 1 programs, 1 crash")

    def test_full_should_report_a_program_whose_help_alone_differs(self) -> None:
        """GIVEN a program whose diagnostic has the same code and position in both builds
        AND a help that differs
        WHEN diag_diff runs without and with `full`
        THEN only the run with `full` reports it, and shows the help of each build
        """
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, "help.sisu").touch()
            base = fake_sisu(
                tmp, "base", {"help": E0032.replace("}]}", '}],"help":"use `match`"}')}
            )
            head = fake_sisu(
                tmp, "head", {"help": E0032.replace("}]}", '}],"help":"use a plain `let`"}')}
            )
            runs = []
            for full in (False, True):
                out = io.StringIO()
                with contextlib.redirect_stdout(out):
                    runs.append((dd.main(base, head, tmp, full=full), out.getvalue().splitlines()))
        self.assertEqual(runs[0], (0, ["diag_diff: 1 programs, no differences"]))
        status, lines = runs[1]
        self.assertEqual(status, 1)
        self.assertTrue(lines[0].startswith("changed: "), lines)
        self.assertIn('"help": "use `match`"', lines[1])
        self.assertIn('"help": "use a plain `let`"', lines[2])
        self.assertEqual(lines[-1], "diag_diff: 1 programs, 1 changed")

    def test_full_should_report_each_text_field_that_alone_differs(self) -> None:
        """GIVEN programs whose diagnostic differs in one field each: the message, the label, a
        secondary label, the message of a fix, an edit of a fix and the safe flag of a fix
        WHEN diag_diff runs from the command line with `--codes`, with no flag and with `--full`
        THEN the run with `--codes` reports none of them, and each other run reports each of them
        """

        def diagnostic(  # noqa: PLR0913  one keyword for each field that a case changes
            message: str = "m",
            label: str = "l",
            other: str = "o",
            fix: str = "f",
            replacement: str = "",
            *,
            safe: bool = True,
        ) -> str:
            return json.dumps(
                {
                    "kind": "diagnostic",
                    "code": "E0131",
                    "severity": "error",
                    "message": message,
                    "primary": {"line": 1, "column": 1, "label": label},
                    "secondary": [{"line": 2, "column": 1, "label": other}],
                    "fixes": [
                        {
                            "message": fix,
                            "safe": safe,
                            "edits": [{"start": 0, "end": 1, "replacement": replacement}],
                        }
                    ],
                }
            )

        changed: dict[str, dict[str, Any]] = {
            "message": {"message": "n"},
            "label": {"label": "k"},
            "secondary": {"other": "p"},
            "fix": {"fix": "g"},
            "edit": {"replacement": "x"},
            "safe": {"safe": False},
        }
        script = Path(dd.__file__).absolute().parent / "diag_diff.py"
        with tempfile.TemporaryDirectory() as tmp:
            for stem in changed:
                Path(tmp, f"{stem}.sisu").touch()
            base = fake_sisu(tmp, "base", {stem: diagnostic() for stem in changed})
            head = fake_sisu(tmp, "head", {stem: diagnostic(**f) for stem, f in changed.items()})
            flags: list[list[str]] = [["--codes"], [], ["--full"]]
            runs = [
                subprocess.run(
                    [sys.executable, script, *flag, base, head, tmp],
                    capture_output=True,
                    text=True,
                    check=False,
                )
                for flag in flags
            ]
        self.assertEqual(
            (runs[0].returncode, runs[0].stdout), (0, "diag_diff: 6 programs, no differences\n")
        )
        for run in runs[1:]:
            self.assertEqual(run.returncode, 1)
            reported = {
                Path(line.split(": ", 1)[1]).stem
                for line in run.stdout.splitlines()
                if line.startswith("changed: ")
            }
            self.assertEqual(reported, set(changed))

    def test_full_should_report_a_diagnostic_whose_notes_alone_differ(self) -> None:
        """GIVEN two diagnostics that differ only in `notes`, and one with no `notes` key and one
        with `"notes":[]`
        WHEN diag_diff runs in the default mode
        THEN it reports the first pair and not the second
        """
        with tempfile.TemporaryDirectory() as tmp:
            for stem in ["noted", "missing"]:
                Path(tmp, f"{stem}.sisu").touch()
            base = fake_sisu(
                tmp,
                "base",
                {"noted": E0032.replace("}]}", '}],"notes":[]}'), "missing": E0032},
            )
            head = fake_sisu(
                tmp,
                "head",
                {
                    "noted": E0032.replace("}]}", '}],"notes":["a note"]}'),
                    "missing": E0032.replace("}]}", '}],"notes":[]}'),
                },
            )
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                status = dd.main(base, head, tmp)
        self.assertEqual(status, 1)
        lines = out.getvalue().splitlines()
        self.assertEqual(
            [line for line in lines if line.startswith("changed: ")],
            [f"changed: {Path(tmp, 'noted.sisu')}"],
        )
        self.assertEqual(lines[-1], "diag_diff: 2 programs, 1 changed")

    def test_primary_should_ignore_a_secondary_position_that_codes_reports(self) -> None:
        """GIVEN two diagnostics that differ only in the position of a secondary label
        WHEN diag_diff runs with `--primary` and with `--codes`
        THEN `--primary` reports no difference and `--codes` reports one
        """
        script = Path(dd.__file__).absolute().parent / "diag_diff.py"
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, "moved.sisu").touch()
            base = fake_sisu(tmp, "base", {"moved": E0032})
            head = fake_sisu(tmp, "head", {"moved": E0032.replace('"column":9', '"column":10')})
            runs = {
                flag: subprocess.run(
                    [sys.executable, script, flag, base, head, tmp],
                    capture_output=True,
                    text=True,
                    check=False,
                )
                for flag in ("--primary", "--codes")
            }
        self.assertEqual(
            (runs["--primary"].returncode, runs["--primary"].stdout),
            (0, "diag_diff: 1 programs, no differences\n"),
        )
        self.assertEqual(runs["--codes"].returncode, 1)

    def test_jobs_should_bound_the_sisu_processes_that_run_at_once(self) -> None:
        self.assertLessEqual(max(peak_runs(lambda sisu, tmp: dd.main(sisu, sisu, tmp, jobs=2))), 2)

    def test_main_should_run_at_most_two_sisu_processes_by_default(self) -> None:
        self.assertLessEqual(max(peak_runs(lambda sisu, tmp: dd.main(sisu, sisu, tmp))), 2)

    def test_command_line_should_bound_the_sisu_processes_by_jobs(self) -> None:
        script = Path(dd.__file__).absolute().parent / "diag_diff.py"
        cases: list[tuple[list[str], int]] = [([], 2), (["--jobs=1"], 1)]
        for flag, limit in cases:

            def run(sisu: str, tmp: str, flag: list[str] = flag) -> None:
                subprocess.run(
                    [sys.executable, script, *flag, sisu, sisu, tmp],
                    check=True,
                    capture_output=True,
                )

            self.assertLessEqual(max(peak_runs(run)), limit, flag)


def peak_runs(run: Callable[[str, str], object]) -> list[int]:
    """The count of fake sisu runs at each start while `run(sisu, dir)` checks six programs."""
    with tempfile.TemporaryDirectory() as tmp:
        for n in range(6):
            Path(tmp, f"p{n}.sisu").touch()
        # Each run marks itself, counts the runs that have a mark, waits, and removes its mark.
        d = shlex.quote(tmp)
        sisu = Path(tmp, "sisu")
        with sisu.open("w", encoding="utf-8") as f:
            f.write(
                f"#!/bin/sh\ntouch {d}/run.$$\n"
                f'ls {d} | grep -c "^run" >> {d}/counts\nsleep 0.1\nrm {d}/run.$$\n'
            )
        sisu.chmod(0o755)
        with contextlib.redirect_stdout(io.StringIO()):
            run(str(sisu), tmp)
        counts = [int(n) for n in Path(tmp, "counts").read_text().split()]
    assert len(counts) == 12, counts
    return counts


if __name__ == "__main__":
    unittest.main()
