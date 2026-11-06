"""Finds generator gaps: functions that a valid program of tests/run or tests/run-fail reaches and
no fixed fuzz seed reaches. It compares two `llvm-cov export` files with a list of known gaps.

A function is (file, first line, name). The file is the part of its path after `crates/sisuc/src/`.
The name is the `c++filt -s rust` name without crate hashes and generic arguments, so the
instances of a generic function merge. A closure folds into its enclosing function, so a name that
ends in a closure is not a function. A closure in a `static` or a `const` has no enclosing function,
so the check does not measure it. Each export must reach a function under `check/`, `mir/` and
`codegen/`.
`generate/`, `fuzz.rs`, `fuzz_program.rs`, `report.rs`, `diagnostic/json.rs` and
`diagnostic/render.rs` are out of scope. A function is reached when one of its records has a count
above 0.

The known list has sections. A section is a line `## <reason>`, then key lines `<file><TAB><name>`.

Usage: python3 fuzz_gaps.py <seeds.json> <valid.json> <known_gaps.txt>
Prints one Markdown section that starts with `## Generator gaps`. Exit codes:
  0  no new gap (a known key that matches no gap is a warning),
  1  a new gap,
  2  the check did not finish: a broken measurement is never a pass.
"""

import collections
import json
import re
import subprocess
import sys
from pathlib import Path
from typing import Any

MARKER = "/crates/sisuc/src/"
OUT_OF_SCOPE_FILES = {
    "fuzz.rs",
    "fuzz_program.rs",
    "report.rs",
    "diagnostic/json.rs",
    "diagnostic/render.rs",
}
CRATE_HASH = re.compile(r"(?<=\w)\[[0-9a-f]+\]")
QUOTED_BRACKET = re.compile(r"'[<>]'")
CLOSURE_END = re.compile(r"\{closure[^}]*\}$")
MANGLED = re.compile(r"(^|\W)_(R|ZN)\w|::h[0-9a-f]{16}$")
STAGES = ("check/", "mir/", "codegen/")
HEADING = "## Generator gaps"


class Broken(Exception):
    """The measurement cannot be trusted. The message is the reason, without a final period."""


def matching_close(name: str, start: int) -> int:
    """The index of the `>` that closes the `<` at `start`. The `>` of `->` does not count."""
    depth = 0
    for i in range(start, len(name)):
        if name[i] == "<":
            depth += 1
        elif name[i] == ">" and name[i - 1] != "-":
            depth -= 1
            if depth == 0:
                return i
    raise Broken(f"the name {name!r} has angle brackets that do not balance")


# shortcut: a generic Self type such as <[T] as Tr> does not merge, upgrade when a new gap names such an impl.
def strip_generics(name: str) -> str:
    """Removes each `<...>` that follows an identifier character or `::`, with that `::`. A `<`
    at the start or after another `<` opens `<Type>` or `<T as Trait>`, and stays.
    """
    out: list[str] = []
    i = 0
    while i < len(name):
        follows_name = bool(out) and (out[-1].isalnum() or out[-1] == "_" or out[-2:] == [":", ":"])
        if name[i] == "<" and follows_name:
            if out[-2:] == [":", ":"]:
                del out[-2:]
            i = matching_close(name, i) + 1
        else:
            out.append(name[i])
            i += 1
    return "".join(out)


def normalize(name: str) -> str | None:
    """The name without crate hashes and generic arguments, or None for a name that ends in a
    closure. The test comes last: a generic argument can hold a closure, and the function is not
    one. A named function inside a closure is a function.
    """
    name = strip_generics(QUOTED_BRACKET.sub("''", CRATE_HASH.sub("", name)))
    return None if CLOSURE_END.search(name) else name


def demangle(names: list[str]) -> list[str]:
    """One `c++filt -s rust` call for all names. It must return one line for each name."""
    try:
        done = subprocess.run(
            ["c++filt", "-s", "rust"],
            input="\n".join(names) + "\n",
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as e:
        raise Broken(f"c++filt did not run ({e.strerror})") from e
    if done.returncode != 0:
        raise Broken(f"c++filt failed with exit code {done.returncode}")
    lines = done.stdout.split("\n")[:-1]
    if len(lines) != len(names):
        raise Broken(
            f"c++filt returned a wrong number of lines: {len(lines)} for {len(names)} names"
        )
    return lines


def load(path: str) -> list[dict[str, Any]]:
    """The `data[0].functions` records of an export."""
    try:
        with Path(path).open(encoding="utf-8") as f:
            document = json.load(f)
    except OSError as e:
        raise Broken(f"cannot read the export {path} ({e.strerror})") from e
    except ValueError as e:
        raise Broken(f"the export {path} is not valid JSON ({e})") from e
    try:
        functions = document["data"][0]["functions"]
    except (KeyError, IndexError, TypeError):
        functions = None
    if not isinstance(functions, list):
        raise Broken(f"the export {path} has no data[0].functions")
    return functions


def in_scope(functions: list[dict[str, Any]], path: str) -> list[tuple[str, int, str, bool]]:
    """(file, first line, mangled name, reached) of each record in scope. A record with no file
    name is broken: it would read as out of scope.
    """
    found = []
    for record in functions:
        files = record.get("filenames")
        if not (isinstance(files, list) and files and isinstance(files[0], str) and files[0]):
            raise Broken(f"a record of the export {path} has no file name")
        if MARKER not in files[0]:
            continue
        file = files[0].rsplit(MARKER, 1)[1]
        if file.startswith("generate/") or file in OUT_OF_SCOPE_FILES:
            continue
        found.append((file, record["regions"][0][0], record["name"], record["count"] > 0))
    return found


def identities(
    rows: list[tuple[str, int, str, bool]], names: dict[str, str | None]
) -> dict[tuple[str, int, str], bool]:
    """Maps (file, first line, name) to whether any record of that function was reached."""
    seen: dict[tuple[str, int, str], bool] = {}
    for file, line, mangled, reached in rows:
        name = names[mangled]
        if name is not None:
            key = (file, line, name)
            seen[key] = seen.get(key, False) or reached
    return seen


def read_known(path: str) -> set[tuple[str, str]]:
    """The (file, name) keys of the known list."""
    try:
        text = Path(path).read_text(encoding="utf-8")
    except OSError as e:
        raise Broken(f"cannot read the known list {path} ({e.strerror})") from e
    keys: set[tuple[str, str]] = set()
    reason = None
    for number, line in enumerate(text.splitlines(), 1):
        if not line.strip():
            continue
        if line.startswith("##"):
            reason = line[2:].strip()
            if not reason:
                raise Broken(f"line {number} of the known list is a section with an empty reason")
        elif reason is None:
            raise Broken(f"line {number} of the known list is a key before any section header")
        else:
            file, tab, name = line.partition("\t")
            if not (tab and file and name):
                raise Broken(f"line {number} of the known list needs a TAB between file and name")
            keys.add((file, name))
    return keys


def check_keys_are_unique(
    known: set[tuple[str, str]], *exports: dict[tuple[str, int, str], bool]
) -> None:
    """A known key must name one definition. Two first lines under one key mean that the key
    would hide the second definition too. The lines of both exports count, reached or not.
    """
    lines = collections.defaultdict(set)
    for seen in exports:
        for file, line, name in seen:
            lines[(file, name)].add(line)
    for key in sorted(known):
        if len(lines[key]) > 1:
            raise Broken(
                f"the known key {key[0]} {key[1]} matches two definitions, on lines "
                + " and ".join(map(str, sorted(lines[key])))
                + ". A key cannot tell them apart, so remove it and add a generator rule"
            )


def fence(lines: list[str]) -> str:
    return "```text\n" + "\n".join(lines) + "\n```"


def section(*parts: str) -> str:
    return "\n\n".join((HEADING, *parts)) + "\n"


def compare(seeds_path: str, valid_path: str, known_path: str) -> tuple[int, str]:
    exports = {
        "seeds": in_scope(load(seeds_path), seeds_path),
        "valid-program": in_scope(load(valid_path), valid_path),
    }
    known = read_known(known_path)
    mangled = sorted({row[2] for rows in exports.values() for row in rows})
    names: dict[str, str | None] = {}
    for original, demangled in zip(mangled, demangle(mangled) if mangled else [], strict=False):
        if MANGLED.search(demangled):
            raise Broken(
                f"the name {original} is still mangled, or is a legacy Rust name, after c++filt"
            )
        names[original] = normalize(demangled)
    seeds, valid = (identities(exports[label], names) for label in ("seeds", "valid-program"))
    for label, seen in (("valid-program", valid), ("seeds", seeds)):
        for stage in STAGES:
            if not any(
                reached and file.startswith(stage) for (file, _, _), reached in seen.items()
            ):
                raise Broken(f"the {label} export reaches no function under {stage}")
    check_keys_are_unique(known, seeds, valid)

    gaps = {key for key, reached in valid.items() if reached} - {k for k, r in seeds.items() if r}
    gap_keys = {(file, name) for file, _, name in gaps}
    new = sorted(gap_keys - known)
    stale = sorted(known - gap_keys)
    stale_text = (
        []
        if not stale
        else [
            "Warning: these known keys match no gap. Remove them from `fuzz/known_gaps.txt`:",
            fence([f"{file}\t{name}" for file, name in stale]),
        ]
    )
    if new:
        return 1, section(
            "Valid programs reach these functions, and no fixed seed reaches them:",
            fence([f"{file}\t{name}" for file, name in new]),
            "Add a generator rule for each function, or add its line to `fuzz/known_gaps.txt` "
            "under a section with a reason.",
            *stale_text,
        )
    return 0, section(
        f"No new generator gap. The check measured {sum(valid.values())} functions that valid "
        f"programs reach. Known keys: {len(known)}.",
        *stale_text,
    )


def main(argv: list[str]) -> int:
    try:
        if len(argv) != 4:
            raise Broken("usage: fuzz_gaps.py <seeds.json> <valid.json> <known_gaps.txt>")
        code, text = compare(*argv[1:])
    except Broken as e:
        code, text = 2, f"{e}"
    # An uncaught failure exits 1, which reads as a new gap.
    except Exception as e:
        code, text = 2, f"an unexpected {type(e).__name__}: {e}"
    if code == 2:
        text = section(
            f"The gap check did not finish, so it proves nothing about gaps. Reason: {text}."
        )
    sys.stdout.write(text)
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv))
