#!/usr/bin/env bash
# Runs the lexer in Sisu, the executable $1, once on each file of the corpus directory $2, and
# prints the output of the runs one after the other. A script does it because an `exec` line of
# codspeed.yml is plain words, which cannot redirect standard input.
#   bench/features/selfhost_lexer/run.sh target/codspeed/selfhost_lexer/main bench/features/selfhost_lexer/corpus
set -euo pipefail

for file in "$2"/*.sisu; do
    "$1" <"$file"
done
