"""Checks that relative links and image sources in Markdown files resolve, anchors included, that
each repository path written as code in prose, such as `crates/sisuc/src/main.rs`, is tracked by
git or ignored on purpose, and that each Rust file named by its last parts, such as `lexer.rs` or
`check/mod.rs`, ends a tracked path.

Usage: python3 check_links.py FILE.md... Prints each broken link or path and exits 1 if any.
"""
import re
import subprocess
import sys
from pathlib import Path

INLINE = re.compile(r'\]\(<?([^)\s>]+)>?(?:\s+"[^"]*")?\)')
REF_DEF = re.compile(r'^\s*\[[^\]]+\]:\s+<?(\S+?)>?(?:\s+"[^"]*")?\s*$', re.M)
ATTR = re.compile(r'\b(src|href|srcset)="([^"]+)"')
FENCE = re.compile(r'`{3,}|~{3,}')
# A code span naming a path under one of the repository's top-level directories. Placeholders
# such as `<name>` and globs such as `*.md` do not match, so they are not checked.
REPO_PATH = re.compile(r'`((?:crates|docs|scripts|\.agents|\.cargo|\.config|\.github)/[\w./-]*)`')
# A code span naming a Rust file without its directory, such as `lexer.rs`. A module that
# becomes a directory leaves such names stale.
RUST_FILE = re.compile(r'`(?!(?:crates|docs|scripts|\.agents|\.cargo|\.config|\.github)/)([\w-]+(?:/[\w-]+)*\.rs)`')
ROOT = Path(__file__).resolve().parents[4]


def git(*args):
    return subprocess.run(['git', '-C', str(ROOT), *args], capture_output=True, text=True)


# What a clean checkout holds: each tracked file and each directory above one. Checking the
# working tree instead would pass on local files that CI's checkout lacks.
TRACKED = set()
for name in git('ls-files', '-z').stdout.split('\0'):
    parts = name.split('/')
    TRACKED.update('/'.join(parts[:i]) for i in range(1, len(parts) + 1))


def in_repo(path):
    """Tracked, or ignored on purpose, like the local-only `docs/superpowers/`."""
    return path.rstrip('/') in TRACKED or git('check-ignore', '-q', path).returncode == 0


def prose(path):
    """The file's text with fenced code blocks removed. A fence closes only on a line of its
    character, at least as long, with only spaces after it, so a ```` fence can show a ``` one."""
    kept, fence = [], None
    for line in path.read_text().split('\n'):
        run = FENCE.match(line)
        if fence is None and run:
            fence = run[0]
        elif fence is None:
            kept.append(line)
        elif run and run[0][0] == fence[0] and len(run[0]) >= len(fence) and not line[run.end():].strip(' \t'):
            fence = None
    return '\n'.join(kept)


def anchors(path):
    """GitHub's heading slugs: link text kept, punctuation dropped, duplicates numbered."""
    seen = {}
    for heading in re.findall(r'^#{1,6}\s+(.*?)\s*#*\s*$', prose(path), re.M):
        text = re.sub(r'!?\[([^\]]*)\]\([^)]*\)', r'\1', heading).replace('`', '')
        slug = re.sub(r'[^\w\- ]', '', text.lower()).replace(' ', '-')
        count = seen.get(slug, 0)
        seen[slug] = count + 1
        yield slug if count == 0 else f'{slug}-{count}'


def targets(text):
    for m in INLINE.finditer(text):
        yield m.group(1)
    for m in REF_DEF.finditer(text):
        yield m.group(1)
    for attr, value in ATTR.findall(text):
        if attr == 'srcset':
            yield from (part.split()[0] for part in value.split(',') if part.strip())
        else:
            yield value


def main():
    bad = 0
    for md in map(Path, sys.argv[1:]):
        for target in targets(prose(md)):
            if re.match(r'[a-z][a-z0-9+.-]*:', target):
                continue
            file, _, anchor = target.split('?')[0].partition('#')
            dest = (md.parent / file).resolve() if file else md.resolve()
            if not dest.exists():
                print(f'{md}: missing {target}')
                bad += 1
            elif anchor and dest.suffix == '.md' and anchor.lower() not in set(anchors(dest)):
                print(f'{md}: missing anchor {target}')
                bad += 1
        for path in REPO_PATH.findall(prose(md)):
            if not in_repo(path):
                print(f'{md}: missing path `{path}`')
                bad += 1
        for name in RUST_FILE.findall(prose(md)):
            if not any(path == name or path.endswith('/' + name) for path in TRACKED):
                print(f'{md}: no tracked file ends with `{name}`')
                bad += 1
    print('broken:', bad)
    sys.exit(1 if bad else 0)


if __name__ == '__main__':
    main()
