"""Add one line to a batch ledger, or set a group's state in it (ledger.md gives its form).

Usage:
  python3 ledger.py <progress.md> event "<text>"
      adds "- <date time> <text>" at the end of Events.
  python3 ledger.py <progress.md> after-merge "<text>"
      adds "- <text>" at the end of "After the merge".
  python3 ledger.py <progress.md> group <group> "<state>"
      sets the State cell of the group's row in Groups.
"""

import sys
from datetime import datetime
from pathlib import Path


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
    row = lines.index("## Groups") + 1
    table_row = 0
    while row < len(lines) and not lines[row].startswith("## "):
        cells = lines[row].split("|")
        if len(cells) > 2:  # noqa: PLR2004  a row has a cell between two bars
            table_row += 1
            # The first two rows of the table are its header and its delimiter, not groups.
            if table_row > 2 and cells[1].strip() == group:  # noqa: PLR2004  the header and the delimiter
                cells[-2] = f" {state} "
                lines[row] = "|".join(cells)
                return "\n".join(lines) + "\n"
        row += 1
    raise ValueError(f"Groups has no row for group {group!r}")


def main(argv: list[str]) -> int:
    """Add the line that `argv` gives to the ledger file, and return the exit code."""
    group = len(argv) > 1 and argv[1] == "group"
    if len(argv) != (4 if group else 3):  # the words of a group line, then of the others
        print(__doc__, file=sys.stderr)
        return 2
    path = argv[0]
    ledger = Path(path).read_text(encoding="utf-8")
    try:
        if group:
            updated = set_group(ledger, argv[2], argv[3])
        else:
            updated = add(ledger, argv[1], argv[2], datetime.now().astimezone())
    except ValueError as e:
        print(f"ledger.py: {e}", file=sys.stderr)
        return 2
    Path(path).write_text(updated, encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
