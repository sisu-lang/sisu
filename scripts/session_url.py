"""Rejects a commit message that holds an agent's session line or session URL: a commit names
its change, and a session link leads to a private chat log.

Usage: python3 session_url.py MESSAGE_FILE
"""

import re
import sys

SESSION = re.compile(r"claude\.ai/code/session|^Claude-Session:", re.M | re.I)


def main(paths: list[str]) -> int:
    bad = 0
    for path in paths:
        with open(path, encoding="utf-8") as f:
            for n, line in enumerate(f, 1):
                if not line.startswith("#") and SESSION.search(line):
                    print(f"{path}:{n}: remove the session line from the commit message")
                    bad = 1
    return bad


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
