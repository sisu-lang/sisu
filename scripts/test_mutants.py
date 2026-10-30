"""Unit tests for mutants.sh: python3 -m unittest discover -s scripts -p 'test_*.py'.

Each test runs the script in a small repository with a fake `cargo` on PATH, so no mutant runs.
"""
import fcntl
import os
import shutil
import stat
import subprocess
import tempfile
import unittest

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'mutants.sh')
# A pre-commit hook runs these tests with GIT_DIR and its kin set to the outer repository.
# A caller's LOG and CARGO_MUTANTS_JOBS would change what the script does.
ENV = {k: v for k, v in os.environ.items()
       if not k.startswith('GIT_') and k not in ('LOG', 'CARGO_MUTANTS_JOBS')}
# The fake cargo mutants records its jobs, niceness and arguments, and whether an old
# mutants.out was there. Unless FAKE_WRITE is 0, it then writes a new mutants.out with the
# result files that the test names in FAKE_CAUGHT, FAKE_MISSED and FAKE_TIMEOUT. It exits
# with FAKE_EXIT. The sleep keeps lock.json strictly newer than the script's start mark.
FAKE_CARGO = """#!/usr/bin/env bash
echo "$CARGO_MUTANTS_JOBS $(nice)" >seen
echo "$*" >args
[[ -e mutants.out/missed.txt ]] && touch saw_old_out
sleep 0.05
[[ ${FAKE_WRITE:-1} == 0 ]] && exit "${FAKE_EXIT:-0}"
rm -rf mutants.out
mkdir -p mutants.out
touch mutants.out/lock.json
for kind in caught missed timeout; do
  var=FAKE_${kind^^}
  for ((i = 0; i < ${!var:-0}; i++)); do echo "mutant $i" >>mutants.out/$kind.txt; done
done
exit "${FAKE_EXIT:-0}"
"""


class Mutants(unittest.TestCase):
    def setUp(self):
        self.repo = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.repo)
        self.scripts = os.path.join(self.repo, 'scripts')
        os.mkdir(self.scripts)
        shutil.copy(SCRIPT, self.scripts)
        self.bin = os.path.join(self.repo, 'bin')
        os.mkdir(self.bin)
        cargo = os.path.join(self.bin, 'cargo')
        with open(cargo, 'w', encoding='utf-8') as f:
            f.write(FAKE_CARGO)
        os.chmod(cargo, os.stat(cargo).st_mode | stat.S_IXUSR)
        self.git = ['git', '-C', self.repo, '-c', 'user.name=t', '-c', 'user.email=t@t']
        subprocess.run(self.git[:3] + ['init', '-q'], check=True, env=ENV)
        self.commit('base')
        self.log = os.path.join(self.repo, 'mutation.log')

    def commit(self, message):
        subprocess.run(self.git + ['add', '-A'], check=True, env=ENV)
        subprocess.run(self.git + ['commit', '-q', '--allow-empty', '-m', message], check=True, env=ENV)

    def head(self):
        return subprocess.run(self.git + ['rev-parse', 'HEAD'], capture_output=True, text=True,
                              check=True, env=ENV).stdout.strip()

    def write_rust(self, text):
        with open(os.path.join(self.repo, 'a.rs'), 'w', encoding='utf-8') as f:
            f.write(text)

    def mutate(self, *args, log=True, env=None, **fake):
        env = dict(ENV, PATH=f'{self.bin}:{ENV["PATH"]}', TMPDIR=self.repo, **(env or {}),
                   **{f'FAKE_{k.upper()}': str(v) for k, v in fake.items()})
        if log:
            env['LOG'] = self.log
        return subprocess.run(['bash', 'scripts/mutants.sh', 'HEAD~1', *args], cwd=self.repo,
                              capture_output=True, text=True, env=env)

    def seen(self, name):
        return os.path.exists(os.path.join(self.repo, name))

    def last_line(self):
        with open(self.log, encoding='utf-8') as f:
            return f.read().splitlines()[-1]

    def rust_commit(self):
        self.write_rust('fn main() {}\n')
        self.commit('rust')

    def test_a_diff_without_rust_should_pass_without_running_cargo(self):
        self.commit('docs')
        run = self.mutate()
        self.assertEqual(run.returncode, 0)
        self.assertEqual(run.stdout, '')
        self.assertEqual(self.last_line(), f'mutation-test: PASS: no Rust changes since HEAD~1 (HEAD {self.head()})')
        self.assertFalse(self.seen('seen'))

    def test_a_second_run_should_fail_and_leave_the_first_runs_log_alone(self):
        with open(self.log, 'w', encoding='utf-8') as f:
            f.write('first run\n')
        os.mkdir(os.path.join(self.repo, 'target'))
        with open(os.path.join(self.repo, 'target', 'mutation-test.lock'), 'w') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            run = self.mutate()
        self.assertEqual(run.returncode, 1)
        self.assertIn('mutation-test: FAIL: another make mutation-test is running in this worktree', run.stdout)
        with open(self.log, encoding='utf-8') as f:
            self.assertEqual(f.read(), 'first run\n')

    def test_a_run_with_no_missed_mutant_should_pass_and_count_a_timeout_as_caught(self):
        self.rust_commit()
        run = self.mutate(caught=3, timeout=1, exit=3)
        self.assertEqual(run.returncode, 0)
        self.assertEqual(self.last_line(),
                         f'mutation-test: PASS: 3 caught, 0 unviable, 0 missed, 1 timeouts (HEAD {self.head()})')

    def test_a_missed_mutant_should_fail_even_with_a_timeout(self):
        self.rust_commit()
        run = self.mutate(caught=1, missed=2, timeout=1, exit=3)
        self.assertEqual(run.returncode, 1)
        self.assertEqual(self.last_line(),
                         f'mutation-test: FAIL: 1 caught, 0 unviable, 2 missed, 1 timeouts (HEAD {self.head()})')

    def test_a_failed_cargo_run_should_fail_and_name_its_exit_code(self):
        self.rust_commit()
        run = self.mutate(exit=4)
        self.assertEqual(run.returncode, 1)
        self.assertIn('(cargo mutants exit 4, see the output above)', self.last_line())

    def test_uncommitted_rust_should_fail_without_running_cargo(self):
        self.rust_commit()
        self.write_rust('fn main() { let _x = 1; }\n')
        run = self.mutate(caught=1)
        self.assertEqual(run.returncode, 1)
        self.assertEqual(self.last_line(), 'mutation-test: FAIL: uncommitted Rust changes; commit them, then run again')
        self.assertFalse(self.seen('seen'))

    def test_cargo_should_run_niced_with_two_jobs_by_default(self):
        self.rust_commit()
        self.mutate(log=False, caught=1)
        with open(os.path.join(self.repo, 'seen'), encoding='utf-8') as f:
            self.assertEqual(f.read().strip(), '2 19')

    def test_a_run_without_new_output_should_pass_and_leave_the_old_mutants_out_alone(self):
        self.rust_commit()
        self.mutate(missed=5, exit=2)
        run = self.mutate('--iterate', write=0)
        self.assertEqual(run.returncode, 0)
        self.assertEqual(self.last_line(), f'mutation-test: PASS: no mutant tested (HEAD {self.head()})')
        self.assertTrue(self.seen('saw_old_out'))
        self.assertTrue(self.seen('mutants.out/missed.txt'))

    def test_a_failed_run_without_new_output_should_fail_and_name_its_exit_code(self):
        self.rust_commit()
        run = self.mutate(write=0, exit=2)
        self.assertEqual(run.returncode, 1)
        self.assertEqual(self.last_line(), 'mutation-test: FAIL: no mutant tested (cargo mutants exit 2, '
                                           f'see the output above) (HEAD {self.head()})')

    def test_an_argument_that_tests_nothing_should_end_with_not_run_and_keep_the_exit_code(self):
        self.rust_commit()
        for arg in ('--list', '--list-files', '--check'):
            with self.subTest(arg):
                run = self.mutate(arg, write=0, exit=0)
                self.assertEqual(run.returncode, 0)
                self.assertEqual(self.last_line(), f'mutation-test: NOT RUN: {arg} tests no mutant')
                self.assertEqual(self.mutate(arg, write=0, exit=2).returncode, 2)

    def test_check_should_not_count_its_output_as_a_pass(self):
        self.rust_commit()
        run = self.mutate('--check', exit=0)
        self.assertEqual(run.returncode, 0)
        self.assertIn('NOT RUN', self.last_line())

    def test_an_argument_that_moves_the_output_should_fail_without_running_cargo(self):
        self.rust_commit()
        for args, env in ((('-o', 'x'), {}), (('--output=x',), {}), (('-d', 'x'), {}), (('--dir', 'x'), {}),
                          (('--manifest-path=x',), {})):
            with self.subTest(args=args, env=env):
                run = self.mutate(*args, env=env)
                self.assertEqual(run.returncode, 1)
                self.assertIn('mutation-test: FAIL:', self.last_line())
                self.assertNotIn('(HEAD', self.last_line())
                self.assertFalse(self.seen('seen'))

    def test_an_argument_after_double_dash_should_go_to_cargo_test(self):
        self.rust_commit()
        run = self.mutate('--', '-o', 'x', caught=1)
        self.assertEqual(run.returncode, 0)
        with open(os.path.join(self.repo, 'args'), encoding='utf-8') as f:
            self.assertTrue(f.read().strip().endswith('-- -o x'))

    def test_cargo_should_get_output_dot_to_beat_the_environment_and_a_config_file(self):
        self.rust_commit()
        self.mutate(log=False, env={'CARGO_MUTANTS_OUTPUT': 'x'}, caught=1)
        with open(os.path.join(self.repo, 'args'), encoding='utf-8') as f:
            self.assertIn(' --output .', f.read())


if __name__ == '__main__':
    unittest.main()
