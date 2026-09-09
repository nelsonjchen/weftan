
The race has a time budget. Selection is deterministic over the set of
candidates that actually complete within that budget, but completion timing can
change that set. Therefore a multi-seed call should not be described as wholly
deterministic across differently loaded machines.

## Two related scores

[`crate::Score`] is a lexicographically ordered diagnostic summary. Its fields
are compared in declaration order: invalid geometry dominates route
collisions, which dominate crossings, and so on.

Layout candidate selection also publishes a TALA-compatible scalar
[`crate::CandidateReport::race_score`] and its optional
[`crate::RaceScoreComponents`] breakdown. The scalar incorporates route turns,
diagonal segments, crossings, area, and label terms with compatibility-specific
precision behavior. Standalone routing does not use that layout-candidate
scalar.

## Reading a report

```text
LayoutReport
├── selected_seed / selected_pass   winner identity
├── score                           winner's diagnostic Score
├── improvement_passes              completed candidates considered
├── candidates[]
│   ├── seed / pass
│   ├── score
│   ├── race_score + components
│   └── accepted                    validation result
└── warnings[]                      non-fatal race conditions
```

Use the report for diagnostics and compatibility work. Treat it as versioned
engine output rather than a permanent database schema.

## Reproducibility checklist

Record the exact input graph, ordered seed list, Weftan version, platform,
layout constraints, and—when testing multi-seed timeout behavior—the completed
candidate set. Use isolated single-seed runs when comparing placement
algorithms rather than race timing.
