"""Unit tests for mutants.sh: python3 -m unittest discover -s scripts -p 'test_*.py'.

Each test runs the script in a small repository with a fake `cargo` on PATH, so no mutant runs.
"""

import fcntl
import glob
import os
import re
import shutil
import stat
import subprocess
import tempfile
import unittest

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "mutants.sh")
# A pre-commit hook runs these tests with GIT_DIR and its kin set to the outer repository.
# A caller's LOG, CARGO_MUTANTS_JOBS, NEXTEST_TEST_THREADS and MUTANTS_PASS2_TIMEOUT would change
# what the script does.
ENV = {
    k: v
    for k, v in os.environ.items()
    if not k.startswith("GIT_")
    and k not in ("LOG", "CARGO_MUTANTS_JOBS", "NEXTEST_TEST_THREADS", "MUTANTS_PASS2_TIMEOUT")
}
# The fake cargo mutants records its jobs, niceness and arguments, and whether an old mutants.out
# was there. A run without an anchored `--re` argument is the first pass: it records `args`. A run
# with one is a group of the second pass: it records its arguments in `args2` (`args_lib` for a run with --lib), and
# one to a line in `lines2` (`lines_lib`). Unless FAKE_WRITE is 0, it writes a new
# mutants.out with the result files that the test names in FAKE_CAUGHT, FAKE_UNVIABLE, FAKE_MISSED
# and FAKE_TIMEOUT; FAKE_MISSED_NAMES and FAKE_TIMEOUT_NAMES, lines of mutant names, replace the
# generated names of the first pass. It exits with FAKE_EXIT. The whole-suite group takes
# FAKE_PASS2_CAUGHT (0 by default), and FAKE_PASS2_MISSED, FAKE_PASS2_TIMEOUT and FAKE_PASS2_EXIT,
# each the first pass's value by default. The lib-only group takes FAKE_LIB_CAUGHT, FAKE_LIB_MISSED,
# FAKE_LIB_TIMEOUT and FAKE_LIB_EXIT, each the whole-suite group's value by default. A group run
# also writes FAKE_OUTSIDE_CAUGHT and FAKE_OUTSIDE_MISSED names that no --re gave, as cargo mutants
# does for its "delete field" mutants.
# FAKE_GROUP_WRITE=0 makes a group run write nothing. A run with --list prints FAKE_LIST mutants (0 by default), records nothing and exits with FAKE_EXIT.
# The sleep keeps lock.json strictly newer than the script's start mark.
FAKE_CARGO = r"""#!/usr/bin/env bash
if [[ " $* " == *" --list "* ]]; then
  for ((i = 0; i < ${FAKE_LIST:-0}; i++)); do echo "mutant $i"; done
  exit "${FAKE_EXIT:-0}"
fi
echo "$CARGO_MUTANTS_JOBS $(nice) $NEXTEST_TEST_THREADS" >seen
prefix=FAKE
if [[ " $* " == *" ^"* ]]; then
  name=args2
  prefix=FAKE_PASS2
  FAKE_PASS2_CAUGHT=${FAKE_PASS2_CAUGHT:-0}
  FAKE_PASS2_UNVIABLE=0
  FAKE_PASS2_MISSED=${FAKE_PASS2_MISSED:-${FAKE_MISSED:-0}}
  FAKE_PASS2_TIMEOUT=${FAKE_PASS2_TIMEOUT:-${FAKE_TIMEOUT:-0}}
  FAKE_PASS2_EXIT=${FAKE_PASS2_EXIT:-${FAKE_EXIT:-0}}
  if [[ " $* " == *" --lib "* ]]; then
    name=args_lib
    prefix=FAKE_LIB
    FAKE_LIB_CAUGHT=${FAKE_LIB_CAUGHT:-$FAKE_PASS2_CAUGHT}
    FAKE_LIB_UNVIABLE=0
    FAKE_LIB_MISSED=${FAKE_LIB_MISSED:-$FAKE_PASS2_MISSED}
    FAKE_LIB_TIMEOUT=${FAKE_LIB_TIMEOUT:-$FAKE_PASS2_TIMEOUT}
    FAKE_LIB_EXIT=${FAKE_LIB_EXIT:-$FAKE_PASS2_EXIT}
  fi
  echo "$*" >$name
  printf '%s\n' "$@" >lines${name#args}
else
  echo "$*" >args
fi
exit_var=${prefix}_EXIT
[[ -e mutants.out/missed.txt ]] && touch saw_old_out
sleep 0.05
write=${FAKE_WRITE:-1}
[[ $prefix == FAKE ]] || write=${FAKE_GROUP_WRITE:-1}
[[ $write == 0 ]] && exit "${!exit_var:-0}"
rm -rf mutants.out.old
[[ -e mutants.out ]] && mv mutants.out mutants.out.old
mkdir -p mutants.out
touch mutants.out/lock.json
if [[ $prefix == FAKE ]]; then
  for kind in caught unviable missed timeout; do
    var=${prefix}_${kind^^}
    for ((i = 0; i < ${!var:-0}; i++)); do echo "mutant $i" >>mutants.out/$kind.txt; done
  done
  [[ -z ${FAKE_MISSED_NAMES:-} ]] || echo "$FAKE_MISSED_NAMES" >mutants.out/missed.txt
  [[ -z ${FAKE_TIMEOUT_NAMES:-} ]] || echo "$FAKE_TIMEOUT_NAMES" >mutants.out/timeout.txt
else
  # A group run writes the names it was given, as many as each count says, in the order caught,
  # unviable, missed, timeout. Fewer names than the counts say are made up; more are not tested.
  mapfile -t given < <(printf '%s\n' "$@" | grep '^\^' | sed 's/^\^//; s/\$$//; s/\\\(.\)/\1/g')
  at=0
  for kind in caught unviable missed timeout; do
    var=${prefix}_${kind^^}
    for ((i = 0; i < ${!var:-0}; i++)); do
      if ((at < ${#given[@]})); then echo "${given[at]}"; else echo "made up $at"; fi >>mutants.out/$kind.txt
      at=$((at + 1))
    done
  done
  # The "delete field" mutants that cargo mutants runs in every group, whatever --re says.
  for ((i = 0; i < ${FAKE_OUTSIDE_CAUGHT:-0}; i++)); do echo "crates/x/src/a.rs:$i:1: delete field f" >>mutants.out/caught.txt; done
  for ((i = 0; i < ${FAKE_OUTSIDE_MISSED:-0}; i++)); do echo "crates/x/src/b.rs:$i:1: delete field g" >>mutants.out/missed.txt; done
fi
exit "${!exit_var:-0}"
"""


def escaped(name):
    """The name with the characters `\\ . + * ? ( ) | [ ] { } ^ $` escaped, as the script does."""
    return re.sub(r"([\\.+*?()|\[\]{}^$])", r"\\\1", name)


class Mutants(unittest.TestCase):
    def setUp(self) -> None:
        self.repo = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.repo)
        self.scripts = os.path.join(self.repo, "scripts")
        os.mkdir(self.scripts)
        shutil.copy(SCRIPT, self.scripts)
        shutil.copy(os.path.join(os.path.dirname(SCRIPT), "heavy-run.sh"), self.scripts)
        self.bin = os.path.join(self.repo, "bin")
        os.mkdir(self.bin)
        cargo = os.path.join(self.bin, "cargo")
        with open(cargo, "w", encoding="utf-8") as f:
            f.write(FAKE_CARGO)
        os.chmod(cargo, os.stat(cargo).st_mode | stat.S_IXUSR)
        self.git = ["git", "-C", self.repo, "-c", "user.name=t", "-c", "user.email=t@t"]
        subprocess.run([*self.git[:3], "init", "-q"], check=True, env=ENV)
        self.commit("base")
        self.log = os.path.join(self.repo, "mutation.log")

    def commit(self, message) -> None:
        subprocess.run([*self.git, "add", "-A"], check=True, env=ENV)
        subprocess.run(
            [*self.git, "commit", "-q", "--allow-empty", "-m", message], check=True, env=ENV
        )

    def head(self):
        return subprocess.run(
            [*self.git, "rev-parse", "HEAD"], capture_output=True, text=True, check=True, env=ENV
        ).stdout.strip()

    def write_rust(self, text) -> None:
        with open(os.path.join(self.repo, "a.rs"), "w", encoding="utf-8") as f:
            f.write(text)

    def mutate(self, *args, log=True, env=None, limit=None, **fake):
        env = dict(
            ENV,
            PATH=f"{self.bin}:{ENV['PATH']}",
            TMPDIR=self.repo,
            **(env or {}),
            **{f"FAKE_{k.upper()}": str(v) for k, v in fake.items()},
        )
        if log:
            env["LOG"] = self.log
        return subprocess.run(
            ["bash", "scripts/mutants.sh", "HEAD~1", *args],
            cwd=self.repo,
            capture_output=True,
            text=True,
            env=env,
            timeout=limit,
        )

    def seen(self, name):
        return os.path.exists(os.path.join(self.repo, name))

    def last_line(self):
        with open(self.log, encoding="utf-8") as f:
            return f.read().splitlines()[-1]

    def rust_commit(self) -> None:
        self.write_rust("fn main() {}\n")
        self.commit("rust")

    def test_a_diff_without_rust_should_pass_without_running_cargo(self) -> None:
        self.commit("docs")
        run = self.mutate()
        self.assertEqual(run.returncode, 0)
        self.assertEqual(run.stdout, "")
        self.assertEqual(
            self.last_line(),
            f"mutation-test: PASS: no Rust changes since HEAD~1 (HEAD {self.head()})",
        )
        self.assertFalse(self.seen("seen"))

    def test_a_second_run_should_fail_and_leave_the_first_runs_log_alone(self) -> None:
        with open(self.log, "w", encoding="utf-8") as f:
            f.write("first run\n")
        os.mkdir(os.path.join(self.repo, "target"))
        # A run in another worktree holds the lock of the clone too; this run must not wait for it.
        with (
            open(os.path.join(self.repo, "target", "mutation-test.lock"), "w") as lock,
            open(os.path.join(self.repo, ".git", "sisu-heavy-run.lock"), "w") as clone,
        ):
            fcntl.flock(lock, fcntl.LOCK_EX)
            fcntl.flock(clone, fcntl.LOCK_EX)
            run = self.mutate(limit=30)
        self.assertEqual(run.returncode, 1)
        self.assertIn(
            "mutation-test: FAIL: another make mutation-test is running in this worktree",
            run.stdout,
        )
        with open(self.log, encoding="utf-8") as f:
            self.assertEqual(f.read(), "first run\n")

    def test_make_mutation_test_should_take_the_clone_lock_once_and_run(self) -> None:
        # A recipe that wraps mutants.sh in heavy-run.sh again waits for its own lock forever.
        shutil.copy(os.path.join(os.path.dirname(SCRIPT), "..", "Makefile"), self.repo)
        self.rust_commit()
        env = dict(ENV, PATH=f"{self.bin}:{ENV['PATH']}", TMPDIR=self.repo, FAKE_CAUGHT="1")
        run = subprocess.run(
            ["make", "mutation-test", "BASE=HEAD~1", f"LOG={self.log}"],
            cwd=self.repo,
            capture_output=True,
            text=True,
            env=env,
            timeout=30,
        )
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        self.assertTrue(self.last_line().startswith("mutation-test: PASS"))

    def test_a_run_with_no_missed_mutant_should_pass_and_count_a_timeout_as_caught(self) -> None:
        self.rust_commit()
        run = self.mutate(caught=3, timeout=1, exit=3)
        self.assertEqual(run.returncode, 0)
        self.assertEqual(
            self.last_line(),
            f"mutation-test: PASS: 3 caught, 0 unviable, 0 missed, 1 timeouts (HEAD {self.head()})",
        )

    def test_a_timeout_of_the_first_pass_should_be_tested_again_in_the_second_pass(self) -> None:
        self.rust_commit()
        run = self.mutate(
            caught=3, timeout=1, exit=3, pass2_caught=1, pass2_timeout=0, pass2_exit=0
        )
        self.assertEqual(run.returncode, 0)
        with open(os.path.join(self.repo, "args2"), encoding="utf-8") as f:
            args2 = f.read().split()
        self.assertEqual(args2[args2.index("--timeout") + 1], "600")
        self.assertEqual(
            self.last_line(),
            f"mutation-test: PASS: 4 caught, 0 unviable, 0 missed, 0 timeouts (HEAD {self.head()})",
        )

    def test_a_timeout_of_the_second_pass_should_still_count_as_caught(self) -> None:
        self.rust_commit()
        run = self.mutate(
            caught=3, timeout=2, exit=3, pass2_caught=1, pass2_timeout=1, pass2_exit=3
        )
        self.assertEqual(run.returncode, 0)
        self.assertTrue(self.seen("args2"))
        self.assertEqual(
            self.last_line(),
            f"mutation-test: PASS: 4 caught, 0 unviable, 0 missed, 1 timeouts (HEAD {self.head()})",
        )

    def test_a_missed_mutant_should_fail_even_with_a_timeout(self) -> None:
        self.rust_commit()
        run = self.mutate(
            caught=1, missed=2, timeout=1, exit=3, missed_names="a\nb", timeout_names="c"
        )
        self.assertEqual(run.returncode, 1)
        self.assertEqual(
            self.last_line(),
            f"mutation-test: FAIL: 1 caught, 0 unviable, 2 missed, 1 timeouts (HEAD {self.head()})",
        )

    def test_a_failed_cargo_run_should_fail_and_name_its_exit_code(self) -> None:
        self.rust_commit()
        run = self.mutate(exit=4)
        self.assertEqual(run.returncode, 1)
        self.assertIn("(cargo mutants exit 4, see the output above)", self.last_line())

    def test_uncommitted_rust_should_fail_without_running_cargo(self) -> None:
        self.rust_commit()
        self.write_rust("fn main() { let _x = 1; }\n")
        run = self.mutate(caught=1)
        self.assertEqual(run.returncode, 1)
        self.assertEqual(
            self.last_line(),
            "mutation-test: FAIL: uncommitted Rust changes; commit them, then run again",
        )
        self.assertFalse(self.seen("seen"))

    def test_cargo_should_run_niced_with_two_jobs_of_eight_test_threads_by_default(self) -> None:
        self.rust_commit()
        self.mutate(log=False, caught=1)
        with open(os.path.join(self.repo, "seen"), encoding="utf-8") as f:
            self.assertEqual(f.read().strip(), "2 19 8")

    def test_a_run_without_new_output_should_pass_and_leave_the_old_mutants_out_alone(self) -> None:
        self.rust_commit()
        self.mutate(missed=5, exit=2)
        run = self.mutate("--iterate", write=0)
        self.assertEqual(run.returncode, 0)
        self.assertEqual(
            self.last_line(), f"mutation-test: PASS: no mutant tested (HEAD {self.head()})"
        )
        self.assertTrue(self.seen("saw_old_out"))
        self.assertTrue(self.seen("mutants.out/missed.txt"))

    def test_a_failed_run_without_new_output_should_fail_and_name_its_exit_code(self) -> None:
        self.rust_commit()
        run = self.mutate(write=0, exit=2)
        self.assertEqual(run.returncode, 1)
        self.assertEqual(
            self.last_line(),
            "mutation-test: FAIL: no mutant tested (cargo mutants exit 2, "
            f"see the output above) (HEAD {self.head()})",
        )

    def test_an_argument_that_tests_nothing_should_end_with_not_run_and_keep_the_exit_code(
        self,
    ) -> None:
        self.rust_commit()
        for arg in ("--list", "--list-files", "--check"):
            with self.subTest(arg):
                run = self.mutate(arg, write=0, exit=0)
                self.assertEqual(run.returncode, 0)
                self.assertEqual(self.last_line(), f"mutation-test: NOT RUN: {arg} tests no mutant")
                self.assertEqual(self.mutate(arg, write=0, exit=2).returncode, 2)

    def test_check_should_not_count_its_output_as_a_pass(self) -> None:
        self.rust_commit()
        run = self.mutate("--check", exit=0)
        self.assertEqual(run.returncode, 0)
        self.assertIn("NOT RUN", self.last_line())

    def test_an_argument_that_moves_the_output_should_fail_without_running_cargo(self) -> None:
        self.rust_commit()
        for args, env in (
            (("-o", "x"), {}),
            (("--output=x",), {}),
            (("-d", "x"), {}),
            (("--dir", "x"), {}),
            (("--manifest-path=x",), {}),
        ):
            with self.subTest(args=args, env=env):
                run = self.mutate(*args, env=env)
                self.assertEqual(run.returncode, 1)
                self.assertIn("mutation-test: FAIL:", self.last_line())
                self.assertNotIn("(HEAD", self.last_line())
                self.assertFalse(self.seen("seen"))

    def test_an_argument_after_double_dash_should_go_to_cargo_test(self) -> None:
        self.rust_commit()
        run = self.mutate("--", "-o", "x", caught=1)
        self.assertEqual(run.returncode, 0)
        with open(os.path.join(self.repo, "args"), encoding="utf-8") as f:
            self.assertTrue(f.read().strip().endswith("-- -o x"))

    def test_a_first_pass_should_test_each_mutant_with_the_unit_tests_only(self) -> None:
        self.rust_commit()
        run = self.mutate(caught=2)
        self.assertEqual(run.returncode, 0)
        with open(os.path.join(self.repo, "args"), encoding="utf-8") as f:
            self.assertTrue(f.read().strip().endswith("-- --lib"))
        self.assertFalse(self.seen("args2"))
        self.assertEqual(
            self.last_line(),
            f"mutation-test: PASS: 2 caught, 0 unviable, 0 missed, 0 timeouts (HEAD {self.head()})",
        )

    def test_a_mutant_that_only_the_whole_suite_kills_should_be_caught_in_the_second_pass(
        self,
    ) -> None:
        self.rust_commit()
        run = self.mutate(
            caught=5, unviable=1, missed=2, exit=2, pass2_caught=2, pass2_missed=0, pass2_exit=0
        )
        self.assertEqual(run.returncode, 0)
        with open(os.path.join(self.repo, "args2"), encoding="utf-8") as f:
            args2 = f.read().split()
        self.assertIn("--re", args2)
        self.assertNotIn("--iterate", args2)
        self.assertNotIn("--lib", args2)
        self.assertEqual(
            self.last_line(),
            f"mutation-test: PASS: 7 caught, 1 unviable, 0 missed, 0 timeouts (HEAD {self.head()})",
        )

    def test_a_mutant_that_survives_both_passes_should_fail(self) -> None:
        self.rust_commit()
        run = self.mutate(caught=5, missed=2, exit=2, pass2_caught=1, pass2_missed=1)
        self.assertEqual(run.returncode, 1)
        self.assertEqual(
            self.last_line(),
            f"mutation-test: FAIL: 6 caught, 0 unviable, 1 missed, 0 timeouts (HEAD {self.head()})",
        )

    def test_a_failed_first_pass_should_not_start_the_second(self) -> None:
        self.rust_commit()
        run = self.mutate(missed=1, exit=4)
        self.assertEqual(run.returncode, 1)
        self.assertFalse(self.seen("args2"))
        self.assertTrue(
            self.last_line().startswith("mutation-test: FAIL: 0 caught, 0 unviable, 1 missed")
        )

    def pass2_args(self, *args, env=None):
        self.rust_commit()
        self.mutate(*args, env=env, missed=1, exit=2, pass2_caught=1, pass2_missed=0, pass2_exit=0)
        with open(os.path.join(self.repo, "args2"), encoding="utf-8") as f:
            return f.read().split()

    def test_the_second_pass_should_wait_600_seconds_for_each_mutant(self) -> None:
        args2 = self.pass2_args()
        self.assertEqual(args2[args2.index("--timeout") + 1], "600")
        with open(os.path.join(self.repo, "args"), encoding="utf-8") as f:
            self.assertNotIn("--timeout", f.read())

    def test_mutants_pass2_timeout_should_change_the_wait_of_the_second_pass(self) -> None:
        args2 = self.pass2_args(env={"MUTANTS_PASS2_TIMEOUT": "90"})
        self.assertEqual(args2[args2.index("--timeout") + 1], "90")

    def test_a_timeout_given_by_the_caller_should_replace_the_wait_of_the_second_pass(self) -> None:
        for given in (
            ["--timeout", "30"],
            ["--timeout=30"],
            ["-t", "30"],
            ["--timeout-multiplier", "3"],
        ):
            with self.subTest(given=given):
                args2 = self.pass2_args(*given)
                self.assertEqual(sum(a.startswith(("--timeout", "-t")) for a in args2), 1)
                self.assertNotIn("600", args2)

    LIB = "crates/sisuc/src/generate/expr.rs:12:5: replace a with b"
    SUITE = "crates/sisuc/src/lib.rs:7:9: replace c with d"

    def log_text(self):
        with open(self.log, encoding="utf-8") as f:
            return f.read()

    def lines(self, name):
        with open(os.path.join(self.repo, name), encoding="utf-8") as f:
            return f.read().splitlines()

    def two_groups(self, *args, **fake):
        self.rust_commit()
        fake = {"missed_names": f"{self.LIB}\n{self.SUITE}", "missed": 2, "exit": 2, **fake}
        return self.mutate(*args, **fake)

    def test_a_missed_mutant_of_a_lib_only_file_should_be_tested_again_with_the_unit_tests_only(
        self,
    ) -> None:
        run = self.two_groups(
            lib_caught=1, pass2_caught=1, pass2_missed=0, lib_missed=0, pass2_exit=0, lib_exit=0
        )
        self.assertEqual(run.returncode, 0)
        lib = self.lines("lines_lib")
        self.assertEqual(lib[-2:], ["--", "--lib"])
        self.assertEqual([a for a in lib if a.startswith("^")], [f"^{escaped(self.LIB)}$"])
        self.assertEqual(lib[lib.index("--timeout") + 1], "600")
        suite = self.lines("lines2")
        self.assertNotIn("--lib", suite)
        self.assertEqual([a for a in suite if a.startswith("^")], [f"^{escaped(self.SUITE)}$"])
        self.assertEqual(suite[suite.index("--timeout") + 1], "600")
        self.assertEqual(
            self.last_line(),
            f"mutation-test: PASS: 2 caught, 0 unviable, 0 missed, 0 timeouts (HEAD {self.head()})",
        )

    def test_the_second_pass_should_say_which_group_it_runs_and_with_which_tests(self) -> None:
        self.two_groups(
            lib_caught=1, pass2_caught=1, pass2_missed=0, lib_missed=0, pass2_exit=0, lib_exit=0
        )
        log = self.log_text()
        self.assertIn(
            "mutation-test: pass 2, lib-only group: 1 mutants, tests: the unit tests (--lib)\n", log
        )
        self.assertIn(
            "mutation-test: pass 2, whole-suite group: 1 mutants, tests: the whole suite\n", log
        )

    def test_a_second_pass_with_lib_only_mutants_alone_should_not_run_the_whole_suite(self) -> None:
        self.rust_commit()
        run = self.mutate(
            missed_names=self.LIB, missed=1, exit=2, lib_caught=1, lib_missed=0, lib_exit=0
        )
        self.assertEqual(run.returncode, 0)
        self.assertTrue(self.seen("args_lib"))
        self.assertFalse(self.seen("args2"))
        self.assertNotIn("whole-suite", self.log_text())

    def test_a_second_pass_with_whole_suite_mutants_alone_should_not_run_the_unit_tests_again(
        self,
    ) -> None:
        self.rust_commit()
        run = self.mutate(
            missed_names=self.SUITE, missed=1, exit=2, pass2_caught=1, pass2_missed=0, pass2_exit=0
        )
        self.assertEqual(run.returncode, 0)
        self.assertTrue(self.seen("args2"))
        self.assertFalse(self.seen("args_lib"))
        self.assertNotIn("lib-only", self.log_text())

    def test_a_timeout_of_the_first_pass_should_go_to_the_group_of_its_file(self) -> None:
        self.rust_commit()
        self.mutate(
            timeout_names=self.LIB, timeout=1, exit=3, lib_caught=1, lib_timeout=0, lib_exit=0
        )
        self.assertTrue(self.seen("args_lib"))
        self.assertFalse(self.seen("args2"))

    def test_the_counts_of_three_passes_should_add_up(self) -> None:
        run = self.two_groups(
            caught=3,
            unviable=1,
            lib_caught=1,
            lib_missed=0,
            lib_exit=0,
            pass2_caught=0,
            pass2_missed=1,
            pass2_exit=2,
        )
        self.assertEqual(run.returncode, 1)
        self.assertEqual(
            self.last_line(),
            f"mutation-test: FAIL: 4 caught, 1 unviable, 1 missed, 0 timeouts (HEAD {self.head()})",
        )

    def test_a_group_should_not_count_a_mutant_that_was_not_in_its_list(self) -> None:
        # cargo mutants runs every "delete field" mutant of the diff in each group, whatever --re says.
        run = self.two_groups(
            caught=3,
            lib_caught=1,
            lib_missed=0,
            lib_exit=0,
            pass2_caught=1,
            pass2_missed=0,
            pass2_exit=0,
            outside_caught=2,
        )
        self.assertEqual(run.returncode, 0)
        self.assertEqual(
            self.last_line(),
            f"mutation-test: PASS: 5 caught, 0 unviable, 0 missed, 0 timeouts (HEAD {self.head()})",
        )

    def test_a_group_should_ignore_a_missed_mutant_that_was_not_in_its_list_and_the_exit_status_it_gives(
        self,
    ) -> None:
        run = self.two_groups(
            lib_caught=1,
            lib_missed=0,
            lib_exit=2,
            pass2_caught=1,
            pass2_missed=0,
            pass2_exit=2,
            outside_missed=1,
        )
        self.assertEqual(run.returncode, 0)
        self.assertEqual(
            self.last_line(),
            f"mutation-test: PASS: 2 caught, 0 unviable, 0 missed, 0 timeouts (HEAD {self.head()})",
        )

    def test_a_missed_mutant_in_the_list_should_still_fail_beside_a_mutant_that_was_not(
        self,
    ) -> None:
        run = self.two_groups(
            lib_caught=0,
            lib_missed=1,
            lib_exit=2,
            pass2_caught=1,
            pass2_missed=0,
            pass2_exit=0,
            outside_caught=1,
        )
        self.assertEqual(run.returncode, 1)
        self.assertEqual(
            self.last_line(),
            f"mutation-test: FAIL: 1 caught, 0 unviable, 1 missed, 0 timeouts (HEAD {self.head()})",
        )

    def test_a_group_that_tests_fewer_mutants_than_it_was_given_should_fail(self) -> None:
        run = self.two_groups(
            lib_caught=0, lib_missed=0, lib_exit=0, pass2_caught=1, pass2_missed=0, pass2_exit=0
        )
        self.assertEqual(run.returncode, 1)
        self.assertEqual(
            self.last_line(),
            "mutation-test: FAIL: 1 caught, 0 unviable, 0 missed, 0 timeouts "
            f"(lib-only group tested 0 of 1 mutants) (HEAD {self.head()})",
        )

    def test_a_group_that_writes_no_result_should_fail_even_when_cargo_exits_zero(self) -> None:
        run = self.two_groups(group_write=0, pass2_exit=0, lib_exit=0)
        self.assertEqual(run.returncode, 1)
        self.assertIn(
            "lib-only group tested 0 of 1 mutants, whole-suite group tested 0 of 1 mutants",
            self.last_line(),
        )

    def test_a_missed_mutant_of_the_lib_only_group_should_fail_the_run(self) -> None:
        run = self.two_groups(
            lib_caught=0, lib_missed=1, lib_exit=2, pass2_caught=1, pass2_missed=0, pass2_exit=0
        )
        self.assertEqual(run.returncode, 1)
        self.assertEqual(
            self.last_line(),
            f"mutation-test: FAIL: 1 caught, 0 unviable, 1 missed, 0 timeouts (HEAD {self.head()})",
        )

    def test_a_failed_group_should_fail_the_run_and_name_its_exit_code(self) -> None:
        run = self.two_groups(
            lib_caught=1, lib_missed=0, lib_exit=0, pass2_caught=0, pass2_missed=0, pass2_exit=4
        )
        self.assertEqual(run.returncode, 1)
        self.assertIn("(cargo mutants exit 4, see the output above)", self.last_line())

    def test_the_second_pass_should_drop_the_regex_and_iterate_options_of_the_caller(self) -> None:
        given = [
            "--re",
            "x",
            "--re=y",
            "-F",
            "z",
            "-Fw",
            "--regex",
            "a",
            "--regex=b",
            "--examine-re",
            "c",
            "--examine-re=d",
            "--examine-regex",
            "e",
            "--examine-regex=f",
            "--iterate",
        ]
        self.two_groups(
            *given,
            "--file",
            "a.rs",
            "-j",
            "3",
            lib_missed=0,
            lib_exit=0,
            pass2_missed=0,
            pass2_exit=0,
        )
        first = self.lines("args")[0].split()
        for option in ("--re", "--iterate", "-F"):
            self.assertIn(option, first)
        for name in ("lines_lib", "lines2"):
            words = self.lines(name)
            for word in ("x", "y", "z", "w", "a", "b", "c", "d", "e", "f", "--iterate", "-Fw"):
                self.assertNotIn(
                    word, [w.removeprefix("--re=") for w in words if not w.startswith("^")]
                )
            self.assertEqual(
                [
                    w
                    for w in words
                    if w in ("--re", "-F", "--regex", "--examine-re", "--examine-regex")
                ],
                ["--re"],
            )
            self.assertEqual(words.count("--re"), 1)
            self.assertIn("--file", words)
            self.assertIn("-j", words)

    def test_a_name_with_regex_characters_should_be_escaped_and_anchored(self) -> None:
        name = r"crates/sisuc/src/generate/expr.rs:12:5: replace a.b|c[d]{1}(e)+f*g?h^i$j\k with <T>::new()"
        self.rust_commit()
        self.mutate(missed_names=name, missed=1, exit=2, lib_missed=0, lib_exit=0)
        pattern = [w for w in self.lines("lines_lib") if w.startswith("^")]
        self.assertEqual(len(pattern), 1)
        self.assertEqual(pattern[0], f"^{escaped(name)}$")
        self.assertTrue(re.fullmatch(pattern[0], name))

    def test_every_pass_should_ask_cargo_to_list_the_caught_mutants(self) -> None:
        self.two_groups(lib_missed=0, lib_exit=0, pass2_missed=0, pass2_exit=0)
        for name in ("args", "args2", "args_lib"):
            self.assertIn(" --caught", self.lines(name)[0])

    LIB_ONLY = [
        "crates/sisuc/src/fuzz.rs",
        "crates/sisuc/src/fuzz_program.rs",
        "crates/sisuc/src/generate/",
    ]

    def lib_only_list(self):
        with open(SCRIPT, encoding="utf-8") as f:
            text = f.read()
        return (
            re.search(r"^LIB_ONLY=\(\n(.*?)\n\)", text, re.MULTILINE | re.DOTALL).group(1).split()
        )

    def test_the_lib_only_list_should_be_the_fuzz_layer_of_the_architecture_test(self) -> None:
        # The architecture test fails when a module of an earlier layer uses a module of the last
        # layer outside test code, so only the unit tests and the `pub use` interface reach it.
        root = os.path.join(os.path.dirname(SCRIPT), "..", "crates", "sisuc")
        with open(os.path.join(root, "tests", "architecture.rs"), encoding="utf-8") as f:
            layers = re.search(r"const LAYERS:.*?\n\];", f.read(), re.DOTALL).group(0)
        last = re.findall(r'"(\w+)"', re.findall(r"&\[[^\[\]]*\],", layers)[-1])
        paths = []
        for module in last:
            flat = f"crates/sisuc/src/{module}.rs"
            paths.append(
                flat
                if os.path.exists(os.path.join(root, "..", "..", flat))
                else f"crates/sisuc/src/{module}/"
            )
        self.assertEqual(sorted(paths), sorted(self.lib_only_list()))

    def test_no_integration_test_and_no_binary_should_reach_the_lib_only_files(self) -> None:
        # lib.rs builds generate and fuzz_program only for tests and the `fuzz` feature, and no
        # package turns that feature on in the workspace. Its `pub use` of the two fuzz entry
        # points is the one way out; only the fuzz crate, which is not in the workspace, calls them.
        top = os.path.join(os.path.dirname(SCRIPT), "..")
        with open(os.path.join(top, "crates", "sisuc", "src", "lib.rs"), encoding="utf-8") as f:
            lib = f.read()
        exports = re.findall(r"^pub use (fuzz\w*|generate)::(\w+)", lib, re.MULTILINE)
        self.assertEqual(sorted(exports), [("fuzz", "front_end"), ("fuzz_program", "program")])
        self.assertIn('#[cfg(any(test, feature = "fuzz"))]\nmod generate;', lib)
        self.assertIn('#[cfg(any(test, feature = "fuzz"))]\nmod fuzz_program;', lib)
        entry = re.compile(r"fuzz_front_end|fuzz_program\b|\bsisuc::\{?[^;]*\b(generate|fuzz)\b")
        users = []
        for crate in sorted(glob.glob(os.path.join(top, "crates", "*"))):
            for where in ("tests", os.path.join("src", "main.rs"), "build.rs"):
                for path in glob.glob(
                    os.path.join(crate, where, "**", "*.rs"), recursive=True
                ) + glob.glob(os.path.join(crate, where)):
                    if os.path.isfile(path) and os.path.basename(path) != "architecture.rs":
                        with open(path, encoding="utf-8") as f:
                            if entry.search(f.read()):
                                users.append(os.path.relpath(path, top))
        self.assertEqual(users, [], "remove the file from LIB_ONLY in mutants.sh, or the use")
        # No workspace package turns the feature on.
        for manifest in glob.glob(os.path.join(top, "crates", "*", "Cargo.toml")):
            with open(manifest, encoding="utf-8") as f:
                text = f.read()
            self.assertNotRegex(text, r"sisuc[^\n]*features[^\n]*fuzz", manifest)

    def test_cargo_should_get_output_dot_to_beat_the_environment_and_a_config_file(self) -> None:
        self.rust_commit()
        self.mutate(log=False, env={"CARGO_MUTANTS_OUTPUT": "x"}, caught=1)
        with open(os.path.join(self.repo, "args"), encoding="utf-8") as f:
            self.assertIn(" --output .", f.read())

    def test_a_diff_with_more_mutants_than_the_limit_should_fail_before_a_pass(self) -> None:
        self.rust_commit()
        run = self.mutate(list=301, caught=1)
        self.assertEqual(run.returncode, 1)
        self.assertEqual(
            self.last_line(),
            "mutation-test: FAIL: 301 mutants, more than 300; "
            "test a part with --file, or set MUTANTS_MAX=301",
        )
        self.assertFalse(self.seen("seen"))

    def test_a_diff_at_the_limit_should_run(self) -> None:
        self.rust_commit()
        self.assertEqual(self.mutate(list=300, caught=1).returncode, 0)
        self.assertTrue(self.seen("args"))

    def test_mutants_max_should_raise_the_limit(self) -> None:
        self.rust_commit()
        run = self.mutate(env={"MUTANTS_MAX": "301"}, list=301, caught=1)
        self.assertEqual(run.returncode, 0)
        self.assertTrue(self.seen("args"))


if __name__ == "__main__":
    unittest.main()
