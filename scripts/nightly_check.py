"""Checks for the nightly build: the binary, the tarball. Usage:

  nightly_check.py version <sisu> <sha>
  nightly_check.py linux <sisu>
  nightly_check.py macos <sisu> <libLLVMSupport.a> <libzstd.a>
  nightly_check.py pack <target> <release dir> <third-party file> <out dir>

Each command exits 0, or prints each problem to stderr and exits 1.
"""
import sys


def version_problems(verbose_text: str, stderr: str, sha: str) -> list[str]:
    return []


def linux_problems(objdump_t: str, objdump_p: str) -> list[str]:
    return []


def macos_link_problems(otool_L: str) -> list[str]:
    return []


def minos_problems(name: str, otool_l: str) -> list[str]:
    return []


def tar_problems(names: list[str]) -> list[str]:
    return []


def main(argv: list[str]) -> int:
    return 1


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
