"""Unit tests for update-docs's check_links.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""

import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT = Path(__file__).resolve().parents[1] / ".agents/skills/update-docs/scripts/check_links.py"


def run_script(markdown: str, **env: str) -> subprocess.CompletedProcess[str]:
    """Run the script on a file that holds `markdown`, with `env` added to the environment."""
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp, "page.md")
        path.write_text(markdown, encoding="utf-8")
        return subprocess.run(
            [sys.executable, SCRIPT, path],
            capture_output=True,
            text=True,
            env={**os.environ, **env},
            check=False,
        )


class CheckLinks(unittest.TestCase):
    def test_a_path_that_git_does_not_know_should_be_reported(self) -> None:
        run = run_script("`docs/no-such-file.md`\n")
        self.assertEqual(run.returncode, 1)
        self.assertIn("missing path `docs/no-such-file.md`", run.stdout)

    def test_a_failed_ls_files_should_stop_with_its_error_and_report_no_path(self) -> None:
        run = run_script("`docs/roadmap.md`\n", GIT_DIR="/no-such-git-dir")
        self.assertEqual(run.returncode, 1)
        self.assertIn("check_links: git ls-files -z: fatal:", run.stderr)
        self.assertNotIn("missing path", run.stdout)

    def test_a_failed_check_ignore_should_stop_with_its_error(self) -> None:
        spec = importlib.util.spec_from_file_location("check_links", SCRIPT)
        # pyrefly: ignore[bad-argument-type]  the script is in the repository, so the spec is found
        check_links = importlib.util.module_from_spec(spec)
        # pyrefly: ignore[missing-attribute]  as above, and a file spec has a loader
        spec.loader.exec_module(check_links)
        failed = subprocess.CompletedProcess([], 128, "", "fatal: broken\n")
        with (
            mock.patch.object(subprocess, "run", return_value=failed),
            self.assertRaises(SystemExit) as stop,
        ):
            check_links.in_repo("docs/no-such-file.md")
        self.assertIn(
            "git check-ignore -q docs/no-such-file.md: fatal: broken", str(stop.exception)
        )


if __name__ == "__main__":
    unittest.main()
