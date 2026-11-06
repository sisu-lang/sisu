"""Tests for nightly_check.py and nightly_smoke.sh: python3 -m unittest discover -s scripts -p 'test_nightly_check.py'."""

import os
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path
from typing import override

import nightly_check as nc

SCRIPTS = Path(__file__).absolute().parent
CHECK = SCRIPTS / "nightly_check.py"
SMOKE = SCRIPTS / "nightly_smoke.sh"
NAMES = [
    "sisu/",
    "sisu/sisu",
    "sisu/libsisu_runtime_mimalloc.a",
    "sisu/libsisu_runtime_system.a",
    "sisu/LICENSE",
    "sisu/THIRD-PARTY-LICENSES.txt",
]

# Lines of `objdump -T` and `objdump -p` from the build machine, with their tabs.
STRONG = "0000000000000000      DF *UND*\t0000000000000000 (%s) f\n"
WEAK = "0000000000000000  w   DF *UND*\t0000000000000000 (%s) f\n"
NEEDED = "  NEEDED               %s\n"
FIXTURE_T = (
    "0000000000000000      DF *UND*\t0000000000000000 (GLIBC_2.34) __libc_start_main\n"
    "0000000000000000  w   DF *UND*\t0000000000000000 (GLIBC_2.39) pidfd_spawnp\n"
    "0000000000000000      DF *UND*\t0000000000000000 (GLIBCXX_3.4.21) "
    "_ZNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEE9_M_createERmm\n"
)
FIXTURE_P = NEEDED % "libm.so.6"
OK_T = STRONG % "GLIBC_2.34"
OK_P = NEEDED % "libc.so.6"


def minos_text(cmd: str, line: str) -> str:
    return f"Load command 9\n      cmd {cmd}\n  cmdsize 32\n{line}\n"


class Versions(unittest.TestCase):
    def test_version_should_want_the_picked_commit(self) -> None:
        text = "sisu 0.1.0\ncommit: %s\nhost: x\nllvm: 23.1.3\n"
        sha = "abc1234def"
        self.assertEqual(nc.version_problems(text % "abc1234", "", sha), [])
        weak = "version 'GLIBC_2.39' not found\n"
        for name, out, err in (
            ("another commit", text % "abc1235", ""),
            ("unknown commit", text % "unknown", ""),
            ("no commit line", "sisu 0.1.0\nhost: x\n", ""),
            ("stderr", text % "abc1234", weak),
        ):
            with self.subTest(name):
                self.assertTrue(nc.version_problems(out, err, sha))


class Linux(unittest.TestCase):
    def test_linux_should_pass_at_the_floor(self) -> None:
        t = (STRONG % "GLIBC_2.35") + (STRONG % "GLIBC_2.2.5") + (STRONG % "GLIBCXX_3.4.30")
        self.assertEqual(nc.linux_problems(t, FIXTURE_P), [])

    def test_linux_should_ignore_a_weak_glibc_reference(self) -> None:
        self.assertEqual(nc.linux_problems(FIXTURE_T, FIXTURE_P), [])
        self.assertEqual(nc.linux_problems(WEAK % "GLIBC_2.39", FIXTURE_P), [])

    def test_linux_should_fail_above_the_glibc_floor(self) -> None:
        problems = nc.linux_problems(OK_T + STRONG % "GLIBC_2.36", OK_P)
        self.assertTrue(any("2.36" in p for p in problems), problems)
        self.assertTrue(nc.linux_problems(OK_T + STRONG % "GLIBC_2.100", OK_P), "GLIBC_2.100")

    def test_linux_should_fail_above_the_glibcxx_floor(self) -> None:
        for version in ("GLIBCXX_3.4.31", "GLIBCXX_3.4.100"):
            with self.subTest(version):
                self.assertTrue(nc.linux_problems(OK_T + STRONG % version, OK_P))

    def test_linux_should_fail_on_a_library_off_the_list(self) -> None:
        problems = nc.linux_problems(OK_T, OK_P + NEEDED % "libfoo.so.1")
        self.assertTrue(any("libfoo.so.1" in p for p in problems), problems)
        for name in (
            "libm.so.6",
            "libz.so.1",
            "libzstd.so.1",
            "libstdc++.so.6",
            "libgcc_s.so.1",
            "libc.so.6",
            "ld-linux-x86-64.so.2",
            "ld-linux-aarch64.so.1",
        ):
            with self.subTest(name):
                self.assertEqual(nc.linux_problems(OK_T, NEEDED % name), [])
        for name in ("libzstd.so.2", "libc.so.999", "ld-linux-evil.so.9"):
            with self.subTest(name):
                self.assertTrue(nc.linux_problems(OK_T, NEEDED % name))

    def test_linux_should_fail_on_output_with_no_glibc_reference(self) -> None:
        self.assertTrue(nc.linux_problems("", OK_P))

    def test_linux_should_fail_on_output_with_no_needed_line(self) -> None:
        self.assertTrue(nc.linux_problems(OK_T, ""))


class MacOS(unittest.TestCase):
    def test_macos_links_should_allow_only_system_paths(self) -> None:
        head = "target/release/sisu:\n"
        lines = (
            "\t/usr/lib/libz.1.dylib (compatibility version 1.0.0, current version 1.3.1)\n"
            "\t/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation (x)\n"
        )
        self.assertEqual(nc.macos_link_problems(head + lines), [])
        bad = "\t/opt/homebrew/lib/libzstd.1.dylib (compatibility version 1.0.0)\n"
        problems = nc.macos_link_problems(head + lines + bad)
        self.assertTrue(any("/opt/homebrew/lib/libzstd.1.dylib" in p for p in problems), problems)
        # The file's own name is not a load path, even when it is not a system path.
        self.assertEqual(nc.macos_link_problems("/opt/work/sisu:\n" + lines), [])
        # A tool that prints no load path cannot pass.
        for name, out in (("empty", ""), ("header only", "x:\n")):
            with self.subTest(name):
                self.assertTrue(nc.macos_link_problems(out))

    def test_minos_should_fail_above_the_floor(self) -> None:
        build = "LC_BUILD_VERSION"
        for line in (
            (
                "    platform 1\n       minos 15.0\n         sdk 26.0\n    ntools 1\n"
                "     tool 3\n  version 1100.0"
            ),
            "       minos 11.0",
            "       minos 9.0",
        ):
            with self.subTest(line):
                self.assertEqual(nc.minos_problems("a.o", minos_text(build, line)), [])
        old = minos_text("LC_VERSION_MIN_MACOSX", "  version 11.0\n      sdk 11.0")
        self.assertEqual(nc.minos_problems("a.o", old), [])
        for minos in ("16.0", "15.1", "100.0"):
            with self.subTest(minos):
                problems = nc.minos_problems("b.o", minos_text(build, "       minos " + minos))
                self.assertTrue(any("b.o" in p for p in problems), problems)
        self.assertTrue(nc.minos_problems("c.o", "Load command 0\n      cmd LC_SEGMENT_64\n"))


class Tar(unittest.TestCase):
    def test_tar_should_want_exact_names(self) -> None:
        self.assertEqual(nc.tar_problems(NAMES), [])
        for name, names in (
            ("._ file", [*NAMES, "sisu/._sisu"]),
            ("no LICENSE", [n for n in NAMES if n != "sisu/LICENSE"]),
            ("no directory", [n for n in NAMES if n != "sisu/"]),
            ("extra", [*NAMES, "sisu/extra"]),
            ("duplicate", [*NAMES, "sisu/sisu"]),
        ):
            with self.subTest(name):
                self.assertTrue(nc.tar_problems(names))


class PackBase(unittest.TestCase):
    @override
    def setUp(self) -> None:
        self.root = Path(tempfile.mkdtemp(prefix="sisu-pack-"))
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)
        self.release = self.root / "release"
        self.out = self.root / "out" / "dist"
        self.third = self.root / "third-party.txt"
        self.release.mkdir(parents=True)
        for name in ("sisu", "libsisu_runtime_mimalloc.a", "libsisu_runtime_system.a"):
            self.write(self.release / name, "content of " + name)
        (self.release / "sisu").chmod(0o755)
        self.write(self.third, "notices")

    def write(self, path: Path, text: str) -> None:
        with path.open("w") as f:
            f.write(text)

    def pack(
        self, target: str = "x86_64-linux", env: dict[str, str] | None = None
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, CHECK, "pack", target, self.release, self.third, self.out],
            capture_output=True,
            text=True,
            env=env,
            check=False,
        )


class Pack(PackBase):
    def test_pack_should_write_the_exact_tarball(self) -> None:
        done = self.pack()
        self.assertEqual(done.returncode, 0, done.stderr)
        with tarfile.open(self.out / "sisu-x86_64-linux.tar.gz") as tar:
            self.assertEqual(sorted(tar.getnames()), sorted(n.rstrip("/") for n in NAMES))
            for name in ("sisu", "libsisu_runtime_mimalloc.a", "libsisu_runtime_system.a"):
                member = tar.extractfile("sisu/" + name)
                assert member is not None
                self.assertEqual(member.read(), ("content of " + name).encode(), name)
            notices = tar.extractfile("sisu/THIRD-PARTY-LICENSES.txt")
            assert notices is not None
            self.assertEqual(notices.read(), b"notices")
            self.assertTrue(tar.getmember("sisu/sisu").mode & stat.S_IXUSR)
            licence = tar.extractfile("sisu/LICENSE")
            assert licence is not None
            with (SCRIPTS / ".." / "LICENSE").open("rb") as f:
                self.assertEqual(licence.read(), f.read())

    def test_pack_should_fail_on_a_missing_file(self) -> None:
        (self.release / "libsisu_runtime_system.a").unlink()
        done = self.pack()
        self.assertEqual(done.returncode, 1)
        self.assertIn("libsisu_runtime_system.a", done.stderr)
        self.assertEqual([p for p in self.out.iterdir() if p.name.endswith(".tar.gz")], [])

    def test_pack_should_fail_on_an_unknown_target(self) -> None:
        done = self.pack("riscv-linux")
        self.assertEqual(done.returncode, 1)
        self.assertIn("riscv-linux", done.stderr)


def fake_tool(directory: Path, name: str, body: str) -> None:
    path = directory / name
    with path.open("w") as f:
        f.write(f'#!/bin/sh\necho "{name} $*" >> "$LOG"\n{body}\n')
    path.chmod(0o755)


def breaks(call: str) -> str:
    """Shell for a fake: fail with a message when $FAIL names this call."""
    return f'if [ "$FAIL" = {call} ]; then echo "{call} broke" >&2; exit 1; fi'


class ToolBase(unittest.TestCase):
    """Runs nightly_check.py with fake tools first on PATH. A fake fails when $FAIL names its call."""

    @override
    def setUp(self) -> None:
        self.root = Path(tempfile.mkdtemp(prefix="sisu-tools-"))
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)
        self.bin = self.root / "bin"
        self.log = self.root / "log"
        self.bin.mkdir(parents=True)

    def check(
        self, *args: str | Path, fail: str = "", path: str | None = None, **env: str
    ) -> subprocess.CompletedProcess[str]:
        search = f"{self.bin}{os.pathsep}{os.environ['PATH']}" if path is None else path
        env = dict(os.environ, PATH=search, LOG=str(self.log), FAIL=fail, **env)
        return subprocess.run(
            [sys.executable, CHECK, *args], capture_output=True, text=True, env=env, check=False
        )

    def calls(self) -> list[str]:
        if not self.log.exists():
            return []
        with self.log.open() as f:
            return f.read().splitlines()


class MacOSTools(ToolBase):
    @override
    def setUp(self) -> None:
        super().setUp()
        fake_tool(
            self.bin,
            "otool",
            r"""
case "$1" in
-L) {}
    printf 'sisu:\n\t%s (compatibility version 1.0.0)\n' "${{LOAD:-/usr/lib/libz.1.dylib}}";;
-l) {}
    printf '      cmd LC_BUILD_VERSION\n     minos %s\n' "${{MINOS:-15.0}}";;
esac""".format(breaks("otool-L"), breaks("otool-l")),
        )
        fake_tool(self.bin, "codesign", breaks("codesign"))
        fake_tool(
            self.bin,
            "ar",
            r"""
case "$1" in
-t) {}
    printf '%s\n' "__.SYMDEF SORTED" "a.native";;
-x) {}
    : > "$3";;
esac""".format(breaks("ar-t"), breaks("ar-x")),
        )
        self.archives = [self.root / n for n in ("libLLVMSupport.a", "libzstd.a")]

    def macos(self, **kwargs: str) -> subprocess.CompletedProcess[str]:
        return self.check("macos", "sisu", *self.archives, **kwargs)

    def test_macos_should_call_each_tool(self) -> None:
        done = self.macos()
        self.assertEqual(done.returncode, 0, done.stderr)
        calls = self.calls()
        for archive in self.archives:
            for call in (f"ar -t {archive}", f"ar -x {archive} a.native"):
                self.assertIn(call, calls)
        self.assertIn("otool -L sisu", calls)
        self.assertIn("codesign -v sisu", calls)
        # The first member is `__.SYMDEF SORTED`, which is skipped; `a.native` is read.
        read = [c for c in calls if c.startswith("otool -l ") and c.endswith("/a.native")]
        self.assertEqual(len(read), 2, calls)
        self.assertFalse(any("SYMDEF" in c for c in calls if c.startswith("ar -x")))

    def test_macos_should_fail_when_a_tool_fails(self) -> None:
        for fail in ("otool-L", "codesign", "ar-t", "ar-x", "otool-l"):
            with self.subTest(fail):
                done = self.macos(fail=fail)
                self.assertEqual(done.returncode, 1)
                self.assertIn(fail + " broke", done.stderr)

    def test_macos_should_fail_on_a_load_path_off_the_system(self) -> None:
        done = self.macos(LOAD="/opt/homebrew/lib/libzstd.1.dylib")
        self.assertEqual(done.returncode, 1)
        self.assertIn("/opt/homebrew/lib/libzstd.1.dylib", done.stderr)

    def test_macos_should_fail_on_an_object_above_the_floor(self) -> None:
        done = self.macos(MINOS="16.0")
        self.assertEqual(done.returncode, 1)
        self.assertIn("a.native", done.stderr)


class LinuxAndVersionTools(ToolBase):
    @override
    def setUp(self) -> None:
        super().setUp()
        fake_tool(
            self.bin,
            "objdump",
            r"""
case "$1" in
-T) {}
    printf '%s' "$OBJDUMP_T";;
-p) {}
    printf '  NEEDED               %s\n' "${{NEEDED:-libc.so.6}}";;
esac""".format(breaks("objdump-T"), breaks("objdump-p")),
        )
        fake_tool(
            self.bin,
            "sisu",
            breaks("sisu")
            + '''
if [ -n "$WARN" ]; then echo "$WARN" >&2; fi
printf "sisu 0.1.0\\ncommit: abc1234\\n"''',
        )

    def test_linux_should_fail_when_a_tool_fails(self) -> None:
        good = self.check("linux", "sisu", OBJDUMP_T=OK_T)
        self.assertEqual(good.returncode, 0, good.stderr)
        for fail in ("objdump-T", "objdump-p"):
            with self.subTest(fail):
                done = self.check("linux", "sisu", fail=fail, OBJDUMP_T=OK_T)
                self.assertEqual(done.returncode, 1)
                self.assertIn(fail + " broke", done.stderr)

    def test_linux_should_fail_on_a_library_off_the_list_in_the_tool_output(self) -> None:
        done = self.check("linux", "sisu", OBJDUMP_T=OK_T, NEEDED="libfoo.so.1")
        self.assertEqual(done.returncode, 1)
        self.assertIn("libfoo.so.1", done.stderr)

    def test_version_should_fail_on_another_commit(self) -> None:
        sisu = self.bin / "sisu"
        done = self.check("version", sisu, "fff1234")
        self.assertEqual(done.returncode, 1)
        self.assertIn("abc1234", done.stderr)

    def test_version_should_fail_on_a_stderr_line(self) -> None:
        sisu = self.bin / "sisu"
        done = self.check("version", sisu, "abc1234def", WARN="weak version not found")
        self.assertEqual(done.returncode, 1)
        self.assertIn("weak version not found", done.stderr)

    def test_version_should_fail_when_the_tool_fails(self) -> None:
        sisu = self.bin / "sisu"
        self.assertEqual(self.check("version", sisu, "abc1234def").returncode, 0)
        done = self.check("version", sisu, "abc1234def", fail="sisu")
        self.assertEqual(done.returncode, 1)
        self.assertIn("sisu broke", done.stderr)

    def test_checks_should_fail_when_a_tool_is_missing(self) -> None:
        empty = self.root / "empty"
        empty.mkdir(parents=True)
        for args in (("linux", "sisu"), ("version", empty / "no-sisu", "abc1234")):
            with self.subTest(args[0]):
                done = self.check(*args, path=str(empty))
                self.assertEqual(done.returncode, 1)
                self.assertIn("cannot run", done.stderr)


class PackTools(PackBase):
    @override
    def setUp(self) -> None:
        super().setUp()
        self.bin = self.root / "bin"
        self.log = self.root / "log"
        self.bin.mkdir(parents=True)
        # A tar that forwards to the real one, records COPYFILE_DISABLE on the create call, and
        # adds a name to the listing when $EXTRA is set.
        fake_tool(
            self.bin,
            "tar",
            r'''
case "$1" in
-czf) echo "COPYFILE_DISABLE=$COPYFILE_DISABLE" >> "$LOG";;
-tzf) {real} "$@"; [ -n "$EXTRA" ] && echo "$EXTRA"; exit 0;;
esac
exec {real} "$@"'''.format(real=shutil.which("tar")),
        )

    def pack_with(self, **extra: str) -> subprocess.CompletedProcess[str]:
        # The caller's shell must not set COPYFILE_DISABLE for the test.
        env = {k: v for k, v in os.environ.items() if k != "COPYFILE_DISABLE"}
        env.update(PATH=f"{self.bin}{os.pathsep}{os.environ['PATH']}", LOG=str(self.log), **extra)
        return self.pack(env=env)

    def test_pack_should_disable_copyfile_in_the_create_call(self) -> None:
        done = self.pack_with()
        self.assertEqual(done.returncode, 0, done.stderr)
        with self.log.open() as f:
            self.assertIn("COPYFILE_DISABLE=1", f.read())

    def test_pack_should_fail_on_an_extra_name_in_the_listing(self) -> None:
        done = self.pack_with(EXTRA="sisu/._sisu")
        self.assertEqual(done.returncode, 1)
        self.assertIn("._sisu", done.stderr)
        self.assertEqual([p for p in self.out.iterdir() if p.name.endswith(".tar.gz")], [])


class Smoke(unittest.TestCase):
    @override
    def setUp(self) -> None:
        self.root = Path(tempfile.mkdtemp(prefix="sisu-smoke-"))
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)
        self.bin = self.root / "bin"
        self.programs = self.root / "programs"
        self.log = self.root / "log"
        self.bin.mkdir(parents=True)
        self.programs.mkdir(parents=True)

    def fake(self, body: str = 'cat "${4%.sisu}.out"') -> None:
        path = self.bin / "sisu"
        with path.open("w") as f:
            f.write('#!/bin/sh\necho "$@" >> "$LOG"\n' + body + "\n")
        path.chmod(0o755)

    def program(self, name: str, out: str | None = "hello\n") -> None:
        with (self.programs / f"{name}.sisu").open("w") as f:
            f.write("fn main() {}\n")
        if out is not None:
            with (self.programs / f"{name}.out").open("w") as f:
                f.write(out)

    def smoke(self) -> subprocess.CompletedProcess[str]:
        env = dict(os.environ, LOG=str(self.log))
        return subprocess.run(
            ["bash", SMOKE, self.bin, self.programs],
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )

    def calls(self) -> list[str]:
        if not self.log.exists():
            return []
        with self.log.open() as f:
            return f.read().splitlines()

    def test_smoke_should_pass_when_each_program_prints_its_output(self) -> None:
        self.fake()
        self.program("a")
        self.program("b", "two\nlines\n")
        done = self.smoke()
        self.assertEqual(done.returncode, 0, done.stderr)
        for name in ("a", "b"):
            path = self.programs / f"{name}.sisu"
            for allocator in ("mimalloc", "system"):
                self.assertIn(f"run --allocator {allocator} {path}", self.calls())

    def test_smoke_should_run_only_programs_with_an_out_file(self) -> None:
        self.fake()
        self.program("with_out")
        self.program("no_out", None)
        done = self.smoke()
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertTrue(self.calls())
        self.assertFalse(any("no_out" in c for c in self.calls()))

    def test_smoke_should_fail_on_a_wrong_output(self) -> None:
        self.fake('if [ "$3" = system ]; then echo wrong; else cat "${4%.sisu}.out"; fi')
        self.program("prog")
        done = self.smoke()
        self.assertEqual(done.returncode, 1)
        self.assertIn("prog.sisu", done.stderr)
        self.assertIn("system", done.stderr)

    def test_smoke_should_fail_on_a_stderr_line(self) -> None:
        self.fake('cat "${4%.sisu}.out"; echo warning >&2')
        self.program("prog")
        self.assertEqual(self.smoke().returncode, 1)

    def test_smoke_should_fail_on_a_non_zero_exit(self) -> None:
        self.fake('cat "${4%.sisu}.out"; exit 3')
        self.program("prog")
        self.assertEqual(self.smoke().returncode, 1)

    def test_smoke_should_name_a_program_before_it_runs(self) -> None:
        # The fake copies the script's stdout, as it is at its first run, to $SEEN.
        seen = self.root / "seen"
        out = self.root / "out"
        self.fake('[ -e "$SEEN" ] || cp "$OUT" "$SEEN"\ncat "${4%.sisu}.out"')
        self.program("hung")
        env = dict(os.environ, LOG=str(self.log), OUT=str(out), SEEN=str(seen))
        with out.open("w") as f:
            done = subprocess.run(
                ["bash", SMOKE, self.bin, self.programs], env=env, stdout=f, check=False
            )
        self.assertEqual(done.returncode, 0)
        with seen.open() as f:
            text = f.read()
        self.assertIn("hung.sisu", text)
        self.assertIn("mimalloc", text)

    def test_smoke_should_fail_with_no_programs(self) -> None:
        self.fake()
        self.assertEqual(self.smoke().returncode, 1)


if __name__ == "__main__":
    unittest.main()
