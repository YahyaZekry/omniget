#!/bin/sh
# Applies a prepared rename plan. One `from<TAB>to` pair per line.
set -eu
while IFS="$(printf '\t')" read -r from to; do
  [ -n "$to" ] || continue
  mv -n -- "$from" "$to"
done
