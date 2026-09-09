#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
  echo "usage: $0 D2_BINARY TALA_PLUGIN_DIRECTORY" >&2
  exit 2
fi

d2_bin=$1
tala_dir=$2
workspace=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
weftan_dir="$workspace/target/debug"
out_dir=$(mktemp -d)
trap 'rm -rf "$out_dir"' EXIT HUP INT TERM

cargo build --manifest-path "$workspace/Cargo.toml" -p d2plugin-weftan >/dev/null

canonicalize() {
  perl -0pe 's/></>\n</g' "$1" |
    rg '(<rect [^>]*stroke="#[0-9A-Fa-f]+"|<text [^>]*class="text|<path d=)' |
    sed -E 's/(mk-d2|d2)-[0-9]+-/\1-HASH-/g; s/url\(#d2-[0-9]+\)/url(#d2-HASH)/g'
}

fixtures='direction-right fixed-container near right-near top-left right-top-left routing grid-routing nested-lock features'
for fixture in $fixtures; do
  source="$workspace/tests/d2/$fixture.d2"
  tala_svg="$out_dir/$fixture-tala.svg"
  weftan_svg="$out_dir/$fixture-weftan.svg"
  PATH="$tala_dir:$(dirname "$d2_bin"):$PATH" \
    "$d2_bin" --layout=tala --omit-version "$source" "$tala_svg" >/dev/null
  PATH="$weftan_dir:$(dirname "$d2_bin"):$PATH" \
    "$d2_bin" --layout=weftan --omit-version "$source" "$weftan_svg" >/dev/null
  canonicalize "$tala_svg" > "$out_dir/$fixture-tala.geometry"
  canonicalize "$weftan_svg" > "$out_dir/$fixture-weftan.geometry"
  diff -u "$out_dir/$fixture-tala.geometry" "$out_dir/$fixture-weftan.geometry"
  echo "$fixture: exact shape, label, and route geometry"
done
