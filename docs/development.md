# Development guide

Use the ordinary development profile while editing:

```sh
cargo check --workspace
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all --check
```

The recovered optimizer evaluates many candidate placements. Build the plugin
with the optimized `oracle` profile before complete corpus, seed-matrix, or
performance-sensitive validation:

```sh
cargo build --profile oracle -p d2plugin-weftan
```

## D2 integration

Run the public feature fixtures against a local D2 executable:

```sh
scripts/check-d2-features.sh /path/to/d2
```

If a TALA plugin directory is available, the same script can also render the
oracle fixtures:

```sh
scripts/check-d2-features.sh /path/to/d2 /path/to/tala/bin
scripts/check-tala-parity.sh /path/to/d2 /path/to/tala/bin
```

## Corpus comparison

Compare independent seeds with the optimized candidate:

```sh
uv run --no-project python scripts/compare-tala-corpus.py \
  /path/to/serialized-graphs \
  /path/to/d2plugin-tala \
  target/oracle/d2plugin-weftan \
  --seeds 1-10 \
  --jobs 8 \
  --summary-only
```

Keep a seed list intact to validate the RaceSeeds contract itself:

```sh
uv run --no-project python scripts/compare-tala-race.py \
  /path/to/serialized-graphs \
  /path/to/d2plugin-tala \
  target/oracle/d2plugin-weftan \
  --seeds 1-10 \
  --summary-only
```

The first command starts a separate process for each case and seed. The second
passes the complete seed list to each plugin and compares the graph each engine
publishes after its internal candidate race.

## License packaging

After changing Cargo dependencies or root license/notice files, run
`node scripts/sync-licenses.mjs` and commit the updated notices. CI checks
that every crate carries current license text and attribution. Build WASM
distributions with `node scripts/build-wasm.mjs --target web --release`
so the npm file allowlist includes all notices and corresponding-source
instructions. Binary release archives include these files automatically.
