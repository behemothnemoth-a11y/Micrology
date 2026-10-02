# Architecture

## The shape of it

```text
engine_core          ← serde only
engine_volume        ← engine_core
engine_geometry      ← engine_core
engine_world         ← engine_core, engine_volume
engine_destruction   ← engine_core, engine_geometry, engine_volume, engine_world
engine_stream        ← engine_core, engine_geometry, engine_volume, engine_world
engine_io            ← engine_core, engine_volume, engine_world, engine_destruction
engine_mechanics     ← engine_core, engine_destruction
engine_stress        ← engine_core, engine_geometry, engine_volume, engine_world,
                       engine_stream, engine_destruction, engine_io
apps/sandbox         ← engine_core, engine_geometry, engine_volume, engine_world,
                       engine_stream, engine_destruction, engine_io, Bevy, Avian
```

Nine library crates and one app, listed in topological order: every crate's
dependencies appear above it, so the dependency arrows only ever point left and
never back down the list. Two facts about that graph are load-bearing:

**`engine_geometry` depends only on `engine_core`.** It never learns how cells are
stored. It asks a `CellSource` "what material is at this global cell address?" and
that is the entire interface. Swapping the dense volume for an octree, or meshing
a procedurally generated region that has no storage at all, changes nothing here.
(`engine_volume` and `engine_world` appear in its `[dev-dependencies]`, as fixtures
the tests build worlds out of; the library itself never sees them.)

**Bevy appears only in `apps/sandbox`,** and almost entirely in one file
(`src/render.rs`). Engine data, editing and geometry are plain Rust that compiles
and tests without a GPU, a window or a display. `cargo test` runs the whole engine
suite in about a second.

## Who owns what

### `engine_core`

The vocabulary every other crate shares.

Coordinate spaces: `CellPos` (global cell address, the authoring space),
`VolumePos` (which storage volume owns it), `LocalPos` (offset inside the volume,
always `0..16`), plus `RegionPos` and `RenderSectionId` for the layers above.
Splitting uses `div_euclid`/`rem_euclid` throughout so every grid is uniform
across the origin — cell `-1` belongs to volume `-1`, not volume `0`. Off-by-one
here is invisible until something builds in negative space.

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

`VOLUME_EDGE = 16` lives here, alongside `REGION_EDGE_VOLUMES = 8`. Sixteen is
the microcell size the `astra-microblocks` research used, which keeps fixtures
portable. Both are tunable constants defined once, not world limits — and
crucially neither is a render boundary.

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

Two levels of sparse storage: the world owns `Region`s, and a region owns the
volumes inside its cube. Only populated volumes exist and only non-empty regions
are kept, so coordinates stay unbounded in every direction and an empty world
costs nothing.

Regions exist because residency needs a unit far larger than 16³: loading or
evicting one is a single map operation rather than 512. `insert_region` and
`remove_region` are the streaming primitives, and both mark the seam neighbours
dirty — faces hidden against a loaded neighbour become exposed when it leaves.

The world answers three questions: what is at this cell, what changed, and which
volumes are now stale. The third is the subtle one. Clearing a cell at the edge of
a volume can *expose* a face that belongs to the **neighbouring** volume's mesh,
so a boundary edit marks the neighbour dirty too — up to three neighbours for a
corner cell. Miss this and you get holes and stale walls at volume seams, visible
only in specific places and maddening to track down.

The world reports dirty **volumes**, never render sections. Which sections those
belong to is the renderer's business, resolved through `SectionGrid`. That
separation is deliberate and load-bearing — see *Spatial roles* below.

### `engine_geometry`

Two compilers behind one `SurfaceCompiler` trait, plus a cache.

`ExactCompiler` emits one unit quad per exposed face. It is simple enough to be
obviously correct, and it is **permanent**. It is written independently of the
greedy mesher rather than sharing its slice machinery, because an oracle that
shares code with the thing it validates is not an oracle.

`GreedyCompiler` sweeps each of the six directions as a stack of slices, builds a
2D mask of exposed faces per slice, and consumes the mask into maximal
same-material rectangles — grow along `u`, then grow that strip along `v`. A solid
volume collapses from 1536 unit faces to 6 quads. Merging never crosses a material
boundary.

Both read cells in global space, so they sample one cell *outside* the volume they
are compiling. That is the whole mechanism for cross-volume hidden-face removal:
volume seams simply are not render boundaries, and nothing special has to happen
at them.

Both are deterministic — fixed direction order, ascending slices, ascending `v`
then `u` — so the same cells always yield the same quads in the same order. The
`QuadSet` they return is a pure value with no timings in it, precisely so two
compilers' output can be compared for equality. That comparison is the engine's
entire correctness story for optimised meshing.

`SectionMeshCache` holds the compiled result per render section and rebuilds only
the sections whose volumes are dirty, deduplicating several dirty volumes that
share a section. It does not depend on `engine_world` — it takes a `CellSource`
and a list of dirty volumes — which is what lets the incremental rebuild path be
tested without a window.

### `engine_io`

Transparent, versioned JSON with run-length encoded cells. Readable, diffable,
hand-editable while the data model is still moving; compactness is explicitly not
a goal yet. See [`native-world-format.md`](native-world-format.md). Format v1
calls a storage volume a "chunk" because that is what the engine called it when
the format was written; those field names are frozen.

### `engine_stress`

Deterministic stress scenarios and the measurement harness over them. Counters
are reproducible and asserted in CI; timings are diagnostic and never committed.
See [`testing.md`](testing.md).

### `engine_mechanics`

Optional material mechanics and structural capacity, downstream of
`engine_destruction` so that material-aware damage can be new implementations of
the existing policy traits rather than a changed contract. Mechanical properties
live in a side table keyed by `MaterialId` rather than on `Material`, and an
empty table is a valid permanent state. Every quantity is fixed-point integer;
no floating point enters the capacity path. Nothing participates unless a host
opts in. See [`drop-0005-scope.md`](drop-0005-scope.md).

### `apps/sandbox`

The thinnest app that proves the engine. Window, free-fly camera, one entity per
section, a DDA ray through the cell grid for picking, and three edit actions. Picking
walks the cell data directly — no mesh intersection, no physics engine, because
the world data *is* the collision representation.

## The editing cycle

```text
input → World::set → volume (and boundary neighbours) marked dirty
      → SectionMeshCache::rebuild(dirty volumes)
      → volumes mapped to sections, deduplicated
      → MeshData → Bevy mesh handle replaced on the existing section entity
```

One frame, one cell changed, one or two sections re-meshed, one buffer
re-uploaded. Nothing else in the world is touched, and the HUD reports exactly how
many sections the last rebuild visited so the claim is checkable rather than
asserted.

## Where the next layers attach

These are attachment points, not plans — none of this is implemented.

- **Streaming (DROP 0002, in progress)** goes above `engine_world`: `RegionPos`
  exists, the region *container* and residency state machine do not yet. An async
  load/compile queue feeds `SectionMeshCache`; the dirty set is already an ordered
  queue of work and becomes the scheduler's input.
- **Destruction (phase C)** attaches at `World::set`. The data path already
  supports arbitrary local topology change with local rebuilds. What is missing is
  connectivity analysis and turning detached regions into bodies — and the rule
  that no cell ever becomes a rigid body still holds.
- **LOD** attaches at `SurfaceCompiler`: a compiler that reads a downsampled
  `CellSource` is a different implementation of the same trait, and the oracle
  still validates the full-resolution one. Note the oracle proves *unit-face
  equality*, which a downsampled representation will not satisfy and was never
  meant to — LOD needs its own correctness properties.
- **Generation (phase E)** can implement `CellSource` without storing anything,
  so geometry compilation works against generated terrain before any save exists.
  Delta saves then mean persisting only the regions that differ from the seed.
- **Import/export (phase G)** sits beside `engine_io` and maps foreign ids onto
  `MaterialId`. No core crate learns about the foreign format.
