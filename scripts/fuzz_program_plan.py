"""Plans a run of .github/workflows/fuzz-program.yml: how long to fuzz, whether to measure
coverage, and the time limits of the fuzz step and of the job. Writes `seconds=`, `coverage=`,
`fuzz_minutes=` and `minutes=` to $GITHUB_OUTPUT, or nothing when the event asks for no run.

- schedule: 1800 s, no coverage.
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

LABEL = re.compile(r'fuzz-program(?: ([0-9]+)([mh]))?')
BARE_LABEL = 2 * 3600
NIGHTLY = 1800
LEAST = 60
# GitHub-hosted jobs stop after 6 hours. 4 hours of fuzzing leaves 75 minutes for the build,
# crash replays, tmin, cmin, setup and the cache, and 45 for --coverage: 360 of 360.
MOST = 4 * 3600
# The Coverage step's timeout-minutes in fuzz-program.yml; keep the two equal.
COVERAGE_MINUTES = 45
# The bound of the Minimize the corpus step in fuzz-program.yml; keep the two equal.
CMIN_MINUTES = 20
# The fuzzer first runs each corpus input once, one at a time, and -max_total_time does not stop
# that pass. The 500 inputs that the Sample the corpus step keeps take about 10 minutes: run
# 42015848679 ran 990 inputs in 20.3 minutes.
CORPUS_PASS_MINUTES = 20


def label_seconds(name: str) -> int | None:
    """The fuzz time a label asks for, or None when it is not a fuzz-program label."""
    m = LABEL.fullmatch(name)
    if m is None:
        return None
    if m[1] is None:
        return BARE_LABEL
    return int(m[1]) * (60 if m[2] == 'm' else 3600)


def in_range(seconds: int) -> int:
    if not LEAST <= seconds <= MOST:
        raise ValueError(f'a fuzz time of {seconds} s is outside {LEAST} s to {MOST // 3600} h')
    return seconds


def from_labels(labels: list[str], added: str | None = None) -> int | None:
    """The fuzz time the labels ask for; None for no run. `added` is a `labeled` event's label."""
    if added is not None and label_seconds(added) is None:
        return None
    matching = [name for name in labels if label_seconds(name) is not None]
    if not matching:
        return None
    if len(matching) > 1:
        raise ValueError('more than one fuzz-program label: ' + ', '.join(matching))
    return in_range(label_seconds(matching[0]))


def fuzz_minutes(seconds: int) -> int:
    """The fuzz step's time limit. It has 15 minutes of build, then the fuzzing or the corpus
    pass, whichever is longer. Then it has 17 minutes of crash replays and 3 to stop the jobs."""
    return 15 + max(math.ceil(seconds / 60), CORPUS_PASS_MINUTES) + 17 + 3


def minutes(seconds: int, coverage: bool) -> int:
    """The job's time limit: the fuzz step, 15 minutes of tmin, cmin, 5 of setup and the cache,
    and the coverage. The 5 minutes are an estimate, not a bound: no step limits them."""
    return (fuzz_minutes(seconds) + 15 + CMIN_MINUTES + 5
            + (COVERAGE_MINUTES if coverage else 0))


def plan(event: str, payload: dict, env: dict) -> tuple[int, bool] | None:
    if event == 'schedule':
        return NIGHTLY, False
    if event == 'workflow_dispatch':
        return in_range(int(env['FUZZ_SECONDS'])), env.get('FUZZ_COVERAGE') == 'true'
    if event == 'pull_request':
        labels = [label['name'] for label in (payload.get('pull_request') or {}).get('labels', [])]
        added = payload['label']['name'] if payload.get('action') == 'labeled' else None
        seconds = from_labels(labels, added)
        return None if seconds is None else (seconds, True)
    raise ValueError(f'no plan for the event {event}')


def main() -> int:
    with open(os.environ['GITHUB_EVENT_PATH'], encoding='utf-8') as f:
        payload = json.load(f)
    try:
        got = plan(os.environ['GITHUB_EVENT_NAME'], payload, os.environ)
    except ValueError as e:
        print(f'fuzz_program_plan: {e}', file=sys.stderr)
        return 1
    if got is None:
        print('No fuzz-program label: no run.')
        return 0
    seconds, coverage = got
    with open(os.environ['GITHUB_OUTPUT'], 'a', encoding='utf-8') as out:
        out.write(f'seconds={seconds}\ncoverage={str(coverage).lower()}\n'
                  f'fuzz_minutes={fuzz_minutes(seconds)}\nminutes={minutes(seconds, coverage)}\n')
    print(f'Fuzz for {seconds} s, coverage {coverage}.')
    return 0


if __name__ == '__main__':
    sys.exit(main())
