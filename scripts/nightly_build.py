"""Steps of the nightly build workflow that need the GitHub API. Usage:

  nightly_build.py pick
  nightly_build.py publish --sha <sha>
  nightly_build.py prune
  nightly_build.py report

`pick` chooses the commit of the nightly build. It reads GITHUB_REPOSITORY, GITHUB_REF, HEAD_SHA
and GH_TOKEN, and writes `sha=`, `build=` and `publish=` to $GITHUB_OUTPUT and its answer to
$GITHUB_STEP_SUMMARY. On `main` it takes the newest commit that has a successful `ci.yml` push
run. The commit must be newer than the commit of the newest published `nightly-*` release. When
the commit has other `.github/workflows/` files than `main`, it takes nothing, because
GITHUB_TOKEN cannot create a release for such a commit. Any other ref is a dry run: it builds
HEAD_SHA, publishes nothing and prints what `main` would do.

`pick` exits 0 on every answer, and 1 when a `gh` call fails. It only reads.

`publish` checks again that --sha is newer than the newest nightly build. It checks too that the
workflow files are still those of `main`. When a check fails, it writes `published=false` and
exits 0. Else it takes the six assets from the artifacts of the run (GITHUB_RUN_ID) and deletes
each `nightly-*` draft. Then it creates a draft release and uploads the assets. In one call it
publishes the draft and marks it latest. Then it writes `published=true`.

`prune` deletes the published `nightly-*` releases beyond the 14 newest, with their tags.

`report` reads NEEDS (the JSON of `toJSON(needs)`) and RUN_URL. It opens the issue
`Nightly build failure`, or it comments on that issue when it is open.

Every command calls `gh api` (REST) or `gh release`, so it needs the network. Python 3.10
standard library only.
"""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import zipfile
from datetime import datetime, timezone

NO_NEW = "no new green commit"
DIFFER = "the newest green commit has other workflow files than `main`"
NEWER = "the commit is not newer than the newest nightly build"
KEEP = 14
TITLE = "Nightly build failure"
LABELS = ["needs-triage", "bug", "nightly-build"]
ASSETS = [
    "sisu-x86_64-linux.tar.gz",
    "sisu-aarch64-linux.tar.gz",
    "sisu-aarch64-macos.tar.gz",
    "install.sh",
    "orders.sisu",
    "SHA256SUMS",
]


def gh(*args: str, stdin: str | None = None) -> object:
    out = subprocess.run(
        ["gh", "api", *args], capture_output=True, text=True, check=True, input=stdin
    ).stdout
    return json.loads(out) if out.strip() else None


def gh_release(*args: str) -> None:
    subprocess.run(["gh", "release", *args], capture_output=True, text=True, check=True)


def releases(repo: str) -> list[dict]:
    # `prune` needs every release, and gh gives one array of all pages when its output is a pipe.
    return gh("--paginate", f"repos/{repo}/releases?per_page=100")


def nightly_releases(repo: str) -> list[dict]:
    """The published `nightly-*` releases, the largest `published_at` first. A draft has none."""
    found = [
        r
        for r in releases(repo)
        if not r["draft"] and r["published_at"] and r["tag_name"].startswith("nightly-")
    ]
    return sorted(found, key=lambda r: r["published_at"], reverse=True)


def newest_green(commits: list[str], green: set[str], released: str | None) -> str | None:
    """The first commit, newest first, that is in `green`, or None at the commit of `released`."""
    for commit in commits:
        if released and commit.startswith(released):
            return None
        if commit in green:
            return commit
    return None


def workflows_tree(repo: str, ref: str) -> str:
    entries = gh(f"repos/{repo}/contents/.github?ref={ref}")
    return next(e["sha"] for e in entries if e["name"] == "workflows")


def released_commit(repo: str) -> tuple[list[str], str | None]:
    """The commits of `main`, newest first, and the short SHA of the newest nightly build."""
    # shortcut: reads one page of 100, more than 100 commits between two nightly builds needs
    # paging
    commits = [c["sha"] for c in gh(f"repos/{repo}/commits?sha=main&per_page=100")]
    newest = nightly_releases(repo)
    if not newest:
        return commits, None
    # A release's tag is the only store of its commit: `nightly-<date>-<short sha>`.
    tag = newest[0]["tag_name"]
    short = tag.rsplit("-", 1)[1]
    if not re.fullmatch("[0-9a-f]{7}", short):
        sys.exit(f"nightly_build.py: the newest nightly tag {tag} does not end with a short SHA")
    return commits, short


def answer_of_main(repo: str) -> tuple[str, str | None]:
    """The text of the answer, and the commit to build or None."""
    commits, released = released_commit(repo)
    # shortcut: reads one page of 100, more than 100 CI successes between two nightly builds
    # needs paging
    runs = gh(
        f"repos/{repo}/actions/workflows/ci.yml/runs?branch=main&event=push"
        "&status=success&per_page=100"
    )
    green = {r["head_sha"] for r in runs["workflow_runs"]}
    sha = newest_green(commits, green, released)
    if sha is None:
        return NO_NEW, None
    if workflows_tree(repo, sha) != workflows_tree(repo, "main"):
        return DIFFER, None
    return f"build and publish {sha}", sha


def append(path: str, text: str) -> None:
    with open(path, "a", encoding="utf-8") as f:
        f.write(text)


def pick() -> int:
    repo = os.environ["GITHUB_REPOSITORY"]
    text, sha = answer_of_main(repo)
    if os.environ["GITHUB_REF"] == "refs/heads/main":
        outputs = (sha or "", "true" if sha else "false", "true" if sha else "false")
    else:
        text = f"main would: {text}"
        outputs = (os.environ["HEAD_SHA"], "true", "false")
    print(text)
    append(os.environ["GITHUB_STEP_SUMMARY"], text + "\n")
    append(os.environ["GITHUB_OUTPUT"], "sha={}\nbuild={}\npublish={}\n".format(*outputs))
    return 0


def summarize(text: str, published: bool) -> None:
    print(text)
    append(os.environ["GITHUB_STEP_SUMMARY"], text + "\n")
    append(os.environ["GITHUB_OUTPUT"], f"published={'true' if published else 'false'}\n")


def recheck(repo: str, sha: str) -> str | None:
    """Why `sha` must not be published now, or None."""
    commits, released = released_commit(repo)
    # This is the walk of `pick`, with `sha` as the only green commit.
    # It gives None for a sha at or after the released commit, and for a sha not in the list.
    if newest_green(commits, {sha}, released) is None:
        return NEWER
    if workflows_tree(repo, sha) != workflows_tree(repo, "main"):
        return DIFFER
    return None


def download(repo: str, run_id: str, into: str) -> dict[str, str]:
    """The paths of the six assets, found by file name in the artifacts of the run."""
    pages = gh(
        "--paginate", "--slurp", f"repos/{repo}/actions/runs/{run_id}/artifacts?per_page=100"
    )
    newest: dict[str, int] = {}
    for artifact in (a for page in pages for a in page["artifacts"]):
        # shortcut: same rule as download-artifact at the pin, when the pin moves read
        # filterLatest and change this to match
        newest[artifact["name"]] = max(artifact["id"], newest.get(artifact["name"], 0))
    found: dict[str, str] = {}
    owner: dict[str, str] = {}
    for name, id_ in newest.items():
        # The id names the local files: an artifact name can hold `..` or `/`.
        archive = os.path.join(into, f"{id_}.zip")
        with open(archive, "wb") as f:
            subprocess.run(
                ["gh", "api", f"repos/{repo}/actions/artifacts/{id_}/zip"],
                stdout=f,
                stderr=subprocess.PIPE,
                text=True,
                check=True,
            )
        with zipfile.ZipFile(archive) as z:
            z.extractall(os.path.join(into, str(id_)))
        for folder, _, files in os.walk(os.path.join(into, str(id_))):
            for f in (f for f in files if f in ASSETS):
                here = f"{name} ({os.path.relpath(os.path.join(folder, f), into)})"
                if f in found:
                    sys.exit(f"nightly_build.py: {f} is in the artifacts {owner[f]} and {here}")
                found[f], owner[f] = os.path.join(folder, f), here
    missing = [a for a in ASSETS if a not in found]
    if missing:
        sys.exit(f"nightly_build.py: the artifacts of run {run_id} lack {', '.join(missing)}")
    return found


def publish(sha: str) -> int:
    repo = os.environ["GITHUB_REPOSITORY"]
    reason = recheck(repo, sha)
    if reason:
        summarize(reason, False)
        return 0
    with tempfile.TemporaryDirectory() as tmp:
        files = download(repo, os.environ["GITHUB_RUN_ID"], tmp)
        # A run on `main` cannot overlap another, so a `nightly-*` draft is what a failed run left.
        for r in releases(repo):
            if r["draft"] and r["tag_name"].startswith("nightly-"):
                gh("--method", "DELETE", f"repos/{repo}/releases/{r['id']}")
        tag = f"nightly-{datetime.now(timezone.utc):%Y-%m-%d}-{sha[:7]}"
        gh_release(
            "create",
            tag,
            "--repo",
            repo,
            "--draft",
            "--target",
            sha,
            "--title",
            tag,
            "--notes",
            f"Nightly build of commit {sha}.",
            "--latest=false",
        )
        gh_release("upload", tag, "--repo", repo, *(files[a] for a in ASSETS))
        # One request publishes the draft and marks it latest.
        gh_release("edit", tag, "--repo", repo, "--draft=false", "--latest")
    summarize(f"published {tag}", True)
    return 0


def prune() -> int:
    repo = os.environ["GITHUB_REPOSITORY"]
    failed = []
    for r in nightly_releases(repo)[KEEP:]:
        try:
            gh_release("delete", r["tag_name"], "--repo", repo, "--cleanup-tag", "--yes")
        except subprocess.CalledProcessError as e:
            print(f"gh release delete {r['tag_name']}: {e.stderr.strip() or e}", file=sys.stderr)
            failed.append(r["tag_name"])
    return 1 if failed else 0


def report() -> int:
    repo = os.environ["GITHUB_REPOSITORY"]
    needs = json.loads(os.environ["NEEDS"])
    jobs = ", ".join(
        f"{name}: {job['result']}"
        for name, job in needs.items()
        if job["result"] in ("failure", "cancelled")
    )
    body = f"The nightly build failed.\n\nRun: {os.environ['RUN_URL']}\nJobs: {jobs}"
    # The issues list holds pull requests too, and they have a `pull_request` key.
    open_issues = gh(
        "--paginate", f"repos/{repo}/issues?state=open&labels=nightly-build&per_page=100"
    )
    found = [i for i in open_issues if not i.get("pull_request") and i["title"] == TITLE]
    if found:
        gh(
            f"repos/{repo}/issues/{found[0]['number']}/comments",
            "--input",
            "-",
            stdin=json.dumps({"body": body}),
        )
    else:
        gh(
            f"repos/{repo}/issues",
            "--input",
            "-",
            stdin=json.dumps({"title": TITLE, "body": body, "labels": LABELS}),
        )
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawTextHelpFormatter
    )
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("pick", help="choose the commit of the nightly build")
    publisher = commands.add_parser("publish", help="publish the draft release of a nightly build")
    publisher.add_argument("--sha", required=True)
    commands.add_parser("prune", help="delete the nightly builds beyond the 14 newest")
    commands.add_parser("report", help="open or update the issue of a failed run")
    args = parser.parse_args(argv)
    try:
        if args.command == "publish":
            return publish(args.sha)
        return {"pick": pick, "prune": prune, "report": report}[args.command]()
    except subprocess.CalledProcessError as e:
        print(f"{' '.join(e.cmd[:3])}: {e.stderr.strip() or e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
