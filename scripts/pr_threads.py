"""Prints the review threads of a pull request of sisu-lang/sisu, one line each.

Usage: python3 scripts/pr_threads.py PR

A thread is an inline review comment from someone other than you, with its replies. Its line is
`open` when you have not replied, and `replied` when you have; open lines come first. Each line
holds the comment id (the id that a reply goes to), the author, path:line, a title and the URL.
The title of a CodeRabbit finding is its severity and its bold first line. For other comments, it
is the first 160 characters of the body.
A review whose body holds findings outside the diff gets an `outside` line with their count: read
that review for them. Run it with the sandbox off (`gh api` is REST and needs the network).
"""

import re
import subprocess
import sys

from watch_pr import REPO, gh, text

# CodeRabbit puts the severity on a line of bold runs joined by " · ", then the title alone in bold.
BOLD_LINE = re.compile(r"^\*\*(.+?)\*\*\s*$")
SEVERITY_LINE = re.compile(r"^\*\*\s*([^*]+?)\s*\*\*(?: · \*\*[^*]+\*\*)+$")
OUTSIDE = re.compile(r"Outside diff range comments \((\d+)\)")


def title(body: str | None) -> str:
    """Return the severity and the title of a CodeRabbit finding, else the start of the body."""
    severity = ""
    for line in (body or "").splitlines():
        shown = text(line)
        header = None if severity else SEVERITY_LINE.match(shown)
        if header:
            severity = header.group(1) + ": "
            continue
        bold = BOLD_LINE.match(shown)
        if bold:
            return severity + bold.group(1).strip()
    return text(body, 160)


def main() -> int:
    """Print one line per thread and per review with findings outside the diff."""
    if len(sys.argv) != 2:  # noqa: PLR2004  the program and the pull request
        sys.exit(__doc__)
    pr = sys.argv[1]
    me = gh("user")["login"]
    comments = gh(f"{REPO}/pulls/{pr}/comments?per_page=100", "")
    replied = {
        c["in_reply_to_id"]
        for c in comments
        if c.get("in_reply_to_id") and c["user"]["login"] == me
    }
    threads = [c for c in comments if not c.get("in_reply_to_id") and c["user"]["login"] != me]
    # A stable sort on "replied" puts the open threads first and keeps the order of each group.
    for c in sorted(threads, key=lambda c: c["id"] in replied):
        where = f"{c['path']}:{c.get('line') or c.get('original_line')}"
        state = "replied" if c["id"] in replied else "open"
        print(f"{state} {c['id']} {c['user']['login']} {where}: {title(c['body'])} {c['html_url']}")
    for review in gh(f"{REPO}/pulls/{pr}/reviews?per_page=100", ""):
        found = OUTSIDE.search(review["body"] or "")
        if found and review["user"]["login"] != me:
            print(
                f"outside {review['id']} {review['user']['login']}: "
                f"{found.group(1)} findings {review['html_url']}"
            )
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except subprocess.CalledProcessError as e:
        # gh prints why it failed (no network, bad credentials); a traceback would hide it.
        sys.exit(e.stderr.strip() or str(e))
