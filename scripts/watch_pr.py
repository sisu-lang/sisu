"""Waits for the next event on a pull request of sisu-lang/sisu, prints it and stops.

Usage: python3 scripts/watch_pr.py PR [STATE_FILE]

An event is a new or edited review, inline comment or issue comment from someone other than you,
the first failed check of the head, or the end of all checks of the head. CodeRabbit's "review in
progress" note is not an event; the review that replaces it is. STATE_FILE (by default
$TMPDIR/watch-pr-<PR>.json) keeps the events already printed, so the next run prints only newer
ones. Run it with `run_in_background` and the sandbox off (`gh api` is REST and needs the
network), then run it again after each event. Exit status: 0 an event, 1 the time limit.
"""

import json
import os
import re
import subprocess
import sys
import time

REPO = "repos/sisu-lang/sisu"
POLL = float(os.environ.get("WATCH_PR_POLL", "60"))
LIMIT = float(os.environ.get("WATCH_PR_LIMIT", str(2 * 60 * 60)))
IN_PROGRESS = "<!-- This is an auto-generated comment: review in progress"
PASSED = {"success", "skipped", "neutral"}


def gh(path):
    out = subprocess.run(["gh", "api", path], capture_output=True, text=True, check=True).stdout
    return json.loads(out)


def text(body, width=300):
    """The body as one line, with HTML tags and comments taken out."""
    body = re.sub(r"<!--.*?-->|<[^>]+>", " ", body or "", flags=re.S)
    return " ".join(body.split())[:width]


def events(pr, me):
    """Each event now on the pull request, as (key, line). An edit gives a new key."""
    found = []
    for review in gh(f"{REPO}/pulls/{pr}/reviews?per_page=100"):
        if review["user"]["login"] != me:
            key = f"review {review['id']} {review['state']}"
            found.append((key, f"{key} {review['user']['login']}: {text(review['body'])}"))
    for c in gh(f"{REPO}/pulls/{pr}/comments?per_page=100"):
        if c["user"]["login"] != me:
            line = c.get("line") or c.get("original_line")
            found.append((f"inline {c['id']} {c['updated_at']}",
                          f"inline {c['id']} {c['user']['login']} {c['path']}:{line}: {text(c['body'])}"))
    for c in gh(f"{REPO}/issues/{pr}/comments?per_page=100"):
        if c["user"]["login"] != me and IN_PROGRESS not in (c["body"] or ""):
            found.append((f"comment {c['id']} {c['updated_at']}",
                          f"comment {c['id']} {c['user']['login']}: {text(c['body'])}"))
    sha = gh(f"{REPO}/pulls/{pr}")["head"]["sha"]
    runs = gh(f"{REPO}/commits/{sha}/check-runs?per_page=100")["check_runs"]
    failed = [r for r in runs if r["status"] == "completed" and r["conclusion"] not in PASSED]
    if failed:
        found.append((f"ci-fail {sha}",
                      "checks failed at " + sha[:8] + ": "
                      + "; ".join(f"{r['name']} {r['html_url']}" for r in failed)))
    elif runs and all(r["status"] == "completed" for r in runs):
        found.append((f"ci-done {sha}", f"checks passed at {sha[:8]}: {len(runs)} checks"))
    return found


def main():
    if len(sys.argv) not in (2, 3):
        sys.exit(__doc__)
    pr = sys.argv[1]
    state = sys.argv[2] if len(sys.argv) == 3 else os.path.join(
        os.environ.get("TMPDIR", "/tmp"), f"watch-pr-{pr}.json")
    seen = set(json.load(open(state))) if os.path.exists(state) else set()
    me = gh("user")["login"]
    start = time.monotonic()
    while True:
        new = [(k, line) for k, line in events(pr, me) if k not in seen]
        if new:
            for _, line in new:
                print(line)
            with open(state, "w") as f:
                json.dump(sorted(seen | {k for k, _ in new}), f)
            return 0
        if time.monotonic() - start >= LIMIT:
            print(f"no event in {LIMIT:.0f} s")
            return 1
        time.sleep(POLL)


if __name__ == "__main__":
    sys.exit(main())
