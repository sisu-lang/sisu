"""Compare the diagnostics of two sisu builds on every `.sisu` file under each DIR.

The script prints each program whose diagnostics differ, and each program that crashes the head,
also when the base crashes. A review uses it to show that a change accepts no program it should
reject, adds no cascade, changes no text it should keep and crashes on no program.

A diagnostic differs when its code, position, message, labels, help or fixes (their message, edits
and safe flag) differ. With `--codes`, only the codes, positions and count count, for a change that
rewords many diagnostics on purpose. `--full` is the default, and is still accepted.

It runs at most two sisu processes at once, so that it can run next to other work. `--jobs=N`
runs at most N.

Usage: python3 diag_diff.py [--codes] [--jobs=N] BASE_SISU HEAD_SISU DIR...
Exits 1 when a program differs or crashes the head.
"""

import json
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

# (code, severity, line, column, secondaries, text), as `diagnostics` documents.
Diagnostic = tuple[str, str, int, int, tuple[tuple[int, int], ...], str]

# The fields of a diagnostic that the default compares, besides its code and positions.
TEXT = ("message", "help", "fixes")


def diagnostics(sisu: str, path: Path, *, full: bool = True) -> list[Diagnostic]:
    """Return each diagnostic sisu reports on `path`.

    A diagnostic is (code, severity, line, column, secondaries, text).

    `text` is '' unless `full`; then it is the JSON of the diagnostic's text and fixes.
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
            secondary = tuple((s["line"], s["column"]) for s in d["secondary"])
            text = (
                json.dumps(
                    {
                        **{k: d.get(k) for k in TEXT},
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


def main(base_sisu: str, head_sisu: str, *roots: str, full: bool = True, jobs: int = 2) -> int:
    """Print each program whose diagnostics differ between the builds, and return the exit code."""
    paths = sorted({p for root in roots for p in Path(root).rglob("*.sisu")})
    with ThreadPoolExecutor(max_workers=jobs) as pool:
        # pyrefly: ignore[implicit-any-lambda]  `pool.map` gives the lambda its type only after `paths`
        bases = pool.map(lambda p: diagnostics(base_sisu, p, full=full), paths)
        # pyrefly: ignore[implicit-any-lambda]  as for `bases`
        heads = pool.map(lambda p: diagnostics(head_sisu, p, full=full), paths)
        rows = list(zip(paths, bases, heads, strict=False))
    counts = {}
    for path, base, head in rows:
        if k := kind(base, head):
            counts[k] = counts.get(k, 0) + 1
            print(f"{k}: {path}\n  base: {show(base)}\n  head: {show(head)}")
    tally = ", ".join(f"{n} {k}" for k, n in sorted(counts.items())) or "no differences"
    print(f"diag_diff: {len(paths)} programs, {tally}")
    return 1 if counts else 0


if __name__ == "__main__":
    jobs = [int(a.removeprefix("--jobs=")) for a in sys.argv[1:] if a.startswith("--jobs=")]
    args = [
        a for a in sys.argv[1:] if a not in ("--codes", "--full") and not a.startswith("--jobs=")
    ]
    if len(args) < 3:  # noqa: PLR2004  the two builds and one directory of the usage line
        sys.exit(__doc__)
    sys.exit(main(*args, full="--codes" not in sys.argv[1:], jobs=jobs[-1] if jobs else 2))
