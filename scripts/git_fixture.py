"""A temporary git repository for unit tests: python3 -m unittest discover -s scripts."""
import contextlib
import os
import subprocess
import tempfile
from unittest import mock


def env() -> dict:
    """The environment without `GIT_*` and `GITHUB_ACTIONS`. A pre-commit hook exports GIT_DIR and
    GIT_INDEX_FILE, which would point git at the hooked repository, and in CI GITHUB_ACTIONS adds
    a workflow warning line per slow test."""
    return {key: value for key, value in os.environ.items()
            if not key.startswith('GIT_') and key != 'GITHUB_ACTIONS'}


def git(path: str, *args: str) -> str:
    """The output of `git -C path ARGS`; fails when git fails."""
    return subprocess.run(['git', '-C', path, *args], check=True, capture_output=True,
                          text=True, errors='surrogateescape', env=env()).stdout


def write(path: str, name: str, content: str | bytes) -> None:
    """Writes `content` to the file `name` under `path`."""
    with open(os.path.join(path, name), 'wb') as file:
        file.write(content if isinstance(content, bytes) else content.encode('utf-8'))


@contextlib.contextmanager
def repo(files: dict[str, str]):
    """The path of a temporary repository with one commit of `files`. It is removed at exit.
    While it exists, `os.environ` has no `GIT_*` variable, so code under test that runs git
    without its own environment reaches this repository."""
    with tempfile.TemporaryDirectory() as tmp, mock.patch.dict(os.environ, env(), clear=True):
        git(tmp, 'init', '-q')
        for name, text in files.items():
            full = os.path.join(tmp, name)
            os.makedirs(os.path.dirname(full), exist_ok=True)
            with open(full, 'w', encoding='utf-8') as file:
                file.write(text)
        git(tmp, 'add', '.')
        git(tmp, '-c', 'user.email=t@t', '-c', 'user.name=t', '-c', 'commit.gpgsign=false',
            'commit', '-qm', 'a')
        yield tmp
