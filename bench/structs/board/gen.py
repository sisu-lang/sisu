#!/usr/bin/env python3
"""Write the commands of the task board: gen.py COMMANDS HISTORY_CAP [--core].

The same arguments give the same text. The first line is `history HISTORY_CAP`.

With `--core`, a drawn `find`, `related`, `del` or `watch` prints nothing. The text then holds
only the commands that touch the board, the index and the events.
"""

import argparse
import random

SEED = 7
VOCAB = [f"w{i}" for i in range(40)]
# A draw that `--core` drops.
DROPPED = ("find", "related", "del", "watch")


def lines(commands: int, history: int, *, core: bool) -> list[str]:
    """Return the lines of the text: `history N`, then one line for each drawn command."""
    rng = random.Random(SEED)  # noqa: S311  a fixed seed gives the same text, and no secret
    out = [f"history {history}"]
    tasks = 0
    for _ in range(commands):
        r = rng.random()
        title = " ".join(rng.choices(VOCAB, k=3))
        pick = rng.randrange(max(tasks, 1))
        line = ""
        if r < 0.45 or tasks == 0:
            line = f"add {title}"
            tasks += 1
        elif r < 0.60:
            line = f"select {pick}"
        elif r < 0.70:
            line = f"rename {title}"
        elif r < 0.80:
            line = f"tag {rng.choice(['auto', 'red', 'blue'])}"
        elif r < 0.87:
            line = f"done {pick}"
        elif r < 0.90:
            line = f"del {pick}"
        elif r < 0.95:
            line = f"find {rng.choice(VOCAB)}"
        elif r < 0.98:
            line = "related"
        elif r < 0.985:
            line = f"watch {rng.choice(VOCAB)}"
        elif r < 0.995:
            line = "show"
        else:
            line = "undo"
        if core and line.split()[0] in DROPPED:
            continue
        if line.startswith("del "):
            tasks = max(tasks - 1, 0)
        out.append(line)
    return out


def main() -> None:
    """Print the text for the command line."""
    parser = argparse.ArgumentParser(description="Write the commands of the task board.")
    parser.add_argument("commands", type=int, help="how many commands to draw")
    parser.add_argument("history", type=int, help="the history cap on the first line")
    parser.add_argument("--core", action="store_true", help="drop find, related, del and watch")
    args = parser.parse_args()
    print("\n".join(lines(args.commands, args.history, core=args.core)))


if __name__ == "__main__":
    main()
