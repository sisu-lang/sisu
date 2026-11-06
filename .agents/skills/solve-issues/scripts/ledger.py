"""Add one line to a batch ledger (ledger.md gives its form).

Usage:
  python3 ledger.py <progress.md> event "<text>"
      adds "- <date time> <text>" at the end of Events
  python3 ledger.py <progress.md> after-merge "<text>"
      adds "- <text>" at the end of "After the merge"
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


def main(argv: list[str]) -> int:
    """Add the line that `argv` gives to the ledger file, and return the exit code."""
    if len(argv) != 3:  # noqa: PLR2004  the three words of the usage line
        print(__doc__, file=sys.stderr)
        return 2
    path, kind, text = argv
    ledger = Path(path).read_text(encoding="utf-8")
    try:
        updated = add(ledger, kind, text, datetime.now().astimezone())
    except ValueError as e:
        print(f"ledger.py: {e}", file=sys.stderr)
        return 2
    Path(path).write_text(updated, encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
