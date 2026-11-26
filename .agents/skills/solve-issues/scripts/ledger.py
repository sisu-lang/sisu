"""Reserve a batch ledger, add one line to it, or set a group's state (ledger.md gives its form).

Usage:
  python3 ledger.py init <scratch-dir> <prefix>
      creates <scratch-dir>/<batch>/progress.md atomically and prints its path.
  python3 ledger.py <progress.md> event "<text>"
  python3 ledger.py event <progress.md> "<text>"
      adds "- <date time> <text>" at the end of Events.
  python3 ledger.py <progress.md> after-merge "<text>"
  python3 ledger.py after-merge <progress.md> "<text>"
      adds "- <text>" at the end of "After the merge".
  python3 ledger.py <progress.md> group <group> "<state>"
  python3 ledger.py group <progress.md> <group> "<state>"
      sets the State cell of the group's row in Groups.
"""

import string
import sys
from datetime import datetime
from pathlib import Path

SUBCOMMANDS = ("event", "after-merge", "group")
TEMPLATE = (
    "# Batch {batch}\n\n"
    "Started {date} on origin/main <sha>. Build: <path of the origin/main sisu>.\n\n"
    "## Issues\n\n"
    "| Issue | State | Group | Note |\n"
    "| --- | --- | --- | --- |\n\n"
    "## Groups\n\n"
    "| Group | Issues | Order | Flow | Branch | Worktree | State |\n"
    "| --- | --- | --- | --- | --- | --- | --- |\n\n"
    "## Decisions\n\n"
    "## Tasks\n\n"
    "| Group | Task | Issues | Base | Head | Mutation (commit, last line) "
    "| Review run | Rulings | Verdict |\n"
    "| --- | --- | --- | --- | --- | --- | --- | --- | --- |\n\n"
    "## After the merge\n\n"
    "## Events\n"
)


def init_batch(scratch_dir: Path, prefix: str, now: datetime) -> Path:
    """Create a unique `<scratch_dir>/<batch>/progress.md` atomically and return its path."""
    if (
        not prefix
        or any(ch.isspace() for ch in prefix)
        or "/" in prefix
        or "\\" in prefix
        or prefix in (".", "..")
    ):
        raise ValueError(f"invalid batch prefix {prefix!r}")
    scratch_dir.mkdir(parents=True, exist_ok=True)
    candidates = [
        prefix,
        *(f"{prefix}{letter}" for letter in string.ascii_lowercase[1:]),
        *(f"{prefix}-{n}" for n in range(2, 100)),
    ]
    for batch in candidates:
        folder = scratch_dir / batch
        try:
            folder.mkdir(exist_ok=False)
        except FileExistsError:
            continue
        ledger_path = folder / "progress.md"
        ledger_path.write_text(
            TEMPLATE.format(batch=batch, date=f"{now:%Y-%m-%d}"), encoding="utf-8"
        )
        return ledger_path
    raise ValueError(f"no free batch folder for prefix {prefix!r} in {scratch_dir}")


def add(ledger: str, kind: str, text: str, now: datetime) -> str:
    """Return the ledger text with the new line in its section."""
    if kind == "event":
        line = f"- {now:%Y-%m-%d %H:%M} {text}"
        heading = "## Events"
    elif kind == "after-merge":
        line = f"- {text}"
        heading = "## After the merge"
    else:
        raise ValueError(f"unknown kind {kind!r}: use event or after-merge")
    lines = ledger.splitlines()
    if heading not in lines:
        raise ValueError(f"the ledger has no {heading!r} section")
    end = lines.index(heading) + 1
    while end < len(lines) and not lines[end].startswith("## "):
        end += 1
    # The new line goes after the last line of the section, before its trailing blank lines.
    while end > 0 and not lines[end - 1].strip():
        end -= 1
    lines.insert(end, line)
    return "\n".join(lines) + "\n"


def set_group(ledger: str, group: str, state: str) -> str:
    """Return the ledger text with `state` in the last cell of the row of `group` in Groups."""
    if "|" in state or state.splitlines() not in ([], [state]):
        raise ValueError("the state has a `|` or a line break, which would split its row")
    lines = ledger.splitlines()
    if "## Groups" not in lines:
        raise ValueError("the ledger has no '## Groups' section")
    rows = lines.index("## Groups") + 1
    table_row = 0
    header: list[str] = []
    for row in range(rows, len(lines)):
        line = lines[row].strip()
        if line.startswith("## "):
            break  # the next section starts
        if not line.startswith("|"):
            if table_row:
                break  # the table has ended
            continue
        table_row += 1
        cells = line.removeprefix("|").removesuffix("|").split("|")
        if table_row == 1:
            header = cells
        # The first two rows of the table are its header and its delimiter, not groups.
        if table_row > 2 and cells[0].strip() == group:  # noqa: PLR2004  the header and the delimiter
            if len(cells) != len(header):
                msg = f"the row of group {group!r} has {len(cells)} cells, the header {len(header)}"
                raise ValueError(msg)
            cells[-1] = f" {state} "
            lines[row] = "|" + "|".join(cells) + "|"
            return "\n".join(lines) + "\n"
    raise ValueError(f"Groups has no row for group {group!r}")


def parse_args(argv: list[str]) -> tuple[str, str, list[str]] | None:
    """Return `(kind, path, rest)` for an edit command, accepting path-first or kind-first."""
    if len(argv) < 2:  # noqa: PLR2004  need at least a path and a subcommand
        return None
    if argv[0] in SUBCOMMANDS and not Path(argv[0]).is_file():
        kind, path, rest = argv[0], argv[1], argv[2:]
    else:
        path, kind, rest = argv[0], argv[1], argv[2:]
    want = 2 if kind == "group" else 1
    if len(rest) != want:
        return None
    return kind, path, rest


def run_init(argv: list[str], now: datetime) -> int:
    """Handle `ledger.py init <scratch-dir> <prefix>` and return its exit code."""
    if len(argv) != 3:  # noqa: PLR2004  init, scratch dir and prefix
        print(__doc__, file=sys.stderr)
        return 2
    try:
        created = init_batch(Path(argv[1]), argv[2], now)
    except (OSError, ValueError) as e:
        print(f"ledger.py: {e}", file=sys.stderr)
        return 2
    print(created)
    return 0


def main(argv: list[str]) -> int:
    """Run the command that `argv` gives on the ledger, and return the exit code."""
    now = datetime.now().astimezone()
    if argv and argv[0] == "init":
        return run_init(argv, now)
    parsed = parse_args(argv)
    if parsed is None:
        print(__doc__, file=sys.stderr)
        return 2
    kind, path, rest = parsed
    try:
        ledger = Path(path).read_text(encoding="utf-8")
        if kind == "group":
            updated = set_group(ledger, rest[0], rest[1])
        else:
            updated = add(ledger, kind, rest[0], now)
        Path(path).write_text(updated, encoding="utf-8")
    except (OSError, ValueError) as e:
        print(f"ledger.py: {e}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
