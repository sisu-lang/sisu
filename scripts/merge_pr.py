"""Squash-merges a pull request of sisu-lang/sisu when it is safe, and removes its local worktree.

Usage: python3 scripts/merge_pr.py [--cleanup] PR [REPO_ROOT]

Run it only after the user says to merge. It stops, with no merge, when a check fails or the head
does not contain main. It waits for running checks, merges at the head it checked, waits for each
`Closes #N` issue to close, and removes a clean local worktree whose branch is at the merged head.
With `--cleanup`, it merges nothing. It is for a pull request that was merged in the GitHub UI.
It removes the same worktree and branch, under the same rules.
It calls `gh api` (REST), so it needs the network: run it with the sandbox off.
"""

import argparse
import json
import os
import re
import subprocess
import sys
import time
from typing import Any

REPO = "repos/sisu-lang/sisu"
POLL = float(os.environ.get("MERGE_PR_POLL", "30"))
CHECKS_LIMIT = float(os.environ.get("MERGE_PR_CHECKS_LIMIT", str(90 * 60)))
NO_CHECKS_LIMIT = min(CHECKS_LIMIT, 10 * 60)
ISSUES_LIMIT = 120
CLOSES = re.compile(r"^Closes #(\d+)\s*$", re.MULTILINE | re.IGNORECASE)
PASSED = {"success", "skipped", "neutral"}


class Stop(Exception):
    pass


def run(args: list[str]) -> str:
    try:
        return subprocess.run(args, capture_output=True, text=True, check=True).stdout
    except subprocess.CalledProcessError as e:
        raise Stop(f"{' '.join(args[:4])}: {e.stderr.strip() or e}") from None


def gh(*args: str) -> dict[str, Any]:
    out = run(["gh", "api", *args])
    # pyrefly: ignore[bad-return]  an empty answer gives None, and no caller reads an empty answer
    return json.loads(out) if out.strip() else None


def wait_for_checks(pr: dict[str, Any], sha: str) -> list[str]:
    """Returns the checks that did not pass, after every check ends. Stops at the time limit, and
    when main moves while a check runs: the head must then merge main and run its checks again.
    """
    start = time.monotonic()
    while True:
        page = gh(f"{REPO}/commits/{sha}/check-runs?per_page=100")
        runs = page["check_runs"]
        if page["total_count"] > len(runs):
            raise Stop(f"{page['total_count']} checks, more than one page: merge by hand")
        if runs and all(r["status"] == "completed" for r in runs):
            return [
                f"{r['name']}: {r['conclusion']}" for r in runs if r["conclusion"] not in PASSED
            ]
        waited = time.monotonic() - start
        if not runs and waited >= NO_CHECKS_LIMIT:
            raise Stop("the head has no checks")
        if waited >= CHECKS_LIMIT:
            raise Stop("the checks did not end in time")
        check_base(pr, sha)
        time.sleep(POLL)


def check_base(pr: dict[str, Any], sha: str) -> None:
    if gh(f"{REPO}/compare/{pr['base']['ref']}...{sha}")["behind_by"]:
        raise Stop(
            f"{pr['head']['ref']} does not contain {pr['base']['ref']}: merge it, run make test"
        )


def wait_for_issues(numbers: list[str]) -> list[str]:
    """Returns `#N state` for each issue, after all close or the time limit."""
    start = time.monotonic()
    while True:
        states = {n: gh(f"{REPO}/issues/{n}")["state"] for n in numbers}
        if all(s == "closed" for s in states.values()) or time.monotonic() - start >= ISSUES_LIMIT:
            return [f"#{n} {s}" for n, s in states.items()]
        time.sleep(min(POLL, 5))


def git(root: str, *args: str) -> str:
    return run(["git", "-C", root, *args])


def remove_worktree(root: str, branch: str, sha: str) -> str:
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


def merge(number: str, root: str) -> None:
    pr = gh(f"{REPO}/pulls/{number}")
    if pr["state"] != "open":
        raise Stop(f"#{number} is {pr['state']}")
    sha = pr["head"]["sha"]
    try:
        check_base(pr, sha)
    except Stop:
        # Right after a push, GitHub can still answer with the old head. Read the pull request again.
        time.sleep(POLL)
        pr = gh(f"{REPO}/pulls/{number}")
        sha = pr["head"]["sha"]
        check_base(pr, sha)
    failed = wait_for_checks(pr, sha)
    if failed:
        raise Stop("; ".join(failed))
    # main can move during the wait.
    check_base(pr, sha)
    merged = gh(
        "-X",
        "PUT",
        f"{REPO}/pulls/{number}/merge",
        "-f",
        "merge_method=squash",
        "-f",
        f"sha={sha}",
        "-f",
        f"commit_title={pr['title']} (#{number})",
    )
    print(f"merged #{number} as {merged['sha'][:8]}")
    issues = CLOSES.findall(pr["body"] or "")
    if issues:
        print("issues: " + ", ".join(wait_for_issues(issues)))
    print(remove_worktree(root, pr["head"]["ref"], sha))


def cleanup(number: str, root: str) -> None:
    pr = gh(f"{REPO}/pulls/{number}")
    if not pr["merged"]:
        raise Stop(f"#{number} is not merged")
    print(remove_worktree(root, pr["head"]["ref"], pr["head"]["sha"]))


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Squash-merges a pull request of sisu-lang/sisu.")
    parser.add_argument("pr")
    parser.add_argument("root", nargs="?", default=".")
    parser.add_argument("--cleanup", action="store_true", help="remove the worktree of a merged PR")
    args = parser.parse_intermixed_args(argv)
    try:
        (cleanup if args.cleanup else merge)(args.pr, args.root)
    except Stop as e:
        print(f"merge_pr: FAIL: {e}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
