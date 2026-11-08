"""Count the hand-written lists that name the number types, for the `structure` area.

A list is a run of two or more `Type::X` or `TypeKind::X` names joined by `|` or `,`, on one line
or on several. It counts when it names two or more of `I64`, `U8` and `F64`, and none of `Unit`,
`Never` and `Param`. A table counts each list in it, such as each row of a `match` on a pair of
types. Test code does not
count: files under `tests/`, files that end in `tests.rs`, and the rest of a file after an inline
`#[cfg(test)] mod`. Comments and the program generator's `SEEDS` table do not count.

Usage: python3 -I number_lists.py REPO REV. Prints `<file>:<line>: <list>` for each list in
`crates/sisuc/src` at REV, then `TOTAL <n>`. A git command that fails stops the script with its
error.
"""

import re
import subprocess
import sys
from collections.abc import Iterator

NAME = r"(?:Type|TypeKind)::\w+"
RUN = re.compile(NAME + r"(?:\s*[|,]\s*" + NAME + r")+")
NUMBERS = frozenset({"I64", "U8", "F64"})
# A list that names one of these lists every leaf kind on purpose (#361), so it is no number list.
LEAVES = frozenset({"Unit", "Never", "Param"})
CFG_TEST = re.compile(r"\s*#\[cfg\(test\)\]\s*$")
INLINE_MOD = re.compile(r"\s*(pub(\([a-z]+\))? )?mod \w+\s*\{")
SEEDS = re.compile(r"const SEEDS.*?\];", re.DOTALL)


def git(repo: str, *args: str) -> str:
    """Return the output of `git -C repo args`."""
    return subprocess.run(
        ["git", "-C", repo, *args], capture_output=True, text=True, check=True
    ).stdout


def code(text: str) -> str:
    """`text` without comments, the `SEEDS` table, and an inline test module and the lines after it.

    The lines keep their numbers.
    """
    keep: list[str] = []
    after_cfg_test = False
    for line in text.split("\n"):
        if after_cfg_test and INLINE_MOD.match(line):
            break
        after_cfg_test = bool(CFG_TEST.match(line)) or (
            after_cfg_test and line.strip().startswith("#[")
        )
        keep.append(re.sub(r"//.*", "", line))
    return SEEDS.sub(lambda m: "\n" * m.group().count("\n"), "\n".join(keep))


def lists(text: str) -> Iterator[tuple[int, str]]:
    """Yield the line and the names of each number-type list in the Rust source `text`."""
    body = code(text)
    for m in RUN.finditer(body):
        names = set(re.findall(r"::(\w+)", m.group()))
        if len(names & NUMBERS) > 1 and not names & LEAVES:
            yield body.count("\n", 0, m.start()) + 1, " ".join(m.group().split())


def main() -> None:
    """Print each list in `crates/sisuc/src` at the revision, then the total."""
    match sys.argv[1:]:
        case [repo, rev]:
            pass
        case _:
            sys.exit(__doc__)
    total = 0
    for path in git(repo, "ls-tree", "-r", "--name-only", rev, "crates/sisuc/src").split():
        if not path.endswith(".rs") or "/tests/" in path or path.endswith("tests.rs"):
            continue
        for line, names in lists(git(repo, "show", f"{rev}:{path}")):
            total += 1
            print(f"{path}:{line}: {names}")
    print("TOTAL", total)


if __name__ == "__main__":
    main()
