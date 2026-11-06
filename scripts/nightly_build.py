"""Steps of the nightly build workflow that need the GitHub API. Usage:

  nightly_build.py pick

`pick` chooses the commit of the nightly build. It reads GITHUB_REPOSITORY, GITHUB_REF, HEAD_SHA
and GH_TOKEN, and writes `sha=`, `build=` and `publish=` to $GITHUB_OUTPUT and its answer to
$GITHUB_STEP_SUMMARY. On `main` it takes the newest commit that has a successful `ci.yml` push
run and is newer than the commit of the newest published `nightly-*` release. When the commit has
other `.github/workflows/` files than `main`, it takes nothing, because GITHUB_TOKEN cannot create
a release for such a commit. Any other ref is a dry run: it builds HEAD_SHA, publishes nothing and
prints what `main` would do.

`pick` exits 0 on every answer, and 1 when a `gh` call fails. It only reads. It calls `gh api`
(REST), so it needs the network. Python 3.10 standard library only.
"""
import argparse
import json
import os
import subprocess
import sys

NO_NEW = 'no new green commit'
DIFFER = 'the newest green commit has other workflow files than `main`'


def gh(*args: str) -> object:
    out = subprocess.run(['gh', 'api', *args], capture_output=True, text=True, check=True).stdout
    return json.loads(out)


def releases(repo: str) -> list[dict]:
    # `prune` needs every release, and gh gives one array of all pages when its output is a pipe.
    return gh('--paginate', f'repos/{repo}/releases?per_page=100')


def nightly_releases(repo: str) -> list[dict]:
    """The published `nightly-*` releases, the largest `published_at` first. A draft has none."""
    found = [r for r in releases(repo)
             if not r['draft'] and r['published_at'] and r['tag_name'].startswith('nightly-')]
    return sorted(found, key=lambda r: r['published_at'], reverse=True)


def newest_green(commits: list[str], green: set[str], released: str | None) -> str | None:
    """The first commit, newest first, that is in `green`, or None at the commit of `released`."""
    for commit in commits:
        if released and commit.startswith(released):
            return None
        if commit in green:
            return commit
    return None


def workflows_tree(repo: str, ref: str) -> str:
    entries = gh(f'repos/{repo}/contents/.github?ref={ref}')
    return next(e['sha'] for e in entries if e['name'] == 'workflows')


def answer_of_main(repo: str) -> tuple[str, str | None]:
    """The text of the answer, and the commit to build or None."""
    # shortcut: reads one page of 100, more than 100 commits or CI successes between two nightly
    # builds needs paging
    commits = [c['sha'] for c in gh(f'repos/{repo}/commits?sha=main&per_page=100')]
    # shortcut: reads one page of 100, more than 100 commits or CI successes between two nightly
    # builds needs paging
    runs = gh(f'repos/{repo}/actions/workflows/ci.yml/runs?branch=main&event=push'
              '&status=success&per_page=100')
    green = {r['head_sha'] for r in runs['workflow_runs']}
    newest = nightly_releases(repo)
    # A release's tag is the only store of its commit: `nightly-<date>-<short sha>`.
    released = newest[0]['tag_name'].rsplit('-', 1)[1] if newest else None
    sha = newest_green(commits, green, released)
    if sha is None:
        return NO_NEW, None
    if workflows_tree(repo, sha) != workflows_tree(repo, 'main'):
        return DIFFER, None
    return f'build and publish {sha}', sha


def append(path: str, text: str) -> None:
    with open(path, 'a', encoding='utf-8') as f:
        f.write(text)


def pick() -> int:
    repo = os.environ['GITHUB_REPOSITORY']
    text, sha = answer_of_main(repo)
    if os.environ['GITHUB_REF'] == 'refs/heads/main':
        outputs = (sha or '', 'true' if sha else 'false', 'true' if sha else 'false')
    else:
        text = f'main would: {text}'
        outputs = (os.environ['HEAD_SHA'], 'true', 'false')
    print(text)
    append(os.environ['GITHUB_STEP_SUMMARY'], text + '\n')
    append(os.environ['GITHUB_OUTPUT'], 'sha={}\nbuild={}\npublish={}\n'.format(*outputs))
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawTextHelpFormatter)
    commands = parser.add_subparsers(dest='command', required=True)
    commands.add_parser('pick', help='choose the commit of the nightly build')
    args = parser.parse_args(argv)
    try:
        return {'pick': pick}[args.command]()
    except subprocess.CalledProcessError as e:
        print(f'{" ".join(e.cmd[:3])}: {e.stderr.strip() or e}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
