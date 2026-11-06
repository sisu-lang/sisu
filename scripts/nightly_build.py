"""Stub of the nightly build's script. Usage: python3 scripts/nightly_build.py pick"""
import argparse
import sys


def pick() -> int:
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    commands.add_parser('pick')
    args = parser.parse_args(argv)
    return pick() if args.command == 'pick' else 2


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
