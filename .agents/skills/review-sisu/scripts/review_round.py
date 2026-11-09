"""Do the fixed steps of one review round, so that a caller does not type them by hand.

prepare fetches origin, builds sisu at both ends of a diff, writes the brief and records the
checkout. verify records the checkout again, compares the records and checks that each report
exists. A reviewer that wrote in the repository, or a report that is not there, makes verify fail.

Run this file from any checkout. It runs origin/main's build_sisu.sh and diff_brief.py, so a
branch that started before a change to the criteria gets the current ones.

Prepare: python3 review_round.py prepare REPO RUN TARGET [--round N] [DIFF_BRIEF_OPTION]...

Verify: python3 review_round.py verify REPO RUN --report STEM [--report STEM]... [--round N]

REPO is the root of the checkout that holds the target. A worktree is fine.
RUN is the run directory.
TARGET is BASE..HEAD for a diff, or the path of a spec or a plan with --kind spec|plan.
STEM is the first part of the name of a report, such as sol or opus: the report of round 1 is
sol.md and the report of round 2 is sol-2.md.

Each file name follows the round: round 1 writes brief.md and tree-1.txt, round 2 writes brief-2.md
and tree-2.txt. RUN/focus.md (focus-N.md in a later round) goes in when it exists. A later round
also takes rulings-<N-1>.md, which must exist, and each report named sol*.md or opus*.md in RUN.
Each other option goes to diff_brief.py as it is, such as --doc, --mutation-log and --coverage.
"""

import argparse
import filecmp
import subprocess
import sys
from pathlib import Path

SCRIPTS = ".agents/skills/review-sisu/scripts"


def git(repo: Path, *args: str) -> bytes:
    """Run git in REPO and return its output."""
    return subprocess.run(["git", "-C", repo, *args], check=True, capture_output=True).stdout


def script(repo: Path, name: str) -> bytes:
    """Return origin/main's copy of a script of this skill."""
    return git(repo, "show", f"origin/main:{SCRIPTS}/{name}")


def numbered(stem: str, round_number: int) -> str:
    """Return a Markdown file name: only a later round has a number in it."""
    return f"{stem}.md" if round_number == 1 else f"{stem}-{round_number}.md"


def record(repo: Path, path: Path) -> None:
    """Write the head, the status and the uncommitted diff of the checkout to PATH."""
    path.write_bytes(
        git(repo, "rev-parse", "HEAD")
        + git(repo, "status", "--porcelain", "--untracked-files=all")
        + git(repo, "diff", "HEAD")
    )


def round_options(run: Path, round_number: int) -> list[str]:
    """Return the options of diff_brief.py that the files in RUN decide."""
    options = ["--round", str(round_number)]
    focus = run / numbered("focus", round_number)
    if focus.exists():
        options += ["--focus", str(focus)]
    if round_number > 1:
        rulings = run / f"rulings-{round_number - 1}.md"
        if not rulings.exists():
            raise SystemExit(f"review_round: {rulings} does not exist")
        options += ["--rulings", str(rulings)]
        for report in sorted([*run.glob("sol*.md"), *run.glob("opus*.md")]):
            options += ["--prior", str(report)]
    return options


def prepare(repo: Path, run: Path, target: str, round_number: int, extra: list[str]) -> None:
    """Build both ends of a diff, write the brief and record the checkout."""
    git(repo, "fetch", "origin")
    if ".." in target and not Path(target).exists():
        base, head = target.split("..", 1)
        subprocess.run(
            ["bash", "-s", "--", repo, run, base, head],
            input=script(repo, "build_sisu.sh"),
            check=True,
        )
    brief = run / numbered("brief", round_number)
    subprocess.run(
        ["python3", "-", repo, target, brief, *round_options(run, round_number), *extra],
        input=script(repo, "diff_brief.py"),
        check=True,
    )
    record(repo, run / f"tree-{round_number}.txt")
    print(brief)


def verify(repo: Path, run: Path, round_number: int, reports: list[str]) -> int:
    """Return 1 when the checkout differs from the record of prepare or a report is missing."""
    before = run / f"tree-{round_number}.txt"
    after = run / f"tree-{round_number}-after.txt"
    if not before.exists():
        raise SystemExit(f"review_round: {before} does not exist: run prepare first")
    record(repo, after)
    paths = [run / numbered(stem, round_number) for stem in reports]
    missing = [path for path in paths if not path.exists()]
    for path in missing:
        print(f"review_round: no report: {path}")
    if not filecmp.cmp(before, after, shallow=False):
        print(f"review_round: a reviewer changed the checkout: compare {before} and {after}")
        return 1
    print("review_round: the checkout did not change")
    return 1 if missing else 0


def main() -> int:
    """Run the action that the command line names."""
    parser = argparse.ArgumentParser(description="Do the fixed steps of one review round.")
    commands = parser.add_subparsers(dest="command", required=True)
    prepare_parser = commands.add_parser("prepare")
    verify_parser = commands.add_parser("verify")
    for command in (prepare_parser, verify_parser):
        command.add_argument("repo", type=Path)
        command.add_argument("run", type=Path)
    prepare_parser.add_argument("target")
    verify_parser.add_argument("--report", action="append", required=True, metavar="STEM")
    for command in (prepare_parser, verify_parser):
        command.add_argument("--round", type=int, default=1)
    args, extra = parser.parse_known_args()
    if args.command == "verify":
        if extra:
            parser.error(f"unrecognized arguments: {' '.join(extra)}")
        return verify(args.repo, args.run, args.round, args.report)
    # parse_known_args would take the value of an option before TARGET as the target.
    if args.target != sys.argv[4]:
        parser.error("write TARGET just after RUN, before each option")
    prepare(args.repo, args.run, args.target, args.round, extra)
    return 0


if __name__ == "__main__":
    sys.exit(main())
