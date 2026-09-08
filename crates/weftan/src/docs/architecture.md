# Engine architecture

Weftan is a staged layout engine. It deliberately does not optimize one global
formula from input to output. Each stage owns a narrower decision once the
facts it needs are available.

## Workspace boundaries

```text
D2 process
   │ binary-plugin commands and serialized graph JSON
   ▼
d2plugin-weftan       executable dispatch, stdout/stderr discipline, reports
   │
   ▼
weftan-d2             lossless D2 JSON ↔ typed graph conversion
   │
   ▼
weftan                diagram-language-independent layout and routing
```

The core crate does not know D2's complete document schema. The adapter owns
shape, label, icon, fixed-position, and serialization conversion. The binary
owns process protocol behavior. This separation lets Rust applications use
[`crate::Graph`] directly without constructing D2 JSON.

> **Working mental model:** recognize structure, place boxes, route paths,
> place typography, then select a complete candidate.

The diagram below groups the production pipeline into its major information
boundaries. Arrows carry owned geometry forward; later stages do not mutate the
caller's [`crate::Graph`].
