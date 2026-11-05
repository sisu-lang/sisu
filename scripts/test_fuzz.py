"""Unit tests for the summary of fuzz.sh: python3 -m unittest discover -s scripts -p 'test_*.py'.

Each test runs the script in a small repository with a fake `cargo` on PATH, so no fuzzer runs.
"""
import os
import shutil
import stat
import subprocess
import tempfile
import unittest

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'fuzz.sh')
# A pre-commit hook runs these tests with GIT_DIR and its kin set to the outer repository.
ENV = {k: v for k, v in os.environ.items() if not k.startswith('GIT_')}
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
# directory argument: its name, its file count and how many of its files hold 4096 bytes. It
# does so while the directory exists, since fuzz.sh deletes its seeds on exit.
PROGRAM_CARGO = """#!/usr/bin/env bash
if [[ $1 == fuzz && $2 == run ]]; then
  for arg in "$@"; do
    if [[ -d $arg ]]; then
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
        os.mkdir(os.path.join(self.repo, 'scripts'))
        shutil.copy(SCRIPT, os.path.join(self.repo, 'scripts'))
        os.makedirs(os.path.join(self.repo, 'crates', 'sisuc', 'tests'))
        os.makedirs(os.path.join(self.repo, 'fuzz'))
        self.bin = os.path.join(self.repo, 'bin')
        os.mkdir(self.bin)
        cargo = os.path.join(self.bin, 'cargo')
        with open(cargo, 'w', encoding='utf-8') as f:
            f.write(FAKE_CARGO)
        os.chmod(cargo, os.stat(cargo).st_mode | stat.S_IXUSR)
        git = ['git', '-C', self.repo, '-c', 'user.name=t', '-c', 'user.email=t@t']
        subprocess.run(git + ['init', '-q'], check=True, env=ENV)
        subprocess.run(git + ['commit', '-q', '--allow-empty', '-m', 'base'], check=True, env=ENV)

    def summary(self, replay):
        env = dict(ENV, PATH=f'{self.bin}:{ENV["PATH"]}', FAKE_REPLAY=replay)
        run = subprocess.run(['bash', 'scripts/fuzz.sh', '1'], cwd=self.repo, capture_output=True,
                             env=env, timeout=60)
        self.assertEqual(run.returncode, 1, run.stderr)
        with open(os.path.join(self.repo, 'fuzz', 'artifacts', 'summary.md'), encoding='utf-8') as f:
            return f.read()

    def test_a_crash_with_a_panic_message_should_show_its_location_and_first_message_line(self):
        # The replay output holds binary bytes, and the panic line comes after some of them.
        text = self.summary(r"\377\000\001\200 noise\n\000thread '<unnamed>' (7) panicked at "
                            r"crates/sisuc/src/fuzz.rs:70:9:\nBraceOnNewLine: a safe fix removes NoMember too:\n"
                            r"second line\n")
        self.assertIn('  Panic at `crates/sisuc/src/fuzz.rs:70:9`: `BraceOnNewLine: a safe fix removes NoMember too:`\n', text)
        self.assertNotIn('second line', text)
        self.assertIn('Its replay is at the end of `fuzz.log`.', text)
        self.assertNotIn('stack overflow', text)

    def test_a_panic_message_should_lose_its_backticks_and_control_characters(self):
        text = self.summary(r"panicked at a.rs:1:2:\nbad `code` \001here\n")
        self.assertIn('  Panic at `a.rs:1:2`: `bad code here`\n', text)

    def test_binary_bytes_before_the_panic_on_the_same_line_should_not_reach_the_location(self):
        text = self.summary(r"garbage \377\376 panicked at a.rs:1:2:\nhello\n")
        self.assertIn('  Panic at `a.rs:1:2`: `hello`\n', text)

    def test_a_long_message_should_be_cut_in_whole_characters(self):
        text = self.summary(r"panicked at a.rs:1:2:\na" + r"\303\251" * 200 + r"\n")
        line = next(line for line in text.splitlines() if 'Panic at' in line)
        self.assertTrue(line.startswith('  Panic at `a.rs:1:2`: `a'))
        self.assertLessEqual(len(line.encode()), 400)

    def test_a_mention_in_a_message_should_stay_in_a_code_span(self):
        text = self.summary(r"panicked at a.rs:1:2:\nhello @octocat #12\n")
        self.assertIn('`hello @octocat #12`', text)

    def test_a_panic_without_a_message_line_should_show_the_location_alone(self):
        text = self.summary(r"panicked at a.rs:1:2:")
        self.assertIn('  Panic at `a.rs:1:2`\n', text)

    def test_a_crash_without_a_panic_message_should_keep_the_note_about_a_fatal_signal(self):
        text = self.summary(r"\377\000 AddressSanitizer: stack-overflow\n")
        self.assertNotIn('Panic at', text)
        self.assertIn('is a stack overflow or another fatal signal', text)


class ProgramSeeds(unittest.TestCase):
    def test_program_run_should_get_the_seeds(self):
        with tempfile.TemporaryDirectory() as repo:
            scripts = os.path.join(repo, 'scripts')
            os.mkdir(scripts)
            for name in ('fuzz.sh', 'fuzz_seeds.py'):
                shutil.copy(os.path.join(os.path.dirname(SCRIPT), name), scripts)
            os.makedirs(os.path.join(repo, 'fuzz'))
            bin_dir = os.path.join(repo, 'bin')
            os.mkdir(bin_dir)
            cargo = os.path.join(bin_dir, 'cargo')
            with open(cargo, 'w', encoding='utf-8') as f:
                f.write(PROGRAM_CARGO)
            os.chmod(cargo, os.stat(cargo).st_mode | stat.S_IXUSR)
            git = ['git', '-C', repo, '-c', 'user.name=t', '-c', 'user.email=t@t']
            subprocess.run(git + ['init', '-q'], check=True, env=ENV)
            subprocess.run(git + ['commit', '-q', '--allow-empty', '-m', 'base'], check=True, env=ENV)
            seen = os.path.join(repo, 'seen.txt')
            env = dict(ENV, PATH=f'{bin_dir}:{ENV["PATH"]}', FAKE_SEEN=seen)
            run = subprocess.run(['bash', 'scripts/fuzz.sh', '--target', 'program', '1'], cwd=repo,
                                 capture_output=True, env=env, timeout=60)
            self.assertEqual(run.returncode, 0, run.stderr)
            with open(seen, encoding='utf-8') as f:
                lines = f.read().splitlines()
            # The corpus is empty. The seeds directory comes second.
            self.assertEqual(lines[0], 'program 0 0')
            self.assertEqual(len(lines), 2, lines)
            self.assertTrue(lines[1].endswith(' 128 128'), lines[1])


if __name__ == '__main__':
    unittest.main()
