#!/usr/bin/env bash
# Runs the lexer in Sisu, the executable $1, once on the file $2, which it reads on standard input.
# A script does it because an `exec` line of codspeed.yml is plain words, which cannot redirect
# standard input.
#   bench/features/selfhost_lexer/run.sh target/codspeed/selfhost_lexer/main bench/features/selfhost_lexer/input.sisu
set -euo pipefail

exec "$1" <"$2"
