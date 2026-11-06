"""Tests for install.sh, run on fake system commands: python3 -m unittest discover -s scripts -p 'test_install.py'.

SISU_TEST_SH names the shell (default /bin/sh) with its arguments, such as 'bash --posix'.
"""

import hashlib
import io
import os
import re
import resource
import shlex
import shutil
import signal
import stat
import subprocess
import tarfile
import tempfile
import time
import unittest
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path
from typing import Any

INSTALL = Path(__file__).absolute().parent / ".." / "install.sh"
BASE = "https://github.com/sisu-lang/sisu/releases/latest/download/"
FILES = ("sisu", "libsisu_runtime_mimalloc.a", "libsisu_runtime_system.a")
TOOLS = [
    "mkdir",
    "rm",
    "mv",
    "cp",
    "tar",
    "gzip",
    "grep",
    "sed",
    "cut",
    "head",
    "cat",
    "ln",
    "readlink",
]


def which(command: str) -> str:
    """The path of `command` on PATH; a test that needs a missing tool fails."""
    path = shutil.which(command)
    assert path is not None, command
    return path


# What a fake command that fails runs: a real one prints its own error.
FAIL = 'echo "fake: failed" >&2; exit 1'
GNU_LDD = "ldd (GNU libc) 2.39"
# The shell is found once, before the Machine replaces PATH, as an absolute path.
SH = shlex.split(os.environ.get("SISU_TEST_SH", "/bin/sh"))
SH[0] = shutil.which(SH[0]) or SH[0]
if not (Path(SH[0]).is_file() and os.access(SH[0], os.X_OK)):
    raise RuntimeError(f"SISU_TEST_SH names no executable shell: {SH[0]}")
# A command that hashes its stdin with SHA-256, for the fake shasum.
SHA256 = [which("sha256sum")] if shutil.which("sha256sum") else [which("shasum"), "-a", "256"]
SLEEP = which("sleep")
UNSUPPORTED = (
    "cannot install on {}: Sisu supports x86-64 and ARM64 Linux with glibc 2.35 or "
    "newer, and ARM64 macOS 15 or newer."
)


def script(path: Path, body: str) -> None:
    with path.open("w") as f:
        f.write("#!/bin/sh\n" + body)
    path.chmod(0o755)


class Machine:
    """A temporary HOME, a download directory, and a PATH with only fake system commands and
    links to the tools the installer needs.
    """

    def __init__(
        self,
        test: unittest.TestCase,
        profile: str = "linux-x86_64",
        cc: bool = True,
        apt: bool = True,
        clt: bool = True,
        prefix: str = "",
    ) -> None:
        self.root = Path(tempfile.mkdtemp(prefix="sisu-install-"))
        test.addCleanup(shutil.rmtree, self.root, ignore_errors=True)
        self.profile = profile
        self.home = self.root / (prefix + "home")
        self.dl = self.root / "dl"
        self.fake = self.root / "fake"
        self.tools = self.root / "tools"
        for d in (self.home, self.dl, self.fake, self.tools):
            d.mkdir(parents=True)
        self.path = f"{self.fake}{os.pathsep}{self.tools}"
        self.sisu = self.home / ".sisu"
        self.lock = self.sisu / "install.lock"
        for tool in [*TOOLS, (shutil.which("sha256sum") and "sha256sum") or "shasum"]:
            (self.tools / tool).symlink_to(which(tool))
        self._fake_commands(profile, cc, apt, clt)

    def _fake_commands(self, profile: str, cc: bool, apt: bool, clt: bool) -> None:
        system, machine, libc, sysctl, version = "Linux", "x86_64", "2.39", None, None
        if profile in ("linux-x86_64", "linux-musl", "linux-unknown-libc"):
            pass
        elif profile == "linux-aarch64":
            machine = "aarch64"
        elif profile == "linux-armv7l":
            machine = "armv7l"
        elif profile.startswith("linux-glibc-"):
            libc = profile[len("linux-glibc-") :]
        elif profile == "freebsd":
            system, machine = "FreeBSD", "amd64"
        elif profile.startswith("macos-"):
            system = "Darwin"
            machine = (
                "x86_64"
                if profile in ("macos-rosetta", "macos-intel", "macos-intel-zero")
                else "arm64"
            )
            sysctl = {
                "macos-intel": ("echo \"sysctl: unknown oid 'hw.optional.arm64'\" >&2\nexit 1\n"),
                "macos-intel-zero": "echo 0\n",
            }.get(profile, "echo 1\n")
            version = profile[len("macos-") :]
            if version in ("arm64", "rosetta", "intel", "intel-zero"):
                version = "15.0"
        else:
            raise ValueError(profile)
        script(self._f("uname"), f'case "$1" in -s) echo {system};; -m) echo {machine};; esac\n')
        if system == "Linux":
            if profile == "linux-musl":
                script(
                    self._f("ldd"),
                    'echo "musl libc (x86_64)" >&2\necho "Version 1.2.4" >&2\nexit 1\n',
                )
                script(self._f("getconf"), "exit 1\n")
            else:
                script(self._f("ldd"), f'echo "{GNU_LDD}"\n')
                script(
                    self._f("getconf"),
                    "exit 1\n" if profile == "linux-unknown-libc" else f'echo "glibc {libc}"\n',
                )
            self.tarball_name = f"sisu-{machine}-linux.tar.gz"
        elif system == "Darwin":
            assert sysctl is not None
            script(self._f("sysctl"), sysctl)
            script(self._f("sw_vers"), f'echo "{version}"\n')
            script(
                self._f("xcode-select"),
                "echo /Library/Developer/CommandLineTools\n" if clt else "exit 2\n",
            )
            self.tarball_name = "sisu-aarch64-macos.tar.gz"
        else:
            self.tarball_name = "sisu-x86_64-linux.tar.gz"
        if cc:
            script(self._f("cc"), "exit 0\n")
        if apt:
            script(self._f("apt-get"), "exit 0\n")

    def _f(self, name: str) -> Path:
        return self.fake / name

    def fake_command(self, name: str, body: str) -> None:
        script(self._f(name), body)

    def fake_curl(self, before: str = "") -> None:
        """A curl that logs each call, runs `before`, and serves the file of the URL from dl."""
        self.curl_log = self.root / "curl.log"
        script(
            self._f("curl"),
            f'printf "%s\\n" "$*" >> {shlex.quote(str(self.curl_log))}\n{before}\n'
            "while [ $# -gt 0 ]; do\n"
            '  case "$1" in -o) shift; out=$1;; https://*) url=$1;; esac\n  shift\ndone\n'
            f'cp {shlex.quote(str(self.dl))}/"${{url##*/}}" "$out"\n',
        )

    def curl_calls(self) -> list[list[str]]:
        if not self.curl_log.exists():
            return []
        with self.curl_log.open() as f:
            return [line.split() for line in f]

    def tarball(
        self,
        name: str | None = None,
        build: str = "new",
        omit: Sequence[str] = (),
        sisu_exit: int = 0,
        pad: int = 0,
    ) -> str:
        """Writes the tarball and its line in SHA256SUMS to the download directory."""
        name = name or self.tarball_name
        stub = '#!/bin/sh\necho "sisu %s"\nexit %d\n#%s\n' % (build, sisu_exit, "x" * pad)
        path = self.dl / name
        with tarfile.open(path, "w:gz") as tar:
            for f in (*FILES, "LICENSE"):
                if f in omit:
                    continue
                data = (stub if f == "sisu" else build + "\n").encode()
                info = tarfile.TarInfo("sisu/" + f)
                info.size = len(data)
                info.mode = 0o755 if f == "sisu" else 0o644
                tar.addfile(info, io.BytesIO(data))
        with path.open("rb") as f:
            digest = hashlib.sha256(f.read()).hexdigest()
        with (self.dl / "SHA256SUMS").open("a") as f:
            f.write(f"{digest}  {name}\n")
        return name

    def sums(self, text: str) -> None:
        with (self.dl / "SHA256SUMS").open("w") as f:
            f.write(text)

    def earlier_install(self, build: str = "old") -> None:
        (self.sisu / "bin").mkdir(parents=True)
        for f in FILES:
            stub = f'#!/bin/sh\necho "sisu {build}"\nexit 0\n'
            with (self.sisu / "bin" / f).open("w") as out:
                out.write(stub if f == "sisu" else build + "\n")
        (self.sisu / "bin" / "sisu").chmod(0o755)

    def _env(self, env: Mapping[str, str | None] | None) -> dict[str, str]:
        e = {"HOME": str(self.home), "PATH": self.path, "SISU_DOWNLOAD_DIR": str(self.dl)}
        for k, v in (env or {}).items():
            if v is None:
                e.pop(k, None)
            else:
                e[k] = v
        return e

    def start(
        self,
        env: Mapping[str, str | None] | None = None,
        preexec_fn: Callable[[], None] | None = None,
    ) -> subprocess.Popen[str]:
        return subprocess.Popen(
            [*SH, INSTALL],
            env=self._env(env),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            preexec_fn=preexec_fn,
        )

    def run(
        self,
        env: Mapping[str, str | None] | None = None,
        on_stdin: bool = False,
        preexec_fn: Callable[[], None] | None = None,
        cwd: Path | None = None,
    ) -> subprocess.CompletedProcess[str]:
        """on_stdin pipes the script to the shell, as the install line does."""
        kw: dict[str, Any] = {}
        if on_stdin:
            with INSTALL.open() as f:
                kw["input"] = f.read()
            argv = SH
        else:
            kw["stdin"] = subprocess.DEVNULL
            argv = [*SH, INSTALL]
        return subprocess.run(
            argv,
            env=self._env(env),
            capture_output=True,
            text=True,
            timeout=120,
            preexec_fn=preexec_fn,
            cwd=cwd,
            **kw,
        )

    def builds(self) -> dict[str, str]:
        """File name -> build, for the files in ~/.sisu/bin."""
        found: dict[str, str] = {}
        for f in FILES:
            path = self.sisu / "bin" / f
            if path.exists():
                with path.open() as fh:
                    text = fh.read(200)
                m = re.search(r'echo "sisu (\w+)"', text)
                found[f] = m.group(1) if f == "sisu" and m else text.strip()
        return found

    def hold(self, target: str | int) -> None:
        """Makes install.lock a symlink to `target`, as an install does with its PID."""
        self.sisu.mkdir(parents=True, exist_ok=True)
        self.lock.symlink_to(str(target))

    def lock_exists(self) -> bool:
        # A PID lock dangles, so os.path.exists says False while it is there.
        return os.path.lexists(self.lock)

    def tmp_exists(self) -> bool:
        return (self.sisu / "tmp").exists()


ALL_OLD = dict.fromkeys(FILES, "old")
ALL_NEW = dict.fromkeys(FILES, "new")


class InstallTest(unittest.TestCase):
    def machine(
        self, profile: str = "linux-x86_64", tarball: bool = True, **kw: bool | str
    ) -> Machine:
        # pyrefly: ignore[bad-argument-type]  `kw` holds only the keywords cc, apt, clt and prefix
        m = Machine(self, profile, **kw)
        if tarball:
            m.tarball()
        return m

    def assertInstalled(self, m: Machine, r: subprocess.CompletedProcess[str]) -> None:
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(m.builds(), ALL_NEW)

    def assertKept(self, m: Machine, r: subprocess.CompletedProcess[str]) -> None:
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(m.builds(), ALL_OLD)
        self.assertFalse(m.tmp_exists())

    def test_unsupported_system_should_stop_with_one_sentence(self) -> None:
        rows = [
            ("linux-armv7l", "armv7l Linux"),
            ("linux-musl", "musl"),
            ("linux-glibc-2.31", "glibc 2.31"),
            ("linux-unknown-libc", "unknown C library"),
            ("macos-intel", "Intel macOS"),
            ("macos-intel-zero", "Intel macOS"),
            ("macos-14.7", "macOS 14"),
            ("freebsd", "FreeBSD"),
        ]
        for profile, named in rows:
            with self.subTest(profile):
                m = self.machine(profile)
                r = m.run()
                self.assertNotEqual(r.returncode, 0)
                self.assertEqual(len(r.stderr.splitlines()), 1, r.stderr)
                for text in (named, "glibc 2.35", "macOS 15"):
                    self.assertIn(text, r.stderr)
                self.assertFalse(m.sisu.exists())

    def test_unsupported_system_should_keep_the_earlier_install(self) -> None:
        for profile in (
            "linux-armv7l",
            "linux-musl",
            "linux-glibc-2.31",
            "linux-unknown-libc",
            "macos-intel",
            "macos-intel-zero",
            "macos-14.7",
            "freebsd",
        ):
            with self.subTest(profile):
                m = self.machine(profile)
                m.earlier_install()
                r = m.run()
                self.assertKept(m, r)
                self.assertIn("cannot install on", r.stderr)

    def test_version_below_the_floor_should_compare_as_numbers(self) -> None:
        for profile, installs in [
            ("linux-glibc-2.4", False),
            ("linux-glibc-2.9", False),
            ("macos-9.6", False),
            ("linux-glibc-2.100", True),
            ("linux-glibc-3.0", True),
            ("macos-100.0", True),
        ]:
            with self.subTest(profile):
                m = self.machine(profile)
                r = m.run()
                if installs:
                    self.assertInstalled(m, r)
                else:
                    self.assertNotEqual(r.returncode, 0)
                    self.assertIn("cannot install on", r.stderr)

    def test_system_at_the_floor_should_install(self) -> None:
        for profile in ("linux-glibc-2.35", "macos-15.0", "macos-15", "macos-26.1"):
            with self.subTest(profile):
                m = self.machine(profile)
                self.assertInstalled(m, m.run())

    def test_platform_should_install_its_own_tarball(self) -> None:
        for profile, name in [
            ("linux-x86_64", "sisu-x86_64-linux.tar.gz"),
            ("linux-aarch64", "sisu-aarch64-linux.tar.gz"),
            ("macos-arm64", "sisu-aarch64-macos.tar.gz"),
        ]:
            with self.subTest(profile):
                m = self.machine(profile, tarball=False)
                m.tarball(name)
                r = m.run()
                self.assertInstalled(m, r)
                self.assertEqual(
                    r.stdout.splitlines()[-1], f"Sisu is installed in {m.home}/.sisu/bin."
                )

    def test_rosetta_terminal_should_install_the_arm64_build(self) -> None:
        m = self.machine("macos-rosetta", tarball=False)
        m.tarball("sisu-aarch64-macos.tar.gz")
        self.assertInstalled(m, m.run())

    def test_missing_compiler_should_install_and_name_the_command(self) -> None:
        rows = [
            ({"cc": False}, "sudo apt install build-essential"),
            ({"cc": False, "apt": False}, "Install a C compiler as `cc`."),
            ({"profile": "macos-arm64", "clt": False}, "xcode-select --install"),
        ]
        for kw, command in rows:
            with self.subTest(kw):
                # pyrefly: ignore[bad-argument-type]  `kw` holds only keywords of `machine`
                m = self.machine(**kw)
                r = m.run()
                self.assertInstalled(m, r)
                self.assertIn("Sisu needs a C compiler to build programs.", r.stdout)
                self.assertIn(command, r.stdout)
                version = subprocess.run(
                    [m.sisu / "bin" / "sisu", "--version"],
                    capture_output=True,
                    text=True,
                )
                self.assertEqual(version.returncode, 0)

    def test_usable_compiler_should_print_no_hint(self) -> None:
        for profile in ("linux-x86_64", "macos-arm64"):
            with self.subTest(profile):
                m = self.machine(profile)
                r = m.run()
                self.assertEqual(r.returncode, 0, r.stderr)
                self.assertNotIn("C compiler", r.stdout)

    def test_cut_download_should_change_nothing(self) -> None:
        m = Machine(self)
        with INSTALL.open("rb") as f:
            size = len(f.read())
        loop = (
            'i=0\nwhile [ "$i" -le "$2" ]; do\n'
            '  head -c "$i" "$1" | {} >/dev/null 2>&1 || true\n'
            '  i=$((i + 1))\ndone\necho "$i"\n'
        ).format(" ".join(shlex.quote(a) for a in SH))
        r = subprocess.run(
            ["/bin/sh", "-c", loop, "loop", INSTALL, str(size - 2)],
            env=m._env(None),
            capture_output=True,
            text=True,
            timeout=300,
        )
        self.assertEqual(r.stdout.strip(), str(size - 1), r.stderr)
        self.assertEqual(list(m.home.iterdir()), [])

    def test_installer_should_run_only_definitions_before_main(self) -> None:
        with INSTALL.open() as f:
            lines = f.read().splitlines()
        self.assertEqual(lines[-1], "main")
        definition = re.compile(r"^[A-Za-z_]\w*\(\) \{(.*\})?$")
        for n, line in enumerate(lines[:-1], 1):
            self.assertTrue(
                line == ""
                or line.startswith(("#", " ", "\t"))
                or line in ("set -eu", "}")
                or definition.match(line),
                "line %d runs before main: %s" % (n, line),
            )

    def test_checksum_mismatch_should_keep_the_earlier_install(self) -> None:
        for row in (
            "a wrong hash",
            "no line for this tarball",
            "a name that differs at a dot",
            "a word before the name",
            "a star in the name",
        ):
            with self.subTest(row):
                m = self.machine()
                m.earlier_install()
                name = m.tarball_name
                with (m.dl / "SHA256SUMS").open() as f:
                    good = f.read().split()[0]
                if row == "a wrong hash":
                    m.sums("{}  {}\n".format("0" * 64, name))
                elif row == "no line for this tarball":
                    m.sums("%s  other.tar.gz\n" % ("0" * 64))
                elif row == "a name that differs at a dot":
                    m.sums("{}  {}\n".format(good, name.replace(".", "X")))
                elif row == "a word before the name":
                    m.sums(f"{good}  x {name}\n")
                else:
                    m.sums(f"{good}  x*{name}\n")
                r = m.run()
                self.assertKept(m, r)
                self.assertIn("does not match", r.stderr)
                self.assertIn("Run the install line again", r.stderr)

    def test_checksum_should_read_both_forms_of_a_line(self) -> None:
        for form in ("%s  %s\n", "%s *%s\n"):
            with self.subTest(form):
                m = self.machine()
                with (m.dl / "SHA256SUMS").open() as f:
                    good = f.read().split()[0]
                m.sums(form % (good, m.tarball_name))
                self.assertInstalled(m, m.run())

    def test_checksum_should_read_only_this_tarballs_line(self) -> None:
        m = self.machine()
        with (m.dl / "SHA256SUMS").open() as f:
            mine = f.read()
        m.sums(
            "{}  sisu-aarch64-linux.tar.gz\n{}{}  sisu-aarch64-macos.tar.gz\n".format(
                "1" * 64, mine, "2" * 64
            )
        )
        self.assertInstalled(m, m.run())

    def test_shasum_should_check_when_sha256sum_is_missing(self) -> None:
        for row, good in (("good", True), ("bad", False)):
            with self.subTest(row):
                m = self.machine()
                for tool in ("sha256sum", "shasum"):
                    if os.path.lexists(m.tools / tool):
                        (m.tools / tool).unlink()
                if good:
                    m.fake_command(
                        "shasum",
                        '[ "$*" = "-a 256" ] || exit 2\nexec {}\n'.format(
                            " ".join(map(shlex.quote, SHA256))
                        ),
                    )
                else:
                    m.fake_command("shasum", 'echo "%s  -"\n' % ("0" * 64))
                r = m.run()
                if good:
                    self.assertInstalled(m, r)
                else:
                    self.assertNotEqual(r.returncode, 0)
                    self.assertIn("does not match", r.stderr)

    def test_no_checksum_tool_should_stop_with_one_sentence(self) -> None:
        m = self.machine()
        m.earlier_install()
        for tool in ("sha256sum", "shasum"):
            if os.path.lexists(m.tools / tool):
                (m.tools / tool).unlink()
        r = m.run()
        self.assertKept(m, r)
        self.assertEqual(r.stderr, "install.sh: this machine has no sha256sum or shasum.\n")

    def test_failed_unpack_should_keep_the_earlier_install(self) -> None:
        def limit() -> None:
            resource.setrlimit(resource.RLIMIT_FSIZE, (1_000_000, 1_000_000))
            signal.signal(signal.SIGXFSZ, signal.SIG_IGN)

        m = self.machine(tarball=False)
        m.tarball(pad=8_000_000)
        m.earlier_install()
        r = m.run(preexec_fn=limit)
        self.assertKept(m, r)
        self.assertIn("tar", r.stderr)
        self.assertNotIn("the tarball has no", r.stderr)
        self.assertNotIn("does not match", r.stderr)
        self.assertNotIn("cannot run", r.stderr)

    def test_wrong_layout_should_keep_the_earlier_install(self) -> None:
        for missing in FILES:
            with self.subTest(missing):
                m = self.machine(tarball=False)
                m.tarball(omit=(missing,))
                m.earlier_install()
                r = m.run()
                self.assertKept(m, r)
                self.assertIn(missing, r.stderr)

    def test_build_that_cannot_start_should_keep_the_earlier_install(self) -> None:
        m = self.machine(tarball=False)
        m.tarball(sisu_exit=1)
        m.earlier_install()
        r = m.run()
        self.assertEqual(m.builds(), ALL_OLD)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("cannot run the Sisu build", r.stderr)

    def fake_mv(self, m: Machine, fails: str) -> Path:
        """A mv that logs each call, and exits 1 when $1 matches the `fails` pattern."""
        log = m.root / "mv.log"
        m.fake_command(
            "mv",
            'echo "$*" >> {}\ncase "$1" in {}) {};; esac\nexec {} "$@"\n'.format(
                shlex.quote(str(log)), fails, FAIL, shlex.quote(which("mv"))
            ),
        )
        return log

    def one_line(self, m: Machine, r: subprocess.CompletedProcess[str]) -> None:
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(r.stderr, f"install.sh: cannot write to {m.sisu}.\n")

    def test_failed_swap_should_restore_the_earlier_install(self) -> None:
        for state in ("bin", "only bin.old", "no ~/.sisu"):
            with self.subTest(state):
                m = self.machine()
                if state == "bin":
                    m.earlier_install()
                elif state == "only bin.old":
                    m.earlier_install()
                    (m.sisu / "bin").rename(m.sisu / "bin.old")
                log = self.fake_mv(m, "*.sisu/tmp/sisu")
                r = m.run()
                self.one_line(m, r)
                self.assertFalse((m.sisu / "bin.old").exists())
                self.assertFalse(m.tmp_exists())
                if state == "no ~/.sisu":
                    self.assertFalse((m.sisu / "bin").exists())
                    continue
                self.assertEqual(m.builds(), ALL_OLD)
                if state == "bin":
                    with log.open() as f:
                        calls = [line.replace(str(m.home), "~").strip() for line in f]
                    self.assertEqual(
                        calls,
                        [
                            "~/.sisu/bin ~/.sisu/bin.old",
                            "~/.sisu/tmp/sisu ~/.sisu/bin",
                            "~/.sisu/bin.old ~/.sisu/bin",
                        ],
                    )

    def test_failed_restore_should_keep_the_earlier_install_in_bin_old(self) -> None:
        m = self.machine()
        m.earlier_install()
        self.fake_mv(m, "*.sisu/tmp/sisu | *.sisu/bin.old")
        r = m.run()
        self.one_line(m, r)
        self.assertFalse((m.sisu / "bin").exists())
        self.assertEqual(sorted(p.name for p in (m.sisu / "bin.old").iterdir()), sorted(FILES))
        self.assertFalse(m.tmp_exists())

    def test_failed_rename_of_bin_should_keep_the_earlier_install(self) -> None:
        m = self.machine()
        m.earlier_install()
        log = self.fake_mv(m, "*.sisu/bin")
        r = m.run()
        self.one_line(m, r)
        with log.open() as f:
            self.assertEqual(len(f.readlines()), 1)
        self.assertEqual(m.builds(), ALL_OLD)
        self.assertFalse((m.sisu / "bin.old").exists())
        self.assertFalse(m.tmp_exists())

    def test_failed_final_removal_should_stop_after_the_swap(self) -> None:
        m = self.machine()
        m.earlier_install()
        m.fake_command(
            "rm",
            'case "$2" in *.sisu/bin.old) [ -e "$2" ] && {{ {}; }};; esac\nexec {} "$@"\n'.format(
                FAIL, shlex.quote(which("rm"))
            ),
        )
        r = m.run()
        self.one_line(m, r)
        self.assertNotIn("Sisu is installed", r.stdout)
        self.assertEqual(m.builds(), ALL_NEW)
        with (m.sisu / "bin.old" / "sisu").open() as f:
            self.assertIn("sisu old", f.read())
        self.assertFalse(m.tmp_exists())

    def test_failed_removal_or_mkdir_should_stop_with_one_line(self) -> None:
        for command, fails, stale in [
            ("rm", "*.sisu/tmp", False),
            ("mkdir", "*.sisu/tmp", False),
            ("rm", "*.sisu/bin.old", True),
        ]:
            with self.subTest(f"{command} {fails}"):
                m = self.machine()
                m.earlier_install()
                if stale:
                    (m.sisu / "bin.old").mkdir(parents=True)
                m.fake_command(
                    command,
                    f'case "$2" in {fails}) {FAIL};; esac\nexec {shlex.quote(which(command))} "$@"\n',
                )
                r = m.run()
                self.one_line(m, r)
                self.assertEqual(m.builds(), ALL_OLD)

    def test_swap_should_work_from_each_starting_state(self) -> None:
        for state in ("no ~/.sisu", "only bin.old", "bin and a stale bin.old"):
            with self.subTest(state):
                m = self.machine()
                if state != "no ~/.sisu":
                    old = m.sisu / "bin.old"
                    old.mkdir(parents=True)
                    with (old / "stale").open("w") as f:
                        f.write("stale\n")
                if state == "bin and a stale bin.old":
                    (m.sisu / "bin").mkdir(parents=True)
                r = m.run()
                self.assertInstalled(m, r)
                self.assertFalse((m.sisu / "bin.old").exists())
                self.assertFalse(m.tmp_exists())

    def test_download_should_ask_for_the_right_files(self) -> None:
        m = self.machine()
        m.fake_curl()
        r = m.run(env={"SISU_DOWNLOAD_DIR": None}, on_stdin=True)
        self.assertInstalled(m, r)
        calls = m.curl_calls()
        self.assertEqual(len(calls), 2)
        for call in calls:
            for token in ("--proto", "=https", "--tlsv1.2", "-fsSL", "--retry"):
                self.assertIn(token, call)
        self.assertEqual(
            sorted(c[-1] for c in calls), sorted([BASE + m.tarball_name, BASE + "SHA256SUMS"])
        )

    def test_download_dir_should_not_call_curl(self) -> None:
        m = self.machine()
        m.fake_curl()
        self.assertInstalled(m, m.run())
        self.assertEqual(m.curl_calls(), [])

    def test_missing_file_in_download_dir_should_stop_with_one_sentence(self) -> None:
        for missing in ("tarball", "SHA256SUMS"):
            with self.subTest(missing):
                m = self.machine()
                m.earlier_install()
                name = m.tarball_name if missing == "tarball" else "SHA256SUMS"
                (m.dl / name).unlink()
                r = m.run()
                self.assertKept(m, r)
                self.assertEqual(r.stderr, f"install.sh: {m.dl} has no {name}.\n")

    def test_path_lines_should_print_only_when_needed(self) -> None:
        m = self.machine()
        r = m.run()
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn('export PATH="$HOME/.sisu/bin:$PATH"\n', r.stdout)
        self.assertIn("fish_add_path $HOME/.sisu/bin\n", r.stdout)
        m = self.machine()
        r = m.run(env={"PATH": f"{m.sisu / 'bin'}{os.pathsep}{m.path}"})
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertNotIn("PATH=", r.stdout)
        self.assertNotIn("fish_add_path", r.stdout)

    def kill_during_curl(self, m: Machine, sig: int) -> int:
        """Runs the install, sends `sig` while the fake curl sleeps, and returns the exit code."""

        def default_int() -> None:
            signal.signal(signal.SIGINT, signal.SIG_DFL)

        marker = m.root / "marker"
        m.fake_curl(f": > {shlex.quote(str(marker))}\n{shlex.quote(SLEEP)} 0.3")
        p = m.start(env={"SISU_DOWNLOAD_DIR": None}, preexec_fn=default_int)
        deadline = time.time() + 5
        while not marker.exists() and time.time() < deadline:
            time.sleep(0.01)
        if not marker.exists():
            p.kill()
            p.communicate()
            self.fail("curl did not start within 5 s")
        p.send_signal(sig)
        # communicate, not wait: the fake curl outlives a killed run and holds its pipes.
        p.communicate(timeout=30)
        return p.returncode

    def test_signal_should_remove_tmp(self) -> None:
        for sig, code in ((signal.SIGTERM, 143), (signal.SIGINT, 130)):
            with self.subTest(sig.name):
                m = self.machine()
                m.earlier_install()
                self.assertEqual(self.kill_during_curl(m, sig), code)
                self.assertEqual(m.builds(), ALL_OLD)
                self.assertFalse(m.tmp_exists())

    def test_home_with_a_space_should_install(self) -> None:
        m = self.machine(prefix="a b/")
        self.assertInstalled(m, m.run())

    def test_home_with_a_backslash_should_install(self) -> None:
        m = self.machine(prefix="a\\b/")
        self.assertInstalled(m, m.run())

    def test_home_unset_should_stop_with_one_sentence(self) -> None:
        m = self.machine()
        r = m.run(env={"HOME": None})
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(r.stderr, "install.sh: HOME is not set.\n")

    @unittest.skipIf(os.geteuid() == 0, "root can write to a read-only directory")
    def test_unwritable_sisu_directory_should_keep_the_earlier_install(self) -> None:
        m = self.machine()
        m.earlier_install()
        m.sisu.chmod(0o555)
        self.addCleanup(os.chmod, m.sisu, stat.S_IRWXU)
        r = m.run()
        self.one_line(m, r)
        self.assertEqual(m.builds(), ALL_OLD)

    def test_cdpath_should_not_move_the_unpack(self) -> None:
        m = self.machine()
        cwd = m.root / "cwd"
        cwd.mkdir(parents=True)
        # CDPATH holds a second ~/.sisu/tmp that a cd to the relative path would find first.
        (m.home / ".sisu" / "tmp").mkdir(parents=True)
        r = m.run(env={"HOME": "home", "CDPATH": str(m.root)}, cwd=cwd)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertLessEqual(
            set(FILES), {p.name for p in (cwd / "home" / ".sisu" / "bin").iterdir()}
        )

    @unittest.skipIf(os.geteuid() == 0, "root can write to a read-only directory")
    def test_tar_stop_should_print_no_error_from_the_cleanup(self) -> None:
        m = self.machine()
        m.fake_command(
            "tar", '{} 555 "$HOME/.sisu"\n{}\n'.format(shlex.quote(which("chmod")), FAIL)
        )
        self.addCleanup(os.chmod, m.sisu, stat.S_IRWXU)
        r = m.run()
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("fake: failed", r.stderr)
        self.assertNotIn("rm:", r.stderr)

    def test_sisu_path_that_is_a_file_should_stop_with_one_sentence(self) -> None:
        m = self.machine()
        with m.sisu.open("w") as f:
            f.write("not a directory\n")
        r = m.run()
        self.one_line(m, r)
        with m.sisu.open() as f:
            self.assertEqual(f.read(), "not a directory\n")

    def test_failed_download_should_keep_the_earlier_install(self) -> None:
        m = self.machine()
        m.earlier_install()
        m.fake_curl('echo "curl: (22) The requested URL returned error: 404" >&2\nexit 22')
        r = m.run(env={"SISU_DOWNLOAD_DIR": None})
        self.assertKept(m, r)
        self.assertIn("cannot download", r.stderr)
        self.assertIn(BASE + m.tarball_name, r.stderr)

    def test_stale_tmp_should_not_reach_the_new_install(self) -> None:
        m = self.machine()
        (m.sisu / "tmp" / "sisu").mkdir(parents=True)
        for junk in ("sisu/junk", "other"):
            with (m.sisu / "tmp" / junk).open("w") as f:
                f.write("junk\n")
        r = m.run()
        self.assertInstalled(m, r)
        self.assertFalse((m.sisu / "bin" / "junk").exists())
        self.assertFalse(m.tmp_exists())

    def live_process(self) -> int:
        p = subprocess.Popen([SLEEP, "60"])
        self.addCleanup(p.wait)
        self.addCleanup(p.kill)
        return p.pid

    def dead_pid(self) -> int:
        p = subprocess.Popen([which("true")])
        p.wait()
        return p.pid

    def one_line_with(self, r: subprocess.CompletedProcess[str], *texts: str) -> None:
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(len(r.stderr.splitlines()), 1, r.stderr)
        for text in texts:
            self.assertIn(text, r.stderr)

    def test_second_install_should_stop_while_a_first_install_runs(self) -> None:
        m = self.machine()
        m.earlier_install()
        pid = self.live_process()
        m.hold(pid)
        (m.sisu / "tmp").mkdir(parents=True)
        mark = m.sisu / "tmp" / "first-run"
        with mark.open("w") as f:
            f.write("x\n")
        m.fake_curl()
        r = m.run(env={"SISU_DOWNLOAD_DIR": None})
        self.one_line_with(r, "another install is running", str(m.lock))
        self.assertEqual(m.builds(), ALL_OLD)
        self.assertEqual(str(m.lock.readlink()), str(pid))
        self.assertTrue(mark.exists())
        self.assertEqual(m.curl_calls(), [])

    def test_lock_of_a_dead_process_should_not_stop_the_install(self) -> None:
        own = 'mkdir -p "$HOME/.sisu"; ln -s $$ "$HOME/.sisu/install.lock"; exec "$@"'
        rows = [
            ("a reaped child", self.dead_pid(), ""),
            ("not-a-pid", "not-a-pid", ""),
            ("a PID that does not exist", 99999999, ""),
            ("zero", 0, ""),
            ("minus one", -1, ""),
            ("a space in HOME", self.dead_pid(), "a b/"),
            ("its own PID", None, ""),
        ]
        for name, target, prefix in rows:
            with self.subTest(name):
                m = self.machine(prefix=prefix)
                if target is None:
                    r = subprocess.run(
                        [*SH, "-c", own, "sh", *SH, INSTALL],
                        env=m._env(None),
                        stdin=subprocess.DEVNULL,
                        capture_output=True,
                        text=True,
                        timeout=120,
                    )
                else:
                    m.hold(target)
                    r = m.run()
                self.assertInstalled(m, r)
                self.assertFalse(m.lock_exists())

    def test_install_should_remove_its_lock_at_the_end(self) -> None:
        for name in (
            "success",
            "checksum mismatch",
            "TERM",
            "INT",
            "failed rm of tmp",
            "tmp goes first",
        ):
            with self.subTest(name):
                m = self.machine()
                m.earlier_install()
                real_rm = shlex.quote(which("rm"))
                if name == "success":
                    self.assertInstalled(m, m.run())
                elif name == "tmp goes first":
                    flag = m.root / "tmp-after-lock"
                    m.fake_command(
                        "rm",
                        'case "$*" in *.sisu/tmp)\n'
                        f'  if [ ! -L {shlex.quote(str(m.lock))} ]; then : > {shlex.quote(str(flag))}; fi;; esac\nexec {real_rm} "$@"\n',
                    )
                    self.assertInstalled(m, m.run())
                    self.assertFalse(flag.exists())
                elif name == "checksum mismatch":
                    m.sums("{}  {}\n".format("0" * 64, m.tarball_name))
                    r = m.run()
                    self.assertKept(m, r)
                    self.assertEqual(r.returncode, 1)
                elif name == "failed rm of tmp":
                    m.fake_command(
                        "rm",
                        'case "$2" in *.sisu/tmp) {};; esac\nexec {} "$@"\n'.format(
                            FAIL, shlex.quote(which("rm"))
                        ),
                    )
                    self.one_line(m, m.run())
                else:
                    sig, code = (signal.SIGTERM, 143) if name == "TERM" else (signal.SIGINT, 130)
                    self.assertEqual(self.kill_during_curl(m, sig), code)
                self.assertFalse(m.lock_exists())

    def test_install_should_exit_zero_when_its_lock_cannot_be_removed(self) -> None:
        # The new build is whole, so the run exits 0 although the lock stays.
        m = self.machine()
        m.earlier_install()
        m.fake_command(
            "rm",
            'for last; do :; done\ncase "$last" in *install.lock) {};; esac\nexec {} "$@"\n'.format(
                FAIL, shlex.quote(which("rm"))
            ),
        )
        r = m.run()
        self.assertInstalled(m, r)
        # Only the fake `rm` leaves the lock, so this proves that the fake fired.
        self.assertTrue(m.lock_exists())

    def test_killed_install_should_not_stop_the_next_install(self) -> None:
        m = self.machine()
        m.earlier_install()
        self.assertEqual(self.kill_during_curl(m, signal.SIGKILL), -signal.SIGKILL)
        self.assertTrue(m.lock_exists())
        self.assertInstalled(m, m.run())
        self.assertFalse(m.lock_exists())

    def test_lock_path_that_is_not_a_pid_lock_should_stop_with_one_line(self) -> None:
        for kind in ("a regular file", "a directory", "a symlink to a directory"):
            with self.subTest(kind):
                m = self.machine()
                m.earlier_install()
                outside = m.root / "outside"
                if kind == "a regular file":
                    with m.lock.open("w") as f:
                        f.write("x\n")
                elif kind == "a directory":
                    m.lock.mkdir()
                else:
                    outside.mkdir()
                    m.lock.symlink_to(outside)
                r = m.run()
                self.one_line_with(r, "is not a lock", str(m.lock))
                self.assertEqual(r.returncode, 1)
                self.assertEqual(m.builds(), ALL_OLD)
                self.assertTrue(m.lock_exists())
                if kind == "a regular file":
                    self.assertTrue(m.lock.is_file())
                elif kind == "a directory":
                    self.assertEqual(list(m.lock.iterdir()), [])
                else:
                    self.assertEqual(m.lock.readlink(), outside)
                    self.assertEqual(list(outside.iterdir()), [])

    def test_installs_at_the_same_time_should_leave_one_whole_build(self) -> None:
        # Each run spins on the gate, so that the four runs start within a few milliseconds.
        wait = 'while [ ! -e "$1" ]; do :; done; shift; exec "$@"'
        for earlier in (True, False):
            with self.subTest("earlier install" if earlier else "no earlier install"):
                m = self.machine()
                gate = m.root / "gate"
                for n in range(25):
                    shutil.rmtree(m.sisu, ignore_errors=True)
                    if gate.exists():
                        gate.unlink()
                    if earlier:
                        m.earlier_install()
                    runs = [
                        subprocess.Popen(
                            [*SH, "-c", wait, "sh", gate, *SH, INSTALL],
                            env=m._env(None),
                            stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE,
                            text=True,
                        )
                        for _ in range(4)
                    ]
                    gate.touch()
                    done = [(p.communicate(timeout=120), p.returncode) for p in runs]
                    where = "round %d: %r" % (n, done)
                    for (_out, err), code in done:
                        if code != 0:
                            self.assertEqual(code, 1, where)
                            self.assertEqual(len(err.splitlines()), 1, where)
                            self.assertIn("another install is running", err, where)
                    self.assertIn(0, [code for _, code in done], where)
                    self.assertEqual(m.builds(), ALL_NEW, where)
                    self.assertFalse((m.sisu / "bin.old").exists(), where)
                    self.assertFalse(m.tmp_exists(), where)
                    self.assertFalse(m.lock_exists(), where)

    def test_failed_lock_should_stop_with_one_sentence(self) -> None:
        m = self.machine()
        m.earlier_install()
        m.fake_command("ln", FAIL + "\n")
        r = m.run()
        self.one_line(m, r)
        self.assertEqual(m.builds(), ALL_OLD)

    def test_lock_that_cannot_be_removed_should_stop_with_one_sentence(self) -> None:
        m = self.machine()
        m.earlier_install()
        m.hold(self.dead_pid())
        m.fake_command("rm", FAIL + "\n")
        r = m.run()
        self.one_line(m, r)
        self.assertEqual(m.builds(), ALL_OLD)

    def once(self, m: Machine, name: str, body: str) -> None:
        """A fake `name` that runs `body` on its first call, and is the real command after."""
        flag = m.root / f"{name}.flag"
        m.fake_command(
            name,
            f'if [ ! -e {shlex.quote(str(flag))} ]; then\n: > {shlex.quote(str(flag))}\n{body}\nfi\nexec {shlex.quote(which(name))} "$@"\n',
        )

    def test_holders_that_end_during_the_lock_check_should_not_give_a_write_error(self) -> None:
        # Three failed `ln` calls in a row, each after the holder ended: the run tries again.
        m = self.machine()
        m.hold(self.live_process())
        count = m.root / "ln.count"
        m.fake_command(
            "ln",
            "echo x >> {}\nn=$(grep -c x {})\n"
            'if [ "$n" -le 3 ]; then rm -f {}; {}; fi\nexec {} "$@"\n'.format(
                shlex.quote(str(count)),
                shlex.quote(str(count)),
                shlex.quote(str(m.lock)),
                FAIL,
                shlex.quote(which("ln")),
            ),
        )
        r = m.run()
        self.assertInstalled(m, r)
        self.assertNotIn("cannot write", r.stderr)
        self.assertFalse(m.lock_exists())

    def test_lock_taken_after_the_first_read_should_stay(self) -> None:
        m = self.machine()
        live = self.live_process()
        dead = self.dead_pid()
        m.hold(dead)
        self.once(
            m,
            "readlink",
            "%s -f %s\n%s -s %d %s\necho %d\nexit 0"
            % (
                shlex.quote(which("rm")),
                shlex.quote(str(m.lock)),
                shlex.quote(which("ln")),
                live,
                shlex.quote(str(m.lock)),
                dead,
            ),
        )
        r = m.run()
        self.one_line_with(r, "another install is running")
        self.assertEqual(str(m.lock.readlink()), str(live))

    def test_failed_lock_read_should_not_remove_the_lock(self) -> None:
        m = self.machine()
        m.earlier_install()
        live = self.live_process()
        m.hold(live)
        m.fake_command("readlink", FAIL + "\n")
        r = m.run()
        self.one_line_with(r, "another install is running")
        self.assertEqual(str(m.lock.readlink()), str(live))
        self.assertEqual(m.builds(), ALL_OLD)


if __name__ == "__main__":
    unittest.main()
