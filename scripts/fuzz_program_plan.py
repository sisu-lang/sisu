"""Plan a run of .github/workflows/fuzz-program.yml.

The plan says how long to fuzz, whether to measure coverage, and the time limits of the fuzz
step and of the job. The script writes `seconds=`, `coverage=`, `fuzz_minutes=` and `minutes=`
to $GITHUB_OUTPUT, or nothing when the event asks for no run.

- schedule: 1800 s, and coverage.
- workflow_dispatch: the inputs, passed in FUZZ_SECONDS and FUZZ_COVERAGE.
- pull_request: the pull request's one label `fuzz-program` (2 h), `fuzz-program <n>m` or
  `fuzz-program <n>h`, and coverage always. A `labeled` event runs only when the label it added
  is such a label; any other event runs while the pull request carries one. Label text comes
  only from the event payload at $GITHUB_EVENT_PATH, never through the workflow's shell.

Usage: python3 fuzz_program_plan.py (under GitHub Actions). Exits 1 on a label or input that
asks for a run it cannot plan: two fuzz-program labels, or a time out of range.
"""

import json
import math
import os
import re
import sys
from collections.abc import Mapping
from pathlib import Path
from typing import Any

LABEL = re.compile(r"fuzz-program(?: ([0-9]+)([mh]))?")
BARE_LABEL = 2 * 3600
NIGHTLY = 1800
LEAST = 60
# GitHub-hosted jobs stop after 6 hours. 4 hours of fuzzing leaves 75 minutes for the build,
# crash replays, tmin, cmin, setup and the cache, and 45 for --coverage and --gaps: 360 of 360.
MOST = 4 * 3600
# The Coverage step's timeout-minutes in fuzz-program.yml; keep the two equal. Coverage measured
# 7.5 minutes on 654 inputs.
COVERAGE_MINUTES = 30
# The Generator gaps step's timeout-minutes in fuzz-program.yml; keep the two equal. The value is
# not yet measured: read the step's first real duration against it.
GAPS_MINUTES = 15
# The bound of the Minimize the corpus step in fuzz-program.yml; keep the two equal.
CMIN_MINUTES = 20
# The fuzzer first runs each corpus input and each of the 128 seeds once, one at a time, and
# -max_total_time does not stop that pass. The 500 inputs that the Sample the corpus step keeps
# take about 10 minutes: run 42015848679 ran 990 inputs in 20.3 minutes. A seed costs more than a
# corpus input. Locally a seed takes 1.24 s, against about 0.6 s for a corpus input. So expect
# about 2.5 s on CI, or about 5 minutes for the 128 seeds. The corpus time holds for today's
# short corpus. New finds are mutants of the full-size seeds, so the corpus costs more as nights
# pass. At 500 full-size inputs the first pass may take most of the 30 minutes (an estimate).
CORPUS_PASS_MINUTES = 30


def label_seconds(name: str) -> int | None:
    """Return the fuzz time a label asks for, or None when it is not a fuzz-program label."""
    m = LABEL.fullmatch(name)
    if m is None:
        return None
    if m[1] is None:
        return BARE_LABEL
    return int(m[1]) * (60 if m[2] == "m" else 3600)


def in_range(seconds: int) -> int:
    """Return `seconds` when the fuzz time is in range, and raise ValueError when it is not."""
    if not LEAST <= seconds <= MOST:
        raise ValueError(f"a fuzz time of {seconds} s is outside {LEAST} s to {MOST // 3600} h")
    return seconds


def from_labels(labels: list[str], added: str | None = None) -> int | None:
    """Return the fuzz time the labels ask for; None for no run.

    `added` is the label of a `labeled` event.
    """
    if added is not None and label_seconds(added) is None:
        return None
    matching = [name for name in labels if label_seconds(name) is not None]
    if not matching:
        return None
    if len(matching) > 1:
        raise ValueError("more than one fuzz-program label: " + ", ".join(matching))
    # pyrefly: ignore[bad-argument-type]  `matching` holds only names with a fuzz time
    return in_range(label_seconds(matching[0]))


def fuzz_minutes(seconds: int) -> int:
    """Return the fuzz step's time limit.

    It has 15 minutes of build, then the fuzzing or the corpus pass, whichever is longer. Then it
    has 17 minutes of crash replays and 3 to stop the jobs.
    """
    return 15 + max(math.ceil(seconds / 60), CORPUS_PASS_MINUTES) + 17 + 3


def minutes(seconds: int, *, coverage: bool) -> int:
    """Return the job's time limit.

    It holds the fuzz step, 15 minutes of tmin, cmin, 5 of setup and the cache, and the coverage
    and the gap check. The 5 minutes are an estimate, not a bound: no step limits them.
    """
    return (
        fuzz_minutes(seconds)
        + 15
        + CMIN_MINUTES
        + 5
        + (COVERAGE_MINUTES + GAPS_MINUTES if coverage else 0)
    )


def plan(event: str, payload: dict[str, Any], env: Mapping[str, str]) -> tuple[int, bool] | None:
    """Return the fuzz seconds and the coverage flag that `event` asks for, or None for no run."""
    if event == "schedule":
        return NIGHTLY, True
    if event == "workflow_dispatch":
        return in_range(int(env["FUZZ_SECONDS"])), env.get("FUZZ_COVERAGE") == "true"
    if event == "pull_request":
        labels = [label["name"] for label in (payload.get("pull_request") or {}).get("labels", [])]
        added = payload["label"]["name"] if payload.get("action") == "labeled" else None
        seconds = from_labels(labels, added)
        return None if seconds is None else (seconds, True)
    raise ValueError(f"no plan for the event {event}")


def main() -> int:
    """Read the event, write the plan to $GITHUB_OUTPUT, and return the exit code."""
    with Path(os.environ["GITHUB_EVENT_PATH"]).open(encoding="utf-8") as f:
        payload = json.load(f)
    try:
        got = plan(os.environ["GITHUB_EVENT_NAME"], payload, os.environ)
    except ValueError as e:
        print(f"fuzz_program_plan: {e}", file=sys.stderr)
        return 1
    if got is None:
        print("No fuzz-program label: no run.")
        return 0
    seconds, coverage = got
    with Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as out:
        out.write(
            f"seconds={seconds}\ncoverage={str(coverage).lower()}\n"
            f"fuzz_minutes={fuzz_minutes(seconds)}\nminutes={minutes(seconds, coverage=coverage)}\n"
        )
    print(f"Fuzz for {seconds} s, coverage {coverage}.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
