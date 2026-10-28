"""Writes a reviewer's brief from origin/main's brief.md: fills every slot above "# Report",
keeps the sections for the target's kind and drops the others. Fails, writing nothing, when
a slot above "# Report" is left unfilled, so a template change cannot slip past it.

Usage: python3 diff_brief.py REPO TARGET OUT [--kind spec|plan|diff] [--doc PATH]... [--focus FILE]
           [--mutation-log LOG] [--coverage FILE] [--round N --prior REPORT... --rulings FILE]
  REPO     the repository root that holds the target (a worktree is fine)
  TARGET   for a diff, BASE..HEAD, written into the brief as full SHAs so that a later commit or
           fetch cannot move it; for a spec or a plan, its path, copied to target-<round>.md next
           to OUT, because these documents are not committed and change between rounds
  --kind   diff by default
  --doc    a spec or plan the reviewer reads with the target; repeat for each
REPO and each --doc are written as absolute paths: the reviewer runs in REPO, and a doc under
the main checkout's gitignored docs/superpowers/ is not in a worktree.
  --focus  a file whose text goes under "# Focus"; without it, Focus is "none"
  --mutation-log  the log of `make mutation-test LOG=...` for the diff; Focus names it by its absolute
           path, so the reviewer checks its summary line. Fails when the file does not exist, when
           its summary line names uncommitted changes, or a commit outside the range. A log measured
           at an earlier commit of the range is accepted, and Focus lists the commits after it,
           because a task runs mutation tests once and later commits are not mutated.
  --coverage  the output of `make coverage` for the diff; Focus names it
  --round  the review round, 1 by default; a later round keeps the "# Re-review" section and
           needs --prior, the earlier reports, and --rulings, the caller's rulings on them
The template comes from origin/main, so a branch that started before a change to the review
criteria still gets the current ones. Without origin/main it falls back to ../brief.md.
"""
import argparse
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

TEMPLATE = Path(__file__).resolve().parents[1] / 'brief.md'
MAIN_TEMPLATE = 'origin/main:.agents/skills/review-sisu/brief.md'
KINDS = ('spec', 'plan', 'diff')


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


def git(repo, *args):
    """The stripped stdout of a git command in `repo`, or '' when it fails."""
    return subprocess.run(['git', '-C', str(repo), *args], capture_output=True, text=True).stdout.strip()


def commit(repo, ref):
    """The full SHA of `ref` in `repo`, or ''."""
    return git(repo, 'rev-parse', '--verify', '--quiet', f'{ref}^{{commit}}')


def is_ancestor(repo, a, b):
    return subprocess.run(['git', '-C', str(repo), 'merge-base', '--is-ancestor', a, b],
                          capture_output=True).returncode == 0


def log_mismatch(log, repo, base, head):
    """Why the summary line of `log` does not describe a commit in `base`..`head` of `repo`, or None."""
    lines = [line for line in log.read_text().splitlines() if line.startswith('mutation-test: ')]
    found = re.search(r'\(HEAD ([0-9a-f]{40})( with uncommitted changes)?\)$', lines[-1]) if lines else None
    if not found:
        return f'no summary line ending "(HEAD <sha>)" in {log}'
    if found[2]:
        return f'{log} measured uncommitted changes; commit them and run make mutation-test again'
    measured, want = found[1], commit(repo, head)
    if measured != want and (measured == commit(repo, base) or not is_ancestor(repo, base, measured)
                             or not is_ancestor(repo, measured, head)):
        return f'{log} measured {measured[:8]}, which is not in {base}..{head} ({want[:8] or "unknown"})'
    return None


def drop_line(t, start):
    """`t` without the "- " list item that begins with `start`, up to the next item or blank line."""
    return re.sub(rf'- {re.escape(start)}.*?(?=\n- |\n\n)\n?', '', t, flags=re.S)


def main():
    p = argparse.ArgumentParser()
    p.add_argument('repo')
    p.add_argument('target')
    p.add_argument('out')
    p.add_argument('--kind', choices=KINDS, default='diff')
    p.add_argument('--doc', action='append', default=[])
    p.add_argument('--focus')
    p.add_argument('--mutation-log')
    p.add_argument('--coverage')
    p.add_argument('--round', type=int, default=1)
    p.add_argument('--prior', action='append', default=[])
    p.add_argument('--rulings')
    a = p.parse_args()
    repo, out = Path(a.repo).resolve(), Path(a.out).resolve()
    if a.round > 1 and not (a.prior and a.rulings):
        sys.exit('diff_brief: a round after the first needs --prior and --rulings')
    if a.kind != 'diff' and (a.mutation_log or a.coverage):
        sys.exit('diff_brief: --mutation-log and --coverage are for a diff')
    for path in [a.mutation_log, a.coverage, *a.prior, a.rulings]:
        if path and not Path(path).is_file():
            sys.exit(f'diff_brief: no file at {Path(path).resolve()}')

    focus_lines = []
    if a.kind == 'diff':
        base, _, head = a.target.partition('..')
        shas = [commit(repo, base), commit(repo, head)]
        if not all(shas):
            sys.exit(f'diff_brief: {a.target} is not a range of two commits in {repo}')
        target = '..'.join(shas)
        if a.mutation_log:
            log = Path(a.mutation_log).resolve()
            if why := log_mismatch(log, repo, shas[0], shas[1]):
                sys.exit(f'diff_brief: {why}')
            measured = re.findall(r'\(HEAD ([0-9a-f]{40})\)', log.read_text())[-1]
            later = git(repo, 'rev-list', '--reverse', f'{measured}..{shas[1]}').split()
            focus_lines.append(f'Mutation-test log: {log}' + (
                f', measured at {measured[:8]}. Commits after it, not mutated: {" ".join(c[:8] for c in later)}'
                if later else ''))
        if a.coverage:
            focus_lines.append(f'Coverage result: {Path(a.coverage).resolve()}')
    else:
        target = out.parent / f'target-{a.round}{Path(a.target).suffix}'

    shown = subprocess.run(['git', '-C', str(repo), 'show', MAIN_TEMPLATE], capture_output=True, text=True)
    if shown.returncode == 0:
        t = shown.stdout
    else:
        print(f'diff_brief: cannot read {MAIN_TEMPLATE}; using {TEMPLATE}', file=sys.stderr)
        t = TEMPLATE.read_text()
    t = t.replace('a <spec | plan | diff> of Sisu', f'a {a.kind} of Sisu')
    t = t.replace('Target: <path, or base..head>.', f'Target: {target}.')
    t = t.replace('<repository root>', str(repo))
    t = t.replace('<base>..<head>', str(target))
    docs = ''.join(f'- {Path(d).resolve()}\n' for d in a.doc)
    t = re.sub(r'- <the other documents to read.*?>\n', docs, t, flags=re.S)
    if a.kind != 'diff':
        t = drop_line(t, 'For a diff,')
        t = drop_line(t, '`<rust-best-practices SKILL.md path>`')
    for slot, path in SKILLS.items():
        if slot not in t:
            continue
        if path and path.exists():
            t = t.replace(slot, str(path))
        else:
            print(f'diff_brief: {path or slot} is missing; its line is dropped, so tell the user', file=sys.stderr)
            t = drop_line(t, f'`{slot}`')
    for kind in KINDS:
        if kind != a.kind:
            t = re.sub(rf'# Check, for a {kind}\n.*?(?=\n# )\n', '', t, flags=re.S)
    if a.round == 1:
        t = re.sub(r'# Re-review\n.*?(?=\n# )\n', '', t, flags=re.S)
    else:
        t = t.replace('<round>', str(a.round))
        t = t.replace('<prior reports>', ', '.join(str(Path(r).resolve()) for r in a.prior))
        t = t.replace('<rulings>', str(Path(a.rulings).resolve()))
        t = t.replace('<first brief>', str(out.parent / 'brief.md'))
        previous = out.parent / f'target-{a.round - 1}{Path(a.target).suffix}'
        if a.kind == 'diff' or not previous.exists():
            t = drop_line(t, 'The previous version:')
        t = t.replace('<previous version>', str(previous))
    # Checked before Focus goes in, so text in the caller's focus cannot trip it.
    focus_slot = '<what the caller knows that the target does not say, or "none">'
    top = t.split('# Report', 1)[0].replace(focus_slot, '', 1)
    left = re.findall(r'<(?:spec|path|repository|base|the other|test-audit|codebase-design|rust-best-practices'
                      r'|round|prior|rulings|first brief|previous|what the caller)[^>]*>', top)
    if left or focus_slot not in t:
        sys.exit(f'diff_brief: unfilled slots: {left or [focus_slot + " missing"]}')
    focus = Path(a.focus).read_text().strip() if a.focus else ''
    focus = '\n\n'.join(filter(None, ['\n'.join(focus_lines), focus])) or 'none'
    if a.kind != 'diff':
        shutil.copyfile(a.target, target)
    out.write_text(t.replace(focus_slot, focus))
    print(out)


if __name__ == '__main__':
    main()
