#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "usage: $0 D2_V090_BINARY [PLUGIN_DIRECTORY]" >&2
  exit 2
fi

workspace="$(cd "$(dirname "$0")/.." && pwd)"
d2_binary="$1"
plugin_directory="${2:-$workspace/target/debug}"
fixture="$workspace/crates/weftan-d2/tests/fixtures/simple.json"
tmp_root="$(mktemp -d "${TMPDIR:-/tmp}/weftan-d2-plugin.XXXXXX")"
cleanup() { rm -rf "$tmp_root"; }
trap cleanup EXIT HUP INT TERM

cargo build --manifest-path "$workspace/Cargo.toml" -p d2plugin-weftan >/dev/null
plugin_directory="$workspace/target/debug"
plugin="$plugin_directory/d2plugin-weftan"

info="$($plugin info)"
flags="$($plugin flags)"
INFO_JSON="$info" FLAGS_JSON="$flags" uv run --no-project python - <<'PY'
import json, os
info = json.loads(os.environ["INFO_JSON"])
flags = json.loads(os.environ["FLAGS_JSON"])
assert info["name"] == "weftan"
assert set(info["features"]) == {
    "descendant_edges", "container_dimensions", "near_object", "top_left", "routes_edges"
}
assert {flag["Name"] for flag in flags} >= {"weftan-seeds", "weftan-report"}
PY

"$plugin" postprocess < "$fixture" > "$tmp_root/postprocess.json"
cmp -s "$fixture" "$tmp_root/postprocess.json"

"$plugin" layout < "$fixture" > "$tmp_root/layout.json"

uv run --no-project python - "$tmp_root/layout.json" "$tmp_root/routeedges.json" <<'PY'
import base64, json, pathlib, sys
full = json.loads(pathlib.Path(sys.argv[1]).read_text())
requested = dict(full)
requested["edges"] = [full["edges"][0]]
envelope = {
    "g": base64.b64encode(json.dumps(full, separators=(",", ":")).encode()).decode(),
    "gEdges": base64.b64encode(json.dumps(requested, separators=(",", ":")).encode()).decode(),
}
pathlib.Path(sys.argv[2]).write_text(json.dumps(envelope, separators=(",", ":")))
PY
route_output="$($plugin routeedges < "$tmp_root/routeedges.json")"
ROUTE_JSON="$route_output" uv run --no-project python - <<'PY'
import json, os
graph = json.loads(os.environ["ROUTE_JSON"])
assert graph["edges"][0].get("route")
PY

PATH="$plugin_directory:$(dirname "$d2_binary"):$PATH" \
  "$d2_binary" --layout=weftan --omit-version \
  "$workspace/tests/d2/direction-right.d2" "$tmp_root/weftan.svg" >/dev/null
test -s "$tmp_root/weftan.svg"

# A PATH executable with the bundled name must not replace D2's built-in TALA.
mkdir -p "$tmp_root/fake-bin"
cat > "$tmp_root/fake-bin/d2plugin-tala" <<'SH'
#!/bin/sh
echo "external tala must not be selected" >&2
exit 97
SH
chmod +x "$tmp_root/fake-bin/d2plugin-tala"
PATH="$tmp_root/fake-bin:$plugin_directory:$(dirname "$d2_binary"):$PATH" \
  "$d2_binary" --layout=tala --omit-version \
  "$workspace/tests/d2/direction-right.d2" "$tmp_root/tala.svg" >/dev/null
test -s "$tmp_root/tala.svg"

echo "D2 v0.9.0 external plugin contract: ok"
