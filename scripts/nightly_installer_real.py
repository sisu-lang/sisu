"""Run the tests of install.sh that need a real machine, for the nightly build workflow.

Usage:

  SISU_ARTIFACTS=<dir> python3 scripts/nightly_installer_real.py

<dir> holds the tarball of this host, install.sh and SHA256SUMS. The Linux tests run on Linux and
need Docker; the macOS tests run on macOS. The name does not match `test_*.py`, so the pre-commit
hook does not run it: the workflow runs it on `ubuntu-24.04` and `macos-15`.
"""

import contextlib
import os
import select
import subprocess
import sys
import tempfile
import time
import unittest
import uuid
from pathlib import Path

ARTIFACTS = os.environ.get("SISU_ARTIFACTS")
LINUX = sys.platform.startswith("linux")
MAC = sys.platform == "darwin"
READY_SECONDS = 60
# `print` to a pipe is held until the program ends, so only `eprint` shows that the build is over.
WAIT_PROGRAM = 'fn main() {\n    eprint("ready")\n    print(read_line() ?? "none")\n}\n'
HELLO_PROGRAM = 'fn main() {\n    print("hello")\n}\n'
HINT = "sudo apt install build-essential"


def environment(home: str, path: str | None = None) -> dict[str, str]:
    """Return the environment of the installer: HOME, the download directory and PATH."""
    # pyrefly: ignore[bad-assignment]  the `__main__` block exits when ARTIFACTS is unset
    env: dict[str, str] = dict(os.environ, HOME=home, SISU_DOWNLOAD_DIR=ARTIFACTS)
    if path:
        env["PATH"] = path
    return env


def install(home: str, path: str | None = None) -> subprocess.CompletedProcess[str]:
    """Run install.sh from ARTIFACTS with `home` as HOME, and return the finished process."""
    return subprocess.run(
        # pyrefly: ignore[bad-argument-type]  the `__main__` block exits when ARTIFACTS is unset
        ["sh", Path(ARTIFACTS, "install.sh")],
        env=environment(home, path),
        capture_output=True,
        text=True,
        timeout=300,
        check=False,
    )


def docker(image: str, command: str) -> subprocess.CompletedProcess[str]:
    """Run `sh -c command` in the image with ARTIFACTS at /dl.

    The pull is quiet, so that its progress does not reach the stderr of the test. A timeout
    kills the docker client, not the container, so the container has a name and `docker rm -f`
    removes it at the end.
    """
    subprocess.run(["docker", "pull", "-q", image], capture_output=True, check=True, timeout=600)
    name = f"sisu-installer-{uuid.uuid4().hex}"
    try:
        return subprocess.run(
            [
                "docker",
                "run",
                "--rm",
                "--name",
                name,
                "-v",
                f"{ARTIFACTS}:/dl:ro",
                "-e",
                "SISU_DOWNLOAD_DIR=/dl",
                image,
                "sh",
                "-c",
                command,
            ],
            capture_output=True,
            text=True,
            timeout=600,
            check=False,
        )
    finally:
        # A timeout of the removal must not hide the failure that is in flight.
        with contextlib.suppress(subprocess.TimeoutExpired):
            subprocess.run(
                ["docker", "rm", "-f", name], capture_output=True, check=False, timeout=60
            )


def write(path: Path, text: str, mode: int = 0o644) -> None:
    """Write `text` to the file `path`, and set its mode."""
    path.write_text(text, encoding="utf-8")
    path.chmod(mode)


class InstallerRealTest(unittest.TestCase):
    """The tests of install.sh on a real machine."""

    def home(self) -> str:
        """Return a new temporary directory, which the test removes at its end."""
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        return directory.name

    def sisu(self, home: str) -> str:
        """Return the path of the installed `sisu` under `home`."""
        return str(Path(home, ".sisu", "bin", "sisu"))

    @staticmethod
    def stop(proc: subprocess.Popen[bytes]) -> None:
        """Kill the process, wait for it and close its pipes."""
        proc.kill()
        proc.wait()
        for pipe in (proc.stdin, proc.stdout, proc.stderr):
            if pipe:
                pipe.close()

    def wait_for_ready(self, proc: subprocess.Popen[bytes]) -> None:
        """Read the stderr of the process until it holds `ready`, for at most READY_SECONDS."""
        seen = b""
        deadline = time.monotonic() + READY_SECONDS
        while b"ready" not in seen:
            left = deadline - time.monotonic()
            if left <= 0:
                self.fail(f"the program printed no `ready` in {READY_SECONDS} s; stderr: {seen!r}")
            # pyrefly: ignore[bad-specialization]  the caller starts the process with stderr=PIPE
            if select.select([proc.stderr], [], [], left)[0]:
                # pyrefly: ignore[missing-attribute]  as above
                chunk = os.read(proc.stderr.fileno(), 4096)
                if not chunk:
                    self.fail(f"the program ended before it printed `ready`; stderr: {seen!r}")
                seen += chunk

    def assert_one_sentence(self, result: subprocess.CompletedProcess[str], words: str) -> None:
        """Check that the installer stopped with one sentence that holds `words`.

        Exit 125 means that Docker did not start the container, and the line must start with the
        prefix of `die` in install.sh, so that no Docker error passes.
        """
        self.assertNotIn(result.returncode, (0, 125), result.stderr)
        lines = result.stderr.strip().splitlines()
        self.assertEqual(len(lines), 1, result.stderr)
        self.assertTrue(lines[0].startswith("install.sh: "), lines[0])
        self.assertIn(words, lines[0])

    @unittest.skipUnless(LINUX, "Linux only")
    def test_alpine_should_stop_with_one_sentence(self) -> None:
        """Check that the installer stops on Alpine with one sentence that names musl."""
        self.assert_one_sentence(docker("alpine:3", "sh /dl/install.sh"), "musl")

    @unittest.skipUnless(LINUX, "Linux only")
    def test_old_glibc_should_stop_with_one_sentence(self) -> None:
        """Check that the installer stops on glibc 2.31 with one sentence."""
        self.assert_one_sentence(docker("ubuntu:20.04", "sh /dl/install.sh"), "glibc 2.31")

    @unittest.skipUnless(LINUX, "Linux only")
    def test_ubuntu_without_a_compiler_should_install_and_name_the_command(self) -> None:
        result = docker("ubuntu:22.04", 'sh /dl/install.sh && "$HOME/.sisu/bin/sisu" --version')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(HINT, result.stdout)

    @unittest.skipUnless(LINUX or MAC, "Linux or macOS only")
    def test_install_over_a_running_sisu_should_succeed(self) -> None:
        """Check that a second install works while the first `sisu` runs, and the run ends well."""
        home = self.home()
        first = install(home)
        self.assertEqual(first.returncode, 0, first.stderr)
        program = Path(home, "wait.sisu")
        write(program, WAIT_PROGRAM)
        proc = subprocess.Popen(
            [self.sisu(home), "run", program],
            env=environment(home),
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.addCleanup(self.stop, proc)
        self.wait_for_ready(proc)
        self.assertIsNone(proc.poll(), "sisu ended before the second install")
        second = install(home)
        self.assertEqual(second.returncode, 0, second.stderr)
        out, err = proc.communicate(b"line\n", timeout=60)
        self.assertEqual(proc.returncode, 0, err)
        self.assertEqual(out.decode().strip(), "line")
        for name in ("sisu", "libsisu_runtime_mimalloc.a", "libsisu_runtime_system.a"):
            self.assertTrue(Path(home, ".sisu", "bin", name).is_file(), name)

    @unittest.skipUnless(LINUX or MAC, "Linux or macOS only")
    def test_build_should_refuse_a_deleted_runtime_archive(self) -> None:
        """Check that `sisu build` names the missing runtime archive and the fix."""
        home = self.home()
        self.assertEqual(install(home).returncode, 0)
        Path(home, ".sisu", "bin", "libsisu_runtime_mimalloc.a").unlink()
        program = Path(home, "hello.sisu")
        write(program, HELLO_PROGRAM)
        result = subprocess.run(
            [
                self.sisu(home),
                "build",
                "--allocator",
                "mimalloc",
                program,
                "-o",
                Path(home, "hello"),
            ],
            env=environment(home),
            capture_output=True,
            text=True,
            timeout=300,
            check=False,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("runtime library not found at", result.stderr)
        self.assertIn("libsisu_runtime_mimalloc.a; run the Sisu install line again", result.stderr)

    @unittest.skipUnless(MAC, "macOS only")
    def test_mac_with_a_rosetta_uname_should_get_the_arm64_build(self) -> None:
        """Check that a Rosetta `uname -m` (x86_64) on macOS gets the arm64 build, which runs."""
        home = self.home()
        fake = self.home()
        write(
            Path(fake, "uname"),
            '#!/bin/sh\nif [ "${1:-}" = -m ]; then echo x86_64; '
            'else exec /usr/bin/uname "$@"; fi\n',
            0o755,
        )
        result = install(home, fake + os.pathsep + os.environ["PATH"])
        self.assertEqual(result.returncode, 0, result.stderr)
        version = subprocess.run(
            [self.sisu(home), "--version"], capture_output=True, text=True, timeout=60, check=False
        )
        self.assertEqual(version.returncode, 0, version.stderr)

    @unittest.skipUnless(MAC, "macOS only")
    def test_mac_without_command_line_tools_should_name_the_command(self) -> None:
        """Check that the installer names `xcode-select --install` when the tools are missing."""
        home = self.home()
        fake = self.home()
        write(Path(fake, "xcode-select"), "#!/bin/sh\nexit 2\n", 0o755)
        result = install(home, fake + os.pathsep + os.environ["PATH"])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("xcode-select --install", result.stdout)


if __name__ == "__main__":
    if not ARTIFACTS:
        print(
            "nightly_installer_real.py: set SISU_ARTIFACTS to the directory with the tarball, "
            "install.sh and SHA256SUMS.",
            file=sys.stderr,
        )
        sys.exit(2)
    unittest.main()
