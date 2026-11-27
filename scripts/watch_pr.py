"""Waits for the next event on a pull request of sisu-lang/sisu, prints it and stops.

Usage: python3 scripts/watch_pr.py PR [STATE_FILE]

An event is a new review with text, or a new inline or issue comment, from someone other than you;
an edit of one of these outside its HTML comments; the first failed check of the head; or the end of
all checks of the head with none failed. CodeRabbit's "review in progress" note is not an event; the
review that replaces it is. A bot comment that asks for no action is not an event: CodeRabbit's
"No actionable comments", and a coverage line with each changed line covered or none. After a failed
check, a rerun that passes is an event. Each list reads 100 items. Each line holds the first 300
characters of the body and the URL of the full text.
STATE_FILE (by default $TMPDIR/watch-pr-<PR>.json) keeps the events already printed, so the next run
prints only newer ones. Run it with `run_in_background` and the sandbox off (`gh api` is REST and
needs the network), then run it again after each event. Exit status: 0 an event, 1 the time limit.
"""

import hashlib
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, overload

REPO = "repos/sisu-lang/sisu"
POLL = float(os.environ.get("WATCH_PR_POLL", "60"))
LIMIT = float(os.environ.get("WATCH_PR_LIMIT", str(2 * 60 * 60)))
LOG_TIMEOUT = float(os.environ.get("WATCH_PR_LOG_TIMEOUT", "60"))
IN_PROGRESS = "<!-- This is an auto-generated comment: review in progress"
PASSED = {"success", "skipped", "neutral"}
# The shown text of a bot comment that asks for no action.
# A rounded 100.0% can hide an uncovered line (1999 of 2000), so the two counts must be equal.
QUIET = re.compile(
    r"No actionable comments were generated"
    r"|Coverage: .*changed lines(?: 100\.0% \((\d+) of \1\)|: none)"
)
JOB_ID = re.compile(r"/job/(\d+)(?:$|[/?#])")
ANSI = re.compile(r"\x1b\[[0-9;]*[a-zA-Z]")
TIMESTAMP = re.compile(r"^\ufeff?\d{4}-\d{2}-\d{2}T\S+(?:\s+|$)")
GENERIC_EXIT = re.compile(r"^##\[error\]Process completed with exit code \d+\.$")
ERROR_LINE = re.compile(r"##\[error\]|^E:\s|\berror\[|\berror:|\bERROR\b|\bFAIL\b|\bFAILED\b")
ERROR_TAIL_LINES = 3


@overload
def gh(path: str, key: None = None) -> dict[str, Any]: ...


@overload
def gh(path: str, key: str) -> list[dict[str, Any]]: ...


def gh(path: str, key: str | None = None) -> dict[str, Any] | list[dict[str, Any]]:
    """Return the answer to one request.

    With `key`, the answer is every page of a list, joined: `key` names the list in each page, or
    is "" when the page is the list.
    """
    pages = ["--paginate", "--slurp"] if key is not None else []
    # A hung request would hold the watcher past its limit; a timeout counts as a failed call.
    out = subprocess.run(
        ["gh", "api", *pages, path], capture_output=True, text=True, check=True, timeout=60
    ).stdout
    if key is None:
        return json.loads(out)
    return [item for page in json.loads(out) for item in (page[key] if key else page)]


def text(body: str | None, width: int = 300) -> str:
    """Return the body as one line, with HTML tags and comments taken out."""
    body = re.sub(r"<!--.*?-->|<[^>]+>", " ", body or "", flags=re.DOTALL)
    return " ".join(body.split())[:width]


def digest(body: str | None) -> str:
    """Return a short hash of the body outside its HTML comments, where bots keep hidden state."""
    shown = re.sub(r"<!--.*?-->", " ", body or "", flags=re.DOTALL)
    return hashlib.sha256(" ".join(shown.split()).encode()).hexdigest()[:12]


def job_error(html_url: str | None) -> str:
    """Return the last error lines of a failed GitHub Actions job, or '' when unavailable."""
    match = JOB_ID.search(html_url or "")
    if not match:
        return ""
    try:
        result = subprocess.run(
            [
                "gh",
                "api",
                "--allow-escape-sequences",
                f"{REPO}/actions/jobs/{match.group(1)}/logs",
            ],
            capture_output=True,
            encoding="utf-8",
            errors="replace",
            check=False,
            timeout=LOG_TIMEOUT,
        )
    except subprocess.TimeoutExpired:
        return ""
    if result.returncode != 0:
        return ""
    cleaned: list[str] = []
    for raw in ANSI.sub("", result.stdout).splitlines():
        line = TIMESTAMP.sub("", raw).strip()
        if line == "Post job cleanup.":
            break
        if line:
            cleaned.append(line)
    picked = [line for line in cleaned if ERROR_LINE.search(line) and not GENERIC_EXIT.match(line)]
    tail = (picked or cleaned)[-ERROR_TAIL_LINES:]
    return " | ".join(tail)[:300]


def events(pr: str, me: str, seen: set[str] | None = None) -> list[tuple[str, str]]:
    """Return each event now on the pull request, as (key, line)."""
    found: list[tuple[str, str]] = [
        (
            f"review {review['id']} {review['state']} {digest(review['body'])}",
            (
                f"review {review['id']} {review['state']} {review['user']['login']}: "
                f"{text(review['body'])} {review['html_url']}"
            ),
        )
        for review in gh(f"{REPO}/pulls/{pr}/reviews?per_page=100", "")
        if review["user"]["login"] != me and review["body"]
    ]
    for c in gh(f"{REPO}/pulls/{pr}/comments?per_page=100", ""):
        if c["user"]["login"] != me:
            line = c.get("line") or c.get("original_line")
            found.append(
                (
                    f"inline {c['id']} {digest(c['body'])}",
                    (
                        f"inline {c['id']} {c['user']['login']} {c['path']}:{line}: "
                        f"{text(c['body'])} {c['html_url']}"
                    ),
                )
            )
    found.extend(
        (
            f"comment {c['id']} {digest(c['body'])}",
            f"comment {c['id']} {c['user']['login']}: {text(c['body'])} {c['html_url']}",
        )
        for c in gh(f"{REPO}/issues/{pr}/comments?per_page=100", "")
        if c["user"]["login"] != me
        and IN_PROGRESS not in (c["body"] or "")
        and not (c["user"]["login"].endswith("[bot]") and QUIET.match(text(c["body"])))
    )
    sha = gh(f"{REPO}/pulls/{pr}")["head"]["sha"]
    runs = gh(f"{REPO}/commits/{sha}/check-runs?per_page=100", "check_runs")
    failed = [r for r in runs if r["status"] == "completed" and r["conclusion"] not in PASSED]
    if failed:
        key = f"ci-fail {sha}"
        summary = (
            "checks failed at "
            + sha[:8]
            + ": "
            + "; ".join(f"{r['name']} {r['html_url']}" for r in failed)
        )
        details = (
            []
            if seen is not None and key in seen
            else [
                f"  {r['name']}: {detail}"
                for r in failed
                if (detail := job_error(r.get("html_url")))
            ]
        )
        found.append((key, "\n".join([summary, *details])))
    # A workflow that has not started yet has a suite but no check runs. Other apps keep a queued
    # suite with no runs (Greptile until someone asks it), so only GitHub Actions suites count.
    elif (
        runs
        and all(r["status"] == "completed" for r in runs)
        and all(
            s["status"] == "completed"
            for s in gh(f"{REPO}/commits/{sha}/check-suites?per_page=100", "check_suites")
            if s["app"]["slug"] == "github-actions"
        )
    ):
        found.append((f"ci-done {sha}", f"checks passed at {sha[:8]}: {len(runs)} checks"))
    return found


def main() -> int:
    """Wait for an event on the pull request, print it, and return the exit code."""
    if len(sys.argv) not in (2, 3):
        sys.exit(__doc__)
    pr = sys.argv[1]
    state = (
        Path(sys.argv[2])
        if len(sys.argv) == 3  # noqa: PLR2004  the program, the pull request and the state file
        else Path(os.environ.get("TMPDIR", "/tmp"), f"watch-pr-{pr}.json")  # noqa: S108  the fallback when TMPDIR is unset; the file holds event keys only
    )
    seen = set(json.loads(state.read_text())) if state.exists() else set()
    me: str | None = None
    start = time.monotonic()
    while True:
        try:
            me = me or gh("user")["login"]
            new = [(k, line) for k, line in events(pr, me, seen) if k not in seen]
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as e:
            # A network fault, a rate limit or a hung request: try again at the next poll.
            print(
                f"gh api failed, trying again: {str(e.stderr or e).strip()[:200]}", file=sys.stderr
            )
            new: list[tuple[str, str]] = []
        if new:
            for _, line in new:
                print(line)
            with state.open("w") as f:
                json.dump(sorted(seen | {k for k, _ in new}), f)
            return 0
        if time.monotonic() - start >= LIMIT:
            print(f"no event in {LIMIT:.0f} s")
            return 1
        time.sleep(POLL)


if __name__ == "__main__":
    sys.exit(main())
