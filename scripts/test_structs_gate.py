"""Unit tests for structs_gate.py.

Run them with: python3 -m unittest discover -s scripts -p 'test_structs_gate.py'.

Each test runs `main` on a fake `bench/structs/` directory and a PATH that holds stub `sisu` and
`valgrind` scripts. The stub `sisu` logs its arguments to `log`. It fails when they are not the
ones that the script must give. The stub `valgrind` logs its arguments and reads the first word of
standard input (`1x` or `4x`, the text of the fake input). It prints the count that the test gave
for the executable, the mode and the size. No test starts a real compiler or valgrind.
"""

import contextlib
import io
import os
import stat
import tempfile
import unittest
from collections.abc import Iterator
from pathlib import Path
from typing import override
from unittest import mock

import structs_gate as sg

# The counts of a program: {"name mode": (count at 1x, count at 4x)}. The name is
# `<program>-<version>`, and the mode is `-` for a program that has none.
Counts = dict[str, tuple[int, int]]


def script(path: Path, text: str) -> None:
    path.write_text("#!/bin/sh\n" + text)
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


def count_lines(counts: Counts) -> Iterator[str]:
    """Yield the lines `name mode size count` of the stub's table."""
    for key, (one, four) in counts.items():
        name, _, mode = key.partition(" ")
        mode = mode or "-"
        yield f"{name} {mode} 1x {one}\n{name} {mode} 4x {four}\n"


def stubs(tmp: Path, counts: Counts) -> str:
    """Write the stub `sisu` and `valgrind` in `tmp/bin`, and return that PATH."""
    bin_dir = tmp / "bin"
    bin_dir.mkdir()
    log = tmp / "log"
    table = tmp / "counts.txt"
    table.write_text("".join(count_lines(counts)))
    bad = 'echo "stub $0: wrong arguments: $*" >&2; exit 2'
    script(
        bin_dir / "sisu",
        f'echo "sisu $*" >> {log}\n'
        '[ $# -eq 8 ] && [ "$1 $2 $3 $4 $5 $6" = "build --allocator system --codegen-units 1 -o" ] '
        f"|| {{ {bad}; }}\n"
        # The executable is a stub too, so that valgrind can name it.
        '/bin/cp "$0" "$7"\n',
    )
    script(
        bin_dir / "valgrind",
        f'echo "valgrind $*" >> {log}\n'
        '[ "$1 $2" = "--tool=callgrind --callgrind-out-file=/dev/null" ] '
        f"|| {{ {bad}; }}\n"
        'name="${3##*/}"; mode="${4:--}"\n'
        "read -r size\n"
        f'n=$(/usr/bin/awk -v k="$name $mode $size" \'$1 " " $2 " " $3 == k {{ print $4 }}\' {table})\n'
        # A key with no count is a run that ends with exit code 0 and no `Collected` line.
        '[ -z "$n" ] || echo "==1== Collected : $n" >&2\n',
    )
    return str(bin_dir)


class GateTest(unittest.TestCase):
    @override
    def setUp(self) -> None:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(tmp.name)
        self.bench = self.tmp / "bench"
        self.out = self.tmp / "out"

    def run_gate(self, program: str, counts: Counts, *argv: str) -> tuple[int, str, str]:
        """Run `main` on one fake program; return the exit code, standard output and error."""
        directory = self.bench / program
        directory.mkdir(parents=True)
        for name in ("struct.sisu", "class.sisu"):
            (directory / name).write_text("fn main() {}\n")
        (directory / "input-1x.txt").write_text("1x\n")
        (directory / "input-4x.txt").write_text("4x\n")
        path = stubs(self.tmp, counts)
        out, err = io.StringIO(), io.StringIO()
        with (
            mock.patch.dict(os.environ, {"PATH": path}),
            contextlib.redirect_stdout(out),
            contextlib.redirect_stderr(err),
        ):
            code = sg.main(
                [
                    "--programs",
                    program,
                    "--bench",
                    str(self.bench),
                    "--sisu",
                    f"{path}/sisu",
                    "--out",
                    str(self.out),
                    *argv,
                ]
            )
        return code, out.getvalue(), err.getvalue()

    def log(self) -> str:
        return (self.tmp / "log").read_text()

    def test_gate_should_pass_a_program_at_the_ratio_limit(self) -> None:
        """GIVEN a program whose struct version has 100 instructions at 1x and 500 at 4x
        WHEN `scripts/structs_gate.py` runs it
        THEN the exit code is 0 and the row says pass
        """
        counts = {"tree-class": (100, 400), "tree-struct": (100, 500)}
        code, out, err = self.run_gate("tree", counts)
        self.assertEqual((code, err), (0, ""))
        self.assertRegex(out, r"tree\s+struct\s+100\s+500\s+5\.00\s+pass")

    def test_gate_should_fail_a_program_above_the_ratio_limit(self) -> None:
        """GIVEN a program whose struct version is above 5x and whose class version is below 5x
        WHEN `scripts/structs_gate.py` runs it
        THEN the exit code is 1 and the row says FAIL
        """
        counts = {"tree-class": (100, 400), "tree-struct": (100, 501)}
        code, out, _ = self.run_gate("tree", counts)
        self.assertEqual(code, 1)
        self.assertRegex(out, r"tree\s+struct\s+100\s+501\s+5\.01\s+FAIL")

    def test_gate_should_excuse_a_program_whose_class_version_is_above_the_limit(self) -> None:
        """GIVEN a program whose struct version and class version are at or above 5x
        WHEN `scripts/structs_gate.py` runs it
        THEN the exit code is 0 and the row says that the class version excuses it
        """
        counts = {"tree-class": (100, 500), "tree-struct": (100, 600)}
        code, out, _ = self.run_gate("tree", counts)
        self.assertEqual(code, 0)
        self.assertRegex(out, r"tree\s+struct\s+100\s+600\s+6\.00\s+excused")

    def test_gate_should_fail_an_observer_above_the_instruction_limit(self) -> None:
        """GIVEN an observer that has 100 000 001 instructions at 20 000 events
        WHEN `scripts/structs_gate.py` runs it
        THEN the exit code is 1 and the row says FAIL
        """
        counts = {
            "observer-class": (25_000_000, 100_000_000),
            "observer-struct": (25_000_000, 100_000_001),
        }
        code, out, _ = self.run_gate("observer", counts)
        self.assertEqual(code, 1)
        self.assertRegex(out, r"observer\s+struct\s+.*FAIL")

    def test_gate_should_not_excuse_an_observer_above_the_instruction_limit(self) -> None:
        """GIVEN an observer above 100 M instructions whose class version is above 5x
        WHEN `scripts/structs_gate.py` runs it
        THEN the exit code is 1, because the class version does not bear on the limit
        """
        counts = {
            "observer-class": (25_000_000, 200_000_000),
            "observer-struct": (25_000_000, 150_000_000),
        }
        code, out, _ = self.run_gate("observer", counts)
        self.assertEqual(code, 1)
        self.assertRegex(out, r"observer\s+struct\s+.*FAIL")

    def test_gate_should_pass_an_observer_at_the_instruction_limit(self) -> None:
        """GIVEN an observer that has 100 000 000 instructions at 20 000 events
        WHEN `scripts/structs_gate.py` runs it
        THEN the exit code is 0
        """
        counts = {
            "observer-class": (25_000_000, 100_000_000),
            "observer-struct": (25_000_000, 100_000_000),
        }
        code, _, _ = self.run_gate("observer", counts)
        self.assertEqual(code, 0)

    def test_gate_should_pass_a_struct_version_at_3x_when_the_class_version_scales(self) -> None:
        """GIVEN a class version at 4x and a struct version at 3.00x
        WHEN `scripts/structs_gate.py` runs them
        THEN the exit code is 0, because a struct version has no lower limit
        """
        counts = {"tree-class": (100, 400), "tree-struct": (100, 300)}
        code, out, _ = self.run_gate("tree", counts)
        self.assertEqual(code, 0, out)
        self.assertRegex(out, r"tree\s+struct\s+100\s+300\s+3\.00\s+pass")

    def test_gate_should_pass_a_struct_version_at_2_65x_when_the_class_version_scales(self) -> None:
        """GIVEN a class version at 3.28x and an observer struct version at 2.65x
        WHEN `scripts/structs_gate.py` runs them
        THEN the exit code is 0, because a struct version has no lower limit
        """
        counts = {
            "observer-class": (1_017_959, 3_334_512),
            "observer-struct": (547_000, 1_447_000),
        }
        code, out, _ = self.run_gate("observer", counts)
        self.assertEqual(code, 0, out)
        self.assertRegex(out, r"observer\s+struct\s+547,000\s+1,447,000\s+2\.65\s+pass")

    def test_gate_should_fail_a_class_version_that_does_not_grow_with_the_input(self) -> None:
        """GIVEN a class version with 100 instructions at 1x and 300 at 4x
        WHEN `scripts/structs_gate.py` runs it
        THEN the exit code is 1 and the class row says that the input does not scale
        """
        counts = {"tree-class": (100, 300), "tree-struct": (100, 400)}
        code, out, _ = self.run_gate("tree", counts)
        self.assertEqual(code, 1)
        self.assertRegex(out, r"tree\s+class\s+100\s+300\s+3\.00\s+FAIL: .*input")

    def test_class_only_should_fail_a_class_version_that_does_not_grow_with_the_input(self) -> None:
        """GIVEN a class version with 100 instructions at 1x and 110 at 4x
        WHEN `scripts/structs_gate.py --class-only` runs it
        THEN the exit code is 1 and the class row says that the input does not scale
        """
        code, out, _ = self.run_gate("tree", {"tree-class": (100, 110)}, "--class-only")
        self.assertEqual(code, 1)
        self.assertRegex(out, r"tree\s+class\s+100\s+110\s+1\.10\s+FAIL: .*input")

    def test_gate_should_measure_the_parser_as_the_dump_less_lexing(self) -> None:
        """GIVEN struct counts `dump` 110 and 480 and `lex` 100 and 400, and a class ratio below 5x
        WHEN `scripts/structs_gate.py` runs the parser
        THEN the parser part is 10 and 80, which is 8x, and the exit code is 1
        """
        counts = {
            "parser-struct dump": (110, 480),
            "parser-struct lex": (100, 400),
            "parser-class dump": (150, 600),
            "parser-class lex": (100, 400),
        }
        code, out, _ = self.run_gate("parser", counts)
        self.assertEqual(code, 1)
        self.assertRegex(out, r"parser\s+struct\s+10\s+80\s+8\.00\s+FAIL")

    def test_gate_should_build_with_the_system_allocator_and_one_unit(self) -> None:
        """GIVEN a program
        WHEN `scripts/structs_gate.py` runs it
        THEN each build has `--allocator system --codegen-units 1`
        """
        counts = {"tree-class": (100, 400), "tree-struct": (100, 400)}
        code, _, _ = self.run_gate("tree", counts)
        self.assertEqual(code, 0)
        builds = [line for line in self.log().splitlines() if line.startswith("sisu")]
        self.assertEqual(len(builds), 2)
        for line in builds:
            self.assertIn("--allocator system --codegen-units 1", line)

    def test_class_only_should_never_build_a_struct_version(self) -> None:
        """GIVEN a program whose struct version would fail the gate
        WHEN `scripts/structs_gate.py --class-only` runs it
        THEN no command names `struct.sisu`, the row is the class version, and the exit code is 0
        """
        counts = {"tree-class": (100, 900)}
        code, out, _ = self.run_gate("tree", counts, "--class-only")
        self.assertEqual(code, 0)
        self.assertNotIn("struct.sisu", self.log())
        self.assertIn("class.sisu", self.log())
        self.assertRegex(out, r"tree\s+class\s+100\s+900\s+9\.00")
        self.assertNotIn("struct", out)

    def test_gate_should_stop_when_callgrind_gives_no_count(self) -> None:
        """GIVEN a valgrind run that exits with code 0 and prints no `Collected` line
        WHEN `scripts/structs_gate.py` runs it
        THEN the exit code is 1 and the error says that callgrind gave no instruction count
        """
        code, _, err = self.run_gate("tree", {})
        self.assertEqual(code, 1)
        self.assertIn("no instruction count", err)

    def test_gate_should_know_each_program_of_the_bench_directory(self) -> None:
        """GIVEN the programs of `bench/structs/`
        WHEN the script lists its programs
        THEN each directory of `bench/structs/` is one of them
        """
        found = {p.name for p in sg.BENCH.iterdir() if p.is_dir()}
        self.assertEqual(found, set(sg.PROGRAMS))


if __name__ == "__main__":
    unittest.main()
