#!/usr/bin/env python3
"""Runs the gates of the structs spec on the programs of `bench/structs/`.

Usage: python3 scripts/structs_gate.py [--class-only] [--programs a,b] [--sisu target/release/sisu]
       [--bench bench/structs] [--out target/structs-gate] [--time-bound SECONDS]

Each program has a `struct.sisu` and a `class.sisu`, and an input at 1x and at 4x. The script
builds each with `sisu build --allocator system --codegen-units 1`. It runs each under callgrind
on each input, and prints a table of program, version, 1x, 4x, ratio and result. The count is the
`Collected` line of callgrind.

An input that has no `input-Nx.txt` in the program's directory is made at the start of the run
into `--out`. So the checkout holds no input over 100 KiB.

The gates of the spec ("The reversal tests"):
- The observer stays at or below `OBSERVER_LIMIT` instructions at `OBSERVER_EVENTS` events.
- Each program stays at or below `RATIO_LIMIT` times the instructions at 4x the input. The
  exception is a program whose class version is at or above that ratio. The row says it is excused.
- The check decides each row of "Decisions". This script does not judge it: it is the test suite
  of part 6.

The `parser` program has two modes. The script runs `dump` and `lex` at each size. It measures
the parser part: the count of `dump` less the count of `lex`.

`--class-only` runs the class versions only. It builds no struct version and judges no gate. It
proves that the programs and the inputs work, before `struct` compiles.

The script exits with code 1 when a gate fails, or when a build or a run fails.
"""

import argparse
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

from compile_speed import Deadline, Fail

ROOT = Path(__file__).resolve().parent.parent
BENCH = ROOT / "bench" / "structs"
DEFAULT_SISU = "target/release/sisu"
DEFAULT_OUT = "target/structs-gate"
# The programs, in the order of the table.
PROGRAMS = ("lexer", "tree", "server", "parser", "board", "observer")
# Gate: instructions at 4x the input, over instructions at 1x.
RATIO_LIMIT = 5.0
# Gate: the observer, at its 4x input of `OBSERVER_EVENTS` events.
OBSERVER_LIMIT = 100_000_000
OBSERVER_EVENTS = 20_000
# A 4x count at or below this many times the 1x count shows an input that the program ignores.
GROWTH_FLOOR = 3.0
# The modes of the `parser` program: `dump` is the work, and `lex` is the lexing in it.
PARSER_MODES = ("dump", "lex")
# The columns of the table that hold numbers: 1x, 4x and ratio.
NUMBER_COLUMNS = (2, 3, 4)
# The last line of a callgrind run: `==123== Collected : 617810176`.
COLLECTED = re.compile(r"^==\d+== Collected : (\d+)$", re.MULTILINE)


@dataclass(frozen=True)
class Row:
    """The counts of one version of one program."""

    program: str
    version: str
    one: int
    four: int

    @property
    def ratio(self) -> float:
        """The count at 4x over the count at 1x, or infinity when the 1x count is not above 0."""
        return self.four / self.one if self.one > 0 else float("inf")


def generated(program: str, size: str, bench: Path, out: Path, deadline: Deadline) -> Path:
    """Return the input of `program` at `size`: the committed file, or one made in `out`."""
    committed = bench / program / f"input-{size}.txt"
    if committed.exists():
        return committed
    one = bench / program / "input-1x.txt"
    target = out / f"{program}-input-{size}.txt"
    if program in ("lexer", "server"):
        target.write_text(one.read_text() * 4)
    elif program == "parser":
        mk4x = bench / program / "mk4x.py"
        deadline.run([sys.executable, mk4x, one, target], "mk4x.py")
    elif program == "board":
        gen = bench / program / "gen.py"
        text = deadline.run([sys.executable, gen, "20000", "0", "--core"], "gen.py").stdout
        target.write_text(text)
    else:
        raise Fail(f"{program}: no input-{size}.txt and no way to make it")
    return target


def instructions(stderr: str) -> int:
    """Return the instruction count in the stderr of a callgrind run, or raise Fail."""
    found = COLLECTED.findall(stderr)
    if not found:
        raise Fail("callgrind gave no instruction count")
    return int(found[-1])


def build(deadline: Deadline, sisu: str, source: Path, exe: Path) -> None:
    """Build `source` into `exe` with the system allocator and one codegen unit."""
    cmd = [sisu, "build", "--allocator", "system", "--codegen-units", "1", "-o", exe, source]
    deadline.run(cmd, f"build {source.parent.name}/{source.name}")


def count(deadline: Deadline, exe: Path, args: list[str], stdin: Path) -> int:
    """Run `exe args` on `stdin` under callgrind, and return its instruction count."""
    cmd = ["valgrind", "--tool=callgrind", "--callgrind-out-file=/dev/null", exe, *args]
    with stdin.open() as source:
        result = deadline.run(cmd, f"callgrind {exe.name}", stdin=source, stdout=subprocess.DEVNULL)
    return instructions(result.stderr)


def measure(
    deadline: Deadline, sisu: str, source: Path, inputs: tuple[Path, Path], out: Path
) -> Row:
    """Build one version of one program, and count it on both inputs.

    For `parser` a count is the count of `dump` less the count of `lex`.
    """
    program, version = source.parent.name, source.stem
    exe = out / f"{program}-{version}"
    build(deadline, sisu, source, exe)
    modes = [[m] for m in PARSER_MODES] if program == "parser" else [[]]
    sizes = []
    for stdin in inputs:
        counts = [count(deadline, exe, mode, stdin) for mode in modes]
        sizes.append(counts[0] - counts[1] if program == "parser" else counts[0])
    return Row(program, version, sizes[0], sizes[1])


def judge(program: str, struct: Row, cls: Row) -> str:
    """Return `pass`, `excused ...`, or `FAIL ...` for the struct version of `program`."""
    problems = []
    excused = ""
    if struct.ratio <= GROWTH_FLOOR:
        problems.append(f"the count grows {struct.ratio:.2f}x at 4x the input")
    elif struct.ratio > RATIO_LIMIT:
        if cls.ratio >= RATIO_LIMIT:
            excused = f"excused: the class version is at {cls.ratio:.2f}x"
        else:
            problems.append(f"{struct.ratio:.2f}x is above {RATIO_LIMIT:g}x")
    # The observer limit has no excuse: the class version does not bear on it.
    if program == "observer" and struct.four > OBSERVER_LIMIT:
        problems.append(
            f"{struct.four:,} is above {OBSERVER_LIMIT:,} at {OBSERVER_EVENTS:,} events"
        )
    if problems:
        return "FAIL: " + "; ".join(problems)
    return excused or "pass"


def table(rows: list[tuple[Row, str]]) -> str:
    """Return the rows as a table with a header."""
    head = ("program", "version", "1x", "4x", "ratio", "result")
    body = [
        (r.program, r.version, f"{r.one:,}", f"{r.four:,}", f"{r.ratio:.2f}", result)
        for r, result in rows
    ]
    widths = [max(len(line[i]) for line in [head, *body]) for i in range(len(head))]
    lines = [head, *body]
    return "\n".join(
        "  ".join(
            cell.rjust(w) if i in NUMBER_COLUMNS else cell.ljust(w)
            for i, (cell, w) in enumerate(zip(line, widths, strict=True))
        )
        for line in lines
    )


def main(argv: list[str]) -> int:
    """Run the command line `argv`, and return the exit code."""
    parser = argparse.ArgumentParser(
        description="Run the gates of the structs spec on bench/structs/.",
        epilog="The third gate, that the check decides each row of Decisions, is not run here: "
        "it is the test suite of part 6.",
    )
    parser.add_argument("--class-only", action="store_true", help="run the class versions only")
    parser.add_argument("--programs", help="a comma list of programs; by default all")
    parser.add_argument("--sisu", default=DEFAULT_SISU, help="the compiler")
    parser.add_argument("--bench", type=Path, default=BENCH, help="the directory of the programs")
    parser.add_argument("--out", type=Path, default=ROOT / DEFAULT_OUT, help="the work directory")
    parser.add_argument("--time-bound", type=float, default=7200, help="seconds for the whole run")
    args = parser.parse_args(argv)
    programs = args.programs.split(",") if args.programs else list(PROGRAMS)
    unknown = [p for p in programs if p not in PROGRAMS]
    if unknown:
        print(f"structs_gate: no such program: {', '.join(unknown)}", file=sys.stderr)
        return 1
    deadline = Deadline(args.time_bound)
    args.out.mkdir(parents=True, exist_ok=True)
    rows: list[tuple[Row, str]] = []
    failed = False
    try:
        for program in programs:
            inputs = (
                generated(program, "1x", args.bench, args.out, deadline),
                generated(program, "4x", args.bench, args.out, deadline),
            )
            cls = measure(
                deadline, args.sisu, args.bench / program / "class.sisu", inputs, args.out
            )
            if args.class_only:
                rows.append((cls, "measured"))
                continue
            struct = measure(
                deadline, args.sisu, args.bench / program / "struct.sisu", inputs, args.out
            )
            result = judge(program, struct, cls)
            failed = failed or result.startswith("FAIL")
            rows.extend([(cls, "-"), (struct, result)])
    except Fail as e:
        print(f"structs_gate: {e}", file=sys.stderr)
        return 1
    print(table(rows))
    if "parser" in programs:
        print("parser: the counts are those of `dump` less those of `lex`.")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
