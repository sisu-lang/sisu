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

from test_fuzz_gaps import ANCHORS, export, record

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fuzz.sh")
# A pre-commit hook runs these tests with GIT_DIR and its kin set to the outer repository.
ENV = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
# The fake cargo fuzz run (the one with --jobs) writes a crash input and fails, as libFuzzer does.
# A replay of that input prints the bytes of $FAKE_REPLAY, which a test sets with printf escapes.
FAKE_CARGO = """#!/usr/bin/env bash
[[ $1 == fuzz ]] || exit 1
case $2 in
build) exit 0 ;;
--version) echo "cargo-fuzz 0.0.0"; exit 0 ;;
esac
if [[ " $* " == *" --jobs "* ]]; then
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
    def setUp(self):
        self.repo = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.repo)
        os.mkdir(os.path.join(self.repo, "scripts"))
        shutil.copy(SCRIPT, os.path.join(self.repo, "scripts"))
        os.makedirs(os.path.join(self.repo, "crates", "sisuc", "tests"))
        os.makedirs(os.path.join(self.repo, "fuzz"))
        self.bin = os.path.join(self.repo, "bin")
        os.mkdir(self.bin)
        cargo = os.path.join(self.bin, "cargo")
        with open(cargo, "w", encoding="utf-8") as f:
            f.write(FAKE_CARGO)
        os.chmod(cargo, os.stat(cargo).st_mode | stat.S_IXUSR)
        git = ["git", "-C", self.repo, "-c", "user.name=t", "-c", "user.email=t@t"]
        subprocess.run(git + ["init", "-q"], check=True, env=ENV)
        subprocess.run(git + ["commit", "-q", "--allow-empty", "-m", "base"], check=True, env=ENV)

    def summary(self, replay):
        env = dict(ENV, PATH=f"{self.bin}:{ENV['PATH']}", FAKE_REPLAY=replay)
        run = subprocess.run(
            ["bash", "scripts/fuzz.sh", "1"],
            cwd=self.repo,
            capture_output=True,
            env=env,
            timeout=60,
        )
        self.assertEqual(run.returncode, 1, run.stderr)
        with open(
            os.path.join(self.repo, "fuzz", "artifacts", "summary.md"), encoding="utf-8"
        ) as f:
            return f.read()

    def test_a_crash_with_a_panic_message_should_show_its_location_and_first_message_line(self):
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

    def test_a_panic_message_should_lose_its_backticks_and_control_characters(self):
        text = self.summary(r"panicked at a.rs:1:2:\nbad `code` \001here\n")
        self.assertIn("  Panic at `a.rs:1:2`: `bad code here`\n", text)

    def test_binary_bytes_before_the_panic_on_the_same_line_should_not_reach_the_location(self):
        text = self.summary(r"garbage \377\376 panicked at a.rs:1:2:\nhello\n")
        self.assertIn("  Panic at `a.rs:1:2`: `hello`\n", text)

    def test_a_long_message_should_be_cut_in_whole_characters(self):
        text = self.summary(r"panicked at a.rs:1:2:\na" + r"\303\251" * 200 + r"\n")
        line = next(line for line in text.splitlines() if "Panic at" in line)
        self.assertTrue(line.startswith("  Panic at `a.rs:1:2`: `a"))
        self.assertLessEqual(len(line.encode()), 400)

    def test_a_mention_in_a_message_should_stay_in_a_code_span(self):
        text = self.summary(r"panicked at a.rs:1:2:\nhello @octocat #12\n")
        self.assertIn("`hello @octocat #12`", text)

    def test_a_panic_without_a_message_line_should_show_the_location_alone(self):
        text = self.summary(r"panicked at a.rs:1:2:")
        self.assertIn("  Panic at `a.rs:1:2`\n", text)

    def test_a_crash_without_a_panic_message_should_keep_the_note_about_a_fatal_signal(self):
        text = self.summary(r"\377\000 AddressSanitizer: stack-overflow\n")
        self.assertNotIn("Panic at", text)
        self.assertIn("is a stack overflow or another fatal signal", text)


class ProgramSeeds(unittest.TestCase):
    def test_program_run_should_get_the_seeds(self):
        with tempfile.TemporaryDirectory() as repo:
            scripts = os.path.join(repo, "scripts")
            os.mkdir(scripts)
            for name in ("fuzz.sh", "fuzz_seeds.py"):
                shutil.copy(os.path.join(os.path.dirname(SCRIPT), name), scripts)
            os.makedirs(os.path.join(repo, "fuzz"))
            bin_dir = os.path.join(repo, "bin")
            os.mkdir(bin_dir)
            cargo = os.path.join(bin_dir, "cargo")
            with open(cargo, "w", encoding="utf-8") as f:
                f.write(PROGRAM_CARGO)
            os.chmod(cargo, os.stat(cargo).st_mode | stat.S_IXUSR)
            git = ["git", "-C", repo, "-c", "user.name=t", "-c", "user.email=t@t"]
            subprocess.run(git + ["init", "-q"], check=True, env=ENV)
            subprocess.run(
                git + ["commit", "-q", "--allow-empty", "-m", "base"], check=True, env=ENV
            )
            seen = os.path.join(repo, "seen.txt")
            env = dict(ENV, PATH=f"{bin_dir}:{ENV['PATH']}", FAKE_SEEN=seen)
            run = subprocess.run(
                ["bash", "scripts/fuzz.sh", "--target", "program", "1"],
                cwd=repo,
                capture_output=True,
                env=env,
                timeout=60,
            )
            self.assertEqual(run.returncode, 0, run.stderr)
            with open(seen, encoding="utf-8") as f:
                lines = f.read().splitlines()
            # The corpus is empty. The seeds directory comes second.
            self.assertEqual(lines[0], "program 0 0")
            self.assertEqual(len(lines), 2, lines)
            self.assertTrue(lines[1].endswith(" 128 128"), lines[1])


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


def write_executable(directory, name, text):
    path = os.path.join(directory, name)
    os.makedirs(directory, exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)
    os.chmod(path, os.stat(path).st_mode | stat.S_IXUSR)


class GapsRun:
    """The result of one run of --gaps: its status, output and what the fake tools saw."""

    def __init__(self, repo, work, run):
        self.code, self.stderr, self.stdout = (
            run.returncode,
            run.stderr.decode(),
            run.stdout.decode(),
        )
        self.log = self.read(os.path.join(work, "log")).splitlines()
        self.summary = self.read(os.path.join(repo, "fuzz", "artifacts", "summary.md"))
        self.dirs = os.path.join(work, "dirs")

    @staticmethod
    def read(path):
        """The text of the file, or an empty text when the run wrote no such file."""
        try:
            with open(path, encoding="utf-8") as f:
                return f.read()
        except FileNotFoundError:
            return ""

    def calls(self, target):
        """The argument lists of the `cargo fuzz coverage` calls for `target`, and whether an old
        raw profile was there at the start of each.
        """
        found = []
        for line in self.log:
            match = re.fullmatch(rf"coverage {target} old=([01])((?: .*)?)", line)
            if match:
                found.append((match[1] == "1", match[2].split()))
        return found

    def files(self, name):
        """(relative path, content) of each regular file of the directory copied under `name`,
        and the number of directories under it.
        """
        root = os.path.join(self.dirs, name)
        found = []
        subdirs = 0
        for directory, dirs, names in os.walk(root):
            subdirs += len(dirs)
            for name in names:
                path = os.path.join(directory, name)
                with open(path, "rb") as f:
                    found.append((os.path.relpath(path, root), f.read()))
        return found, subdirs


class GapsCheck(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.root)
        self.repo = os.path.join(self.root, "repo")
        self.work = os.path.join(self.root, "work")
        sysroot = os.path.join(self.root, "sysroot")
        os.makedirs(os.path.join(self.work, "dirs"))
        scripts = os.path.join(self.repo, "scripts")
        os.makedirs(scripts)
        for name in ("fuzz.sh", "fuzz_seeds.py", "fuzz_gaps.py"):
            shutil.copy(os.path.join(os.path.dirname(SCRIPT), name), scripts)
        os.makedirs(os.path.join(self.repo, "fuzz"))
        tests = os.path.join(self.repo, "crates", "sisuc", "tests")
        for name, text in {**PROGRAMS, **NOT_PROGRAMS}.items():
            os.makedirs(os.path.join(tests, os.path.dirname(name)), exist_ok=True)
            with open(os.path.join(tests, name), "w", encoding="utf-8") as f:
                f.write(text)
        self.bin = os.path.join(self.root, "bin")
        write_executable(self.bin, "cargo", GAPS_CARGO)
        write_executable(self.bin, "rustc", GAPS_RUSTC)
        write_executable(
            os.path.join(sysroot, "lib", "rustlib", HOST, "bin"), "llvm-cov", GAPS_LLVM_COV
        )
        self.sysroot = sysroot
        git = ["git", "-C", self.repo, "-c", "user.name=t", "-c", "user.email=t@t"]
        subprocess.run(git + ["init", "-q"], check=True, env=ENV)
        subprocess.run(git + ["commit", "-q", "--allow-empty", "-m", "base"], check=True, env=ENV)

    def put(self, name, text):
        path = os.path.join(self.repo, name)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as f:
            f.write(text)

    def gaps(
        self,
        args=("--target", "program", "--gaps"),
        seeds=None,
        valid=None,
        known="## a reason\n",
        fail="",
        summary=None,
        old_profiles=False,
        corpus=False,
    ):
        """Runs fuzz.sh with `args`. The fake llvm-cov prints `seeds` for the program target and
        `valid` for front_end, which both default to the three anchors.
        """
        fixtures = {}
        for label, content in (("seeds", seeds), ("valid", valid)):
            fixtures[label] = os.path.join(self.work, f"{label}.json")
            with open(fixtures[label], "w", encoding="utf-8") as f:
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
            FAKE_LOG=os.path.join(self.work, "log"),
            FAKE_DIRS=os.path.join(self.work, "dirs"),
            FAKE_SYSROOT=self.sysroot,
            FAKE_FAIL=fail,
            FAKE_SEEDS_JSON=fixtures["seeds"],
            FAKE_VALID_JSON=fixtures["valid"],
        )
        run = subprocess.run(
            ["bash", "scripts/fuzz.sh", *args],
            cwd=self.repo,
            capture_output=True,
            env=env,
            timeout=120,
        )
        return GapsRun(self.repo, self.work, run)

    def test_gaps_should_read_only_programs(self):
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

    def test_gaps_should_clear_old_profiles(self):
        run = self.gaps(old_profiles=True)
        for target in ("program", "front_end"):
            with self.subTest(target):
                self.assertEqual([old for old, _ in run.calls(target)], [False], run.log)

    def test_gaps_should_fail_with_its_tool(self):
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

    def test_a_new_gap_should_reach_the_summary(self):
        before = "# Fuzzing: program\n\n## Failures\n\nNone.\n"
        run = self.gaps(valid=export(*ANCHORS, NEW_GAP), summary=before)
        self.assertEqual(run.code, 1, run.stdout + run.stderr)
        self.assertTrue(run.summary.startswith(before), run.summary)
        section = run.summary[len(before) :]
        self.assertRegex(section, r"(?m)^## Generator gaps$")
        self.assertIn(NEW_GAP_KEY, section.splitlines())
        self.assertIn(NEW_GAP_KEY, run.stdout.splitlines())

    def test_gaps_should_measure_only_the_seeds(self):
        run = self.gaps(corpus=True)
        for target in ("program", "front_end"):
            calls = run.calls(target)
            self.assertEqual(len(calls), 1, run.log)
            self.assertEqual(len(calls[0][1]), 1, calls)
            self.assertNotIn("corpus", calls[0][1][0])
        files, _ = run.files("program-1")
        self.assertEqual([len(content) for _, content in files], [4096] * 128)
        self.assertEqual(len(run.files("front_end-1")[0]), len(PROGRAMS))

    def test_program_coverage_should_measure_the_corpus_and_the_seeds(self):
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

    def test_a_broken_measurement_should_fail_with_its_reason(self):
        # The valid export reaches no function under check/.
        run = self.gaps(valid=export(ANCHORS[1], ANCHORS[2]))
        self.assertEqual(run.code, 2, run.stdout + run.stderr)
        self.assertRegex(run.summary, r"(?ms)^## Generator gaps$.*did not finish")

    def test_no_new_gap_should_pass_with_a_section(self):
        run = self.gaps(valid=export(*ANCHORS, NEW_GAP), known=f"## a reason\n{NEW_GAP_KEY}\n")
        self.assertEqual(run.code, 0, run.stdout + run.stderr)
        self.assertRegex(run.summary, r"(?m)^## Generator gaps$")

    def test_gaps_should_need_the_program_target(self):
        for args in (("--target", "front_end", "--gaps"), ("--gaps",)):
            with self.subTest(args):
                run = self.gaps(args)
                self.assertEqual(run.code, 2, run.stderr)
                self.assertIn("usage:", run.stderr)
                self.assertIn("--gaps", run.stderr)
                self.assertEqual([line for line in run.log if line.startswith("coverage")], [])

    def check_missing_directory(self, name):
        """Removes the directory tests/<name>, runs --gaps, and checks that it measures nothing."""
        shutil.rmtree(os.path.join(self.repo, "crates", "sisuc", "tests", name))
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

    def test_gaps_should_fail_without_tests_run(self):
        self.check_missing_directory("run")

    def test_gaps_should_fail_without_tests_run_fail(self):
        self.check_missing_directory("run-fail")

    def test_front_end_seeds_should_fail_on_a_missing_test_tree(self):
        shutil.rmtree(os.path.join(self.repo, "crates", "sisuc", "tests"))
        run = self.gaps(args=("--target", "front_end", "--coverage"))
        self.assertNotEqual(run.code, 0, run.stdout + run.stderr)
        self.assertEqual([line for line in run.log if line.startswith("coverage front_end")], [])

    def check_directory_without_program(self, name):
        """Keeps one other file in tests/<name> and no program, as the real run-fail keeps its .out files."""
        root = os.path.join(self.repo, "crates", "sisuc", "tests", name)
        for directory, _, names in os.walk(root):
            for file in names:
                if file.endswith(".sisu"):
                    os.remove(os.path.join(directory, file))
        with open(os.path.join(root, "other.out"), "w", encoding="utf-8") as f:
            f.write("output")
        run = self.gaps()
        self.assertEqual(run.code, 2, run.stdout + run.stderr)
        self.assertIn(f"crates/sisuc/tests/{name}", run.stderr)
        self.assertEqual([line for line in run.log if line.startswith("coverage")], [])
        self.assertNotIn("## Generator gaps", run.summary)

    def test_gaps_should_fail_when_tests_run_has_no_program(self):
        self.check_directory_without_program("run")

    def test_gaps_should_fail_when_tests_run_fail_has_no_program(self):
        self.check_directory_without_program("run-fail")

    def test_gaps_should_fail_when_a_copy_fails(self):
        write_executable(self.bin, "cp", "#!/bin/sh\nexit 7\n")
        run = self.gaps()
        self.assertEqual(run.code, 7, run.stdout + run.stderr)
        self.assertEqual([line for line in run.log if line.startswith("coverage")], [])
        self.assertNotIn("## Generator gaps", run.summary)


if __name__ == "__main__":
    unittest.main()
