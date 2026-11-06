"""Checks for the nightly build: the binary and the tarball. Usage:

  nightly_check.py version <sisu> <sha>
  nightly_check.py linux <sisu>
  nightly_check.py macos <sisu> <libLLVMSupport.a> <libzstd.a>
  nightly_check.py pack <target> <release dir> <third-party file> <out dir>

Each command exits 0, or prints each problem to stderr and exits 1. The text checks read text, so
their tests need no tool. Python 3.10 standard library only: ubuntu:22.04 has 3.10.
"""

import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

GLIBC_FLOOR = (2, 35)
GLIBCXX_FLOOR = (3, 4, 30)
MINOS_FLOOR = (15, 0)
TARGETS = ("x86_64-linux", "aarch64-linux", "aarch64-macos")
ARCHIVES = ("libsisu_runtime_mimalloc.a", "libsisu_runtime_system.a")
THIRD_PARTY = "THIRD-PARTY-LICENSES.txt"
TARBALL_NAMES = ["sisu/", *("sisu/" + n for n in ("sisu", *ARCHIVES, "LICENSE", THIRD_PARTY))]
# The libraries of the Linux build, and the loader, which has another name on each architecture.
# The list is exact: a new version of a library is a decision, not a pass.
NEEDED_OK = frozenset(
    (
        "libm.so.6",
        "libz.so.1",
        "libzstd.so.1",
        "libstdc++.so.6",
        "libgcc_s.so.1",
        "libc.so.6",
        "ld-linux-x86-64.so.2",
        "ld-linux-aarch64.so.1",
    )
)
# `GLIBC_` must not match `GLIBCXX_`, and a version can have three parts, as in `GLIBC_2.2.5`.
GLIBC_REF = re.compile(r"\bGLIBC_(\d+(?:\.\d+)*)")
GLIBCXX_REF = re.compile(r"\bGLIBCXX_(\d+(?:\.\d+)*)")
WEAK_LINE = re.compile(r"^[0-9a-f]+\s+w\s")
NEEDED_LINE = re.compile(r"^\s*NEEDED\s+(\S+)")
LOAD_PATH_OK = ("/usr/lib/", "/System/")


def dotted(version: str) -> tuple[int, ...]:
    return tuple(int(part) for part in version.split("."))


def text(version: tuple[int, ...]) -> str:
    return ".".join(map(str, version))


def version_problems(verbose_text: str, stderr: str, sha: str) -> list[str]:
    problems = []
    want = sha[:7]
    match = re.search(r"^commit: (\S*)$", verbose_text, re.MULTILINE)
    if not match:
        problems.append("`--version --verbose` prints no `commit:` line")
    elif match.group(1) != want:
        problems.append(f"the binary has commit {match.group(1)}, not the picked commit {want}")
    if stderr:
        problems.append(f"`--version --verbose` printed to stderr: {stderr.strip()}")
    return problems


def linux_problems(objdump_t: str, objdump_p: str) -> list[str]:
    problems = []
    glibc = []
    glibcxx = []
    for line in objdump_t.splitlines():
        weak = WEAK_LINE.match(line)
        ref = GLIBC_REF.search(line)
        if ref:
            glibc.append((dotted(ref.group(1)), bool(weak)))
        ref = GLIBCXX_REF.search(line)
        if ref:
            glibcxx.append(dotted(ref.group(1)))
    if not glibc:
        problems.append("`objdump -T` shows no GLIBC_ reference: the tool call is broken")
    # A weak reference that is missing at run time only costs a loader warning.
    # The build container keeps those out, so only the strong references set the floor.
    strong = [v for v, weak in glibc if not weak]
    if strong and max(strong) > GLIBC_FLOOR:
        problems.append(f"GLIBC_{text(max(strong))} is above the floor GLIBC_{text(GLIBC_FLOOR)}")
    if glibcxx and max(glibcxx) > GLIBCXX_FLOOR:
        problems.append(
            f"GLIBCXX_{text(max(glibcxx))} is above the floor GLIBCXX_{text(GLIBCXX_FLOOR)}"
        )
    libraries = [m.group(1) for m in map(NEEDED_LINE.match, objdump_p.splitlines()) if m]
    if not libraries:
        problems.append("`objdump -p` shows no NEEDED line: the tool call is broken")
    for library in libraries:
        if library not in NEEDED_OK:
            problems.append(f"the binary needs {library}, which is not in the allow-list")
    return problems


def macos_link_problems(otool_L: str) -> list[str]:
    problems = []
    seen = False
    for line in otool_L.splitlines():
        # The first line is the file's own name, and ends with a colon.
        if not line.strip() or line.endswith(":"):
            continue
        seen = True
        path = line.strip().split(" (")[0]
        if not path.startswith(LOAD_PATH_OK):
            problems.append(f"the binary loads {path}, which is not a system path")
    if not seen:
        problems.append("`otool -L` shows no load path: the tool call is broken")
    return problems


def minos_problems(name: str, otool_l: str) -> list[str]:
    found = []
    command = ""
    for line in otool_l.splitlines():
        words = line.split()
        if words[:1] == ["cmd"]:
            command = words[1] if len(words) > 1 else ""
        # Under LC_BUILD_VERSION, `version` is a tool's, so only `minos` counts there.
        elif (command == "LC_BUILD_VERSION" and words[:1] == ["minos"] and len(words) > 1) or (
            command == "LC_VERSION_MIN_MACOSX" and words[:1] == ["version"] and len(words) > 1
        ):
            found.append(words[1])
    if not found:
        return [f"{name} has no LC_BUILD_VERSION or LC_VERSION_MIN_MACOSX command"]
    return [
        f"{name} has minos {v}, above the floor {text(MINOS_FLOOR)}"
        for v in found
        if dotted(v) > MINOS_FLOOR
    ]


def tar_problems(names: list[str]) -> list[str]:
    problems = []
    for name in sorted(set(TARBALL_NAMES) - set(names)):
        problems.append(f"the tarball has no {name}")
    for name in sorted(set(names) - set(TARBALL_NAMES)):
        problems.append(f"the tarball has {name}, which it must not have")
    if len(names) != len(set(names)):
        problems.append("the tarball lists a name twice")
    return problems


def run(
    command: list[str], cwd: str | None = None, env: dict[str, str] | None = None
) -> tuple[list[str], str, str]:
    """Runs a tool and gives its problems, stdout and stderr."""
    try:
        done = subprocess.run(command, capture_output=True, text=True, cwd=cwd, env=env)
    except OSError as error:
        return [f"cannot run {command[0]}: {error}"], "", ""
    problems = []
    if done.returncode != 0:
        problems.append(
            "`%s` exited with %d: %s" % (" ".join(command), done.returncode, done.stderr.strip())
        )
    return problems, done.stdout, done.stderr


def check_version(sisu: str, sha: str) -> list[str]:
    problems, out, err = run([sisu, "--version", "--verbose"])
    return problems or version_problems(out, err, sha)


def check_linux(sisu: str) -> list[str]:
    problems_t, out_t, _ = run(["objdump", "-T", sisu])
    problems_p, out_p, _ = run(["objdump", "-p", sisu])
    return problems_t + problems_p or linux_problems(out_t, out_p)


def check_macos(sisu: str, archives: list[str]) -> list[str]:
    problems, out, _ = run(["otool", "-L", sisu])
    problems = problems or macos_link_problems(out)
    problems += run(["codesign", "-v", sisu])[0]
    for archive in archives:
        archive = str(Path(archive).absolute())
        listed_problems, members, _ = run(["ar", "-t", archive])
        # shortcut: the first member stands for the archive, because one deployment target built
        # them all; check every member if an archive ever mixes targets.
        member = next((m for m in members.splitlines() if not m.startswith("__.SYMDEF")), None)
        if listed_problems or member is None:
            problems += listed_problems or [f"{archive} has no member"]
            continue
        with tempfile.TemporaryDirectory() as tmp:
            extract = run(["ar", "-x", archive, member], cwd=tmp)[0]
            if extract:
                problems += extract
                continue
            read_problems, otool_l, _ = run(["otool", "-l", str(Path(tmp, member))])
            problems += read_problems or minos_problems(f"{archive}({member})", otool_l)
    return problems


def pack(target: str, release: str, third_party: str, out_dir: str) -> list[str]:
    if target not in TARGETS:
        return ["unknown target {}: use one of {}".format(target, ", ".join(TARGETS))]
    Path(out_dir).mkdir(parents=True, exist_ok=True)
    root = Path(__file__).absolute().parent / ".."
    sources = {
        "sisu": Path(release, "sisu"),
        **{a: Path(release, a) for a in ARCHIVES},
        "LICENSE": root / "LICENSE",
        THIRD_PARTY: Path(third_party),
    }
    missing = [f"{path} is missing" for path in sources.values() if not path.is_file()]
    if missing:
        return missing
    with tempfile.TemporaryDirectory() as tmp:
        Path(tmp, "sisu").mkdir()
        for name, path in sources.items():
            shutil.copy(path, Path(tmp, "sisu", name))  # copy keeps the mode of `sisu`
        tarball = f"sisu-{target}.tar.gz"
        command = ["tar", "-czf", tarball, "sisu"]
        problems = run(command, cwd=tmp, env=dict(os.environ, COPYFILE_DISABLE="1"))[0]
        if problems:
            return problems
        problems, listing, _ = run(["tar", "-tzf", tarball], cwd=tmp)
        problems = problems or tar_problems(listing.splitlines())
        if not problems:
            shutil.move(Path(tmp, tarball), Path(out_dir, tarball))
        return problems


def main(argv: list[str]) -> int:
    arity = {"version": 3, "linux": 2, "macos": 4, "pack": 5}
    if not argv or arity.get(argv[0]) != len(argv):
        print(__doc__, file=sys.stderr)
        return 2
    command, args = argv[0], argv[1:]
    if command == "version":
        problems = check_version(*args)
    elif command == "linux":
        problems = check_linux(*args)
    elif command == "macos":
        problems = check_macos(args[0], args[1:])
    else:
        problems = pack(*args)
    for problem in problems:
        print(problem, file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
