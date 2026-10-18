#!/bin/sh
# Assembles each page: _head.html + src/<page>.html + _foot.html.
# The first line of a src file is the page title; the rest is the page body.
set -eu
cd "$(dirname "$0")"
for src in src/*.html; do
  page=$(basename "$src")
  title=$(head -n 1 "$src")
  # Source comments are notes for editors; the built page leaves them out.
  { sed "s|__TITLE__|$title|g" _head.html; tail -n +2 "$src" | sed '/^ *<!--.*-->$/d'; cat _foot.html; } > "$page"
done
