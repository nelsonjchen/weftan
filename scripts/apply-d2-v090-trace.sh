#!/usr/bin/env bash
set -euo pipefail

source_root="${D2_SOURCE:-${1:-}}"
if [[ -z "$source_root" || ! -d "$source_root/.git" ]]; then
  echo "D2_SOURCE (or the first argument) must be a git checkout of D2 v0.9.0" >&2
  exit 2
fi

git -C "$source_root" apply --check "$(cd "$(dirname "$0")" && pwd)/d2-v090-trace.patch"
git -C "$source_root" apply "$(cd "$(dirname "$0")" && pwd)/d2-v090-trace.patch"
echo "diagnostic trace hooks applied to $source_root"
