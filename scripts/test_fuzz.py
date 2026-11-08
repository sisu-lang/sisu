"""Unit tests for the summary and the gap check of fuzz.sh.

Run them with: python3 -m unittest discover -s scripts -p 'test_*.py'

Each test runs the script in a small repository with a fake `cargo` on PATH, so no fuzzer runs.
"""

import json
import os
import re
import shutil
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path
from typing import Any, override

from test_fuzz_gaps import ANCHORS, export, record

SCRIPT = Path(__file__).absolute().parent / "fuzz.sh"
# A pre-commit hook runs these tests with GIT_DIR and its kin set to the outer repository.
ENV = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
# The fake cargo fuzz run (the one with --jobs) writes a crash input and fails, as libFuzzer does.
# A replay of that input prints the bytes of $FAKE_REPLAY, which a test sets with printf escapes.
# Before the crash, the run prints the bytes of $FAKE_JOBS: its job lines.
FAKE_CARGO = """#!/usr/bin/env bash
[[ $1 == fuzz ]] || exit 1
case $2 in
build) exit 0 ;;
--version) echo "cargo-fuzz 0.0.0"; exit 0 ;;
esac
if [[ " $* " == *" --jobs "* ]]; then
  printf "${FAKE_JOBS:-}"
  sleep 0.05
  mkdir -p fuzz/artifacts/front_end
  echo x >fuzz/artifacts/front_end/crash-abc
  exit 77
fi
printf "$FAKE_REPLAY"
exit 77
"""

# The fake cargo of the program target. `cargo fuzz run` writes one line to $FAKE_SEEN for each
# directory argument with a slash (the subcommand `fuzz` names a directory too). The line holds
# the name of the directory, its file count and how many of its files hold 4096 bytes. It is
# written while the directory exists, since fuzz.sh deletes its seeds on exit.
PROGRAM_CARGO = """#!/usr/bin/env bash
if [[ $1 == fuzz && $2 == run ]]; then
  for arg in "$@"; do
    if [[ $arg == */* && -d $arg ]]; then
      echo "${arg##*/} $(find "$arg" -type f | wc -l) $(find "$arg" -type f -size 4096c | wc -l)" >>"$FAKE_SEEN"
    fi
  done
  exit 0
fi
[[ $1 == fuzz && $2 == --version ]] && echo "cargo-fuzz 0.0.0"
exit 0
"""


class FuzzSummary(unittest.TestCase):
    @override
    def setUp(self) -> None:
        self.repo = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.repo)
        (self.repo / "scripts").mkdir()
        shutil.copy(SCRIPT, self.repo / "scripts")
        (self.repo / "crates" / "sisuc" / "tests").mkdir(parents=True)
        (self.repo / "fuzz").mkdir(parents=True)
        self.bin = self.repo / "bin"
        self.bin.mkdir()
        cargo = self.bin / "cargo"
        with cargo.open("w", encoding="utf-8") as f:
            f.write(FAKE_CARGO)
        cargo.chmod(cargo.stat().st_mode | stat.S_IXUSR)
        git = ["git", "-C", self.repo, "-c", "user.name=t", "-c", "user.email=t@t"]
        subprocess.run([*git, "init", "-q"], check=True, env=ENV)
        subprocess.run([*git, "commit", "-q", "--allow-empty", "-m", "base"], check=True, env=ENV)

    def summary(self, replay: str, jobs: str = "", seconds: str = "1") -> str:
        env = dict(ENV, PATH=f"{self.bin}:{ENV['PATH']}", FAKE_REPLAY=replay, FAKE_JOBS=jobs)
        run = subprocess.run(
            ["bash", "scripts/fuzz.sh", seconds],
            cwd=self.repo,
            capture_output=True,
            env=env,
            timeout=60,
            check=False,
        )
        self.assertEqual(run.returncode, 1, run.stderr)
        with (self.repo / "fuzz" / "artifacts" / "summary.md").open(encoding="utf-8") as f:
            return f.read()

    def test_a_crash_with_a_panic_message_should_show_its_location_and_first_message_line(
        self,
    ) -> None:
        # The replay output holds binary bytes, and the panic line comes after some of them.
        text = self.summary(
            r"\377\000\001\200 noise\n\000thread '<unnamed>' (7) panicked at "
            r"crates/sisuc/src/fuzz.rs:70:9:\nBraceOnNewLine: a safe fix removes NoMember too:\n"
            r"second line\n"
        )
        self.assertIn(
            "  Panic at `crates/sisuc/src/fuzz.rs:70:9`: `BraceOnNewLine: a safe fix removes NoMember too:`\n",
            text,
        )
        self.assertNotIn("second line", text)
        self.assertIn("Its replay is at the end of `fuzz.log`.", text)
        self.assertNotIn("stack overflow", text)

    def test_a_panic_message_should_lose_its_backticks_and_control_characters(self) -> None:
        text = self.summary(r"panicked at a.rs:1:2:\nbad `code` \001here\n")
        self.assertIn("  Panic at `a.rs:1:2`: `bad code here`\n", text)

    def test_binary_bytes_before_the_panic_on_the_same_line_should_not_reach_the_location(
        self,
    ) -> None:
        text = self.summary(r"garbage \377\376 panicked at a.rs:1:2:\nhello\n")
        self.assertIn("  Panic at `a.rs:1:2`: `hello`\n", text)

    def test_a_long_message_should_be_cut_in_whole_characters(self) -> None:
        text = self.summary(r"panicked at a.rs:1:2:\na" + r"\303\251" * 200 + r"\n")
        line = next(line for line in text.splitlines() if "Panic at" in line)
        self.assertTrue(line.startswith("  Panic at `a.rs:1:2`: `a"))
        self.assertLessEqual(len(line.encode()), 400)

    def test_a_mention_in_a_message_should_stay_in_a_code_span(self) -> None:
        text = self.summary(r"panicked at a.rs:1:2:\nhello @octocat #12\n")
        self.assertIn("`hello @octocat #12`", text)

    def test_a_panic_without_a_message_line_should_show_the_location_alone(self) -> None:
        text = self.summary(r"panicked at a.rs:1:2:")
        self.assertIn("  Panic at `a.rs:1:2`\n", text)

    def test_a_crash_without_a_panic_message_should_keep_the_note_about_a_fatal_signal(
        self,
    ) -> None:
        text = self.summary(r"\377\000 AddressSanitizer: stack-overflow\n")
        self.assertNotIn("Panic at", text)
        self.assertIn("is a stack overflow or another fatal signal", text)

    @staticmethod
    def job(executions: int, seconds: int) -> str:
        return (
            f"#{executions}: cov: 9 ft: 9 corp: 9 exec/s: 1 oom/timeout/crash: 0/0/0 "
            rf"time: {seconds}s job: 1 dft_time: 0\n"
        )

    def test_a_short_first_pass_should_be_reported_with_no_warning(self) -> None:
        # A child's line starts `#<n>\t` and has no `time:`, so it does not count as a job.
        jobs = rf"#5\tNEW cov: 3\n{self.job(2, 40)}{self.job(9, 300)}"
        text = self.summary("x", jobs, "300")
        self.assertIn("- First pass over the corpus: about 40 s.\n", text)
        self.assertNotIn("new inputs", text)

    def test_a_first_pass_over_half_the_run_should_warn_about_new_inputs(self) -> None:
        text = self.summary("x", self.job(155, 151), "300")
        self.assertIn("- First pass over the corpus: about 151 s.\n", text)
        self.assertIn("so this run tried few new inputs", text)

    def test_a_first_pass_of_exactly_half_the_run_should_not_warn(self) -> None:
        self.assertNotIn("new inputs", self.summary("x", self.job(155, 150), "300"))

    def test_a_run_with_no_finished_job_should_say_the_new_inputs_are_not_known(self) -> None:
        text = self.summary("x", "", "300")
        self.assertIn("- No job finished, so the number of new inputs is not known.\n", text)
        self.assertNotIn("First pass", text)


class ProgramSeeds(unittest.TestCase):
    def test_program_run_should_get_the_seeds(self) -> None:
        with tempfile.TemporaryDirectory() as repo:
            scripts = Path(repo, "scripts")
            scripts.mkdir()
            for name in ("fuzz.sh", "fuzz_seeds.py"):
                shutil.copy(SCRIPT.parent / name, scripts)
            Path(repo, "fuzz").mkdir(parents=True)
            bin_dir = Path(repo, "bin")
            bin_dir.mkdir()
            cargo = bin_dir / "cargo"
            with cargo.open("w", encoding="utf-8") as f:
                f.write(PROGRAM_CARGO)
            cargo.chmod(cargo.stat().st_mode | stat.S_IXUSR)
            git = ["git", "-C", repo, "-c", "user.name=t", "-c", "user.email=t@t"]
            subprocess.run([*git, "init", "-q"], check=True, env=ENV)
            subprocess.run(
                [*git, "commit", "-q", "--allow-empty", "-m", "base"], check=True, env=ENV
            )
            seen = Path(repo, "seen.txt")
            env = dict(ENV, PATH=f"{bin_dir}:{ENV['PATH']}", FAKE_SEEN=str(seen))
            run = subprocess.run(
                ["bash", "scripts/fuzz.sh", "--target", "program", "1"],
                cwd=repo,
                capture_output=True,
                env=env,
                timeout=60,
                check=False,
            )
            self.assertEqual(run.returncode, 0, run.stderr)
            with seen.open(encoding="utf-8") as f:
                lines = f.read().splitlines()
            # The corpus is empty. The seeds directory comes second.
            self.assertEqual(lines[0], "program 0 0")
            self.assertEqual(len(lines), 2, lines)
            self.assertTrue(lines[1].endswith(" 128 128"), lines[1])


# The fake cargo of the known-cascades warning. Its `fuzz run` appends the lines of $FAKE_CASCADES
# to the file $SISU_FUZZ_CASCADES, as the single-mistake check does, and exits with $FAKE_STATUS.
# Its `fuzz build` fails when $FAKE_BUILD_FAILS is set. The program target runs before it, so a run
# that did not fuzz gives no line.
CASCADES_CARGO = """#!/usr/bin/env bash
if [[ $1 == fuzz && $2 == run ]]; then
  if [[ -n ${SISU_FUZZ_CASCADES:-} ]]; then printf "${FAKE_CASCADES:-}" >>"$SISU_FUZZ_CASCADES"; fi
  exit "${FAKE_STATUS:-0}"
fi
[[ $1 == fuzz && $2 == build && -n ${FAKE_BUILD_FAILS:-} ]] && exit 1
[[ $1 == fuzz && $2 == --version ]] && echo "cargo-fuzz 0.0.0"
exit 0
"""
BASELINE = "# a comment\nlocal E0050\n\ncallee E0099\n"
WARNING = """## Known cascades

Warning: no input of this run gave these pairs of `fuzz/known_cascades.txt`. \
A pair that no nightly run gives can go:

- `callee E0099`
"""


class KnownCascades(unittest.TestCase):
    def fuzz(
        self,
        *args: str,
        cascades: str = "",
        status: int = 0,
        build_fails: bool = False,
        baseline: str = BASELINE,
    ) -> tuple[int, str, str]:
        """Runs fuzz.sh in a small repository. Returns its status, its output and summary.md."""
        with tempfile.TemporaryDirectory() as repo:
            scripts = Path(repo, "scripts")
            scripts.mkdir()
            for name in ("fuzz.sh", "fuzz_seeds.py"):
                shutil.copy(SCRIPT.parent / name, scripts)
            Path(repo, "fuzz").mkdir()
            Path(repo, "crates", "sisuc", "tests").mkdir(parents=True)
            with Path(repo, "fuzz", "known_cascades.txt").open("w", encoding="utf-8") as f:
                f.write(baseline)
            write_executable(Path(repo, "bin"), "cargo", CASCADES_CARGO)
            git = ["git", "-C", repo, "-c", "user.name=t", "-c", "user.email=t@t"]
            subprocess.run([*git, "init", "-q"], check=True, env=ENV)
            subprocess.run(
                [*git, "commit", "-q", "--allow-empty", "-m", "base"], check=True, env=ENV
            )
            env = dict(
                ENV,
                PATH=f"{repo}/bin:{ENV['PATH']}",
                FAKE_CASCADES=cascades,
                FAKE_STATUS=str(status),
            )
            if build_fails:
                env["FAKE_BUILD_FAILS"] = "1"
            run = subprocess.run(
                ["bash", "scripts/fuzz.sh", *args, "1"],
                cwd=repo,
                capture_output=True,
                env=env,
                timeout=60,
                check=False,
            )
            with Path(repo, "fuzz", "artifacts", "summary.md").open(encoding="utf-8") as f:
                summary = f.read()
            return run.returncode, run.stdout.decode(errors="replace"), summary

    def test_a_known_pair_that_no_input_gave_should_warn(self) -> None:
        code, out, summary = self.fuzz("--target", "program", cascades="local E0050\n", status=3)
        self.assertEqual(code, 3)
        self.assertIn(WARNING, summary)
        self.assertNotIn("`local E0050`", summary)
        self.assertIn(WARNING, out)

    def test_each_known_pair_that_occurred_should_give_no_warning(self) -> None:
        code, _, summary = self.fuzz(
            "--target", "program", cascades="callee E0099\nlocal E0050\nlocal E0050\n"
        )
        self.assertEqual(code, 0)
        self.assertIn(
            "## Known cascades\n\nEach pair of `fuzz/known_cascades.txt` occurred.\n", summary
        )
        self.assertNotIn("Warning", summary)

    def test_a_baseline_line_with_extra_white_space_should_match_its_pair(self) -> None:
        # parse_cascades (fuzz_program.rs) splits a line on white space, so the script must too.
        _, _, summary = self.fuzz(
            "--target",
            "program",
            cascades="local E0050\ncallee E0099\n",
            baseline="  local   E0050 \r\ncallee\tE0099 \n",
        )
        self.assertIn("Each pair of `fuzz/known_cascades.txt` occurred.", summary)
        self.assertNotIn("Warning", summary)

    def test_a_run_that_did_not_fuzz_should_have_no_cascade_section(self) -> None:
        code, out, summary = self.fuzz("--target", "program", build_fails=True)
        self.assertNotEqual(code, 0)
        self.assertNotIn("## Known cascades", summary)
        self.assertNotIn("## Known cascades", out)

    def test_the_front_end_target_should_have_no_cascade_section(self) -> None:
        code, _, summary = self.fuzz()
        self.assertEqual(code, 0)
        self.assertNotIn("## Known cascades", summary)


# The fake tools of the gap check. Each writes to $FAKE_LOG what it saw, so that a test can tell
# a run that stopped at a tool from one that never reached it. $FAKE_FAIL names the step to fail:
# "coverage program", "coverage front_end" or "llvm-cov".
# cargo fuzz coverage copies each directory it gets into $FAKE_DIRS/<target>-<n> while the
# directory exists, since fuzz.sh deletes them on exit, and makes the profile and the binary.
# It records whether an old raw profile was still there when it started.
GAPS_CARGO = """#!/usr/bin/env bash
[[ $1 == build ]] && exit 0
if [[ $1 == fuzz && $2 == coverage ]]; then
  shift 4
  target=$1
  shift
  old=0
  [[ -e fuzz/coverage/$target/raw/batch-9.profraw ]] && old=1
  echo "coverage $target old=$old $*" >>"$FAKE_LOG"
  n=0
  for arg in "$@"; do
    if [[ -d $arg ]]; then
      n=$((n + 1))
      cp -r "$arg" "$FAKE_DIRS/$target-$n"
    fi
  done
  [[ ${FAKE_FAIL:-} == "coverage $target" ]] && exit 1
  mkdir -p "fuzz/coverage/$target" target/x86_64-unknown-linux-gnu/coverage/x86_64-unknown-linux-gnu/release
  : >"fuzz/coverage/$target/coverage.profdata"
  : >"target/x86_64-unknown-linux-gnu/coverage/x86_64-unknown-linux-gnu/release/$target"
  exit 0
fi
echo "unexpected cargo $*" >>"$FAKE_LOG"
exit 1
"""
GAPS_RUSTC = """#!/usr/bin/env bash
case $* in
-vV) echo "host: x86_64-unknown-linux-gnu" ;;
"--print sysroot") echo "$FAKE_SYSROOT" ;;
*) exit 1 ;;
esac
"""
# The fake llvm-cov takes its fixture from the pair of binary and profile, as the real one takes
# its counts from them, and exits 1 on any other pair or when one of the two files is missing.
GAPS_LLVM_COV = """#!/usr/bin/env bash
[[ $1 == export ]] || exit 1
binary=$2
profile=
for arg in "$@"; do
  case $arg in -instr-profile=*) profile=${arg#*=} ;; esac
done
echo "llvm-cov ${binary##*/} $profile" >>"$FAKE_LOG"
[[ ${FAKE_FAIL:-} == llvm-cov ]] && exit 1
[[ -e $binary && -e $profile ]] || exit 1
case "$binary $profile" in
*/program\\ fuzz/coverage/program/coverage.profdata) cat "$FAKE_SEEDS_JSON" ;;
*/front_end\\ fuzz/coverage/front_end/coverage.profdata) cat "$FAKE_VALID_JSON" ;;
*) exit 1 ;;
esac
"""
HOST = "x86_64-unknown-linux-gnu"
NEW_GAP = record("check/new.rs", "sisuc[abc]::check::new_gap", 1)
NEW_GAP_KEY = "check/new.rs\tsisuc::check::new_gap"
# The programs the reference compiles: each file of run/ and run-fail/, nested ones too, and no
# other file. Two files share a basename, so a copy that keeps the name loses one of them.
PROGRAMS = {
    "run/a.sisu": "run a",
    "run/dup.sisu": "run dup",
    "run/sub/x.sisu": "run sub x",
    "run-fail/dup.sisu": "fail dup",
    "run-fail/b.sisu": "fail b",
}
NOT_PROGRAMS = {
    "run/notes.txt": "notes",
    "diagnostics/d.sisu": "diagnostic",
    "programs/p.sisu": "programs",
    "tokens/t.sisu": "tokens",
}


def write_executable(directory: Path, name: str, text: str) -> None:
    path = directory / name
    directory.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as f:
        f.write(text)
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


class GapsRun:
    """The result of one run of --gaps: its status, output and what the fake tools saw."""

    def __init__(self, repo: Path, work: Path, run: subprocess.CompletedProcess[bytes]) -> None:
        self.code, self.stderr, self.stdout = (
            run.returncode,
            run.stderr.decode(),
            run.stdout.decode(),
        )
        self.log = self.read(work / "log").splitlines()
        self.summary = self.read(repo / "fuzz" / "artifacts" / "summary.md")
        self.dirs = work / "dirs"

    @staticmethod
    def read(path: Path) -> str:
        """The text of the file, or an empty text when the run wrote no such file."""
        try:
            with path.open(encoding="utf-8") as f:
                return f.read()
        except FileNotFoundError:
            return ""

    def calls(self, target: str) -> list[tuple[bool, list[str]]]:
        """The argument lists of the `cargo fuzz coverage` calls for `target`, and whether an old
        raw profile was there at the start of each.
        """
        found = []
        for line in self.log:
            match = re.fullmatch(rf"coverage {target} old=([01])((?: .*)?)", line)
            if match:
                found.append((match[1] == "1", match[2].split()))
        return found

    def files(self, name: str) -> tuple[list[tuple[str, bytes]], int]:
        """(relative path, content) of each regular file of the directory copied under `name`,
        and the number of directories under it.
        """
        root = self.dirs / name
        found = []
        subdirs = 0
        for directory, dirs, names in os.walk(root):
            subdirs += len(dirs)
            for file_name in names:
                path = Path(directory, file_name)
                with path.open("rb") as f:
                    found.append((os.path.relpath(path, root), f.read()))
        return found, subdirs


class GapsCheck(unittest.TestCase):
    @override
    def setUp(self) -> None:
        self.root = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.root)
        self.repo = Path(self.root, "repo")
        self.work = Path(self.root, "work")
        sysroot = Path(self.root, "sysroot")
        (self.work / "dirs").mkdir(parents=True)
        scripts = self.repo / "scripts"
        scripts.mkdir(parents=True)
        for name in ("fuzz.sh", "fuzz_seeds.py", "fuzz_gaps.py"):
            shutil.copy(SCRIPT.parent / name, scripts)
        (self.repo / "fuzz").mkdir(parents=True)
        tests = self.repo / "crates" / "sisuc" / "tests"
        for name, text in {**PROGRAMS, **NOT_PROGRAMS}.items():
            (tests / Path(name).parent).mkdir(parents=True, exist_ok=True)
            with (tests / name).open("w", encoding="utf-8") as f:
                f.write(text)
        self.bin = Path(self.root, "bin")
        write_executable(self.bin, "cargo", GAPS_CARGO)
        write_executable(self.bin, "rustc", GAPS_RUSTC)
        write_executable(sysroot / "lib" / "rustlib" / HOST / "bin", "llvm-cov", GAPS_LLVM_COV)
        self.sysroot = sysroot
        git = ["git", "-C", self.repo, "-c", "user.name=t", "-c", "user.email=t@t"]
        subprocess.run([*git, "init", "-q"], check=True, env=ENV)
        subprocess.run([*git, "commit", "-q", "--allow-empty", "-m", "base"], check=True, env=ENV)

    def put(self, name: str, text: str) -> None:
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("w", encoding="utf-8") as f:
            f.write(text)

    def gaps(  # noqa: PLR0913  one keyword for each thing that a case can change
        self,
        args: tuple[str, ...] = ("--target", "program", "--gaps"),
        *,
        seeds: dict[str, Any] | None = None,
        valid: dict[str, Any] | None = None,
        known: str = "## a reason\n",
        fail: str = "",
        summary: str | None = None,
        old_profiles: bool = False,
        corpus: bool = False,
    ) -> GapsRun:
        """Runs fuzz.sh with `args`. The fake llvm-cov prints `seeds` for the program target and
        `valid` for front_end, which both default to the three anchors.
        """
        fixtures = {}
        for label, content in (("seeds", seeds), ("valid", valid)):
            fixtures[label] = self.work / f"{label}.json"
            with fixtures[label].open("w", encoding="utf-8") as f:
                json.dump(export(*ANCHORS) if content is None else content, f)
        self.put("fuzz/known_gaps.txt", known)
        if summary is not None:
            self.put("fuzz/artifacts/summary.md", summary)
        for target in ("program", "front_end"):
            if old_profiles:
                self.put(f"fuzz/coverage/{target}/raw/batch-9.profraw", "old")
            if corpus:
                self.put(f"fuzz/corpus/{target}/input", "corpus input")
        env = dict(
            ENV,
            PATH=f"{self.bin}:{ENV['PATH']}",
            FAKE_LOG=str(self.work / "log"),
            FAKE_DIRS=str(self.work / "dirs"),
            FAKE_SYSROOT=str(self.sysroot),
            FAKE_FAIL=fail,
            FAKE_SEEDS_JSON=str(fixtures["seeds"]),
            FAKE_VALID_JSON=str(fixtures["valid"]),
        )
        run = subprocess.run(
            ["bash", "scripts/fuzz.sh", *args],
            cwd=self.repo,
            capture_output=True,
            env=env,
            timeout=120,
            check=False,
        )
        return GapsRun(self.repo, self.work, run)

    def test_gaps_should_read_only_programs(self) -> None:
        # Two programs share a basename, one is nested, and four other .sisu or other files must stay out.
        run = self.gaps()
        calls = run.calls("front_end")
        self.assertEqual(len(calls), 1, run.log)
        files, subdirs = run.files("front_end-1")
        self.assertEqual(
            sorted(content.decode() for _, content in files), sorted(PROGRAMS.values())
        )
        self.assertTrue(all(name.endswith(".sisu") for name, _ in files), files)
        self.assertEqual(subdirs, 0)

    def test_gaps_should_clear_old_profiles(self) -> None:
        run = self.gaps(old_profiles=True)
        for target in ("program", "front_end"):
            with self.subTest(target):
                self.assertEqual([old for old, _ in run.calls(target)], [False], run.log)

    def test_gaps_should_fail_with_its_tool(self) -> None:
        # A fake that fails every cargo call stops at `cargo build`, so each row fails one step only.
        for step, record_start in (
            ("coverage program", "coverage program "),
            ("coverage front_end", "coverage front_end "),
            ("llvm-cov", "llvm-cov "),
        ):
            with self.subTest(step):
                run = self.gaps(fail=step)
                self.assertNotEqual(run.code, 0, run.stderr)
                self.assertNotIn("## Generator gaps", run.summary)
                self.assertTrue(any(line.startswith(record_start) for line in run.log), run.log)

    def test_a_new_gap_should_reach_the_summary(self) -> None:
        before = "# Fuzzing: program\n\n## Failures\n\nNone.\n"
        run = self.gaps(valid=export(*ANCHORS, NEW_GAP), summary=before)
        self.assertEqual(run.code, 1, run.stdout + run.stderr)
        self.assertTrue(run.summary.startswith(before), run.summary)
        section = run.summary[len(before) :]
        self.assertRegex(section, r"(?m)^## Generator gaps$")
        self.assertIn(NEW_GAP_KEY, section.splitlines())
        self.assertIn(NEW_GAP_KEY, run.stdout.splitlines())

    def test_gaps_should_measure_only_the_seeds(self) -> None:
        run = self.gaps(corpus=True)
        for target in ("program", "front_end"):
            calls = run.calls(target)
            self.assertEqual(len(calls), 1, run.log)
            self.assertEqual(len(calls[0][1]), 1, calls)
            self.assertNotIn("corpus", calls[0][1][0])
        files, _ = run.files("program-1")
        self.assertEqual([len(content) for _, content in files], [4096] * 128)
        self.assertEqual(len(run.files("front_end-1")[0]), len(PROGRAMS))

    def test_program_coverage_should_measure_the_corpus_and_the_seeds(self) -> None:
        stages = ("lexer", "parser", "check", "mir", "codegen")
        files = [
            {
                "filename": f"/r/crates/sisuc/src/{stage}/mod.rs",
                "summary": {"lines": {"count": 10, "covered": 5}},
            }
            for stage in stages
        ]
        run = self.gaps(
            args=("--target", "program", "--coverage"),
            seeds={"data": [{"files": files}]},
            old_profiles=True,
            corpus=True,
        )
        self.assertEqual(run.code, 0, run.stdout + run.stderr)
        calls = run.calls("program")
        self.assertEqual([old for old, _ in calls], [False], run.log)
        self.assertEqual(len(calls[0][1]), 2, calls)
        self.assertEqual(calls[0][1][0], "fuzz/corpus/program")
        self.assertEqual(len(run.files("program-2")[0]), 128)
        self.assertIn("inputs reaching codegen: all 129 (corpus and seeds", run.stdout)
        for stage in stages:
            self.assertRegex(run.stdout, rf"(?m)^{stage} +50\.0% of 10 lines$")

    def test_a_broken_measurement_should_fail_with_its_reason(self) -> None:
        # The valid export reaches no function under check/.
        run = self.gaps(valid=export(ANCHORS[1], ANCHORS[2]))
        self.assertEqual(run.code, 2, run.stdout + run.stderr)
        self.assertRegex(run.summary, r"(?ms)^## Generator gaps$.*did not finish")

    def test_no_new_gap_should_pass_with_a_section(self) -> None:
        run = self.gaps(valid=export(*ANCHORS, NEW_GAP), known=f"## a reason\n{NEW_GAP_KEY}\n")
        self.assertEqual(run.code, 0, run.stdout + run.stderr)
        self.assertRegex(run.summary, r"(?m)^## Generator gaps$")

    def test_gaps_should_need_the_program_target(self) -> None:
        for args in (("--target", "front_end", "--gaps"), ("--gaps",)):
            with self.subTest(args):
                run = self.gaps(args)
                self.assertEqual(run.code, 2, run.stderr)
                self.assertIn("usage:", run.stderr)
                self.assertIn("--gaps", run.stderr)
                self.assertEqual([line for line in run.log if line.startswith("coverage")], [])

    def check_missing_directory(self, name: str) -> None:
        """Removes the directory tests/<name>, runs --gaps, and checks that it measures nothing."""
        shutil.rmtree(self.repo / "crates" / "sisuc" / "tests" / name)
        run = self.gaps()
        self.assertNotEqual(run.code, 0, run.stdout + run.stderr)
        self.assertEqual(
            [
                line
                for line in run.log
                if line.startswith(("coverage front_end", "coverage program"))
            ],
            [],
        )
        self.assertNotIn("## Generator gaps", run.summary)

    def test_gaps_should_fail_without_tests_run(self) -> None:
        self.check_missing_directory("run")

    def test_gaps_should_fail_without_tests_run_fail(self) -> None:
        self.check_missing_directory("run-fail")

    def test_front_end_seeds_should_fail_on_a_missing_test_tree(self) -> None:
        shutil.rmtree(self.repo / "crates" / "sisuc" / "tests")
        run = self.gaps(args=("--target", "front_end", "--coverage"))
        self.assertNotEqual(run.code, 0, run.stdout + run.stderr)
        self.assertEqual([line for line in run.log if line.startswith("coverage front_end")], [])

    def check_directory_without_program(self, name: str) -> None:
        """Keeps one other file in tests/<name> and no program, as the real run-fail keeps its .out files."""
        root = self.repo / "crates" / "sisuc" / "tests" / name
        for directory, _, names in os.walk(root):
            for file in names:
                if file.endswith(".sisu"):
                    Path(directory, file).unlink()
        with (root / "other.out").open("w", encoding="utf-8") as f:
            f.write("output")
        run = self.gaps()
        self.assertEqual(run.code, 2, run.stdout + run.stderr)
        self.assertIn(f"crates/sisuc/tests/{name}", run.stderr)
        self.assertEqual([line for line in run.log if line.startswith("coverage")], [])
        self.assertNotIn("## Generator gaps", run.summary)

    def test_gaps_should_fail_when_tests_run_has_no_program(self) -> None:
        self.check_directory_without_program("run")

    def test_gaps_should_fail_when_tests_run_fail_has_no_program(self) -> None:
        self.check_directory_without_program("run-fail")

    def test_gaps_should_fail_when_a_copy_fails(self) -> None:
        write_executable(self.bin, "cp", "#!/bin/sh\nexit 7\n")
        run = self.gaps()
        self.assertEqual(run.code, 7, run.stdout + run.stderr)
        self.assertEqual([line for line in run.log if line.startswith("coverage")], [])
        self.assertNotIn("## Generator gaps", run.summary)


if __name__ == "__main__":
    unittest.main()
