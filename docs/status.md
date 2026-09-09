# Compatibility status

Weftan is being brought to parity with the released D2 v0.9.0 TALA and its
external layout-plugin protocol. The oracle pin, source trace patch, and build
instructions are in [`d2-v0.9.0-oracle.md`](d2-v0.9.0-oracle.md).

The checked-in corpus now has identical public boxes, routes, labels, icons,
and normalized traces for every successful case: 120 cases at seed 1. One
large checkered-grid case reaches the same documented TALA work-limit error on
both engines. The full measurement is recorded in
[`tala-parity-baseline.json`](tala-parity-baseline.json).

The D2 v0.9.0 external-plugin contract passes, including protocol discovery,
feature advertisement, layout selection, routing, and the bundled-TALA
override check.

`tala-re` is frozen historical reference material and is not changed by this
parity work.
