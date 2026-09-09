# weftan-wasm

WebAssembly bindings for Weftan's core graph API and serialized D2 adapter.

Build an npm-style package with `wasm-pack`:

```sh
node scripts/build-wasm.mjs --target web --release
```

The generated module exports two synchronous functions:

```js
import init, { layout_d2_json, layout_graph_json } from "./pkg/weftan_wasm.js";

await init();

const result = JSON.parse(layout_d2_json(d2GraphJson, '{"seeds":[1]}'));
const coreResult = JSON.parse(
  layout_graph_json(weftanGraphJson, '{"seeds":[1]}'),
);
```

Both functions use JSON strings at the boundary so signed 64-bit seeds remain
exact and the same package works in browsers, Node.js, and bundlers. Layout is
synchronous and single-threaded in the initial WASM build.

For a lower-level build without `wasm-pack`, install a `wasm-bindgen-cli`
version matching the `wasm-bindgen` version in `Cargo.lock`, then run:

```sh
cargo build -p weftan-wasm --target wasm32-unknown-unknown --release
wasm-bindgen \
  --target web \
  --out-dir crates/weftan-wasm/pkg \
  target/wasm32-unknown-unknown/release/weftan_wasm.wasm
```

## Licensing and distribution

MPL-2.0; third-party terms also apply. Run the build wrapper from the
repository root to include license, attribution, and corresponding-source
files in the npm package. For the lower-level build above, run
`node scripts/package-wasm.mjs` after generating the bindings.
