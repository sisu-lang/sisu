"""Squash-merges a pull request of sisu-lang/sisu when it is safe, then cleans up after it.

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
CHECKS_LIMIT = 90 * 60
ISSUES_LIMIT = 120
CLOSES = re.compile(r"^Closes #(\d+)\s*$", re.M | re.I)
PASSED = {"success", "skipped", "neutral"}


def gh(*args):
    out = subprocess.run(["gh", "api", *args], capture_output=True, text=True, check=True).stdout
    return json.loads(out) if out.strip() else None


def wait_for_checks(sha):
    """Returns the names of the checks that did not pass, after every check ends."""
    waited = 0.0
    while True:
        runs = gh(f"{REPO}/commits/{sha}/check-runs?per_page=100")["check_runs"]
        if runs and all(r["status"] == "completed" for r in runs):
            return [f'{r["name"]}: {r["conclusion"]}' for r in runs if r["conclusion"] not in PASSED]
        if waited >= CHECKS_LIMIT:
            return ["the checks did not end in time"]
        time.sleep(POLL)
        waited += max(POLL, 1)


def wait_for_issues(numbers):
    """Returns `#N state` for each issue, after all close or the time limit."""
    waited = 0.0
    while True:
        states = {n: gh(f"{REPO}/issues/{n}")["state"] for n in numbers}
        if all(s == "closed" for s in states.values()) or waited >= ISSUES_LIMIT:
            return [f"#{n} {s}" for n, s in states.items()]
        time.sleep(min(POLL, 5))
        waited += max(min(POLL, 5), 1)


def git(root, *args):
    return subprocess.run(["git", "-C", root, *args], capture_output=True, text=True, check=True).stdout


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
    git(root, "worktree", "remove", "--force", path)
    # A separate command: the branch delete writes .git/config, and a long one races the sandbox.
    git(root, "branch", "-D", branch)
    return f"removed {path} and branch {branch}"


def main(argv):
    number, root = argv[0], argv[1] if len(argv) > 1 else "."
    pr = gh(f"{REPO}/pulls/{number}")
    if pr["state"] != "open":
        print(f"merge_pr: FAIL: #{number} is {pr['state']}")
        return 1
    sha, branch = pr["head"]["sha"], pr["head"]["ref"]
    if gh(f"{REPO}/compare/{pr['base']['ref']}...{sha}")["behind_by"]:
        print(f"merge_pr: FAIL: {branch} does not contain {pr['base']['ref']}: merge it, run make test")
        return 1
    failed = wait_for_checks(sha)
    if failed:
        print("merge_pr: FAIL: " + "; ".join(failed))
        return 1
    merged = gh("-X", "PUT", f"{REPO}/pulls/{number}/merge", "-f", "merge_method=squash",
                "-f", f"sha={sha}", "-f", f"commit_title={pr['title']} (#{number})")
    print(f"merged #{number} as {merged['sha'][:8]}")
    issues = CLOSES.findall(pr["body"] or "")
    if issues:
        print("issues: " + ", ".join(wait_for_issues(issues)))
    print(remove_worktree(root, branch, sha))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
