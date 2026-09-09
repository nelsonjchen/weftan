#!/bin/sh
set -eu

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
  echo "usage: $0 D2_BINARY [TALA_PLUGIN_DIRECTORY]" >&2
  exit 2
fi

d2_bin=$1
tala_dir=${2:-}
workspace=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
weftan_dir="$workspace/target/debug"
out_dir=$(mktemp -d)
trap 'rm -rf "$out_dir"' EXIT HUP INT TERM

cargo build --manifest-path "$workspace/Cargo.toml" -p d2plugin-weftan >/dev/null

check_svg() {
  svg=$1
  engine=$2
  test -s "$svg"

  fixed=$(perl -0777 -ne 'if (/class="Zml4ZWQ=".*?<rect x="([^"]+)" y="([^"]+)" width="([^"]+)" height="([^"]+)"/s) { print "$1 $2 $3 $4" }' "$svg")
  anchor=$(perl -0777 -ne 'if (/class="YW5jaG9y".*?<rect x="([^"]+)" y="([^"]+)" width="([^"]+)" height="([^"]+)"/s) { print "$1 $2 $3 $4" }' "$svg")
  near=$(perl -0777 -ne 'if (/class="bmVhci1hbmNob3I=".*?<rect x="([^"]+)" y="([^"]+)" width="([^"]+)" height="([^"]+)"/s) { print "$1 $2 $3 $4" }' "$svg")
  test -n "$fixed" && test -n "$anchor" && test -n "$near"

  set -- $fixed
  awk -v width="$3" -v height="$4" 'BEGIN { exit !(width >= 240 && height >= 220) }'
  set -- $anchor
  anchor_x=$1
  anchor_y=$2
  anchor_right=$(awk -v x="$1" -v width="$3" 'BEGIN { print x + width }')
  awk -v x="$anchor_x" -v y="$anchor_y" 'BEGIN { exit !(x == 520 && y == 140) }'
  set -- $near
  awk -v x="$1" -v right="$anchor_right" 'BEGIN { exit !(x > right) }'

  grep -Fq 'class="KGZpeGVkIC0mZ3Q7IGZpeGVkLmEpWzBd"' "$svg"
  echo "$engine feature contract: ok"
}

check_nested_svg() {
  svg=$1
  container=$(perl -0777 -ne 'if (/class="Y29udGFpbmVy".*?<rect x="([^"]+)" y="([^"]+)" width="([^"]+)" height="([^"]+)"/s) { print "$1 $2 $3 $4" }' "$svg")
  child=$(perl -0777 -ne 'if (/class="Y29udGFpbmVyLmNoaWxk".*?<rect x="([^"]+)" y="([^"]+)" width="([^"]+)" height="([^"]+)"/s) { print "$1 $2 $3 $4" }' "$svg")
  outside=$(perl -0777 -ne 'if (/class="b3V0c2lkZQ==".*?<rect x="([^"]+)" y="([^"]+)" width="([^"]+)" height="([^"]+)"/s) { print "$1 $2 $3 $4" }' "$svg")
  set -- $container
  container_x=$1
  container_y=$2
  set -- $child
  awk -v child_x="$1" -v child_y="$2" -v parent_x="$container_x" -v parent_y="$container_y" \
    'BEGIN { exit !(child_x > parent_x + 120 && child_y > parent_y + 110) }'
  set -- $outside
  awk -v x="$1" -v y="$2" 'BEGIN { exit !(x == 600 && y == 80) }'
}

PATH="$weftan_dir:$(dirname "$d2_bin"):$PATH" \
  "$d2_bin" --layout=weftan "$workspace/tests/d2/features.d2" "$out_dir/weftan-features.svg" >/dev/null
PATH="$weftan_dir:$(dirname "$d2_bin"):$PATH" \
  "$d2_bin" --layout=weftan "$workspace/tests/d2/routing.d2" "$out_dir/weftan-routing.svg" >/dev/null
PATH="$weftan_dir:$(dirname "$d2_bin"):$PATH" \
  "$d2_bin" --layout=weftan "$workspace/tests/d2/grid-routing.d2" "$out_dir/weftan-grid-routing.svg" >/dev/null
PATH="$weftan_dir:$(dirname "$d2_bin"):$PATH" \
  "$d2_bin" --layout=weftan "$workspace/tests/d2/nested-lock.d2" "$out_dir/weftan-nested-lock.svg" >/dev/null
check_svg "$out_dir/weftan-features.svg" weftan
check_nested_svg "$out_dir/weftan-nested-lock.svg"
test -s "$out_dir/weftan-routing.svg"
test -s "$out_dir/weftan-grid-routing.svg"

if [ -n "$tala_dir" ]; then
  PATH="$tala_dir:$(dirname "$d2_bin"):$PATH" \
    "$d2_bin" --layout=tala "$workspace/tests/d2/features.d2" "$out_dir/tala-features.svg" >/dev/null
  PATH="$tala_dir:$(dirname "$d2_bin"):$PATH" \
    "$d2_bin" --layout=tala "$workspace/tests/d2/routing.d2" "$out_dir/tala-routing.svg" >/dev/null
  PATH="$tala_dir:$(dirname "$d2_bin"):$PATH" \
    "$d2_bin" --layout=tala "$workspace/tests/d2/grid-routing.d2" "$out_dir/tala-grid-routing.svg" >/dev/null
  PATH="$tala_dir:$(dirname "$d2_bin"):$PATH" \
    "$d2_bin" --layout=tala "$workspace/tests/d2/nested-lock.d2" "$out_dir/tala-nested-lock.svg" >/dev/null
  check_svg "$out_dir/tala-features.svg" tala
  check_nested_svg "$out_dir/tala-nested-lock.svg"
  test -s "$out_dir/tala-routing.svg"
  test -s "$out_dir/tala-grid-routing.svg"
fi
