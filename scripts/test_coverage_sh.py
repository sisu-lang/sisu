"""Unit tests for coverage.sh: python3 -m unittest discover -s scripts -p 'test_*.py'.

Each test runs the script in a small repository with a fake `cargo` on PATH and a stub
coverage_report.py, so no instrumented run happens.
"""

import os
import shutil
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path
from typing import override

SCRIPT = Path(__file__).absolute().parent / "coverage.sh"
# A pre-commit hook runs these tests with GIT_DIR and its kin set to the outer repository.
# The tests set NEXTEST_PROFILE themselves, and RUSTC must come only from the script.
ENV = {
    k: v
    for k, v in os.environ.items()
    if not k.startswith("GIT_") and k not in {"NEXTEST_PROFILE", "RUSTC"}
}
# The fake cargo does nothing, except that `nextest` writes its NEXTEST_PROFILE to PROFILE_LOG
# and runs the action that the test names in FAKE_ACTION: it edits the tracked file or it makes
# a commit.
FAKE_CARGO = """#!/usr/bin/env bash
printf '%s RUSTC=%s\\n' "$*" "${RUSTC-unset}" >>"$CARGO_LOG"
[[ $2 == nextest ]] || exit 0
printf '%s' "${NEXTEST_PROFILE-unset}" >"$PROFILE_LOG"
case ${FAKE_ACTION:-} in
  edit) echo changed >a.txt ;;
  commit) echo changed >a.txt && git -c user.name=t -c user.email=t@t commit -q -am during ;;
esac
"""
# The fake rustc of the pinned toolchain: its target-libdir is next to a bin directory.
# The tests remove bin/llvm-cov to model a toolchain without the llvm-tools-preview component.
FAKE_RUSTC = """#!/usr/bin/env bash
[[ $1 == --print && $2 == target-libdir ]] && echo "${0%/bin/rustc}/lib"
"""
# `rustup which --toolchain <name> rustc` prints $FAKE_RUSTC when FAKE_NIGHTLY is not `missing`.
# A real rustup installs a missing toolchain unless RUSTUP_AUTO_INSTALL is 0; the fake fails then.
FAKE_RUSTUP = """#!/usr/bin/env bash
[[ ${FAKE_NIGHTLY:-installed} == missing ]] && exit 1
[[ ${RUSTUP_AUTO_INSTALL-} == 0 ]] || { echo "fake rustup: would install" >&2; exit 1; }
echo "$FAKE_RUSTC_PATH"
"""
STUB_REPORT = "print('Coverage: stub')\n"


class CoverageSh(unittest.TestCase):
    @override
    def setUp(self) -> None:
        self.repo = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.repo)
        scripts = Path(self.repo, "scripts")
        scripts.mkdir()
        shutil.copy(SCRIPT, scripts)
        with (scripts / "coverage_report.py").open("w", encoding="utf-8") as f:
            f.write(STUB_REPORT)
        bin_dir = Path(self.repo, "bin")
        bin_dir.mkdir()
        cargo = bin_dir / "cargo"
        with cargo.open("w", encoding="utf-8") as f:
            f.write(FAKE_CARGO)
        cargo.chmod(cargo.stat().st_mode | stat.S_IXUSR)
        rustup = bin_dir / "rustup"
        with rustup.open("w", encoding="utf-8") as f:
            f.write(FAKE_RUSTUP)
        rustup.chmod(rustup.stat().st_mode | stat.S_IXUSR)
        shutil.copy(SCRIPT.with_name("coverage-toolchain"), scripts)
        self.cargo_log = bin_dir / "cargo.log"
        toolchain = Path(self.repo, "toolchain")
        (toolchain / "bin").mkdir(parents=True)
        (toolchain / "lib").mkdir()
        self.rustc = toolchain / "bin" / "rustc"
        self.rustc.write_text(FAKE_RUSTC, encoding="utf-8")
        self.rustc.chmod(self.rustc.stat().st_mode | stat.S_IXUSR)
        self.llvm_cov = toolchain / "bin" / "llvm-cov"
        self.llvm_cov.write_text("", encoding="utf-8")
        # Untracked, so the write does not change the marker.
        self.profile_log = bin_dir / "profile"
        self.env = dict(
            ENV,
            PATH=f"{bin_dir}:{ENV['PATH']}",
            PROFILE_LOG=str(self.profile_log),
            CARGO_LOG=str(self.cargo_log),
            FAKE_RUSTC_PATH=str(self.rustc),
        )
        self.git = ["git", "-C", self.repo, "-c", "user.name=t", "-c", "user.email=t@t"]
        subprocess.run([*self.git[:3], "init", "-q"], check=True, env=ENV)
        self.write_tracked("base\n")
        subprocess.run([*self.git, "add", "-A"], check=True, env=ENV)
        subprocess.run([*self.git, "commit", "-q", "-m", "base"], check=True, env=ENV)
        self.head = self.rev_parse()

    def write_tracked(self, text: str) -> None:
        with Path(self.repo, "a.txt").open("w", encoding="utf-8") as f:
            f.write(text)

    def rev_parse(self) -> str:
        return subprocess.run(
            [*self.git, "rev-parse", "HEAD"], capture_output=True, text=True, check=True, env=ENV
        ).stdout.strip()

    def measure(self, action: str = "", extra: dict[str, str] | None = None) -> str:
        return self.run_script(action, extra).stdout.splitlines()[0]

    def run_script(
        self, action: str = "", extra: dict[str, str] | None = None
    ) -> subprocess.CompletedProcess[str]:
        run = subprocess.run(
            ["bash", "scripts/coverage.sh", "HEAD"],
            cwd=self.repo,
            capture_output=True,
            text=True,
            env=dict(self.env, FAKE_ACTION=action, **(extra or {})),
            check=False,
        )
        self.assertEqual(run.returncode, 0, run.stderr)
        # The marker line comes before the report, which CI copies from `Coverage:` on.
        self.assertEqual(run.stdout.splitlines()[1:], ["Coverage: stub"])
        return run

    def test_a_clean_tree_should_give_the_clean_marker(self) -> None:
        self.assertEqual(self.measure(), f"coverage: measured (HEAD {self.head})")

    def test_a_change_before_the_run_should_give_the_dirty_marker(self) -> None:
        self.write_tracked("changed\n")
        self.assertEqual(
            self.measure(), f"coverage: measured (HEAD {self.head} with uncommitted changes)"
        )

    def test_a_change_during_the_run_should_give_the_dirty_marker(self) -> None:
        self.assertEqual(
            self.measure("edit"), f"coverage: measured (HEAD {self.head} with uncommitted changes)"
        )

    def test_a_commit_during_the_run_should_give_the_dirty_marker_with_the_first_head(self) -> None:
        marker = self.measure("commit")
        self.assertNotEqual(self.rev_parse(), self.head)
        self.assertEqual(marker, f"coverage: measured (HEAD {self.head} with uncommitted changes)")

    def test_an_unset_profile_should_run_the_ci_profile(self) -> None:
        self.measure()
        self.assertEqual(self.profile_log.read_text(encoding="utf-8"), "ci")

    def test_a_set_profile_should_run_that_profile(self) -> None:
        self.measure(extra={"NEXTEST_PROFILE": "default"})
        self.assertEqual(self.profile_log.read_text(encoding="utf-8"), "default")

    def test_an_installed_pinned_nightly_should_measure_branches_with_its_rustc(self) -> None:
        self.run_script()
        calls = self.cargo_log.read_text(encoding="utf-8").splitlines()
        for call in calls:
            self.assertTrue(call.endswith(f"RUSTC={self.rustc}"), call)
        measured = [c for c in calls if c.split()[1] in {"nextest", "report"}]
        self.assertEqual(len(measured), 3)  # one nextest run, two reports
        for call in measured:
            self.assertIn(" --branch", call)

    def assert_lines_only_with_warning(self, run: subprocess.CompletedProcess[str]) -> None:
        pin = Path(self.repo, "scripts", "coverage-toolchain").read_text(encoding="utf-8").strip()
        self.assertEqual(
            run.stderr.strip(),
            f"coverage: no branch coverage: run `rustup toolchain install {pin} "
            "--profile minimal --component llvm-tools-preview` to measure it",
        )
        for call in self.cargo_log.read_text(encoding="utf-8").splitlines():
            self.assertTrue(call.endswith("RUSTC=unset"), call)
            self.assertNotIn("--branch", call)

    def test_a_missing_pinned_nightly_should_warn_and_measure_lines_only(self) -> None:
        self.assert_lines_only_with_warning(self.run_script(extra={"FAKE_NIGHTLY": "missing"}))

    def test_a_pinned_nightly_without_llvm_tools_should_warn_and_measure_lines_only(self) -> None:
        self.llvm_cov.unlink()
        self.assert_lines_only_with_warning(self.run_script())


if __name__ == "__main__":
    unittest.main()
