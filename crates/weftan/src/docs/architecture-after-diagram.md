
## Step 1: normalize and classify

The input is copied into an internal arena with stable node and edge IDs. The
engine validates references, derives a cell size, records container scopes, and
recognizes sequences, trees, hierarchies, clusters, and hubs. Some structures
temporarily become carrier nodes so an outer scope can place the whole
structure as one object.

Recognition order is behavioral:

```text
sequences → ordinary preprocessing → trees → containers
          → hierarchies → clusters → hubs
```

Each pass sees the temporary graph produced by the previous pass. Changing
that order can change component membership, random-number consumption, and
equal-score choices.

## Step 2: solve recursive scopes

Containers are visited in postorder:

```text
outer container
├── ordinary sibling
└── inner container
    └── deepest container
        ├── B
        └── C

1. place B and C
2. fit deepest container
3. place and fit inner container
4. place inner container beside ordinary sibling
5. fit outer container
```

Within a scope, a strongly directed eligible component can use layered
hierarchy placement. General components use seeded cell-grid placement followed
by size-aware optimization. Disconnected components are solved independently
and packed together later.

## Step 3: refine real boxes

Placement is followed by small, ordered improvement passes: swaps, directional
mirroring, transposition, axis alignment, gap normalization, cluster shaping,
symmetry, equidistance, and bin packing. Repeated stages are intentional; one
pass can create an opportunity owned by a later repeat.

## Step 4: route and finish

Only settled rectangles become routing obstacles. The router selects shape
ports, builds orthogonal visibility corridors, searches legal paths, and then
runs a cleanup pipeline. Labels and icons are placed after their owning boxes
or routes have final geometry. A final translation normalizes the output frame.

## Step 5: select a candidate

Every seed produces an entire candidate—boxes, routes, labels, and score.
Candidate geometry is never mixed or averaged. See
[`crate::guide::seeds_and_scoring`] for concurrency and ordering details.

## Where code lives

| Concern | Internal module family |
|---|---|
| Stable arena and snapshots | `engine::model`, `engine::snapshot` |
| Stage ordering | `engine::pipeline` |
| Structure recognition | `sequences`, `trees`, `clusters`, `hierarchy_assignment` |
| General placement | `scoring_sizeless`, `sizeless`, `sized`, `optimization` |
| Layered placement | `hierarchy_network_simplex`, `hierarchy_order`, `hierarchy_placement` |
| Recursive ownership and packing | `hierarchy`, `containers`, `binpack`, `root_packing` |
| Edge paths | `routing` and its submodules |
| Typography | `labels`, `labels::node` |
| Candidate evaluation | `evaluation` |

Run `cargo doc --workspace --document-private-items --open` when working on
the engine to include those internal modules in the generated reference.
