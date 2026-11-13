r"""Opens a pull request of sisu-lang/sisu from a title and a body file.

Usage: python3 scripts/open_pr.py --title TITLE --body-file FILE [--head BRANCH] [--base B] [ROOT]

The body goes from the file into the JSON of the request, and never through a shell. So
backticks and `$` in the body stay as they are. The title is an argument: put it in single
quotes, and write an apostrophe in it as '\''. HEAD is the branch checked out in ROOT, the
current directory by default. Push it first.
The script prints the URL of the new pull request.
It calls `gh api` (REST), so it needs the network: run it with the sandbox off.
"""

import argparse
import json
import subprocess
import sys
from pathlib import Path

REPO = "repos/sisu-lang/sisu"


def main(argv: list[str]) -> int:
    """Run the command line `argv`, and return the exit code."""
    parser = argparse.ArgumentParser(description="Opens a pull request of sisu-lang/sisu.")
    parser.add_argument("root", nargs="?", default=".")
    parser.add_argument("--title", required=True)
    parser.add_argument("--body-file", required=True, type=Path)
    parser.add_argument("--head")
    parser.add_argument("--base", default="main")
    args = parser.parse_args(argv)
    try:
        head = (
            args.head
            or subprocess.run(
                ["git", "-C", args.root, "symbolic-ref", "--short", "HEAD"],
                capture_output=True,
                text=True,
                check=True,
            ).stdout.strip()
        )
        body = args.body_file.read_text(encoding="utf-8")
    except subprocess.CalledProcessError as e:
        print(f"open_pr: FAIL: cannot read the branch of {args.root}: {e.stderr.strip()}")
        return 1
    except (OSError, UnicodeDecodeError) as e:
        print(f"open_pr: FAIL: {e}")
        return 1
    request = {"title": args.title, "head": head, "base": args.base, "body": body}
    result = subprocess.run(
        ["gh", "api", f"{REPO}/pulls", "--input", "-", "--jq", ".html_url"],
        input=json.dumps(request),
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        print(f"open_pr: FAIL: {result.stderr.strip() or result.stdout.strip()}")
        return 1
    print(result.stdout.strip())
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
