"""Rejects a changed sentence of over 25 words in a comment, a docstring or Markdown. Also rejects a
changed Python or shell comment or docstring line of over 100 columns. `CODING_STANDARDS.md`
(Writing) sets the sentence limit. A sentence that a change touches counts in full, also over
wrapped lines.

The check cannot tell an instruction, which has a limit of 20 words, from a description. It also
does not read diagnostic text in Rust strings. A review checks those.

Usage: python3 text_limits.py --diff REF FILE...
"""

import argparse
import ast
import io
import re
import subprocess
import sys
import tokenize

SENTENCE = 25
WIDTH = 100

CODE_SPAN = re.compile(r"`[^`]*`")
LINK = re.compile(r"\[([^\]]*)\]\([^)]*\)")
# Each of these starts a new block, because the line before it often has no full stop.
STARTS_BLOCK = re.compile(r"([-*+] |\d+[.)] |GIVEN\b|WHEN\b|THEN\b|AND\b|BUT\b|Examples\b|Scenario\b)")
FENCE = re.compile(r"\s*(```|~~~)")
# A semicolon or a colon also ends a count, so that a false alarm stays rare.
ENDS_SENTENCE = re.compile(r"[.!?;:][)\]\"'*_]*$")


def outside_fences(numbered: list[tuple[int, str]]) -> list[tuple[int, str]]:
    """The lines that are not in a fenced code block."""
    kept = []
    fenced = False
    for n, text in numbered:
        if FENCE.match(text):
            fenced = not fenced
        elif not fenced:
            kept.append((n, text))
    return kept


def python_prose(lines: list[str]) -> list[tuple[int, str]]:
    """The whole-line comments and the docstrings of Python source. Nothing if it does not parse."""
    source = "".join(lines)
    found = []
    try:
        for token in tokenize.generate_tokens(io.StringIO(source).readline):
            if token.type == tokenize.COMMENT and token.line.lstrip().startswith("#") and not token.string.startswith("#!"):
                found.append((token.start[0], token.string.lstrip("#").strip()))
        nodes = (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)
        for node in ast.walk(ast.parse(source)):
            if isinstance(node, nodes) and ast.get_docstring(node, clean=False) is not None:
                first = node.body[0]
                for n in range(first.lineno, first.end_lineno + 1):
                    found.append((n, lines[n - 1].strip().strip('"').strip("'").strip()))
    except (SyntaxError, tokenize.TokenError):
        return []
    return sorted(found)


def prose(path: str, lines: list[str]) -> list[tuple[int, str]]:
    """Each line of prose in the file, as (line number, text)."""
    numbered = list(enumerate((line.rstrip("\n") for line in lines), 1))
    if path.endswith(".md"):
        body = numbered
        if lines and lines[0].startswith("---"):
            end = next((i for i, line in enumerate(lines[1:], 1) if line.startswith("---")), 0)
            body = numbered[end + 1:]
        return outside_fences([(n, t) for n, t in body if not t.lstrip().startswith(("#", "<!--", "|"))])
    if path.endswith(".py"):
        return outside_fences(python_prose(lines))
    if path.endswith(".rs"):
        comments = [(n, t.lstrip().lstrip("/!").strip()) for n, t in numbered if t.lstrip().startswith("//")]
        return outside_fences(comments)
    if path.endswith(".sh"):
        return [(n, t.lstrip().lstrip("#").strip()) for n, t in numbered
                if t.lstrip().startswith("#") and not t.startswith("#!")]
    return []


def long_sentences(numbered: list[tuple[int, str]], changed: set[int]) -> list[tuple[int, int]]:
    """Each sentence over the limit with a changed line, as (first line, words). A gap in the line
    numbers, an empty line or a line that `STARTS_BLOCK` matches ends the sentence."""
    found = []
    words = 0
    first = 0
    touched = False
    last = None

    def end():
        nonlocal words, touched
        if words > SENTENCE and touched:
            found.append((first, words))
        words = 0
        touched = False

    for n, text in numbered:
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
    out = [f"{path}:{n}: a sentence has {words} words; the limit is {SENTENCE}: split it"
           for n, words in long_sentences(numbered, changed)]
    if path.endswith((".py", ".sh")):
        for n, _ in numbered:
            if n in changed and len(lines[n - 1].rstrip("\n")) > WIDTH:
                out.append(f"{path}:{n}: a comment line has {len(lines[n - 1].rstrip())} columns; wrap it at {WIDTH}")
    return out


def changed_lines(ref: str, path: str) -> set[int]:
    """The line numbers of `path` that differ from `ref`."""
    diff = subprocess.run(["git", "diff", "--unified=0", ref, "--", path],
                          capture_output=True, text=True, check=True).stdout
    changed = set()
    for start, count in re.findall(r"^@@ -\S+ \+(\d+)(?:,(\d+))? @@", diff, flags=re.M):
        first = int(start)
        changed.update(range(first, first + int(count or 1)))
    return changed


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--diff", metavar="REF", required=True)
    parser.add_argument("paths", nargs="*")
    args = parser.parse_args(argv)
    # The diff of a merge holds what the other branch changed.
    merging = subprocess.run(["git", "rev-parse", "-q", "--verify", "MERGE_HEAD"], capture_output=True, check=False)
    if merging.returncode == 0:
        return 0
    bad = 0
    for path in args.paths:
        changed = changed_lines(args.diff, path)
        if not changed:
            continue
        with open(path, encoding="utf-8") as f:
            lines = f.readlines()
        for line in findings(path, lines, changed):
            print(line)
            bad = 1
    return bad


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
