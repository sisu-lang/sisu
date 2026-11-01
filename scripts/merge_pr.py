"""Squash-merges a pull request of sisu-lang/sisu when it is safe, and removes its local worktree.

Usage: python3 scripts/merge_pr.py PR [REPO_ROOT]

Run it only after the user says to merge. It stops, with no merge, when a check fails or the head
does not contain main. It waits for running checks, merges at the head it checked, waits for each
`Closes #N` issue to close, and removes a clean local worktree whose branch is at the merged head.
It calls `gh api` (REST), so it needs the network: run it with the sandbox off.
"""

import json
import os
import re
import subprocess
import sys
import time

REPO = "repos/sisu-lang/sisu"
POLL = float(os.environ.get("MERGE_PR_POLL", "30"))
CHECKS_LIMIT = float(os.environ.get("MERGE_PR_CHECKS_LIMIT", str(90 * 60)))
NO_CHECKS_LIMIT = min(CHECKS_LIMIT, 10 * 60)
ISSUES_LIMIT = 120
CLOSES = re.compile(r"^Closes #(\d+)\s*$", re.M | re.I)
PASSED = {"success", "skipped", "neutral"}


class Stop(Exception):
    pass


def run(args):
    try:
        return subprocess.run(args, capture_output=True, text=True, check=True).stdout
    except subprocess.CalledProcessError as e:
        raise Stop(f"{' '.join(args[:4])}: {e.stderr.strip() or e}") from None


def gh(*args):
    out = run(["gh", "api", *args])
    return json.loads(out) if out.strip() else None


def wait_for_checks(sha):
    """Returns the checks that did not pass, after every check ends; stops at the time limit."""
    start = time.monotonic()
    while True:
        page = gh(f"{REPO}/commits/{sha}/check-runs?per_page=100")
        runs = page["check_runs"]
        if page["total_count"] > len(runs):
            raise Stop(f"{page['total_count']} checks, more than one page: merge by hand")
        if runs and all(r["status"] == "completed" for r in runs):
            return [f'{r["name"]}: {r["conclusion"]}' for r in runs if r["conclusion"] not in PASSED]
        waited = time.monotonic() - start
        if not runs and waited >= NO_CHECKS_LIMIT:
            raise Stop("the head has no checks")
        if waited >= CHECKS_LIMIT:
            raise Stop("the checks did not end in time")
        time.sleep(POLL)


def check_base(pr, sha):
    if gh(f"{REPO}/compare/{pr['base']['ref']}...{sha}")["behind_by"]:
        raise Stop(f"{pr['head']['ref']} does not contain {pr['base']['ref']}: merge it, run make test")


def wait_for_issues(numbers):
    """Returns `#N state` for each issue, after all close or the time limit."""
    start = time.monotonic()
    while True:
        states = {n: gh(f"{REPO}/issues/{n}")["state"] for n in numbers}
        if all(s == "closed" for s in states.values()) or time.monotonic() - start >= ISSUES_LIMIT:
            return [f"#{n} {s}" for n, s in states.items()]
        time.sleep(min(POLL, 5))


def git(root, *args):
    return run(["git", "-C", root, *args])


def remove_worktree(root, branch, sha):
    """Removes the worktree of `branch` and the branch, only when it is clean and at `sha`."""
    path = None
    for block in git(root, "worktree", "list", "--porcelain").split("\n\n"):
        fields = dict(line.split(" ", 1) for line in block.splitlines() if " " in line)
        if fields.get("branch") == f"refs/heads/{branch}":
            path = fields["worktree"]
            if fields.get("HEAD") != sha:
                return f"kept {path}: its head is not the merged head"
    if path is None:
        return f"no local worktree for {branch}"
    if git(path, "status", "--porcelain"):
        return f"kept {path}: it has uncommitted changes"
    # No --force: git checks again for changes when it removes the worktree.
    git(root, "worktree", "remove", path)
    # The branch delete writes .git/config. A separate short command lowers the chance that the
    # sandbox of another command puts its lock file there first.
    git(root, "branch", "-D", branch)
    return f"removed {path} and branch {branch}"


def merge(number, root):
    pr = gh(f"{REPO}/pulls/{number}")
    if pr["state"] != "open":
        raise Stop(f"#{number} is {pr['state']}")
    sha = pr["head"]["sha"]
    check_base(pr, sha)
    failed = wait_for_checks(sha)
    if failed:
        raise Stop("; ".join(failed))
    # main can move during the wait.
    check_base(pr, sha)
    merged = gh("-X", "PUT", f"{REPO}/pulls/{number}/merge", "-f", "merge_method=squash",
                "-f", f"sha={sha}", "-f", f"commit_title={pr['title']} (#{number})")
    print(f"merged #{number} as {merged['sha'][:8]}")
    issues = CLOSES.findall(pr["body"] or "")
    if issues:
        print("issues: " + ", ".join(wait_for_issues(issues)))
    print(remove_worktree(root, pr["head"]["ref"], sha))


def main(argv):
    try:
        merge(argv[0], argv[1] if len(argv) > 1 else ".")
    except Stop as e:
        print(f"merge_pr: FAIL: {e}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
