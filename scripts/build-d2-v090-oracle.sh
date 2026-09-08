#!/usr/bin/env bash
set -euo pipefail

# Build the small oracle in scripts/oss-tala-oracle against a pinned v0.9.0
# source checkout. The replacement is kept in a temporary module file so the
# repository never records a machine-specific absolute path.
script_root="$(cd "$(dirname "$0")" && pwd)"
source_root="${D2_SOURCE:-}"
output_path="${D2_ORACLE_OUTPUT:-$script_root/../target/oss-tala-oracle-v0.9.0}"

if [[ -z "$source_root" || ! -f "$source_root/go.mod" ]]; then
  echo "D2_SOURCE must point to the pinned D2 v0.9.0 source checkout" >&2
  exit 2
fi

build_root="$(mktemp -d "${TMPDIR:-/tmp}/weftan-d2-oracle.XXXXXX")"
cleanup() { rm -rf "$build_root"; }
trap cleanup EXIT

cp "$script_root/oss-tala-oracle/main.go" "$script_root/oss-tala-oracle/go.mod" \
  "$script_root/oss-tala-oracle/go.sum" "$build_root/"
go mod edit -C "$build_root" -dropreplace=github.com/d2lang/d2 \
  -replace="github.com/d2lang/d2=$source_root"
mkdir -p "$(dirname "$output_path")"
(cd "$build_root" && go build -o "$output_path" .)
echo "$output_path"
