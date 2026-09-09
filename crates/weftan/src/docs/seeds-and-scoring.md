# Seeds, determinism, and scoring

A seed controls deterministic shuffles, annealing proposals, and equal-choice
ordering. It does not perturb finished coordinates after layout.

## One seed

With one seed, one complete pipeline runs:

```text
Graph + seed 7
      → recognize → place → route → label → normalize → Candidate(7)
```

For the same graph, engine version, platform assumptions, options, and seed,
this is the clearest reproducibility mode.

## Multiple seeds

With several seeds, each seed owns an independent copy of the graph and runs a
complete candidate. Workers send finished candidates to a selector. Candidate
rectangles and routes are never combined.
