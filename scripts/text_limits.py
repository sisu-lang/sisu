"""Reject a changed sentence of over 25 words in a comment, a docstring or Markdown.

A comment is a whole-line comment of Rust, Sisu, Python or shell.

Also reject a changed Python or shell comment or docstring line of over 100 columns.
`CODING_STANDARDS.md` (Writing) sets the sentence limit. A sentence that a change touches counts in
full, also over wrapped lines.

The check cannot tell an instruction, which has a limit of 20 words, from a description. It also
does not read diagnostic text in Rust strings or a comment after code. A review checks those.

Usage: python3 text_limits.py --diff REF FILE...
"""

import argparse
import ast
import io
import os
import re
import subprocess
import sys
import tokenize
from pathlib import Path

SENTENCE = 25
WIDTH = 100

CODE_SPAN = re.compile(r"`[^`]*`")
LINK = re.compile(r"\[([^\]]*)\]\([^)]*\)")
# Each of these starts a new block, because the line before it often has no full stop.
STARTS_BLOCK = re.compile(
    r"([-*+] |\d+[.)] |GIVEN\b|WHEN\b|THEN\b|AND\b|BUT\b|Examples\b|Scenario\b)"
)
FENCE = re.compile(r"\s*(```|~~~)")
# A semicolon or a colon also ends a count, so that a false alarm stays rare.
ENDS_SENTENCE = re.compile(r"[.!?;:][)\]\"'*_]*$")
# A heading, an HTML comment or a table row, in Markdown or in a comment.
NOT_PROSE = ("#", "<!--", "|")
HUNK = re.compile(r"@@ -\S+ \+(\d+)(?:,(\d+))? @@")


def outside_fences(numbered: list[tuple[int, str]]) -> list[tuple[int, str]]:
    """Return the lines that are not in a fenced code block."""
    kept = []
    fenced = False
    for n, text in numbered:
        if FENCE.match(text):
            fenced = not fenced
        elif not fenced:
            kept.append((n, text))
    return kept


def python_prose(lines: list[str]) -> list[tuple[int, str]]:
    """Return the whole-line comments and the docstrings of Python source.

    The result is empty when the source does not parse.
    """
    source = "".join(lines)
    found = []
    try:
        found.extend(
            (token.start[0], token.string.lstrip("#").strip())
            for token in tokenize.generate_tokens(io.StringIO(source).readline)
            if token.type == tokenize.COMMENT
            and token.line.lstrip().startswith("#")
            and not token.string.startswith("#!")
        )
        nodes = (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)
        for node in ast.walk(ast.parse(source)):
            if isinstance(node, nodes) and ast.get_docstring(node, clean=False) is not None:
                first = node.body[0]
                found.extend(
                    (n, lines[n - 1].strip().strip('"').strip("'").strip())
                    for n in range(first.lineno, (first.end_lineno or first.lineno) + 1)
                )
    except (SyntaxError, tokenize.TokenError):
        return []
    return sorted(found)


def prose(path: str, lines: list[str]) -> list[tuple[int, str]]:
    """Each line of prose in the file, as (line number, text)."""
    # shortcut: a line in a Rust string or a shell heredoc counts as a comment when it starts with
    # a comment mark. No tracked line does that. Read the strings if a false alarm comes.
    numbered = list(enumerate((line.rstrip("\n") for line in lines), 1))
    if path.endswith(".md"):
        body = numbered
        if lines and lines[0].startswith("---"):
            end = next((i for i, line in enumerate(lines[1:], 1) if line.startswith("---")), 0)
            body = numbered[end + 1 :]
        return outside_fences(body)
    if path.endswith(".py"):
        return outside_fences(python_prose(lines))
    if path.endswith((".rs", ".sisu")):
        comments = [
            (n, t.lstrip().lstrip("/!").strip()) for n, t in numbered if t.lstrip().startswith("//")
        ]
        return outside_fences(comments)
    if path.endswith(".sh"):
        return [
            (n, t.lstrip().lstrip("#").strip())
            for n, t in numbered
            if t.lstrip().startswith("#") and not t.startswith("#!")
        ]
    return []


def long_sentences(numbered: list[tuple[int, str]], changed: set[int]) -> list[tuple[int, int]]:
    """Return each sentence over the limit with a changed line, as (first line, words).

    A gap in the line numbers, an empty line, a line of `NOT_PROSE` or a line that `STARTS_BLOCK`
    matches ends the sentence.
    """
    found = []
    words = 0
    first = 0
    touched = False
    last = None

    def end() -> None:
        nonlocal words, touched
        if words > SENTENCE and touched:
            found.append((first, words))
        words = 0
        touched = False

    for n, text in numbered:
        if text.lstrip().startswith(NOT_PROSE):
            continue
        if last is None or n != last + 1 or not text.strip() or STARTS_BLOCK.match(text.lstrip()):
            end()
        last = n
        plain = LINK.sub(r"\1", CODE_SPAN.sub("code", text))
        for word in plain.split():
            if not any(c.isalnum() for c in word):
                continue
            if words == 0:
                first = n
            words += 1
            touched = touched or n in changed
            if ENDS_SENTENCE.search(word):
                end()
    end()
    return found


def findings(path: str, lines: list[str], changed: set[int]) -> list[str]:
    """One line for each limit that a changed line breaks."""
    numbered = prose(path, lines)
    out = [
        f"{path}:{n}: a sentence has {words} words; the limit is {SENTENCE}: split it"
        for n, words in long_sentences(numbered, changed)
    ]
    if path.endswith((".py", ".sh")):
        for n, _ in numbered:
            columns = len(lines[n - 1].rstrip("\n"))
            if n in changed and columns > WIDTH:
                out.append(f"{path}:{n}: a comment line has {columns} columns; wrap it at {WIDTH}")
    return out


def changed_lines(ref: str) -> dict[str, set[int]]:
    """Return the line numbers that differ from `ref`, for the absolute path of each file.

    A rename counts only the lines that it changes, so Git must see the whole diff and not one
    path.
    """
    top = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True
    ).stdout.rstrip("\n")
    # The flags hold the output in one form, whatever the user's Git settings and attributes are.
    # shortcut: Git still quotes a name with a tab or a quotation mark, so that file is not read.
    # No file here has such a name. Read `--name-status -z` if one comes.
    # shortcut: a clean filter that changes the line count shifts the numbers, and no flag stops it.
    # No filter applies here. Compare with the file on disk if one comes.
    # shortcut: `--text` keeps the whole diff of a binary file in memory, about 14 times its size.
    # The largest tracked file here is 430 KB. Limit the diff to file types if a large one comes.
    command = [
        "git",
        "-c",
        "core.quotePath=false",
        "diff",
        "-M",
        "--no-relative",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--text",
        "--unified=0",
        "--inter-hunk-context=0",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        ref,
    ]
    diff = subprocess.run(
        command, capture_output=True, text=True, encoding="utf-8", errors="replace", check=True
    ).stdout
    changed: dict[str, set[int]] = {}
    current: set[int] = set()
    in_header = False
    for line in diff.split("\n"):
        if line.startswith("diff --git "):
            current = set()
            in_header = True
        elif in_header and line.startswith("+++ b/"):
            # Git ends the name with a tab when the name holds a space.
            changed[str(Path(top) / line[6:].rstrip("\t"))] = current
        elif hunk := HUNK.match(line):
            in_header = False
            first = int(hunk[1])
            current.update(range(first, first + int(hunk[2] or 1)))
    return changed


def main(argv: list[str]) -> int:
    """Check each path in `argv`, and return 1 when a changed line breaks a limit."""
    parser = argparse.ArgumentParser()
    parser.add_argument("--diff", metavar="REF", required=True)
    parser.add_argument("paths", nargs="*")
    args = parser.parse_args(argv)
    # The diff of a merge holds what the other branch changed.
    merging = subprocess.run(
        ["git", "rev-parse", "-q", "--verify", "MERGE_HEAD"], capture_output=True, check=False
    )
    if merging.returncode == 0:
        return 0
    bad = 0
    changed = changed_lines(args.diff)
    for path in args.paths:
        ours = changed.get(os.path.realpath(path))
        if not ours:
            continue
        # A fixture such as `E0072.sisu` holds bytes that are not UTF-8 on purpose.
        with Path(path).open(encoding="utf-8", errors="replace") as f:
            lines = f.readlines()
        for line in findings(path, lines, ours):
            print(line)
            bad = 1
    return bad


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
