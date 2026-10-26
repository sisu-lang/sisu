"""Writes a reviewer's brief for a diff from origin/main's brief.md: fills every slot above "# Report",
keeps the sections for a diff and drops the spec and plan sections. Fails, writing nothing, when
a slot above "# Report" is left unfilled, so a template change cannot slip past it.

Usage: python3 diff_brief.py REPO BASE..HEAD OUT [--doc PATH]... [--focus FILE] [--verify-log LOG]
  REPO     the repository root that holds the diff (a worktree is fine)
  --doc    a spec or plan the reviewer reads with the diff; repeat for each
REPO and each --doc are written as absolute paths: the reviewer runs in REPO, and a doc under
the main checkout's gitignored docs/superpowers/ is not in a worktree.
  --focus  a file whose text goes under "# Focus"; without it, Focus is "none"
  --verify-log  the log of `make verify LOG=...` for the diff; Focus names it by its absolute
           path, so the reviewer checks its verdict line. Fails when the file does not exist.
The template comes from origin/main, so a branch that started before a change to the review
criteria still gets the current ones. Without origin/main it falls back to ../brief.md.
"""
import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

TEMPLATE = Path(__file__).resolve().parents[1] / 'brief.md'
MAIN_TEMPLATE = 'origin/main:.agents/skills/review-sisu/brief.md'


def plugin_skill(plugin, skill):
    """The SKILL.md of an installed Claude Code plugin, at its installPath, or None."""
    try:
        installed = json.loads((Path.home() / '.claude/plugins/installed_plugins.json').read_text())
        return Path(installed['plugins'][plugin][0]['installPath']) / 'skills' / skill / 'SKILL.md'
    except (OSError, ValueError, KeyError, IndexError):
        return None


SKILLS = {
    '<test-audit SKILL.md path>': Path.home() / '.claude/skills/test-audit/SKILL.md',
    '<codebase-design SKILL.md path>': Path.home() / '.agents/skills/codebase-design/SKILL.md',
    '<rust-best-practices SKILL.md path>': plugin_skill('beautiful-rust@beautiful-rust', 'rust-best-practices'),
}


def main():
    p = argparse.ArgumentParser()
    p.add_argument('repo')
    p.add_argument('range')
    p.add_argument('out')
    p.add_argument('--doc', action='append', default=[])
    p.add_argument('--focus')
    p.add_argument('--verify-log')
    a = p.parse_args()
    log = Path(a.verify_log).resolve() if a.verify_log else None
    if log and not log.is_file():
        sys.exit(f'diff_brief: no verify log at {log}')

    shown = subprocess.run(['git', '-C', str(TEMPLATE.parent), 'show', MAIN_TEMPLATE],
                           capture_output=True, text=True)
    if shown.returncode == 0:
        t = shown.stdout
    else:
        print(f'diff_brief: cannot read {MAIN_TEMPLATE}; using {TEMPLATE}', file=sys.stderr)
        t = TEMPLATE.read_text()
    t = t.replace('a <spec | plan | diff> of Sisu', 'a diff of Sisu')
    t = t.replace('Target: <path, or base..head>.', f'Target: {a.range}.')
    t = t.replace('<repository root>', str(Path(a.repo).resolve()))
    t = t.replace('<base>..<head>', a.range)
    docs = ''.join(f'- {Path(d).resolve()}\n' for d in a.doc)
    t = re.sub(r'- <the other documents to read.*?>\n', docs, t, flags=re.S)
    for slot, path in SKILLS.items():
        if path and path.exists():
            t = t.replace(slot, str(path))
        else:
            print(f'diff_brief: {path or slot} is missing; its line is dropped, so tell the user', file=sys.stderr)
            t = re.sub(rf'- `{re.escape(slot)}`.*?(?=\n- |\n\n)', '', t, flags=re.S)
    t = re.sub(r'# Check, for a spec\n.*?(?=# Check, for a diff)', '', t, flags=re.S)
    # Checked before Focus goes in, so text in the caller's focus cannot trip it.
    focus_slot = '<what the caller knows that the target does not say, or "none">'
    head = t.split('# Report', 1)[0].replace(focus_slot, '', 1)
    left = re.findall(r'<(?:spec|path|repository|base|the other|test-audit|codebase-design|rust-best-practices|what the caller)[^>]*>', head)
    if left or focus_slot not in t:
        sys.exit(f'diff_brief: unfilled slots: {left or [focus_slot + " missing"]}')
    focus = Path(a.focus).read_text().strip() if a.focus else 'none'
    if log:
        focus = f'Verify log: {log}\n\n{focus}' if a.focus else f'Verify log: {log}'
    Path(a.out).write_text(t.replace(focus_slot, focus))
    print(a.out)


if __name__ == '__main__':
    main()
