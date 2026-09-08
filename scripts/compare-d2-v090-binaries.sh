#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 RELEASE_D2_BINARY SOURCE_D2_BINARY" >&2
  exit 2
fi

release_binary="$1"
source_binary="$2"
workspace="$(cd "$(dirname "$0")/.." && pwd)"
tmp_root="$(mktemp -d "${TMPDIR:-/tmp}/weftan-d2-binary-compare.XXXXXX")"
cleanup() { rm -rf "$tmp_root"; }
trap cleanup EXIT HUP INT TERM

for input in \
  "$workspace/tests/d2/direction-right.d2" \
  "$workspace/tests/d2/features.d2" \
  "$workspace/tests/d2/routing.d2"; do
  base="$(basename "$input" .d2)"
  "$release_binary" --layout=tala --omit-version "$input" \
    "$tmp_root/${base}-release.svg" >/dev/null
  "$source_binary" --layout=tala --omit-version "$input" \
    "$tmp_root/${base}-source.svg" >/dev/null
  if ! cmp -s "$tmp_root/${base}-release.svg" "$tmp_root/${base}-source.svg"; then
    echo "D2 v0.9.0 binary mismatch: $base" >&2
    exit 1
  fi
  echo "D2 v0.9.0 binary output exact: $base"
done
