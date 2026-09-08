#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
  echo "usage: $0 DIRECTORY" >&2
  exit 2
fi

corpus_dir=$1
plugin=${WEFTAN_PLUGIN:-target/debug/d2plugin-weftan}

if [ ! -x "$plugin" ]; then
  cargo build -p d2plugin-weftan
fi

count=0
find "$corpus_dir" -type f \( -name '*.json' -o -name '*.graph.bin' \) -print |
  sort |
  while IFS= read -r input; do
  "$plugin" layout --weftan-seeds 1 < "$input" > /dev/null
  count=$((count + 1))
  echo "$count $input"
  done
