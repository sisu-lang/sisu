"""Unit tests for count_instructions.py.

Run: python3 -m unittest discover -s scripts -p 'test_count_instructions.py'.

A test that runs `main` puts stub `git`, `tar`, `cargo` and `valgrind` scripts in a directory, and
makes that directory the whole PATH. Each stub logs its arguments to `log`. No test builds or runs
a real compiler.
"""

import contextlib
import io
import os
import shutil
import stat
import tempfile
import unittest
from pathlib import Path
from typing import override
from unittest import mock

import count_instructions as ci
from compile_speed import Fail


def script(path: Path, text: str) -> None:
    path.write_text("#!/bin/sh\n" + text)
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


class CountInstructionsTest(unittest.TestCase):
    @override
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        self.log = self.tmp / "log"

    def stubs(self, *, base_build_fails: bool = False) -> str:
        """Write the stubs and return their PATH.

        The stub `valgrind` gives 2000 instructions for the `sisu` of the base and 1000 for the
        other. With `base_build_fails`, the stub `cargo` fails when it builds into the base target.
        """
        bin_dir = self.tmp / "bin"
        bin_dir.mkdir()
        log = self.log
        script(
            bin_dir / "git",
            f'echo "git $*" >> {log}\n'
            'case "$3" in\n'
            "  merge-base) echo 0123456789abcdef;;\n"
            "  rev-parse) echo abc1234;;\n"
            '  archive) echo tar > "${4#--output=}";;\n'
            "esac\n",
        )
        # The stub `tar` puts a file in the export, so a test can see that nothing is left.
        script(bin_dir / "tar", f'echo "tar $*" >> {log}\n/bin/touch "$6/Cargo.toml"\n')
        fail = (
            '[ "$CARGO_TARGET_DIR" = "${CARGO_TARGET_DIR%count-base}" ] || exit 3\n'
            if base_build_fails
            else ""
        )
        script(
            bin_dir / "cargo",
            f'echo "cargo $* in $(pwd) target=${{CARGO_TARGET_DIR:-}} '
            f'llvm=${{LLVM_SYS_231_PREFIX:-}}" >> {log}\n{fail}',
        )
        script(
            bin_dir / "valgrind",
            f'echo "valgrind $*" >> {log}\n'
            'case "$3" in */count-base/*) n=2000;; *) n=1000;; esac\n'
            'echo "==7== Collected : $n" >&2\n',
        )
        return str(bin_dir)

    def run_main(self, *argv: str, path: str) -> tuple[int, str, str]:
        out, err = io.StringIO(), io.StringIO()
        env = {"PATH": path}
        with (
            mock.patch.dict(os.environ, env),
            contextlib.redirect_stdout(out),
            contextlib.redirect_stderr(err),
        ):
            code = ci.main(list(argv))
        return code, out.getvalue(), err.getvalue()

    def test_total_should_read_the_last_collected_line(self) -> None:
        """GIVEN the stderr of a callgrind run
        WHEN `total` reads it
        THEN it gives the count of the last `Collected` line, and raises Fail when there is none
        """
        self.assertEqual(ci.total("==1== Events    : Ir\n==1== Collected : 617810176\n"), 617810176)
        with self.assertRaises(Fail):
            ci.total("==1== Events    : Ir\n")

    def test_change_should_give_a_percent_and_a_factor(self) -> None:
        """GIVEN two counts
        WHEN `change` compares them
        THEN it gives the signed percent and how many times fewer or more
        """
        self.assertEqual(ci.change(2000, 1000), "-50.00% (2.0x fewer)")
        self.assertEqual(ci.change(1000, 1250), "+25.00% (1.2x more)")
        self.assertEqual(ci.change(0, 1000), "no change can be computed")

    def test_main_should_count_the_base_and_the_working_tree(self) -> None:
        """GIVEN stub tools
        WHEN `count_instructions.py -- build x.sisu` runs
        THEN it exports the merge base and builds the export into target/count-base
        AND it builds the working tree into target/
        AND it runs the same arguments under callgrind with each `sisu`, and prints both totals
        AND it removes the export, and adds no worktree
        """
        code, out, err = self.run_main("--", "build", "x.sisu", path=self.stubs())
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(
            out,
            "base abc1234: 2,000\nworking tree: 1,000\nchange: -50.00% (2.0x fewer)\n",
        )
        rows = self.log.read_text().splitlines()
        exported = next(r for r in rows if " archive " in r)
        tar = exported.split()[-2].removeprefix("--output=")
        self.assertTrue(exported.endswith(" abc1234"))
        tree = str(Path(tar).parent / "base")
        self.assertIn(f"tar -x -m -f {tar} -C {tree}", rows)
        builds = [r for r in rows if r.startswith("cargo build ")]
        self.assertEqual(len(builds), 2)
        self.assertIn(f" in {tree} target={ci.BASE_TARGET} llvm=", builds[0])
        self.assertNotIn("llvm= ", builds[0] + " ")
        self.assertIn(f" in {ci.ROOT} target= ", builds[1])
        profiled = [r for r in rows if r.startswith("valgrind ")]
        self.assertEqual(len(profiled), 2)
        self.assertTrue(profiled[0].endswith(f"{ci.BASE_TARGET}/release/sisu build x.sisu"))
        self.assertTrue(profiled[1].endswith(f"{ci.ROOT}/target/release/sisu build x.sisu"))
        self.assertFalse(Path(tar).parent.exists())
        self.assertFalse(any(" worktree " in r for r in rows))

    def test_main_should_remove_the_export_when_the_base_build_fails(self) -> None:
        """GIVEN a stub cargo that fails on the base
        WHEN `count_instructions.py --base abc -- check x.sisu` runs
        THEN it exits with code 1 and names the failed build
        AND it removes the export, and runs nothing under callgrind
        """
        code, out, err = self.run_main(
            "--base", "abc", "--", "check", "x.sisu", path=self.stubs(base_build_fails=True)
        )
        self.assertEqual((code, out), (1, ""))
        self.assertTrue(err.startswith("count_instructions: build abc1234: "))
        rows = self.log.read_text().splitlines()
        self.assertFalse(any(" merge-base " in r for r in rows))
        tar = next(r for r in rows if " archive " in r).split()[-2].removeprefix("--output=")
        self.assertFalse(Path(tar).parent.exists())
        self.assertFalse(any(r.startswith("valgrind ") for r in rows))


if __name__ == "__main__":
    unittest.main()
