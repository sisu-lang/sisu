"""Do the fixed steps of one review round, so that a caller does not type them by hand.

prepare fetches origin, builds sisu at both ends of a diff, writes the brief and records the
checkout. verify records the checkout again, compares the records and checks that each report
exists. A reviewer that wrote in the repository, or a report that is not there, makes verify fail.
save-report copies the final reply of a subagent reviewer from its transcript to its report file,
word for word.

Run this file from any checkout. It runs origin/main's build_sisu.sh and diff_brief.py, so a
branch that started before a change to the criteria gets the current ones.

Prepare: python3 review_round.py prepare REPO RUN TARGET [--round N] [--old-main] [OPTION]...

Verify: python3 review_round.py verify REPO RUN --report STEM [--report STEM]... [--round N]

Save a report: python3 review_round.py save-report RUN STEM --agent ID [--round N]

REPO is the root of the checkout that holds the target. A worktree is fine.
RUN is the run directory.
TARGET is BASE..HEAD for a diff, or the path of a spec or a plan with --kind spec|plan.
STEM is the first part of the name of a report, such as sol or opus: the report of round 1 is
sol.md and the report of round 2 is sol-2.md.

Each file name follows the round: round 1 writes brief.md and tree-1.txt, round 2 writes brief-2.md
and tree-2.txt. RUN/focus.md (focus-N.md in a later round) goes in when it exists. A later round
also takes rulings-<N-1>.md, which must exist, and each report named sol*.md or opus*.md in RUN.
Each other OPTION goes to diff_brief.py as it is, such as --doc, --mutation-log and --coverage.

prepare stops before it builds when a diff starts on main but origin/main is not an ancestor of
its head: main moved, so the diff is not what will merge. --old-main lets the diff of one task
through, such as task 1 of a branch, whose base is where the branch left main.

verify names the commits when HEAD is the only thing that changed. It still fails, because a
reviewer can commit too: go on only when you made each of those commits.

ID is the agent ID that the Agent tool returned. save-report finds agent-ID.jsonl under
~/.claude/projects, or under --transcripts DIR. It reads the last SubagentHandback message of the
agent, or else its last text.
"""

import argparse
import json
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


def is_ancestor(repo: Path, a: str, b: str) -> bool:
    """Tell whether the commit A is an ancestor of the commit B. Stop when git cannot tell."""
    args = ["git", "-C", repo, "merge-base", "--is-ancestor", a, b]
    result = subprocess.run(args, check=False, capture_output=True, text=True)
    if result.returncode not in {0, 1}:
        raise SystemExit(f"review_round: {result.stderr.strip()}")
    return result.returncode == 0


def diff_ends(target: str) -> tuple[str, str] | None:
    """Return the base and the head of a diff TARGET, or None for the path of a spec or a plan."""
    if ".." in target and not Path(target).exists():
        base, head = target.split("..", 1)
        return base, head
    return None


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


def check_main(repo: Path, target: str) -> None:
    """Stop when a diff starts on main but origin/main is not an ancestor of its head."""
    ends = diff_ends(target)
    if ends is None:
        return
    base, head = ends
    main = "origin/main"
    if is_ancestor(repo, base, main) and not is_ancestor(repo, main, head):
        raise SystemExit(
            f"review_round: {main} is not an ancestor of {head}: main moved, so the diff is not"
            " what will merge. Bring main in first (SKILL.md, step 1.5), or pass --old-main"
            " for the diff of one task or a range that main has passed."
        )


def prepare(repo: Path, run: Path, target: str, round_number: int, extra: list[str]) -> None:
    """Build both ends of a diff, write the brief and record the checkout."""
    if ends := diff_ends(target):
        base, head = ends
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
    missing = [path for path in paths if not (path.is_file() and path.stat().st_size > 0)]
    for path in missing:
        print(f"review_round: no report: {path}")
    old_head, old_tree = before.read_bytes().split(b"\n", 1)
    new_head, new_tree = after.read_bytes().split(b"\n", 1)
    if old_tree != new_tree:
        print(f"review_round: a reviewer changed the checkout: compare {before} and {after}")
        return 1
    if old_head != new_head:
        commits = git(
            repo, "log", "--format=%h %an %s", f"{old_head.decode()}..{new_head.decode()}"
        )
        print(
            f"review_round: only HEAD moved after prepare; the work tree is the same. The new"
            f" commits:\n{commits.decode()}review_round: when you made each of them, no reviewer"
            " changed the checkout"
        )
        return 1
    print("review_round: the checkout did not change")
    return 1 if missing else 0


def handback(transcript: Path) -> str:
    """Return the last SubagentHandback message of a subagent transcript.

    Without one, return the text of the last assistant entry when that entry ends with text: an
    agent that stopped during its work gives no reply. Each JSON line ends at a newline only,
    because a reply can hold U+2028 or U+0085 as they are.
    """
    message, last = "", None
    for line in transcript.read_text(encoding="utf-8").split("\n"):
        if not line.strip():
            continue
        entry = json.loads(line)
        content = entry.get("message", {}).get("content")
        if entry.get("type") != "assistant" or not isinstance(content, list) or not content:
            continue
        last = content[-1]
        for block in content:
            if block.get("type") == "tool_use" and block.get("name") == "SubagentHandback":
                message = block.get("input", {}).get("message", "")
    if message or last is None or last.get("type") != "text":
        return message
    return last.get("text", "")


def save_report(run: Path, stem: str, agent: str, round_number: int, transcripts: Path) -> int:
    """Write the final reply of the agent AGENT to RUN/<stem>.md, word for word."""
    name = f"agent-{agent.removeprefix('agent-')}.jsonl"
    found = sorted(transcripts.glob(f"*/*/subagents/{name}"))
    if len(found) != 1:
        print(f"review_round: {len(found)} transcripts named {name} under {transcripts}")
        return 1
    reply = handback(found[0])
    if not reply.strip():
        print(f"review_round: {found[0]} holds no final reply")
        return 1
    report = run / numbered(stem, round_number)
    report.write_text(reply if reply.endswith("\n") else reply + "\n", encoding="utf-8")
    print(report)
    return 0


def main() -> int:
    """Run the action that the command line names."""
    parser = argparse.ArgumentParser(description="Do the fixed steps of one review round.")
    commands = parser.add_subparsers(dest="command", required=True)
    prepare_parser = commands.add_parser("prepare")
    verify_parser = commands.add_parser("verify")
    save_parser = commands.add_parser("save-report")
    for command in (prepare_parser, verify_parser):
        command.add_argument("repo", type=Path)
        command.add_argument("run", type=Path)
    prepare_parser.add_argument("target")
    prepare_parser.add_argument("--old-main", action="store_true")
    verify_parser.add_argument("--report", action="append", required=True, metavar="STEM")
    save_parser.add_argument("run", type=Path)
    save_parser.add_argument("stem")
    save_parser.add_argument("--agent", required=True, metavar="ID")
    save_parser.add_argument(
        "--transcripts", type=Path, default=Path.home() / ".claude" / "projects"
    )
    for command in (prepare_parser, verify_parser, save_parser):
        command.add_argument("--round", type=int, default=1)
    args, extra = parser.parse_known_args()
    if args.command != "prepare" and extra:
        parser.error(f"unrecognized arguments: {' '.join(extra)}")
    if args.command == "verify":
        return verify(args.repo, args.run, args.round, args.report)
    if args.command == "save-report":
        return save_report(args.run, args.stem, args.agent, args.round, args.transcripts)
    # parse_known_args would take the value of an option before TARGET as the target.
    if args.target != sys.argv[4]:
        parser.error("write TARGET just after RUN, before each option")
    git(args.repo, "fetch", "origin")
    if not args.old_main:
        check_main(args.repo, args.target)
    prepare(args.repo, args.run, args.target, args.round, extra)
    return 0


if __name__ == "__main__":
    sys.exit(main())
