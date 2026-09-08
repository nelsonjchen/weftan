# Compatibility status

Weftan is being brought to parity with the released D2 v0.9.0 TALA and its
external layout-plugin protocol. The oracle pin, source trace patch, and build
instructions are in [`d2-v0.9.0-oracle.md`](d2-v0.9.0-oracle.md).

The simple, label-position, icon-position, all-shapes, and flipt witnesses have
identical normalized traces, including stable cluster-vessel projection.
The broad baseline has 108 exact cases out of 116 that completed within the
diagnostic budget; five cases timed out or exceeded the oracle work limit.
The full measurement is recorded in
[`tala-parity-baseline.json`](tala-parity-baseline.json).

The Weftan engine suite currently has 487 passing tests and 15 older exact
recovery assertions that still fail while the OSS parity work is in progress;
the plugin protocol tests all pass.
Those failures are kept visible rather than being changed to conceal the
remaining implementation gaps.

`tala-re` is frozen historical reference material and is not changed by this
parity work.
