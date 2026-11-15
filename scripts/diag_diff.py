"""Compare the diagnostics of two sisu builds on every `.sisu` file under each DIR.

The script prints each program whose diagnostics differ, and each program that crashes the head,
also when the base crashes. A review uses it to show that a change accepts no program it should
reject, adds no cascade, changes no text it should keep and crashes on no program.

A diagnostic differs when its code, position, message, labels, help, notes or fixes (their message,
edits and safe flag) differ. With `--codes`, only the codes, positions and count count, for a change
that rewords many diagnostics on purpose. With `--primary`, only the code, severity, primary
position and count count, for a change that adds or moves secondary labels on purpose. `--full` is
the default, and is still accepted.

With `--apply`, the script looks at each safe fix that only one build gives on a changed program.
It applies the fix alone, and the build that gave it checks the result again. Under the
program, the script prints `lost clean fix` or `lost fix` for a fix of the base, and `new fix` or
`new broken fix` for a fix of the head. A fix is clean when it keeps the contract of
`safe_fix_breaches` in `crates/sisuc/src/fuzz.rs`: no report of a diagnostic that carries it is
left where it was, every other code stays, and no lexer or parser error is added. A fix that is not
clean gives the reason.

It runs at most two sisu processes at once, so that it can run next to other work. `--jobs=N`
runs at most N.

Usage: python3 diag_diff.py [--codes | --primary] [--apply] [--jobs=N] BASE_SISU HEAD_SISU DIR...
Exits 1 when a program differs or crashes the head.
"""

import json
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Any

# (code, severity, line, column, secondaries, text), as `diagnostics` documents.
Diagnostic = tuple[str, str, int, int, tuple[tuple[int, int], ...], str]

# The fields of a diagnostic that the default compares, besides its code and positions.
TEXT = ("message", "help", "notes", "fixes")


def diagnostics(
    sisu: str, path: Path, *, full: bool = True, primary: bool = False
) -> list[Diagnostic]:
    """Return each diagnostic sisu reports on `path`.

    A diagnostic is (code, severity, line, column, secondaries, text).

    `text` is '' unless `full`; then it is the JSON of the diagnostic's text and fixes. A missing
    `notes` key counts as `[]`, so a build from before the key compares equal to one with none.
    With `primary`, `secondaries` is empty.
    """
    run = subprocess.run(
        [sisu, "check", "--format", "json", str(path)], capture_output=True, text=True, check=False
    )
    if run.returncode not in (0, 1):
        return [("crash", "error", run.returncode, 0, (), "")]
    found: list[Diagnostic] = []
    for line in run.stderr.splitlines():
        d = json.loads(line)
        if d["kind"] == "diagnostic":
            p = d["primary"]
            secondary = () if primary else tuple((s["line"], s["column"]) for s in d["secondary"])
            text = (
                json.dumps(
                    {
                        **{k: d.get(k, [] if k == "notes" else None) for k in TEXT},
                        "label": p.get("label"),
                        "secondary": d["secondary"],
                    },
                    sort_keys=True,
                )
                if full
                else ""
            )
            found.append((d["code"], d["severity"], p["line"], p["column"], secondary, text))
    return found


def kind(base: list[Diagnostic], head: list[Diagnostic]) -> str | None:
    """Return how the head's diagnostics differ from the base's, or None when they are the same."""
    if any(d[0] == "crash" for d in head):
        return "crash"
    if base == head:
        return None
    rejects = (any(d[1] == "error" for d in base), any(d[1] == "error" for d in head))
    return {(True, False): "newly accepted", (False, True): "newly rejected"}.get(
        rejects, "changed"
    )


def show(ds: list[Diagnostic]) -> str:
    """Return the diagnostics `ds` as one line of text, or `none`."""
    return (
        ", ".join(
            f"{c} {line}:{col}" + "".join(f" +{a}:{b}" for a, b in s) + (f" {t}" if t else "")
            for c, _, line, col, s, t in ds
        )
        or "none"
    )


def reports(sisu: str, args: list[str]) -> list[dict[str, Any]]:
    """Return each diagnostic, as its JSON object, that `sisu ARGS --format json` reports."""
    run = subprocess.run(
        [sisu, *args, "--format", "json"], capture_output=True, text=True, check=False
    )
    lines = (run.stdout + run.stderr).splitlines()
    found = [json.loads(line) for line in lines if line.startswith("{")]
    return [d for d in found if d["kind"] == "diagnostic"]


def applied(source: bytes, fix: dict[str, Any]) -> bytes:
    """Return `source` with the edits of `fix`, whose offsets count bytes, applied."""
    for e in sorted(fix["edits"], key=lambda e: -e["start"]):
        source = source[: e["start"]] + e["replacement"].encode() + source[e["end"] :]
    return source


def moved(fix: dict[str, Any], at: int) -> int:
    """Return where offset `at` of a source is after `fix`: the edits that end before it move it."""
    edits = [e for e in fix["edits"] if e["end"] <= at]
    return at + sum(len(e["replacement"].encode()) - (e["end"] - e["start"]) for e in edits)


def minus(a: list[str], b: list[str]) -> list[str]:
    """Return `a` with one occurrence of each code of `b` removed."""
    left = list(a)
    for code in b:
        if code in left:
            left.remove(code)
    return left


def breaks(sisu: str, source: bytes, found: list[dict[str, Any]], fix: dict[str, Any]) -> str:
    """Return how `fix`, applied alone to `source`, breaks the contract, or '' when it is clean.

    `found` holds the diagnostics that `sisu` reports for `source`.
    """
    text = applied(source, fix).decode()
    after = reports(sisu, ["check", "-e", text])
    carriers = [d for d in found if fix in d["fixes"]]
    others = [d["code"] for d in found if fix not in d["fixes"]]
    at = {(a["code"], a["primary"]["start"], a["primary"]["end"]) for a in after}
    left = [
        d["code"]
        for d in carriers
        if (d["code"], moved(fix, d["primary"]["start"]), moved(fix, d["primary"]["end"])) in at
    ]
    removed = minus(others, [a["code"] for a in after])
    syntax = minus(
        [d["code"] for d in reports(sisu, ["build", "--emit", "ast", "-e", text])],
        [d["code"] for d in reports(sisu, ["build", "--emit", "ast", "-e", source.decode()])],
    )
    parts = [
        f"{what} {', '.join(codes)}"
        for what, codes in (("left", left), ("removed", removed), ("added syntax", syntax))
        if codes
    ]
    return "; ".join(parts)


def fix_lines(base_sisu: str, head_sisu: str, path: Path) -> list[str]:
    """Return a line for each safe fix that one build gives on `path` and the other does not."""
    source = path.read_bytes()
    found = {sisu: reports(sisu, ["check", str(path)]) for sisu in (base_sisu, head_sisu)}

    def safe(sisu: str) -> list[tuple[dict[str, Any], dict[str, Any]]]:
        pairs = [(d, f) for d in found[sisu] for f in d["fixes"] if f["safe"]]
        return [(d, f) for i, (d, f) in enumerate(pairs) if f not in [g for _, g in pairs[:i]]]

    lines = []
    sides = ((base_sisu, head_sisu, "lost clean fix", "lost fix"),)
    sides += ((head_sisu, base_sisu, "new fix", "new broken fix"),)
    for sisu, other, clean, broken in sides:
        theirs = [f for _, f in safe(other)]
        for d, fix in safe(sisu):
            if fix in theirs:
                continue
            p = d["primary"]
            where = f"{d['code']} {p['line']}:{p['column']} {fix['message']}"
            reason = breaks(sisu, source, found[sisu], fix)
            lines.append(f"  {broken}: {where}: {reason}" if reason else f"  {clean}: {where}")
    return lines


def main(  # noqa: PLR0913  the builds, the directories and one keyword for each flag
    base_sisu: str,
    head_sisu: str,
    *roots: str,
    full: bool = True,
    primary: bool = False,
    apply: bool = False,
    jobs: int = 2,
) -> int:
    """Print each program whose diagnostics differ between the builds, and return the exit code."""
    paths = sorted({p for root in roots for p in Path(root).rglob("*.sisu")})
    with ThreadPoolExecutor(max_workers=jobs) as pool:
        # pyrefly: ignore[implicit-any-lambda]  `pool.map` gives the lambda its type only after `paths`
        bases = pool.map(lambda p: diagnostics(base_sisu, p, full=full, primary=primary), paths)
        # pyrefly: ignore[implicit-any-lambda]  as for `bases`
        heads = pool.map(lambda p: diagnostics(head_sisu, p, full=full, primary=primary), paths)
        rows = list(zip(paths, bases, heads, strict=False))
    counts = {}
    for path, base, head in rows:
        if k := kind(base, head):
            counts[k] = counts.get(k, 0) + 1
            print(f"{k}: {path}\n  base: {show(base)}\n  head: {show(head)}")
            if apply and k != "crash":
                for line in fix_lines(base_sisu, head_sisu, path):
                    print(line)
    tally = ", ".join(f"{n} {k}" for k, n in sorted(counts.items())) or "no differences"
    print(f"diag_diff: {len(paths)} programs, {tally}")
    return 1 if counts else 0


if __name__ == "__main__":
    jobs = [int(a.removeprefix("--jobs=")) for a in sys.argv[1:] if a.startswith("--jobs=")]
    args = [
        a
        for a in sys.argv[1:]
        if a not in ("--codes", "--primary", "--full", "--apply") and not a.startswith("--jobs=")
    ]
    if len(args) < 3:  # noqa: PLR2004  the two builds and one directory of the usage line
        sys.exit(__doc__)
    flags = sys.argv[1:]
    sys.exit(
        main(
            *args,
            full="--codes" not in flags and "--primary" not in flags,
            primary="--primary" in flags,
            apply="--apply" in flags,
            jobs=jobs[-1] if jobs else 2,
        )
    )
