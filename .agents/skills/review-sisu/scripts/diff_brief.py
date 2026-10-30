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
           path, so the reviewer checks its summary line. Fails when the file does not exist, or
           when its summary line names a commit outside the range. A log measured
           at an earlier commit of the range is accepted, and Focus lists the commits after it,
           because a task runs mutation tests once and later commits are not mutated. A log
           measured before a rebase is accepted when a commit of the range has the same
           `git patch-id` as the measured commit; Focus names both.
  --coverage  the output of `make coverage` for the diff; Focus names it
For a diff, Focus also lists each line the diff removes from the tests with no identical line
added back, and each test file it deletes, and each sisu build in OUT's bin/<sha>/ that
build_sisu.sh wrote.
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


def summary(log):
    """The match of the last `mutation-test:` summary line of `log`, whose group 1 is the measured SHA,
    or None."""
    lines = [line for line in log.read_text().splitlines() if line.startswith('mutation-test: ')]
    return re.search(r'\(HEAD ([0-9a-f]{40})\)$', lines[-1]) if lines else None


def patch_id(repo, sha):
    """The stable `git patch-id` of the commit `sha`, or '' for a commit that changes nothing."""
    shown = subprocess.run(['git', '-C', str(repo), 'show', sha], capture_output=True).stdout
    return subprocess.run(['git', '-C', str(repo), 'patch-id', '--stable'], input=shown,
                          capture_output=True).stdout.decode().partition(' ')[0]


def in_range(repo, sha, base, head):
    """The commit of `base`..`head` that is `sha`, or that a rebase made of it (the same patch-id), or ''."""
    # In the range means reachable from head and not from base; base need not precede it after a merge of main.
    if sha == commit(repo, head) or (not is_ancestor(repo, sha, base) and is_ancestor(repo, sha, head)):
        return sha
    want = patch_id(repo, sha)
    return next((c for c in git(repo, 'rev-list', f'{base}..{head}').split() if want and patch_id(repo, c) == want), '')


def log_mismatch(log, repo, base, head):
    """Why the summary line of `log` does not describe a commit in `base`..`head` of `repo`, or None."""
    found = summary(log)
    if not found:
        return f'no summary line ending "(HEAD <sha>)" in {log}'
    if not in_range(repo, found[1], base, head):
        want = commit(repo, head)
        return f'{log} measured {found[1][:8]}, which is not in {base}..{head} ({want[:8] or "unknown"})'
    return None


# Test files whose removed lines Focus lists. Blessed output is left out: golden_diff.py checks a rewording.
TESTS = [':(glob)**/tests/**', ':(glob)**/tests.rs', ':(glob)scripts/test_*.py']
EXPECTED = [':(exclude,glob)**/*.err', ':(exclude,glob)**/*.json', ':(exclude,glob)**/*.out']
SHOWN = 40


def removed_tests(repo, base, head):
    """Each line that base..head removes from a test file with no identical line added back to any
    test file, as "path:line: text", then each deleted test file. Blank lines and comments are left out."""
    deleted = git(repo, 'diff', '--name-only', '--diff-filter=D', base, head, '--', *TESTS).splitlines()
    diff = git(repo, 'diff', '--unified=0', '--no-color', '--diff-filter=MR', base, head, '--', *TESTS, *EXPECTED)
    # A removed line is named by its base path and line, so a renamed file reads as the base has it.
    removed, added, old, new, number = [], set(), None, None, 0
    for line in diff.splitlines():
        if line.startswith('diff --git'):
            new = None
        elif new is None:
            if line.startswith('--- a/'):
                old = line[6:]
            elif line.startswith('+++ b/'):
                new = line[6:]
        elif line.startswith('@@'):
            number = int(re.match(r'@@ -(\d+)', line)[1])
        elif line.startswith('-'):
            removed.append((old, new, number, line[1:].strip()))
            number += 1
        elif line.startswith('+'):
            added.add(line[1:].strip())
    lines = [f'{o}:{n}: {text[:100]}' for o, p, n, text in removed
             if re.search(r'\w', text) and text not in added
             and not text.startswith('//') and not (p.endswith('.py') and text.startswith('#'))]
    return lines + [f'{p}: deleted' for p in deleted]


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
    for path in [a.mutation_log, a.coverage, *a.prior, a.rulings, a.target if a.kind != 'diff' else None]:
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
            measured = summary(log)[1]
            twin = in_range(repo, measured, *shas)
            later = git(repo, 'rev-list', '--reverse', f'{twin}..{shas[1]}').split()
            note = f', measured at {measured[:8]}' if later or twin != measured else ''
            if twin != measured:
                note += f', which a rebase made {twin[:8]}'
            if later:
                note += f'. Commits after it, not mutated: {" ".join(c[:8] for c in later)}'
            focus_lines.append(f'Mutation-test log: {log}{note}')
        if a.coverage:
            focus_lines.append(f'Coverage result: {Path(a.coverage).resolve()}')
        if removed := removed_tests(repo, *shas):
            more = [f'- and {len(removed) - SHOWN} more: see the diff'] if len(removed) > SHOWN else []
            focus_lines.append('Removed from the tests, with no identical line added back. For each, find its '
                               'keeper or the reason:\n' + '\n'.join([f'- {r}' for r in removed[:SHOWN]] + more))
        builds = sorted((out.parent / 'bin').glob('*/sisu'))
        if builds:
            focus_lines.append('Prebuilt sisu, one for each commit:\n' + '\n'.join(
                f'- {b.parent.name[:8]} {git(repo, "log", "-1", "--format=%s", b.parent.name)}: {b}' for b in builds))
    else:
        target = out.parent / f'target-{a.round}{Path(a.target).suffix}'

    shown = subprocess.run(['git', '-C', str(repo), 'show', MAIN_TEMPLATE], capture_output=True, text=True)
    if shown.returncode == 0:
        t = shown.stdout
    elif TEMPLATE.is_file():
        print(f'diff_brief: cannot read {MAIN_TEMPLATE}; using {TEMPLATE}', file=sys.stderr)
        t = TEMPLATE.read_text()
    else:
        sys.exit(f'diff_brief: cannot read {MAIN_TEMPLATE}; run git fetch origin')
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
        t = t.replace('<path of the first brief>', str(out.parent / 'brief.md'))
        previous = out.parent / f'target-{a.round - 1}{Path(a.target).suffix}'
        if a.kind == 'diff' or not previous.exists():
            t = drop_line(t, 'The previous version:')
        t = t.replace('<previous version>', str(previous))
    # Checked before Focus goes in, so text in the caller's focus cannot trip it.
    focus_slot = '<what the caller knows that the target does not say, or "none">'
    top = t.split('# Report', 1)[0].replace(focus_slot, '', 1)
    left = re.findall(r'<(?:spec|path|repository|base|the other|test-audit|codebase-design|rust-best-practices'
                      r'|round|prior|rulings|previous|what the caller)[^>]*>', top)
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
