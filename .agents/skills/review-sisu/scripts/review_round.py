"""Do the fixed steps of one review round, so that a caller does not type them by hand.

prepare fetches origin, builds sisu at both ends of a diff, writes the brief and records the
checkout. verify records the checkout again, compares the records and checks that each report
exists. A reviewer that wrote in the repository, or a report that is not there, makes verify fail.
save-report copies the final reply of a subagent reviewer from its transcript to its report file,
word for word. rulings checks that each finding of each report has a ruling, and counts them.
record only records the checkout, for a review that has no target to prepare, such as a
cross-check.

Run this file from any checkout. It runs origin/main's build_sisu.sh and diff_brief.py, so a
branch that started before a change to the criteria gets the current ones.

Prepare: python3 review_round.py prepare REPO RUN TARGET [--round N] [--old-main] [--pr-body FILE]
    [OPTION]...

Verify: python3 review_round.py verify REPO RUN --report STEM [--report STEM]... [--round N]

Save a report: python3 review_round.py save-report RUN STEM --agent ID [--round N]

Check the rulings: python3 review_round.py rulings RUN --report STEM [--report STEM]... [--round N]

Record only: python3 review_round.py record REPO RUN [--round N]

REPO is the root of the checkout that holds the target. A worktree is fine.
RUN is the run directory.
TARGET is BASE..HEAD for a diff, or the path of a spec or a plan with --kind spec|plan.
STEM is the first part of the name of a report, such as sol or opus: the report of round 1 is
sol.md and the report of round 2 is sol-2.md. A finding of a report starts with "### <n>.", and
its ruling in rulings-<N>.md starts with "<STEM> <n>: <ruling>". A report's path, such as
RUN/sol-2.md, is accepted in place of its STEM.

Each file name follows the round: round 1 writes brief.md and tree-1.txt, round 2 writes brief-2.md
and tree-2.txt. RUN/focus.md (focus-N.md in a later round) goes in when it exists. A later round
also takes rulings-<N-1>.md, which must exist, and each report in RUN: each Markdown file that is
not a brief, a focus, a rulings file, a copy of the target or diagnostic-issues.md.
Each other OPTION goes to diff_brief.py as it is, such as --doc, --mutation-log and --coverage.

prepare stops before it builds when a diff starts on main but origin/main is not an ancestor of
its head: main moved, so the diff is not what will merge. --old-main lets the diff of one task
through, such as task 1 of a branch, whose base is where the branch left main.

--pr-body FILE goes to diff_brief.py as one more --doc. prepare first checks each of its lines with
origin/main's scripts/text_limits.py, and stops when a line breaks a limit.

prepare also stops when a diff makes an "in N fields" count of docs/register.md larger. The Focus
or the pull request body must name the row by its Where cell, such as check::Body. A diff that
grows a debt row names the row (docs/register.md, How to use it).

verify also fails on a report whose first line does not start with "Model:".

verify names the commits when HEAD is the only thing that changed. It still fails, because a
reviewer can commit too: go on only when you made each of those commits.

ID is the agent ID that the Agent tool returned. save-report finds agent-ID.jsonl under
~/.claude/projects, or under --transcripts DIR. It reads the last SubagentHandback message of the
agent, or else its last text.
"""

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

SCRIPTS = ".agents/skills/review-sisu/scripts"
REGISTER = "docs/register.md"
# A debt row with a field count: its count, then its Where cell without the backticks.
FIELD_ROW = re.compile(r"^\|[^|]* in (\d+) fields[^|]*\|\s*`?([^|`]*?)`?\s*\|", re.MULTILINE)
# The Markdown files of a run directory that are not reports, by the stem of their name.
NOT_REPORTS = {"brief", "focus", "rulings", "target", "diagnostic-issues"}


def git(repo: Path, *args: str) -> bytes:
    """Run git in REPO and return its output."""
    return subprocess.run(["git", "-C", repo, *args], check=True, capture_output=True).stdout


def script(repo: Path, name: str) -> bytes:
    """Return origin/main's copy of a script of this skill."""
    return git(repo, "show", f"origin/main:{SCRIPTS}/{name}")


def show(repo: Path, rev: str, path: str) -> str:
    """Return PATH at the commit REV, or an empty string when REV has no such file."""
    args = ["git", "-C", repo, "show", f"{rev}:{path}"]
    result = subprocess.run(args, check=False, capture_output=True, text=True)
    return result.stdout if result.returncode == 0 else ""


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


def unnumbered(name: str) -> str:
    """Return a file stem without its round number: sol-2 gives sol."""
    return re.sub(r"-\d+$", "", name)


def stem_of(value: str, run: Path, round_number: int) -> str:
    """Return the STEM of `value`: a stem as it is, or the stem of a path such as RUN/sol.md.

    A path must name the report of RUN for this round, or the script stops.
    """
    if not value.endswith(".md"):
        return value
    path = Path(value)
    stem = unnumbered(path.stem)
    if path.name != numbered(stem, round_number) or (
        path.parent != Path() and path.parent.resolve() != run.resolve()
    ):
        raise SystemExit(
            f"review_round: {value} is not the report of round {round_number} in {run}"
        )
    return stem


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
        for report in sorted(run.glob("*.md")):
            if unnumbered(report.stem) not in NOT_REPORTS:
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
            " what will merge. Bring main in first (final-diff.md, step 1), or pass --old-main"
            " for the diff of one task or a range that main has passed."
        )


def field_counts(register: str) -> dict[str, int]:
    """Return the count of each "in N fields" row of the register, by its Where cell."""
    return {place: int(count) for count, place in FIELD_ROW.findall(register)}


def check_debt(repo: Path, run: Path, target: str, round_number: int, body: Path | None) -> None:
    """Stop when a diff grows a field count of the register, and no Focus or body names the row."""
    ends = diff_ends(target)
    if ends is None:
        return
    base, head = ends
    named = "".join(
        path.read_text(encoding="utf-8")
        for path in (run / numbered("focus", round_number), body)
        if path is not None and path.exists()
    )
    before = field_counts(show(repo, base, REGISTER))
    for place, count in field_counts(show(repo, head, REGISTER)).items():
        old = before.get(place)
        if old is not None and count > old and place not in named:
            raise SystemExit(
                f"review_round: the diff grows the `{place}` row of {REGISTER} from {old} to"
                f" {count} fields. Name the row and the reason in the Focus or the pull request"
                f" body ({REGISTER}, How to use it)."
            )


def check_body(repo: Path, body: Path | None) -> list[str]:
    """Stop when a line of the pull request body breaks a limit of text_limits.py.

    Return the options that give the body to diff_brief.py.
    """
    if body is None:
        return []
    # text_limits.py reads prose by the file name, so another name would pass with no check.
    if body.suffix != ".md":
        raise SystemExit(f"review_round: {body}: the pull request body must be a .md file")
    result = subprocess.run(
        ["python3", "-", "--whole", str(body)],
        input=git(repo, "show", "origin/main:scripts/text_limits.py"),
        check=False,
        capture_output=True,
    )
    if result.returncode != 0:
        found = (result.stdout + result.stderr).decode()
        raise SystemExit(f"review_round: correct the pull request body:\n{found}")
    return ["--doc", str(body)]


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
    for path in paths:
        if path not in missing and not path.read_text(encoding="utf-8").startswith("Model:"):
            print(f"review_round: no Model line: {path}")
            missing.append(path)
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


def rulings(run: Path, round_number: int, reports: list[str]) -> int:
    """Return 1 when a finding of a report has no ruling. Print the rulings of each reviewer."""
    path = run / f"rulings-{round_number}.md"
    lines = path.read_text(encoding="utf-8") if path.exists() else ""
    # The ruling ends at the next colon; "Defer to <where>" with a <where> counts as Defer.
    ruled = {
        finding: "Defer" if ruling.startswith("Defer to ") and ruling[9:].strip() else ruling
        for finding, ruling in re.findall(r"^(\S+ \d+): ([^:\n]+)", lines, flags=re.MULTILINE)
    }
    status = 0
    for stem in reports:
        report = (run / numbered(stem, round_number)).read_text(encoding="utf-8")
        found = re.findall(r"^### (\d+)\.", report, flags=re.MULTILINE)
        counts: dict[str, int] = {}
        for n in found:
            ruling = ruled.get(f"{stem} {n}")
            if ruling is None:
                print(f"review_round: no ruling: {stem} {n}")
                status = 1
            elif ruling not in ("Accept", "Reject", "Defer"):
                print(f"review_round: not Accept, Reject or Defer: {stem} {n}: {ruling}")
                status = 1
            else:
                counts[ruling] = counts.get(ruling, 0) + 1
        tally = ", ".join(f"{count} {ruling}" for ruling, count in counts.items())
        print(f"{stem}: {len(found)} findings: {tally or 'none'}")
    return status


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
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            raise SystemExit(f"review_round: {transcript} holds a line that is not JSON") from None
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


def stems(args: argparse.Namespace) -> list[str]:
    """Return the STEM of each --report value of the command line."""
    return [stem_of(r, args.run, args.round) for r in args.report]


def main() -> int:
    """Run the action that the command line names."""
    parser = argparse.ArgumentParser(description="Do the fixed steps of one review round.")
    commands = parser.add_subparsers(dest="command", required=True)
    prepare_parser = commands.add_parser("prepare")
    verify_parser = commands.add_parser("verify")
    save_parser = commands.add_parser("save-report")
    rulings_parser = commands.add_parser("rulings")
    record_parser = commands.add_parser("record")
    rulings_parser.add_argument("run", type=Path)
    rulings_parser.add_argument("--report", action="append", required=True, metavar="STEM")
    for command in (prepare_parser, verify_parser, record_parser):
        command.add_argument("repo", type=Path)
        command.add_argument("run", type=Path)
    prepare_parser.add_argument("target")
    prepare_parser.add_argument("--old-main", action="store_true")
    prepare_parser.add_argument("--pr-body", type=Path, metavar="FILE")
    verify_parser.add_argument("--report", action="append", required=True, metavar="STEM")
    save_parser.add_argument("run", type=Path)
    save_parser.add_argument("stem")
    save_parser.add_argument("--agent", required=True, metavar="ID")
    save_parser.add_argument(
        "--transcripts", type=Path, default=Path.home() / ".claude" / "projects"
    )
    for command in (prepare_parser, verify_parser, save_parser, rulings_parser, record_parser):
        command.add_argument("--round", type=int, default=1)
    args, extra = parser.parse_known_args()
    if args.command != "prepare" and extra:
        parser.error(f"unrecognized arguments: {' '.join(extra)}")
    if args.command == "verify":
        return verify(args.repo, args.run, args.round, stems(args))
    if args.command == "rulings":
        return rulings(args.run, args.round, stems(args))
    if args.command == "record":
        record(args.repo, args.run / f"tree-{args.round}.txt")
        return 0
    if args.command == "save-report":
        stem = stem_of(args.stem, args.run, args.round)
        return save_report(args.run, stem, args.agent, args.round, args.transcripts)
    # parse_known_args would take the value of an option before TARGET as the target.
    if args.target != sys.argv[4]:
        parser.error("write TARGET just after RUN, before each option")
    git(args.repo, "fetch", "origin")
    if not args.old_main:
        check_main(args.repo, args.target)
    extra += check_body(args.repo, args.pr_body)
    check_debt(args.repo, args.run, args.target, args.round, args.pr_body)
    prepare(args.repo, args.run, args.target, args.round, extra)
    return 0


if __name__ == "__main__":
    sys.exit(main())
