"""Write a reviewer's brief from origin/main's brief.md.

The script fills every slot above "# Report", keeps the sections for the target's kind and drops
the others. It fails, writing nothing, when a slot above "# Report" is left unfilled, so a
template change cannot slip past it. A git command that fails stops the script with its error, so
a failure cannot read as an empty result.

Usage: python3 diff_brief.py REPO TARGET OUT [--kind spec|plan|diff] [--doc PATH]... [--focus FILE]
           [--mutation-log LOG]... [--coverage FILE] [--round N --prior REPORT... --rulings FILE]
  REPO     the repository root that holds the target (a worktree is fine)
  TARGET   for a diff, BASE..HEAD, written into the brief as full SHAs so that a later commit or
           fetch cannot move it; for a spec or a plan, its path, copied to target-<round>.md next
           to OUT, because these documents are not committed and change between rounds
  --kind   diff by default
  --doc    a spec or plan the reviewer reads with the target; repeat for each
REPO and each --doc are written as absolute paths: the reviewer runs in REPO, and a doc under
the main checkout's gitignored docs/superpowers/ is not in a worktree.
  --focus  a file whose text goes under "# Focus"; without it, Focus is "none"
  --mutation-log  the log of `make mutation-test LOG=...` for the diff; repeat for each log, such as
           one for each part of a large diff. Focus has one line for each log, with its absolute
           path, so the reviewer checks its summary line. Fails when a file does not exist, or
           when the summary line of a log names a commit outside the range. A log measured
           at an earlier commit of the range is accepted, and Focus lists the commits after it,
           because later commits (such as a fix round's) are not mutated. A log
           measured before a rebase is accepted when a commit of the range has the same
           `git patch-id` as the measured commit; Focus names both. With --round above 1,
           a log measured at the range's base or before it is accepted as an earlier round's
           log, and Focus says so.
  --coverage  the output of `make coverage` for the diff; Focus names it. Fails when the file does
           not exist, or when its last `coverage:` line does not name the diff's head commit, or
           says the run measured uncommitted changes or saw HEAD change. Unlike a mutation log,
           coverage of an earlier commit is refused: it is cheap to run again at the head
  --memory-safety-log  the output of `make memory-safety` for the diff; Focus names it with its
           result. Fails as --coverage does, but a log of an earlier commit of the range is
           accepted. With --round above 1, so is a log of the range's base or before it. Focus
           then lists the commits after it. CI runs the memory tests only after the
           merge. So when the commits that no log covers change MEMORY_PATHS, Focus says that
           the tests did not run on them. That line only reports; it stops nothing.
For a diff, Focus also lists each line the diff removes from the tests with no identical line
added back, and each test file it deletes, and each sisu build in OUT's bin/<sha>/ that
build_sisu.sh wrote.
  --round  the review round, 1 by default; a later round keeps the "# Re-review" section and
           needs --prior, the earlier reports, and --rulings, the caller's rulings on them
The template comes from origin/main, so a branch that started before a change to the review
criteria still gets the current ones. Without origin/main it falls back to ../brief.md.
CODING_STANDARDS.md also comes from origin/main: the script copies it next to OUT, and the brief
names the copy, so a diff cannot change the standards that judge it. A diff that changes the file
gets a Focus line that says so. Without origin/main's copy, the brief names the checkout's file,
and Focus says so.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

TEMPLATE = Path(__file__).resolve().parents[1] / "brief.md"
MAIN_TEMPLATE = "origin/main:.agents/skills/review-sisu/brief.md"
STANDARDS = "CODING_STANDARDS.md"
KINDS = ("spec", "plan", "diff")
# A full SHA-1 or SHA-256 commit name.
FULL_SHA = re.compile(r"[0-9a-f]{40}|[0-9a-f]{64}")


def plugin_skill(plugin: str, skill: str) -> Path | None:
    """Return the SKILL.md of an installed Claude Code plugin, at its installPath, or None."""
    try:
        installed = json.loads((Path.home() / ".claude/plugins/installed_plugins.json").read_text())
        return Path(installed["plugins"][plugin][0]["installPath"]) / "skills" / skill / "SKILL.md"
    except (OSError, ValueError, KeyError, IndexError):
        return None


SKILLS: dict[str, Path | None] = {
    "<test-audit SKILL.md path>": Path.home() / ".claude/skills/test-audit/SKILL.md",
    "<codebase-design SKILL.md path>": Path.home() / ".agents/skills/codebase-design/SKILL.md",
    "<rust-best-practices SKILL.md path>": plugin_skill(
        "beautiful-rust@beautiful-rust", "rust-best-practices"
    ),
}


def run_git(
    repo: Path | str, *args: str, stdin: bytes | None = None, answers: tuple[int, ...] = (0,)
) -> subprocess.CompletedProcess[bytes]:
    """Run git in `repo`, and stop the script when its exit code is not in `answers`.

    GIT_DIFF_OPTS would beat `--unified=0`.
    """
    env = {key: value for key, value in os.environ.items() if key != "GIT_DIFF_OPTS"}
    done = subprocess.run(
        ["git", "-C", str(repo), *args], input=stdin, capture_output=True, env=env, check=False
    )
    if done.returncode not in answers:
        error = done.stderr.decode(errors="replace").strip()
        sys.exit(f"diff_brief: git {' '.join(args)}: {error}")
    return done


def git(repo: Path | str, *args: str) -> str:
    """Return the stripped stdout of a git command in `repo`.

    A byte that is not UTF-8, as some test fixtures hold, reads as U+FFFD, so the brief can still
    be written.
    """
    return run_git(repo, *args).stdout.decode(errors="replace").strip()


def commit(repo: Path | str, ref: str) -> str:
    """Return the full SHA of `ref` in `repo`, or '' when `ref` names no commit."""
    args = ("rev-parse", "--verify", "--quiet", f"{ref}^{{commit}}")
    return run_git(repo, *args, answers=(0, 1)).stdout.decode().strip()


def is_ancestor(repo: Path | str, a: str, b: str) -> bool:
    """Tell whether the commit `a` is an ancestor of the commit `b`; both must be in `repo`."""
    return run_git(repo, "merge-base", "--is-ancestor", a, b, answers=(0, 1)).returncode == 0


def summary(log: Path) -> re.Match[str] | None:
    """Return the match of the last `mutation-test:` summary line of `log`, or None.

    Group 1 of the match is the measured SHA.
    """
    lines = [line for line in log.read_text().splitlines() if line.startswith("mutation-test: ")]
    return re.search(r"\(HEAD ([0-9a-f]{40})\)$", lines[-1]) if lines else None


def patch_id(repo: Path | str, sha: str) -> str:
    """Return the stable `git patch-id` of the commit `sha`, or '' for an empty commit."""
    shown = run_git(repo, "show", sha).stdout
    return run_git(repo, "patch-id", "--stable", stdin=shown).stdout.decode().partition(" ")[0]


def in_range(repo: Path | str, sha: str, base: str, head: str) -> str:
    """Return the commit of `base`..`head` that is `sha`, or that a rebase made of it, or ''.

    A rebase keeps the patch-id of a commit.
    """
    # In the range means reachable from head and not from base; base need not precede it after a
    # merge of main.
    if sha == commit(repo, head) or (
        not is_ancestor(repo, sha, base) and is_ancestor(repo, sha, head)
    ):
        return sha
    want = patch_id(repo, sha)
    return next(
        (
            c
            for c in git(repo, "rev-list", f"{base}..{head}").split()
            if want and patch_id(repo, c) == want
        ),
        "",
    )


def log_mismatch(
    log: Path, repo: Path | str, base: str, head: str, *, earlier: bool = False
) -> str | None:
    """Return why the summary line of `log` does not describe a commit in `base`..`head`, or None.

    With `earlier`, a commit that `base` reaches is accepted too: an earlier round's log.
    """
    found = summary(log)
    if not found:
        return f'no summary line ending "(HEAD <sha>)" in {log}'
    # A log can come from another clone, so this clone may not have the commit that it measured.
    known = commit(repo, found[1])
    if known and earlier and is_ancestor(repo, found[1], base):
        return None
    if not known or not in_range(repo, found[1], base, head):
        want = commit(repo, head)
        return (
            f"{log} measured {found[1][:8]}, which is not in {base}..{head} "
            f"({want[:8] or 'unknown'})"
        )
    return None


def last_line(path: Path, target: str) -> re.Match[str] | None:
    """Return the match of the last line of `path` that `make <target>` ends with, or None.

    The group `word` is the first word after the target, such as PASS; `sha` is the measured
    commit; `dirty` is set when the tree was not clean.
    """
    lines = [line for line in path.read_text().splitlines() if line.startswith(f"{target}: ")]
    pattern = (
        rf"^{re.escape(target)}: (?P<word>\S+).*"
        r"\(HEAD (?P<sha>[0-9a-f]{40})(?P<dirty> with uncommitted changes)?\)$"
    )
    return re.search(pattern, lines[-1]) if lines else None


def head_mismatch(  # noqa: PLR0913  two keyword options of one check: which earlier commits it accepts
    path: Path,
    repo: Path | str,
    head: str,
    target: str,
    *,
    base: str | None = None,
    earlier: bool = False,
) -> str | None:
    """Return why the last line of `make <target>` in `path` does not describe `head`, or None.

    With `base`, a commit of `base`..`head` is accepted too. With `earlier` as well, so is a
    commit that `base` reaches: an earlier round's run.
    """
    found = last_line(path, target)
    if not found:
        return f'no {target} line ending "(HEAD <sha>)" in {path}'
    measured, want = found["sha"], commit(repo, head)
    if found["dirty"]:
        return (
            f"{path} measured uncommitted changes on {measured[:8]}, or HEAD changed during the "
            f"run; run make {target} again at the head on a clean tree"
        )
    accepted = (
        base is not None
        and commit(repo, measured) != ""
        and is_ancestor(repo, measured, head)
        and (earlier or not is_ancestor(repo, measured, base))
    )
    if measured != want and not accepted:
        return (
            f"{path} measured {measured[:8]}, not the head {want[:8] or 'unknown'}; "
            f"run make {target} again at the head"
        )
    return None


def changes(repo: Path | str, base: str, head: str, *paths: str) -> bool:
    """Tell whether `base`..`head` changes a file that one of the pathspecs `paths` matches."""
    return (
        run_git(repo, "diff", "--quiet", base, head, "--", *paths, answers=(0, 1)).returncode == 1
    )


# The paths whose change needs `make memory-safety`, as AGENTS.md says: codegen, drops (MIR), the
# runtimes and the harness, with the programs that the memory tests build and run. The Makefile is
# not here: most of its changes do not touch the memory tests.
MEMORY_PATHS = (
    "crates/runtime*",
    "crates/sisuc/src/codegen",
    "crates/sisuc/src/mir",
    "crates/sisuc/src/link.rs",
    "crates/sisuc/src/prelude.sisu",
    "crates/sisuc/tests/cases.rs",
    "crates/sisuc/tests/support",
    "crates/sisuc/tests/run",
    "site/programs/run",
    ".config/nextest.toml",
    "scripts/build-runtime.sh",
)


def main_standards(repo: Path) -> bytes | None:
    """Return origin/main's CODING_STANDARDS.md, or None when origin/main has none."""
    shown = subprocess.run(
        ["git", "-C", str(repo), "show", f"origin/main:{STANDARDS}"],
        capture_output=True,
        check=False,
    )
    if shown.returncode != 0:
        print(
            f"diff_brief: cannot read origin/main:{STANDARDS}; using the checkout's",
            file=sys.stderr,
        )
        return None
    return shown.stdout


# Test files whose removed lines Focus lists. Blessed output is left out: golden_diff.py checks
# a rewording.
TESTS = [":(glob)**/tests/**", ":(glob)**/tests.rs", ":(glob)scripts/test_*.py"]
EXPECTED = [":(exclude,glob)**/*.err", ":(exclude,glob)**/*.json", ":(exclude,glob)**/*.out"]
SHOWN = 40


def removed_tests(repo: Path | str, base: str, head: str) -> list[str]:
    """Return each line that base..head removes from a test file, then each deleted test file.

    A line counts when no identical line is added back to any test file. A line reads
    "path:line: text". Blank lines and comments are left out.
    """
    deleted = git(
        repo, "diff", "--name-only", "--diff-filter=D", base, head, "--", *TESTS
    ).splitlines()
    # The same output under any user config: no quoted paths, external diff driver or text
    # conversion, and the `a/` and `b/` prefixes that the loop reads.
    diff = git(
        repo,
        "-c",
        "core.quotepath=off",
        "diff",
        "--unified=0",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "--diff-filter=MR",
        base,
        head,
        "--",
        *TESTS,
        *EXPECTED,
    )
    # A removed line is named by its base path and line, so a renamed file reads as the base has
    # it.
    removed: list[tuple[str | None, str, int, str]] = []
    added: set[str] = set()
    old: str | None = None
    new: str | None = None
    number = 0
    for line in diff.splitlines():
        if line.startswith("diff --git"):
            new = None
        elif new is None:
            if line.startswith("--- a/"):
                old = line[6:]
            elif line.startswith("+++ b/"):
                new = line[6:]
        elif line.startswith("@@"):
            # pyrefly: ignore[unsupported-operation]  every hunk header starts with "@@ -N"
            number = int(re.match(r"@@ -(\d+)", line)[1])
        elif line.startswith("-"):
            removed.append((old, new, number, line[1:].strip()))
            number += 1
        elif line.startswith("+"):
            added.add(line[1:].strip())
    lines = [
        f"{o}:{n}: {text[:100]}"
        for o, p, n, text in removed
        if re.search(r"\w", text)
        and text not in added
        and not text.startswith("//")
        and not (p.endswith(".py") and text.startswith("#"))
    ]
    return lines + [f"{p}: deleted" for p in deleted]


def drop_line(t: str, start: str) -> str:
    """Return `t` without the "- " list item that begins with `start`.

    The item ends at the next item or blank line.
    """
    return re.sub(rf"- {re.escape(start)}.*?(?=\n- |\n\n)\n?", "", t, flags=re.DOTALL)


def main() -> None:  # noqa: C901, PLR0912, PLR0915  one linear pass: a branch for each option, a statement for each step, and each step reads what the step before it set
    """Write the brief that the command line asks for, and print its path."""
    p = argparse.ArgumentParser()
    p.add_argument("repo")
    p.add_argument("target")
    p.add_argument("out")
    p.add_argument("--kind", choices=KINDS, default="diff")
    p.add_argument("--doc", action="append", default=[])
    p.add_argument("--focus")
    p.add_argument("--mutation-log", action="append", default=[])
    p.add_argument("--coverage")
    p.add_argument("--memory-safety-log")
    p.add_argument("--round", type=int, default=1)
    p.add_argument("--prior", action="append", default=[])
    p.add_argument("--rulings")
    a = p.parse_args()
    repo, out = Path(a.repo).resolve(), Path(a.out).resolve()
    if a.round > 1 and not (a.prior and a.rulings):
        sys.exit("diff_brief: a round after the first needs --prior and --rulings")
    if a.kind != "diff" and (a.mutation_log or a.coverage or a.memory_safety_log):
        sys.exit("diff_brief: --mutation-log, --coverage and --memory-safety-log are for a diff")
    for path in [
        *a.mutation_log,
        a.coverage,
        a.memory_safety_log,
        *a.prior,
        a.rulings,
        a.target if a.kind != "diff" else None,
    ]:
        if path and not Path(path).is_file():
            sys.exit(f"diff_brief: no file at {Path(path).resolve()}")

    rules = main_standards(repo)
    focus_lines = []
    if a.kind == "diff":
        base, _, head = a.target.partition("..")
        shas = [commit(repo, base), commit(repo, head)]
        if not all(shas):
            sys.exit(f"diff_brief: {a.target} is not a range of two commits in {repo}")
        target = "..".join(shas)
        for mutation_log in a.mutation_log:
            log = Path(mutation_log).resolve()
            if why := log_mismatch(log, repo, shas[0], shas[1], earlier=a.round > 1):
                sys.exit(f"diff_brief: {why}")
            # pyrefly: ignore[unsupported-operation]  `log_mismatch` has found the summary line
            measured = summary(log)[1]
            if a.round > 1 and is_ancestor(repo, measured, shas[0]):
                focus_lines.append(
                    f"Mutation-test log: {log}, measured at {measured[:8]}, "
                    "before this round: it covers the code of earlier rounds"
                )
                continue
            twin = in_range(repo, measured, *shas)
            later = git(repo, "rev-list", "--reverse", f"{twin}..{shas[1]}").split()
            note = f", measured at {measured[:8]}" if later or twin != measured else ""
            if twin != measured:
                note += f", which a rebase made {twin[:8]}"
            if later:
                note += f". Commits after it, not mutated: {' '.join(c[:8] for c in later)}"
            focus_lines.append(f"Mutation-test log: {log}{note}")
        if a.coverage:
            coverage = Path(a.coverage).resolve()
            if why := head_mismatch(coverage, repo, shas[1], "coverage"):
                sys.exit(f"diff_brief: {why}")
            focus_lines.append(f"Coverage result: {coverage}")
        ran = shas[0]
        if a.memory_safety_log:
            log = Path(a.memory_safety_log).resolve()
            if why := head_mismatch(
                log, repo, shas[1], "memory-safety", base=shas[0], earlier=a.round > 1
            ):
                sys.exit(f"diff_brief: {why}")
            found = last_line(log, "memory-safety")
            # pyrefly: ignore[unsupported-operation]  `head_mismatch` has found the last line
            word, ran = found["word"], found["sha"]
            note = ""
            if ran != shas[1]:
                later = git(repo, "rev-list", "--reverse", f"{ran}..{shas[1]}").split()
                note = (
                    f", measured at {ran[:8]}. Commits after it: {' '.join(c[:8] for c in later)}"
                )
            focus_lines.append(f"Memory-safety log: {log}: {word}{note}")
        if changes(repo, ran, shas[1], *MEMORY_PATHS):
            after = " on the commits after the log" if a.memory_safety_log else ""
            focus_lines.append(
                f"Memory tests: not run{after}. CI runs `make memory-safety` only after the merge. "
                "Name the riskiest changed path that only ASan, Valgrind or `--rc-checks` would "
                "catch, or say that there is none."
            )
        if rules is None:
            focus_lines.append(
                f"The brief names the checkout's `{STANDARDS}`, because origin/main has none. "
                "So a change in the diff to those standards also judges the diff."
            )
        if changes(repo, shas[0], shas[1], STANDARDS):
            judge = (
                "Judge the diff by the copy that the brief names, and review" if rules else "Review"
            )
            focus_lines.append(
                f"The diff changes `{STANDARDS}`. {judge} the changed rules as part of the target."
            )
        if removed := removed_tests(repo, *shas):
            more = (
                [f"- and {len(removed) - SHOWN} more: see the diff"] if len(removed) > SHOWN else []
            )
            focus_lines.append(
                "Removed from the tests, with no identical line added back. For each, find its "
                "keeper or the reason:\n" + "\n".join([f"- {r}" for r in removed[:SHOWN]] + more)
            )
        # build_sisu.sh names each folder by a full SHA; another folder there is not a build.
        builds = sorted(
            b for b in (out.parent / "bin").glob("*/sisu") if FULL_SHA.fullmatch(b.parent.name)
        )
        if builds:
            focus_lines.append(
                "Prebuilt sisu, one for each commit:\n"
                + "\n".join(
                    f"- {b.parent.name[:8]} "
                    f"{git(repo, 'log', '-1', '--format=%s', b.parent.name)}: {b}"
                    for b in builds
                )
            )
    else:
        target = out.parent / f"target-{a.round}{Path(a.target).suffix}"

    shown = subprocess.run(
        ["git", "-C", str(repo), "show", MAIN_TEMPLATE], capture_output=True, text=True, check=False
    )
    if shown.returncode == 0:
        t = shown.stdout
    elif TEMPLATE.is_file():
        print(f"diff_brief: cannot read {MAIN_TEMPLATE}; using {TEMPLATE}", file=sys.stderr)
        t = TEMPLATE.read_text()
    else:
        sys.exit(f"diff_brief: cannot read {MAIN_TEMPLATE}; run git fetch origin")
    t = t.replace("a <spec | plan | diff> of Sisu", f"a {a.kind} of Sisu")
    t = t.replace("Target: <path, or base..head>.", f"Target: {target}.")
    t = t.replace("<repository root>", str(repo))
    t = t.replace("<base>..<head>", str(target))
    if "<coding standards path>" not in t:
        rules = None
    rules_path = out.parent / STANDARDS if rules is not None else repo / STANDARDS
    t = t.replace("<coding standards path>", str(rules_path))
    docs = "".join(f"- {Path(d).resolve()}\n" for d in a.doc)
    t = re.sub(r"- <the other documents to read.*?>\n", docs, t, flags=re.DOTALL)
    if a.kind != "diff":
        t = drop_line(t, "For a diff,")
    # A departure from the Rust idiom is a nit, and a later round accepts no nit.
    if a.kind != "diff" or a.round > 1:
        t = drop_line(t, "`<rust-best-practices SKILL.md path>`")
    for slot, path in SKILLS.items():
        if slot not in t:
            continue
        if path and path.exists():
            t = t.replace(slot, str(path))
        else:
            print(
                f"diff_brief: {path or slot} is missing; its line is dropped, so tell the user",
                file=sys.stderr,
            )
            t = drop_line(t, f"`{slot}`")
    for kind in KINDS:
        if kind != a.kind:
            t = re.sub(rf"# Check, for a {kind}\n.*?(?=\n# )\n", "", t, flags=re.DOTALL)
    if a.round == 1:
        t = re.sub(r"# Re-review\n.*?(?=\n# )\n", "", t, flags=re.DOTALL)
    else:
        t = t.replace("<round>", str(a.round))
        t = t.replace("<prior reports>", ", ".join(str(Path(r).resolve()) for r in a.prior))
        t = t.replace("<rulings>", str(Path(a.rulings).resolve()))
        t = t.replace("<path of the first brief>", str(out.parent / "brief.md"))
        previous = out.parent / f"target-{a.round - 1}{Path(a.target).suffix}"
        if a.kind == "diff" or not previous.exists():
            t = drop_line(t, "The previous version:")
        t = t.replace("<previous version>", str(previous))
    # Checked before Focus goes in, so text in the caller's focus cannot trip it.
    focus_slot = '<what the caller knows that the target does not say, or "none">'
    top = t.split("# Report", 1)[0].replace(focus_slot, "", 1)
    left = re.findall(
        r"<(?:spec|path|repository|base|coding standards|the other|test-audit|codebase-design"
        r"|rust-best-practices"
        r"|round|prior|rulings|previous|what the caller)[^>]*>",
        top,
    )
    if left or focus_slot not in t:
        sys.exit(f"diff_brief: unfilled slots: {left or [focus_slot + ' missing']}")
    focus = Path(a.focus).read_text().strip() if a.focus else ""
    focus = "\n\n".join(filter(None, ["\n".join(focus_lines), focus])) or "none"
    if a.kind != "diff":
        shutil.copyfile(a.target, target)
    if rules is not None:
        rules_path.write_bytes(rules)
    out.write_text(t.replace(focus_slot, focus))
    print(out)


if __name__ == "__main__":
    main()
