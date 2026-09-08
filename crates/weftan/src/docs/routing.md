# Edge routing, step by step

Routing starts after node and container rectangles stop moving. An edge must
choose where it touches each shape, which obstacle-free corridors it follows,
and how it interacts with routes accepted earlier.

```text
settled boxes
    → legal source/target ports
    → visibility corridors around obstacles
    → candidate path search and interaction scoring
    → route cleanup
    → clip endpoints to exact shape borders
```

The SVG below illustrates the central search problem: the direct segment is
blocked, so a legal orthogonal route uses visibility lanes around the obstacle.
