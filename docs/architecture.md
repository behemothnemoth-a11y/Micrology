# Architecture

## The shape of it

```text
engine_core ──┬── engine_volume ──── engine_world ──┐
              │                                     ├── engine_io
              └── engine_geometry ──────────────────┤
                                                    └── apps/sandbox (Bevy)
```

Five small library crates and one app. The dependency arrows only ever point left,
and two of them are load-bearing:

**`engine_geometry` depends only on `engine_core`.** It never learns how cells are
stored. It asks a `CellSource` "what material is at this global cell address?" and
that is the entire interface. Swapping the dense volume for an octree, or meshing
a procedurally generated region that has no storage at all, changes nothing here.

**Bevy appears only in `apps/sandbox`,** and almost entirely in one file
(`src/render.rs`). Engine data, editing and geometry are plain Rust that compiles
and tests without a GPU, a window or a display. `cargo test` runs the whole engine
suite in about a second.

## Who owns what

### `engine_core`

The vocabulary every other crate shares.

Three coordinate spaces: `CellPos` (global cell address, the authoring space),
`ChunkPos` (which chunk owns it), `LocalPos` (offset inside the chunk, always
`0..16`). Splitting uses `div_euclid`/`rem_euclid` so the grid is uniform across
the origin — cell `-1` belongs to chunk `-1`, not chunk `0`. Off-by-one here is
invisible until something builds in negative space.

`FaceDir` carries more weight than it looks. Each of the six directions exposes an
in-plane basis `(u, v)` chosen so that `u × v` equals the outward normal. Every
mesher builds quads as `origin, +u, +u+v, +v`, which is therefore
counter-clockwise viewed from outside — consistent winding falls out of the basis
instead of being hand-maintained per direction. A test asserts the cross product
for all six.

`MaterialId` is the engine's own stable id — never a foreign registry id. A
`Material` is an id, an RGB colour and an optional name. That is all DROP 0001
needs; the type is additive-extensible and the save format ignores unknown keys.

`Revision` exists so derived data can be cached against the version of the data it
came from rather than a dirty flag, which distinguishes "unchanged" from "changed
and changed back".

`CHUNK_EDGE = 16` lives here. It is 16 because that is the microcell size the
`astra-microblocks` research used, which keeps fixtures portable. It is a tunable
constant, not a world limit — and crucially not a render boundary.

### `engine_volume`

One 16³ volume: a palette index per cell, `u16`.

Storage has three tiers, and the tier is invisible to callers:

| tier | when | cost |
|------|------|------|
| `Empty` | nothing occupied | no heap |
| `Uniform` | every cell the same material | no heap |
| `Dense` | anything else | 8 KiB |

This is design principle 4 ("static geometry should become cheap") applied to
storage. A volume promotes to `Dense` on the first edit that needs it and demotes
on `compact()`. Sparser representations later — octree, bitset, RLE in memory —
are changes behind the same `get`/`set` API.

`compact()` also sorts the palette by material id and drops dead entries. That is
what makes serialization canonical: two volumes with the same cells produce
identical bytes no matter what order they were painted in.

Volumes compare by **contents**, not representation. An `Empty` volume and a
`Dense` one full of zeros are equal. Without that, "save and reload are equal" is
not a meaningful test.

### `engine_world`

A sparse `BTreeMap<ChunkPos, Chunk>`. Only chunks holding something exist, so
coordinates are unbounded in every direction and an empty world costs nothing.

The world answers three questions: what is at this cell, what changed, and which
chunks must be re-meshed. The third is the subtle one. Clearing a cell at the edge
of a chunk can *expose* a face that belongs to the **neighbouring** chunk's mesh,
so a boundary edit marks the neighbour dirty too — up to three neighbours for a
corner cell. Miss this and you get holes and stale walls at chunk seams, visible
only in specific places and maddening to track down.

Deliberate non-decisions: a chunk is currently exactly one volume, and regions
above chunks do not exist. Decoupling chunk size from volume size, and adding a
region tier, is phase B.

### `engine_geometry`

Two compilers behind one `SurfaceCompiler` trait, plus a cache.

`ExactCompiler` emits one unit quad per exposed face. It is simple enough to be
obviously correct, and it is **permanent**. It is written independently of the
greedy mesher rather than sharing its slice machinery, because an oracle that
shares code with the thing it validates is not an oracle.

`GreedyCompiler` sweeps each of the six directions as a stack of slices, builds a
2D mask of exposed faces per slice, and consumes the mask into maximal
same-material rectangles — grow along `u`, then grow that strip along `v`. A solid
chunk collapses from 1536 unit faces to 6 quads. Merging never crosses a material
boundary.

Both read cells in global space, so they sample one cell *outside* the chunk they
are compiling. That is the whole mechanism for cross-chunk hidden-face removal:
chunk seams simply are not render boundaries, and nothing special has to happen at
them.

Both are deterministic — fixed direction order, ascending slices, ascending `v`
then `u` — so the same cells always yield the same quads in the same order. The
`QuadSet` they return is a pure value with no timings in it, precisely so two
compilers' output can be compared for equality. That comparison is the engine's
entire correctness story for optimised meshing.

`ChunkMeshCache` holds the compiled result per chunk and rebuilds only the chunks
it is handed. It does not depend on `engine_world` — it takes a `CellSource` and a
list of dirty chunks — which is what lets the incremental rebuild path be tested
without a window.

### `engine_io`

Transparent, versioned JSON with run-length encoded cells. Readable, diffable,
hand-editable while the data model is still moving; compactness is explicitly not
a goal yet. See [`native-world-format.md`](native-world-format.md).

### `apps/sandbox`

The thinnest app that proves the engine. Window, free-fly camera, one entity per
chunk, a DDA ray through the cell grid for picking, and three edit actions. Picking
walks the cell data directly — no mesh intersection, no physics engine, because
the world data *is* the collision representation.

## The editing cycle

```text
input → World::set → chunk (and boundary neighbours) marked dirty
      → ChunkMeshCache::rebuild(dirty) → MeshData
      → Bevy mesh handle replaced on the existing chunk entity
```

One frame, one cell changed, one or two chunks re-meshed, one buffer re-uploaded.
Nothing else in the world is touched, and the HUD reports exactly how many chunks
the last rebuild visited so the claim is checkable rather than asserted.

## Where the next layers attach

These are attachment points, not plans — none of this is implemented.

- **Streaming (phase B)** goes above `engine_world`: a region tier over chunks, an
  async load/compile queue feeding `ChunkMeshCache`, camera-relative residency.
  The dirty set is already an ordered queue of work; it becomes a scheduler input.
- **Destruction (phase C)** attaches at `World::set`. The data path already
  supports arbitrary local topology change with local rebuilds. What is missing is
  connectivity analysis and turning detached regions into bodies — and the rule
  that no cell ever becomes a rigid body still holds.
- **LOD** attaches at `SurfaceCompiler`: a compiler that reads a downsampled
  `CellSource` is a different implementation of the same trait, and the oracle
  still validates the full-resolution one.
- **Generation (phase E)** can implement `CellSource` without storing anything,
  so geometry compilation works against generated terrain before any save exists.
  Delta saves then mean persisting only the chunks that differ from the seed.
- **Import/export (phase G)** sits beside `engine_io` and maps foreign ids onto
  `MaterialId`. No core crate learns about the foreign format.
