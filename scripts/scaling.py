#!/usr/bin/env python3
"""Finds the shapes of program for which a stage of the Sisu compiler is superlinear.

Usage: python3 scripts/scaling.py [--shapes a,b] [--counts FILE] [--out target/scaling]
       [--sisu target/release/sisu] [--time-bound SECONDS]

A shape is a kind of program that grows in one direction. For each shape, the script makes the
program at size 0 and at the sizes n, 2n, 4n and 8n. It runs `sisu build --emit ir-raw` on each
under callgrind, and reads the instruction count of each stage function. It prints the growth of
each stage (its count less its count at size 0), the ratio at each doubling, and the verdict.

Before callgrind, `sisu check` runs on each shape at size 0 and at 8n. A shape that gives a
diagnostic stops the run: without this check, the failure shows as a missing stage function.

The script exits with code 1 in these cases:
- A superlinear case has no row in `KNOWN`.
- A row of `KNOWN` looks fixed.
- A check, a command or a file operation fails.

`--counts FILE` reads the `counts.json` of an earlier run and measures nothing.
Every measurement command runs through one `Deadline`.
"""

import argparse
import itertools
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from collections.abc import Callable
from fractions import Fraction
from pathlib import Path
from typing import Any

from compile_speed import Deadline, Fail

# The name of a stage and its function, in the order of the stages.
STAGES = [
    ("lex", "sisuc::lexer::lex"),
    ("parse", "sisuc::parser::parse"),
    ("check", "sisuc::check::check"),
    ("mono", "sisuc::mono::instantiate"),
    ("mir", "sisuc::mir::build"),
    ("codegen", "sisuc::codegen::compile"),
]
# The cases that are superlinear today, and the cases just under the rule that have a known cause:
# (shape, stage, issue). The pull request that fixes a case removes its row.
KNOWN = [
    ("mut exits", "mir", 502),
    ("mut exits", "codegen", 502),
    ("owned exits", "mir", 502),
    ("owned exits", "codegen", 502),
]
ANNOTATE = ["callgrind_annotate", "--inclusive=yes", "--threshold=100", "--show=Ir", "--auto=no"]
DIAGNOSTIC = re.compile(r"^(error|warning)\[([EW]\d{4})\]")
DEFAULT_SISU = "target/release/sisu"
DEFAULT_OUT = "target/scaling"
ROOT = Path(__file__).resolve().parent.parent


# The width of a number in a name. The largest size of any shape is 8 * 250 = 2000.
# So each name has the same length at each size. Then the text of a program grows with the
# number of copies.
WIDTH = 4


def tag(i: int) -> str:
    """The number `i` of a copy, with the width of the largest size."""
    return f"{i:0{WIDTH}d}"


def nest(k: int, wrap: Callable[[str], str], core: str) -> str:
    """`core` inside `k` layers: each layer is `wrap(inner)`."""
    for _ in range(k):
        core = wrap(core)
    return core


def functions(k: int) -> str:
    funs = [f"fn step{tag(0)}() -> i64 {{\n    1\n}}\n"] if k else []
    funs += [f"fn step{tag(i)}() -> i64 {{\n    step{tag(i - 1)}() + 1\n}}\n" for i in range(1, k)]
    shown = f'"{{step{tag(k - 1)}()}}"' if k else '"none"'
    return "\n".join(funs) + f"\nfn main() {{\n    print({shown})\n}}\n"


def statements(k: int) -> str:
    lets = "".join(f"    let a{tag(i)} = a{tag(i - 1)} + 1\n" for i in range(1, k + 1))
    return f'fn main() {{\n    let a{tag(0)} = 0\n{lets}    print("{{a{tag(k)}}}")\n}}\n'


def expression_depth(k: int) -> str:
    return f'fn main() {{\n    let x = {nest(k, lambda s: f"1 + ({s})", "1")}\n    print("{{x}}")\n}}\n'


def float_left(k: int) -> str:
    chain = nest(k, lambda s: f"({s}) + 1.1", "1.1")
    return f'fn main() {{\n    let x = {chain}\n    print("{{x}}")\n}}\n'


def string_left(k: int) -> str:
    chain = nest(k, lambda s: f'({s}) + "a"', '"a"')
    return f'fn main() {{\n    let x = {chain}\n    print("{{x}}")\n}}\n'


def string_call_depth(k: int) -> str:
    call = nest(k, lambda s: f'_g("a", {s})', '"a"')
    return (
        "fn _g(_a: string, b: string) -> string {\n    b\n}\n\n"
        f'fn main() {{\n    let x = {call}\n    print("{{x}}")\n}}\n'
    )


def block_depth(k: int) -> str:
    # No indentation: it would make the text grow with the square of the depth, and so the lexer.
    body = nest(k, lambda s: f"if c {{\n{s}\n}}", 'print("in")')
    return f'fn main() {{\n    let c = true\n    print("{{c}}")\n{body}\n}}\n'


def generic_call_depth(k: int) -> str:
    call = nest(k + 1, lambda s: f"id({s})", "1")
    return (
        "fn id<T>(x: T) -> T {\n    x\n}\n\n"
        f'fn main() {{\n    let v = {call}\n    print("{{v}}")\n}}\n'
    )


def array_literal_depth(k: int) -> str:
    return (
        f"fn main() {{\n    let a = {nest(k + 1, lambda s: f'[{s}]', '1')}\n"
        '    print("{a.len()}")\n}\n'
    )


def array_literal_width(k: int) -> str:
    return (
        f'fn main() {{\n    let a: [i64] = [{", ".join(["7"] * k)}]\n    print("{{a.len()}}")\n}}\n'
    )


def match_arms(k: int) -> str:
    names = ["First"] + [f"V{tag(i)}" for i in range(k)]
    variants = "".join(f"    {name}\n" for name in names)
    arms = "".join(f"        {name} => 1\n" for name in names)
    built = ", ".join(f"Kind.{name}" for name in names)
    return (
        f"enum Kind {{\n{variants}}}\n\nfn pick(k: Kind) -> i64 {{\n    match k {{\n{arms}    }}\n}}\n\n"
        f"fn main() {{\n    let mut total = 0\n    for k in [{built}] {{\n        total += pick(k)\n"
        '    }\n    print("{total}")\n}\n'
    )


def fields(k: int) -> str:
    declared = "".join(f"    f{tag(i)}: i64\n" for i in range(k))
    given = "".join(f", f{tag(i)}: 1" for i in range(k))
    shown = "".join(f'    print("{{r.f{tag(i)}}}")\n' for i in range(k))
    return (
        f"class Row {{\n    id: i64\n{declared}}}\n\n"
        f'fn main() {{\n    let r = Row(id: 0{given})\n    print("{{r.id}}")\n{shown}}}\n'
    )


def conformers(k: int) -> str:
    classes = "".join(
        f"class C{tag(i)}: Speak {{\n    fn say(self) -> i64 {{\n        1\n    }}\n}}\n\n"
        for i in range(k)
    )
    calls = "".join(f"    total += ask(C{tag(i)}())\n" for i in range(k))
    return (
        "interface Speak {\n    fn say(self) -> i64\n}\n\n"
        "class Base: Speak {\n    fn say(self) -> i64 {\n        0\n    }\n}\n\n"
        + classes
        + "fn ask<T: Speak>(x: T) -> i64 {\n    x.say()\n}\n\n"
        f'fn main() {{\n    let mut total = 0\n    total += ask(Base())\n{calls}    print("{{total}}")\n}}\n'
    )


def type_depth(k: int) -> str:
    pair = nest(k + 1, lambda s: f"Pair(a: {s}, b: 1)", "1")
    return (
        "class Pair<A, B> {\n    a: A\n    b: B\n}\n\n"
        f'fn main() {{\n    let v = {pair}\n    let _first = v.a\n    print("{{v.b}}")\n}}\n'
    )


def interpolations(k: int) -> str:
    return f'fn main() {{\n    let x = 1\n    let s = "{"{x}" * (k + 1)}"\n    print(s)\n}}\n'


def live_locals(k: int) -> str:
    locals_ = "".join(f'    let live{tag(i)} = "v"\n' for i in range(k))
    uses = "".join(f"    total += live{tag(i)}.len()\n" for i in range(k))
    return (
        "fn main() {\n" + locals_ + "    let mut total = 0\n    let mut i = 0\n"
        "    while i < 3 {\n        i += 1\n        total += i\n    }\n"
        + uses
        + '    print("{total}")\n}\n'
    )


def method_chain(k: int) -> str:
    chain = "c" + ".next()" * (k + 1)
    return (
        "class Chain {\n    n: i64\n\n    fn next(self) -> Chain {\n        self\n    }\n}\n\n"
        f'fn main() {{\n    let c = Chain(n: 1)\n    let d = {chain}\n    print("{{d.n}}")\n}}\n'
    )


def comments(k: int) -> str:
    # Each row is a comment and a `let ... else` whose block is one plain exit: the checker looks
    # for a comment in the text that the fix of E0131 would remove, once per row. Row 0 is the base.
    rows = "".join(
        f'    // c{tag(i)}\n    let v{tag(i)} = o else {{ return 0 }}\n    print("{{v{tag(i)}}}")\n'
        for i in range(k + 1)
    )
    return f'fn f(o: i64?) -> i64 {{\n{rows}    0\n}}\n\nfn main() {{\n    print("{{f(1)}}")\n}}\n'


def mut_parameters(k: int) -> str:
    # Each function has two `mut` parameters and each call passes two `mut` arguments, so that the
    # W0002 judgement of a parameter and the copy-in and copy-out of a call grow with `k`.
    funs = "".join(
        f"fn set{tag(i)}(mut xs: [i64], mut n: i64) {{\n    xs[0] = n\n    n += 1\n}}\n\n"
        for i in range(k)
    )
    calls = "".join(f"    set{tag(i)}(mut xs, mut n)\n" for i in range(k))
    # Main writes both locals itself, so that the program at size 0 has no W0002.
    return (
        funs
        + f"fn main() {{\n    let mut xs = [0]\n    let mut n = 0\n    xs[0] = 1\n    n = 1\n{calls}"
        '    print("{xs[0]} {n}")\n}\n'
    )


def mut_returns(k: int) -> str:
    # `wide` has k plain parameters, one `mut` parameter and k early returns, each of which stores
    # the final value of the `mut` parameter. A pass that scans every parameter at each return is
    # quadratic in k. `main` adds to `total` once, so that size 0 has no W0002.
    plain = "".join(f"p{tag(i)}: i64, " for i in range(k))
    returns = "".join(f"    if p{tag(i)} == 1 {{\n        return\n    }}\n" for i in range(k))
    zeros = "0, " * k
    return (
        f"fn wide({plain}mut xs: [i64]) {{\n{returns}    xs[0] = 1\n}}\n\n"
        "fn main() {\n    let mut xs = [0]\n    let mut total = 0\n"
        f'    wide({zeros}mut xs)\n    total += xs[0]\n    print("{{total}}")\n}}\n'
    )


def mut_call_width(k: int) -> str:
    # `many` has k `mut` parameters of a class and k of `i64`. `main` calls it with the k `mut`
    # roots `mut v`, and then with a field of each: `mut v.x`. A search of all the arguments for
    # each `mut` argument, or of all the roots for each field, is quadratic in k. `main` adds to
    # `total` once, and reads a `Cell` once, so that size 0 has no W0002 or W0012.
    params = ", ".join(
        [f"mut a{tag(i)}: Cell" for i in range(k)] + [f"mut c{tag(i)}: i64" for i in range(k)]
    )
    bumps = "".join(f"    a{tag(i)} = Cell(x: 0)\n    c{tag(i)} += 1\n" for i in range(k))
    locals_ = "".join(f"    let mut v{tag(i)} = Cell(x: 0)\n" for i in range(k))
    given = ", ".join([f"mut v{tag(i)}" for i in range(k)] + [f"mut v{tag(i)}.x" for i in range(k)])
    shown = "".join(f"    total += v{tag(i)}.x\n" for i in range(k))
    return (
        "class Cell {\n    mut x: i64\n}\n\n"
        f"fn many({params}) {{\n{bumps}}}\n\n"
        "fn main() {\n    let mut total = 0\n    let base = Cell(x: 1)\n    total += base.x\n"
        + locals_
        + f'    many({given})\n{shown}    print("{{total}}")\n}}\n'
    )


def mut_exits(k: int) -> str:
    # `many` has k `mut` parameters and k early returns, and each return stores every `mut`
    # parameter, so `mir` and `codegen` grow with k squared (#502). `main` reads each local, so that
    # no size has a W0014, and adds to `total` once, so that size 0 has no W0002.
    params = "".join(f"mut x{tag(i)}: [i64], " for i in range(k))
    returns = "".join(f"    if x{tag(i)}[0] == stop {{\n        return\n    }}\n" for i in range(k))
    writes = "".join(f"    x{tag(i)}[0] = 1\n" for i in range(k))
    locals_ = "".join(f"    let mut x{tag(i)} = [0]\n" for i in range(k))
    args = "".join(f"mut x{tag(i)}, " for i in range(k))
    shown = "".join(f"    total += x{tag(i)}[0]\n" for i in range(k))
    return (
        f"fn many({params}stop: i64) {{\n    if stop == -1 {{\n        return\n    }}\n{returns}{writes}}}\n\n"
        f"fn main() {{\n{locals_}    many({args}1)\n    let mut total = 0\n    total += 1\n{shown}"
        '    print("{total}")\n}\n'
    )


def owned_exits(k: int) -> str:
    # `owned` has k owned locals and an early return after each, and each return releases every
    # local in scope, so `mir` and `codegen` grow with k squared (#502).
    body = "".join(
        f"    let x{tag(i)} = [0]\n    if x{tag(i)}.len() == stop {{\n        return x{tag(i)}.len()\n    }}\n"
        for i in range(k)
    )
    return f"fn owned(stop: i64) -> i64 {{\n{body}    return stop\n}}\n\nfn main() {{\n    print(owned(1))\n}}\n"


def optional_depth(k: int) -> str:
    # An optional of k + 1 layers, in the field of a class and in a parameter. An array has a niche
    # for one layer only, so each layer past the first is a `{ flag, value }` pair, which codegen
    # lays out and counts one layer at a time (#461).
    q = "?" * (k + 1)
    return (
        f"class Holder {{\n    mut xs: [i64]{q}\n}}\n\n"
        f"fn store(h: Holder, xs: [i64]{q}) {{\n    h.xs = xs\n}}\n\n"
        "fn main() {\n    let h = Holder(xs: None)\n    store(h, None)\n"
        "    print(h.xs == None)\n}\n"
    )


def alias_chain(k: int) -> str:
    # A chain of k + 1 aliases, each of which names the one before it twice. Types are interned, so
    # the chain has k + 1 distinct types, but it has 2^k paths. The checker walks that read a field
    # type must visit each distinct type once. The chain `Ak` is in the field of a class and in
    # the payloads of an enum, as a value, an optional and an array. The chain `Bk<T>` is the
    # payload of the generic enum `G<T>`, which `lower_enums` reads. Main builds each variant of `P`
    # and each type, and reads them, so that size 0 has no W0011, W0012, W0013 or W0014.
    t = tag
    a = [f"type A{t(0)} = P<i64, i64>"] + [
        f"type A{t(i)} = P<A{t(i - 1)}, A{t(i - 1)}>" for i in range(1, k + 1)
    ]
    b = [f"type B{t(0)}<T> = P<T, T>"] + [
        f"type B{t(i)}<T> = P<B{t(i - 1)}<T>, B{t(i - 1)}<T>>" for i in range(1, k + 1)
    ]
    lets = [f"    let a{t(0)}: A{t(0)} = P.Y(2)"] + [
        f"    let a{t(i)}: A{t(i)} = P.X(a{t(i - 1)})" for i in range(1, k + 1)
    ]
    lets += [f"    let b{t(0)}: B{t(0)}<i64> = P.X(1)"] + [
        f"    let b{t(i)}: B{t(i)}<i64> = P.X(b{t(i - 1)})" for i in range(1, k + 1)
    ]
    last, top = f"A{t(k)}", f"a{t(k)}"
    return (
        "enum P<A, B> {\n    X(A)\n    Y(B)\n}\n\n" + "\n".join(a) + "\n" + "\n".join(b) + "\n\n"
        f"class C {{\n    v: {last}\n    o: {last}?\n}}\n\n"
        f"enum E {{\n    V({last})\n    W([{last}])\n}}\n\n"
        f"enum G<T> {{\n    V(B{t(k)}<T>)\n    W\n}}\n\n"
        "fn main() {\n" + "\n".join(lets) + "\n"
        f"    let c = C(v: {top}, o: {top})\n    let es = [E.V({top}), E.W([{top}])]\n"
        f"    let gs = [G.V(b{t(k)}), G.W]\n"
        "    print(c.o == None)\n    print(shape(c.v))\n"
        "    for e in es {\n        match e {\n            E.V(x) => print(shape(x))\n"
        "            E.W(xs) => print(xs.len())\n        }\n    }\n"
        "    for g in gs {\n        match g {\n            G.V(q) => print(shape(q))\n"
        "            G.W => print(0)\n        }\n    }\n}\n\n"
        "fn shape<A, B>(p: P<A, B>) -> i64 {\n    match p {\n        P.X(_) => 1\n"
        "        P.Y(_) => 2\n    }\n}\n"
    )


# The name of a shape, its `n` and the program at a size. The sizes are 0, n, 2n, 4n and 8n.
SHAPES = {
    "functions": (250, functions),
    "statements": (250, statements),
    "expression depth": (12, expression_depth),
    "block depth": (12, block_depth),
    "generic call depth": (24, generic_call_depth),
    "array literal depth": (24, array_literal_depth),
    "array literal width": (250, array_literal_width),
    "match arms": (50, match_arms),
    "fields": (50, fields),
    "conformers": (50, conformers),
    "type depth": (24, type_depth),
    "interpolations": (50, interpolations),
    "live locals": (50, live_locals),
    "method chain": (24, method_chain),
    "float left": (12, float_left),
    "string left": (12, string_left),
    "string call depth": (12, string_call_depth),
    "comments": (250, comments),
    "mut parameters": (250, mut_parameters),
    "mut returns": (250, mut_returns),
    "mut call width": (1000, mut_call_width),
    "mut exits": (25, mut_exits),
    "owned exits": (25, owned_exits),
    "alias chain": (12, alias_chain),
    "optional depth": (24, optional_depth),
}


# The numbers of the rule. They are exact fractions: a float would turn `2.45 - 0.05` into
# 2.4000000000000004, and a ratio on a boundary would fall on the wrong side.
SUPERLINEAR_R3 = Fraction("2.3")
SUPERLINEAR_R2 = Fraction("2.1")
FALL_ALLOWED = Fraction("0.05")
STEEP = Fraction(3)
KEPT_R3 = Fraction("2.15")
FALL_FIXED = Fraction("0.15")


def judge(growth: list[int], total_8n: int) -> tuple[str, list[Fraction | None]]:
    """The verdict for one stage, and its three ratios as exact fractions.

    `growth` is the growth at n, 2n, 4n and 8n. `total_8n` is the sum of the growth at 8n of the
    six stages. A ratio is None when its divisor is 0 or less.

    The verdict is `not judged` when the growth at 8n is not positive, or is under 1% of `total_8n`.
    Else it is `no ratio` when a ratio is None. Else it is `superlinear` or `linear`.
    """
    ratios = [Fraction(b, a) if a > 0 else None for a, b in itertools.pairwise(growth)]
    if growth[3] <= 0 or growth[3] * 100 < total_8n:
        return "not judged", ratios
    if None in ratios:
        return "no ratio", ratios
    r1, r2, r3 = ratios
    # pyrefly: ignore[unsupported-operation]  `None in ratios` has returned: no ratio is None here
    if r3 > SUPERLINEAR_R3 and r2 >= SUPERLINEAR_R2 and (r3 >= r1 - FALL_ALLOWED or r3 >= STEEP):
        return "superlinear", ratios
    return "linear", ratios


def looks_fixed(verdict: str, ratios: list[Fraction | None]) -> bool:
    """True when the stage of a known row is not superlinear any more, and not in the gap.

    The gap keeps a row from flapping when a change to the linear part moves a ratio a little.
    """
    if verdict in ("not judged", "no ratio"):
        return True
    r1, _, r3 = ratios
    # pyrefly: ignore[unsupported-operation]  only the verdict "no ratio" has a None ratio
    return r3 < KEPT_R3 or (r3 < STEEP and r3 < r1 - FALL_FIXED)


KNOWN_PREFIX = "known case: "


def compare(
    verdicts: dict[tuple[str, str], tuple[str, list[Fraction | None]]],
    known: list[tuple[str, str, int]],
    measured: set[str],
) -> tuple[list[str], list[str], int]:
    """Compares the verdicts with the known list.

    Returns the lines for known cases, the lines for failures and the exit code. A row of `known`
    whose shape is not in `measured` is skipped.
    """
    rows = {(shape, stage): issue for shape, stage, issue in known}
    lines: list[str] = []
    failures: list[str] = []
    for (shape, stage), (verdict, _) in verdicts.items():
        if verdict == "superlinear" and (shape, stage) not in rows:
            failures.append(f"{shape}, {stage} is superlinear: open an issue and add a row")
    for (shape, stage), issue in rows.items():
        if shape not in measured:
            continue
        case = f"{shape}, {stage} (#{issue})"
        if looks_fixed(*verdicts[shape, stage]):
            failures.append(f"#{issue} looks fixed: remove its row ({shape}, {stage})")
        else:
            lines.append(KNOWN_PREFIX + case)
    return lines, failures, 1 if failures else 0


ANNOTATE_LINE = re.compile(r"^\s*([\d,]+) \(\s*[\d.]+%\)\s+(.*)$")
OBJECT_FIELD = re.compile(r" \[[^\]]*\]$")


def stage_counts(annotate: str) -> dict[str, int]:
    """The count of each stage, from the output of `callgrind_annotate`.

    A line is `<count> (<percent>)  <file>:<function> [<object>]`. A line without the object field
    is a row for an inlined file, not a function entry, and is skipped.

    A line counts for a stage when its function is the function of the stage. It also counts when
    the function has `.` and a suffix after it (an LTO clone). A closure of the function has
    another name and does not count. It raises Fail when a stage has no line or more than one.
    """
    rows = []
    for line in annotate.splitlines():
        match = ANNOTATE_LINE.match(line)
        if match is None or ":" not in match[2]:
            continue
        field = OBJECT_FIELD.search(match[2])
        if field is not None:
            where = match[2][: field.start()]
            rows.append((where, where.split(":", 1)[1], int(match[1].replace(",", ""))))
    counts: dict[str, int] = {}
    for stage, function in STAGES:
        found = [
            (where, count)
            for where, name, count in rows
            if name == function or name.startswith(function + ".")
        ]
        if not found:
            raise Fail(f"{function} is not in the callgrind output")
        if len(found) > 1:
            names = ", ".join(where for where, _ in found)
            raise Fail(f"{function} is in the callgrind output {len(found)} times: {names}")
        counts[stage] = found[0][1]
    return counts


def is_count(value: object) -> bool:
    """True for an integer. A boolean is not a count."""
    return type(value) is int


def invalid(data: object) -> str | None:
    """What is wrong with the structure of a counts file, or None."""
    shapes = data.get("shapes") if isinstance(data, dict) else None
    if not isinstance(shapes, dict):
        return 'it has no "shapes" object'
    for name, shape in shapes.items():
        sizes = shape.get("sizes") if isinstance(shape, dict) else None
        if not (
            isinstance(sizes, list)
            and len(sizes) == 5
            and all(is_count(s) for s in sizes)
            and sizes[1] > 0
            and sizes == [0, sizes[1], 2 * sizes[1], 4 * sizes[1], 8 * sizes[1]]
        ):
            return f'{name}: "sizes" must be 0, n, 2n, 4n and 8n, with n above 0'
        stages = shape.get("counts")
        for stage, _ in STAGES:
            values = stages.get(stage) if isinstance(stages, dict) else None
            if not (
                isinstance(values, list) and len(values) == 5 and all(is_count(v) for v in values)
            ):
                return f'{name}: "counts" must have a list of 5 integers for {stage}'
    return None


def load(path: Path) -> dict[str, Any]:
    """The counts of an earlier run (`counts.json`). It raises Fail when the file is not valid."""
    try:
        data = json.loads(path.read_text())
    except (OSError, ValueError) as e:
        raise Fail(f"{path}: {e}") from None
    problem = invalid(data)
    if problem is not None:
        raise Fail(f"{path}: {problem}")
    return data


def verdicts(counts: dict[str, Any]) -> dict[tuple[str, str], tuple[str, list[Fraction | None]]]:
    """`judge` for each shape and stage of `counts`: {(shape, stage): (verdict, ratios)}."""
    result: dict[tuple[str, str], tuple[str, list[Fraction | None]]] = {}
    for shape, data in counts["shapes"].items():
        growth = {
            stage: [c - data["counts"][stage][0] for c in data["counts"][stage][1:]]
            for stage, _ in STAGES
        }
        total = sum(g[3] for g in growth.values())
        for stage, _ in STAGES:
            result[shape, stage] = judge(growth[stage], total)
    return result


def table(
    counts: dict[str, Any], verdicts: dict[tuple[str, str], tuple[str, list[Fraction | None]]]
) -> str:
    """The markdown table of the run: the growths, the ratios and the verdict of each stage."""
    lines = ["# Scaling sweep", ""]
    for key in ("commit", "cpu", "valgrind", "glibc"):
        lines.append(f"- {key}: {counts.get(key, 'unknown')}")
    for shape, data in counts["shapes"].items():
        sizes = ", ".join(str(k) for k in data["sizes"])
        lines += [
            "",
            f"## {shape}",
            "",
            f"Sizes: {sizes}. Growth is the count less the count at size 0.",
            "",
            "| stage | g(n) | g(2n) | g(4n) | g(8n) | r1 | r2 | r3 | verdict |",
            "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |",
        ]
        for stage, _ in STAGES:
            cs = data["counts"][stage]
            verdict, ratios = verdicts[shape, stage]
            growth = [str(c - cs[0]) for c in cs[1:]]
            shown = ["-" if r is None else f"{float(r):.2f}" for r in ratios]
            lines.append(f"| {stage} | " + " | ".join(growth + shown) + f" | {verdict} |")
    return "\n".join(lines) + "\n"


def glibc() -> str:
    """The version of the C library, or `unknown`."""
    try:
        return os.confstr("CS_GNU_LIBC_VERSION") or "unknown"
    except (ValueError, OSError):
        return "unknown"


def cpu_model() -> str:
    """The model name of the first CPU, or `unknown`."""
    try:
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            if line.startswith("model name"):
                return line.split(":", 1)[1].strip()
    except OSError:
        pass
    return "unknown"


def commit() -> str:
    """The short hash of HEAD in this repository, or `unknown`."""
    try:
        done = subprocess.run(
            ["git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"], capture_output=True, text=True
        )
    except OSError:
        return "unknown"
    return done.stdout.strip() if done.returncode == 0 and done.stdout.strip() else "unknown"


def check_clean(deadline: Deadline, sisu: str, path: Path, what: str) -> None:
    """Runs `sisu check`. A failed command, or a diagnostic on stderr, raises Fail."""
    for line in deadline.run([sisu, "check", path], what).stderr.splitlines():
        match = DIAGNOSTIC.match(line)
        if match:
            raise Fail(f"{what}: {match[2]}")


def measure(args: argparse.Namespace, counts: dict[str, Any]) -> None:
    """Measures the shapes of `args` and puts each into `counts['shapes']` when it is done."""
    if shutil.which("valgrind") is None:
        raise Fail("valgrind is not on PATH")
    deadline = Deadline(args.time_bound)
    counts.update(commit=commit(), cpu=cpu_model(), glibc=glibc())
    counts["valgrind"] = deadline.run(
        ["valgrind", "--version"], "valgrind --version"
    ).stdout.splitlines()[0]
    for name in args.shapes or SHAPES:
        n, make = SHAPES[name]
        sizes = [0, n, 2 * n, 4 * n, 8 * n]
        directory = Path(args.out) / name.replace(" ", "-")
        directory.mkdir(parents=True, exist_ok=True)
        files = [directory / f"size-{k}.sisu" for k in sizes]
        for k, path in zip(sizes, files, strict=False):
            path.write_text(make(k))
        for k, path in ((sizes[0], files[0]), (sizes[-1], files[-1])):
            check_clean(deadline, args.sisu, path, f"{name}, size {k}")
        per_stage: dict[str, list[int]] = {stage: [] for stage, _ in STAGES}
        with tempfile.TemporaryDirectory() as tmp:
            profile = os.path.join(tmp, "callgrind.out")
            for k, path in zip(sizes, files, strict=False):
                what = f"{name}, size {k}"
                deadline.run(
                    [
                        "valgrind",
                        "--tool=callgrind",
                        f"--callgrind-out-file={profile}",
                        args.sisu,
                        "build",
                        "--emit",
                        "ir-raw",
                        path,
                    ],
                    what,
                    stdout=subprocess.DEVNULL,
                )
                found = stage_counts(deadline.run([*ANNOTATE, profile], what).stdout)
                for stage, count in found.items():
                    per_stage[stage].append(count)
        counts["shapes"][name] = {"sizes": sizes, "counts": per_stage}


def shape_list(text: str) -> list[str]:
    """The shapes of a `--shapes` value."""
    names = text.split(",")
    for name in names:
        if name not in SHAPES:
            raise argparse.ArgumentTypeError(f"{name!r} is not a shape: {', '.join(SHAPES)}")
    return names


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="Find the shapes for which a stage is superlinear."
    )
    parser.add_argument(
        "--shapes", type=shape_list, help="the shapes to measure, separated by commas"
    )
    parser.add_argument(
        "--counts", help="read the counts.json of an earlier run, and measure nothing"
    )
    parser.add_argument(
        "--out", default=DEFAULT_OUT, help="the directory for table.md and counts.json"
    )
    parser.add_argument("--sisu", default=DEFAULT_SISU, help="the compiler")
    parser.add_argument("--time-bound", type=float, default=1800, help="seconds for the whole run")
    args = parser.parse_args(argv)
    counts: dict[str, Any] = {
        "commit": "unknown",
        "cpu": "unknown",
        "valgrind": "unknown",
        "glibc": "unknown",
        "shapes": {},
    }
    failure = None
    try:
        if args.counts:
            counts = load(Path(args.counts))
        else:
            measure(args, counts)
    except (Fail, OSError) as e:
        failure = str(e)
        if args.counts:
            # Write nothing: the output directory may hold the file that was read.
            print(f"scaling: {failure}", file=sys.stderr)
            return 1
    results = verdicts(counts)
    text = table(counts, results)
    print(text)
    out = Path(args.out)
    try:
        out.mkdir(parents=True, exist_ok=True)
        (out / "counts.json").write_text(json.dumps(counts, indent=2) + "\n")
        table_text = f"{text}\nscaling: {failure}\n" if failure else text
        (out / "table.md").write_text(table_text)
    except OSError as e:
        if failure:
            print(f"scaling: {failure}", file=sys.stderr)
        print(f"scaling: cannot write the report: {e}", file=sys.stderr)
        return 1
    if failure:
        print(f"scaling: {failure}", file=sys.stderr)
        return 1
    lines, failures, code = compare(results, KNOWN, set(counts["shapes"]))
    for line in lines:
        print(f"scaling: {line}")
        if os.environ.get("GITHUB_ACTIONS") == "true":
            print(f"::warning title=Known case::{line.removeprefix(KNOWN_PREFIX)}")
    for line in failures:
        print(f"scaling: {line}", file=sys.stderr)
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
