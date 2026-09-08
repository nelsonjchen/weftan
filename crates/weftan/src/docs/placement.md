# Node placement, step by step

General placement uses two resolutions: a cheap integer lattice discovers
topology, then real rectangles refine geometry.

```text
input topology
     │
     ▼
neighbor medians ──► free integer cells ──► sizeless annealing
                                                │
                                                ▼
                                          pixel rectangles
                                                │
                                                ▼
                         sized annealing + compaction + refinement
```

## 1. Initialize a cell lattice

The engine walks reachable nodes in stable breadth-first order. A node begins
near the median cell of already placed neighbors. Free candidate cells are
enumerated in expanding Manhattan diamonds:

```text
distance 2:        ·  2  ·
distance 1:     ·  2  1  2  ·
center:         2  1  M  1  2
                ·  2  1  2  ·
                   ·  2  ·

M = neighbor median; occupied cells are skipped
```

Temporary edge length and requested direction break candidate ties. Fixed
positions anchor the search.

## 2. Anneal without dimensions

The sizeless optimizer moves cell occupants. At high temperature it can accept
worse local moves to escape a poor arrangement; cooling makes acceptance more
conservative. Horizontal and vertical compaction remove unused rows and
columns while preserving occupancy.

This phase answers “which nodes should be neighbors, and on which side?”
without repeatedly performing rectangle-overlap geometry.

## 3. Transition to real rectangles

Cell positions are scaled into layout pixels and actual node dimensions become
active. From this point the score can distinguish clearance, overlap, shape
ports, container padding, table structure, and long-distance requirements.
The same seeded random stream continues across the transition.

## 4. Optimize with sizes

For each movable node, the sized optimizer evaluates legal positions around a
neighbor-derived median. It considers edge length and direction, overlaps,
container relationships, table columns, herding, symmetry, fixed axes, and
aggregate membership. Alternating compaction keeps empty space from
accumulating during the search.

## 5. Use layered placement when appropriate

Eligible directed components follow a different branch:

```text
directed component
  → feasible DAG
  → network-simplex integer levels
  → crossing reduction within levels
  → vertical alignment blocks
  → horizontal compaction
  → real node rectangles
```

Automatic hierarchy selection is conservative. A component needs useful
source-to-sink flow, enough levels, a plausible density, bounded degree, and a
strong majority of forward directed edges. Forced hierarchy scopes attempt the
branch directly.

## 6. Refine and pack

Deterministic passes swap, mirror, transpose, align, normalize gaps, improve
clusters, balance symmetry, and equalize repeated distances. Disconnected
components are finally bin-packed. Container scopes then publish their fitted
bounds to their parents.

The split between search and refinement is important: a seed explores
topological alternatives, while the later passes regularize the chosen
candidate without adding arbitrary visual noise.
