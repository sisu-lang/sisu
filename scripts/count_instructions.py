#!/usr/bin/env python3
"""Counts the instructions of one `sisu` command on a base commit and on the working tree.

Usage: python3 scripts/count_instructions.py [--base REV] -- SISU_ARGS...

The script builds the release `sisu` and runtime twice: from the working tree into `target/`, and
from REV in a temporary worktree into `target/count-base/`. It runs `sisu SISU_ARGS` with each under
callgrind, and prints the two totals and the change. REV is by default where HEAD left
`origin/main`. Both runs start in the current directory, so the paths in SISU_ARGS are the same.

Example: python3 scripts/count_instructions.py -- build -O2 nest.sisu -o "$TMPDIR/nest"

It exits with code 1 when a build or a run fails.
"""

import argparse
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

from compile_speed import Deadline, Fail

ROOT = Path(__file__).resolve().parent.parent
BUILD = ["cargo", "build", "--release", "--locked", "-p", "sisuc", "-p", "sisu-runtime-mimalloc"]
BASE_TARGET = ROOT / "target" / "count-base"
# The last line of a callgrind run: `==123== Collected : 617810176`.
COLLECTED = re.compile(r"^==\d+== Collected : (\d+)$", re.MULTILINE)


def total(stderr: str) -> int:
    """Return the instruction count in the stderr of a callgrind run, or raise Fail."""
    found = COLLECTED.findall(stderr)
    if not found:
        raise Fail("callgrind gave no instruction count")
    return int(found[-1])


def change(base: int, head: int) -> str:
    """Return the change from `base` to `head`, as a percent and as a factor."""
    if base <= 0 or head <= 0:
        return "no change can be computed"
    percent = (head - base) * 100 / base
    factor = f"{base / head:.1f}x fewer" if head < base else f"{head / base:.1f}x more"
    return f"{percent:+.2f}% ({factor})"


def count(deadline: Deadline, sisu: Path, args: list[str], what: str) -> int:
    """Run `sisu args` under callgrind in the current directory, and return its total."""
    # `--emit` can print much; only the stderr of callgrind is read.
    with tempfile.TemporaryDirectory() as tmp:
        cmd = ["valgrind", "--tool=callgrind", f"--callgrind-out-file={tmp}/out", sisu, *args]
        return total(deadline.run(cmd, what, stdout=subprocess.DEVNULL).stderr)


def build_base(deadline: Deadline, rev: str) -> Path:
    """Build `rev` in a temporary worktree into BASE_TARGET, and return its `sisu`."""
    env = dict(os.environ, CARGO_TARGET_DIR=str(BASE_TARGET))
    # A new worktree has no `.llvm`; the prefix in the environment wins over `.cargo/config.toml`.
    env.setdefault("LLVM_SYS_231_PREFIX", str((ROOT / ".llvm" / "23").resolve()))
    with tempfile.TemporaryDirectory() as tmp:
        tree = Path(tmp) / "base"
        deadline.run(["git", "-C", ROOT, "worktree", "add", "--detach", tree, rev], "worktree add")
        try:
            deadline.run(BUILD, f"build {rev}", cwd=tree, env=env)
        finally:
            deadline.run(
                ["git", "-C", ROOT, "worktree", "remove", "--force", tree], "worktree remove"
            )
    return BASE_TARGET / "release" / "sisu"


def main(argv: list[str]) -> int:
    """Run the command line `argv`, and return the exit code."""
    parser = argparse.ArgumentParser(
        description="Count the instructions of one sisu command on a base and on the working tree."
    )
    parser.add_argument("--base", help="the base commit; by default where HEAD left origin/main")
    parser.add_argument("--time-bound", type=float, default=3600, help="seconds for the whole run")
    parser.add_argument("sisu_args", nargs="+", metavar="SISU_ARGS", help="the arguments of sisu")
    args = parser.parse_args(argv)
    deadline = Deadline(args.time_bound)
    try:
        rev = (
            args.base
            or deadline.run(
                ["git", "-C", ROOT, "merge-base", "HEAD", "origin/main"], "merge-base"
            ).stdout.strip()
        )
        rev = deadline.run(["git", "-C", ROOT, "rev-parse", "--short", rev], "rev-parse").stdout
        rev = rev.strip()
        base_sisu = build_base(deadline, rev)
        deadline.run(BUILD, "build the working tree", cwd=ROOT)
        base = count(deadline, base_sisu, args.sisu_args, f"sisu of {rev}")
        head = count(deadline, ROOT / "target" / "release" / "sisu", args.sisu_args, "sisu")
    except Fail as e:
        print(f"count_instructions: {e}", file=sys.stderr)
        return 1
    print(f"base {rev}: {base:,}")
    print(f"working tree: {head:,}")
    print(f"change: {change(base, head)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
