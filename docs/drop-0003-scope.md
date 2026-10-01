# Micrology — DROP 0003: Destruction Foundation

*The scope as handed to the implementation, preserved. Progress is tracked in
[`drop-0003.md`](drop-0003.md); the final summary will be in
`drop-0003-report.md`.*

## Mission

Turn Micrology's static editable world into a structurally destructible world.

By the end of this drop the engine must be able to: remove cells from a supported
structure → determine which remaining cells are no longer connected to support →
extract those cells from the static world → create persistent volumetric
fragments → generate render geometry and collision for them → simulate them as
moving rigid bodies.

This is the foundation for the eventual Teardown-like side of Micrology. It is
not yet a complete destruction simulation.

## The critical rule

**Do not model physics at the cell level.** Cells remain compact data exactly as
they are now.

```
STATIC WORLD
    cells
      ↓
 structural connectivity
      ↓
detached component
      ↓
VOLUMETRIC FRAGMENT
      ↓
one rigid body
      ↓
compiled render mesh
+
compiled collision representation
```

A 5,000-cell chunk of wall is **one** volumetric body, not 5,000 physics
entities.

## Deliberately out of scope

Realistic material strength; stress/torque simulation; beams bending; fracture
propagation; fire; fluids; smoke; particle systems; explosions with sophisticated
pressure simulation; terrain generation; LOD; GPU-driven renderer redesign;
networking; gameplay scripting.

**A component attached to the world by one cell is considered supported.** That is
intentionally simplistic. The goal is topological destruction first; mechanical
failure can build on it later.

## Architecture

One new engine crate, `crates/engine_destruction/`, independent of Bevy and of any
physics library. Responsibilities: support, damage/edit analysis, connectivity,
structural jobs, fragments, collision compilation, destruction diagnostics. It is
not split into five crates up front.

The existing dependency rule stands:

```
engine code        knows volumetric data
sandbox/game host  knows Bevy, knows renderer, knows physics backend
```

## Architecture rules

- **Cells remain data.** Never create ECS entities for cells.
- **Physics bodies represent fragments**, not cells.
- **The static world stays static until something detaches.** Buildings are not
  preemptively rigid bodies.
- **Structural support is separate from material.** `Material` does not become a
  gameplay-stat dumping ground.
- **Connectivity is exact.** Later optimisations must retain an exact oracle.
- **Unknown is not empty.** Unloaded data can never be read as missing structure.
- **Workers never mutate the live world.** They return results; the owner
  validates and applies.
- **Destruction respects streaming.** Nothing here may require the whole world in
  RAM again.

## Implementation sequence

The scope's prose sections are numbered 0003.1–0003.13 by *topic*; the build order
below is the sequence given separately, and is what the pass numbers in
`drop-0003.md` follow.

```
0003.1   destruction fixtures + measurement harness
0003.2   batched transactional world edits
0003.3   anchor/support storage + SupportSource
0003.4   world format v3 foundation + support persistence
0003.5   exact connectivity/component classifier
0003.6   bounded async structural jobs + fingerprints
0003.7   detachment transaction + native Fragment
0003.8   fragment CellSource + render meshing
0003.9   exact + greedy collision compiler
0003.10  static collision host integration
0003.11  dynamic fragment physics adapter
0003.12  fragment budgets + sleeping lifecycle
0003.13  fragment persistence + spatial streaming index
0003.14  interactive carve/destruction sandbox
0003.15  torture suite + real-hardware measurements
0003.16  final DROP 0003 report
```

Each pass must retain `cargo fmt --check`, `cargo clippy --workspace
--all-targets -- -D warnings` and `cargo test --workspace`. Not one giant commit.

## Topic detail

### Measurement harness

Extend `engine_stress` with deterministic structural scenarios: `supported_column`,
`cantilever`, `two_support_bridge`, `cut_column`, `multi_island`,
`volume_boundary_break`, `region_boundary_break`, `material_noise_structure`,
`large_supported_structure`, `fragment_storm`.

Deterministic counters: cells removed; candidate roots; cells visited by
connectivity search; components discovered; supported, detached and indeterminate
components; fragment cells; largest fragment; fragment count; collision boxes
before and after optimisation; fragment mesh bytes; collision bytes.

Timings separate and never asserted: batch edit, connectivity analysis, fragment
extraction, fragment mesh compile, collision compile.

### Batched world edits

`WorldEditBatch` maps `CellPos` → new state and applies atomically: sort and
deduplicate deterministically, group by volume, modify cells, advance affected
revisions, dirty each affected volume once, dirty required neighbours once, dirty
persistence regions, and return a structured `EditOutcome { changed_cells,
changed_volumes, removed_cells, added_cells, structural_candidates }`. A one-cell
edit uses the same path.

### Support / anchor data

`trait SupportSource { fn is_anchor(CellPos) -> bool }`. `engine_destruction`
depends on `CellSource + SupportSource`, not on concrete world storage — the
`CellSource` pattern from meshing, repeated.

Storage is a compact optional sidecar per volume: `Empty` / `Uniform` / dense
bitset. A dense 16³ field is 4,096 bits = **512 bytes**, and fully
anchored/unanchored volumes need no dense allocation. Anchor state is persisted;
world format v3 is prepared during this drop.

### Exact structural connectivity

6-neighbour **face** connectivity; edge and corner contact give no support. After
removal, only surviving occupied cells adjacent to removed cells need
investigating. Per candidate component: search connected occupied cells; reaching
an anchor is `Supported`; closing without one is `Detached`; reaching unknown or
unloaded data is `Indeterminate`.

```
Supported
Detached      { cells }
Indeterminate { required_regions }
Deferred      { reason }
```

`Deferred` covers an exceeded analysis budget. **Never turn "budget exceeded" into
"detached."** Conservative failure means geometry stays static until the engine
knows.

### Bounded asynchronous structural analysis

An owned, `Send`, deterministic, bounded, revision-fingerprinted
`StructureJobInput` carrying a structural snapshot. No world locks from a worker.

Snapshot occupancy has three states: `Empty`, `Occupied`, `Unknown`. Unknown means
the data was outside the snapshot or not resident. Reaching Unknown must not
detach; return the required regions/volumes so streaming can fetch them and retry.

The job records revisions for everything it examined, support state included. If
the source changed, discard and requeue — an explosion during analysis must never
detach geometry based on an obsolete world.

### Deterministic detach transaction

Workers identify; the owning thread validates and applies, in deterministic
order (minimum global `CellPos` in the component).

```
verify structural fingerprint
↓ remove component cells from the static world
↓ create Fragment
↓ dirty affected static volumes
↓ queue static mesh/collision rebuilds
↓ queue fragment mesh/collision builds
```

Validation failure removes nothing, discards the result and retries. No
half-detached structures.

### Native Fragment

`Fragment { id, local volumes, global pose, linear velocity, angular velocity,
bounds, revision }`, owning ordinary Micrology volume data. Detached cells are
normalised into fragment-local coordinates (world `x = 1050..1080` becomes local
`x = 0..30` with global translation `x = 1050`), so gigantic coordinates never
enter moving geometry.

Fragment IDs are deterministic from the accepted transaction (destruction sequence
plus sorted component index), never from worker completion order.

A `Fragment` exposes `CellSource`, so `ExactCompiler` and `GreedyCompiler` keep
working. **No second dynamic voxel renderer** — static and detached geometry share
one compiler. That is a payoff of `CellSource`.

### Collision compiler

No collider per cell. `ExactCollisionCompiler` emits one unit box per occupied
cell and is kept permanently as the oracle. `GreedyCollisionCompiler` merges into
maximal axis-aligned boxes — a solid 16³ volume goes from 4,096 boxes to 1.
Material boundaries need not block collision merging in this drop. The invariant:
**merged boxes cover exactly the same occupied cell volume as the exact
representation**, proven with seeded random oracle tests as the render geometry
was.

Measure separately from the surface mesher: occupied cells, exact boxes, merged
boxes, bytes, compile time.

### Physics host adapter

Physics stays outside engine crates. Avian 3D 0.7 is the first integration
candidate for the sandbox: it targets Bevy 0.19, supports f32 and f64, and
supports compound colliders from multiple shapes, which maps onto merged boxes.
The adapter stays thin enough that the core does not care if Avian stays.

```
Micrology Fragment → FragmentPhysicsDescriptor → sandbox adapter → Avian rigid body
```

Micrology owns persistent fragment state; the backend owns active simulation and
reports back global pose, linear and angular velocity, and sleep state. **No Avian
type appears inside `engine_destruction`.**

Persistent fragment translation uses Micrology's large-world representation, not
global `f32`. Where the backend works locally, `global − physics origin =
physics-local`, and origin shifts preserve global pose, rotation, velocity and
contacts as far as the backend permits. Explicit far-from-origin tests: do not
solve rendering precision and then reintroduce it through physics.

Static-world collision is compiled for resident sections around the simulation
area. Render residency and physics residency may differ, usually with a smaller
physics radius:

```
storage residency · render residency · physics residency
```

Do not conflate them.

### Fragment lifecycle and budgets

Track active and sleeping fragments, fragment cells, fragment mesh bytes,
collision boxes, physics bodies, active collider bytes, structural jobs. Budget in
bytes and work, not in a fragment count — a one-cell fragment is not a
100,000-cell fragment.

The structural engine reports **every** exact detached component and never
silently deletes tiny ones during analysis. A higher-level fragment policy chooses
`RigidBody` / `Debris` / `Discard` / `Custom`; the sandbox may have a simple
debris policy. Core analysis stays exact so future games can choose differently
without rewriting destruction.

A fragment that stops moving goes `Dynamic → Sleeping` and is **not** voxelized
back into the world: it may have arbitrary rotation and cannot be losslessly
inserted into the axis-aligned grid. Sleeping fragments stay independent
volumetric objects.

### Fragment streaming and persistence

World format v3 gains persistent fragment records: id, volumetric contents, global
pose, rotation, linear and angular velocity, sleep state, bounds, revision.

A small sharded spatial index maps a region to the fragment IDs whose AABBs
overlap it. Fragment data exists **once**; it is not duplicated into every
overlapping region. As a fragment moves, the overlap set is updated
deterministically and the `FragmentId` is retained.

Becoming relevant loads static region data and discovers/loads relevant fragments.
Leaving the active area persists latest state, releases renderer assets, releases
the physics body, and releases volumetric data when safe. No fragment leak behind
the camera.

### Sandbox interaction

One simple tool: left click carves a small sphere, shift+click a larger one. Not a
weapon system. A sphere carve builds a `WorldEditBatch` and the whole pipeline
runs through to a falling structure — the first true destruction vertical slice.

The sandbox may apply an outward impulse to newly detached fragments from the
carve centre, as host/demo behaviour only. Structural detachment must not assume
every edit was an explosion: a game may remove support with a drill, corrosion,
scripting, gravity or construction changes.

### Torture pass

Pillar sever; partial cut (one remaining face path is enough); multiple islands
with deterministic identity; region-boundary collapse that must not read unloaded
space as empty; edit during analysis (old result rejected); load during analysis
(`Indeterminate` → region loads → retry → matches a fully resident synchronous
analysis); fragment collision onto floor, volume seam, region seam and other
fragments, watching for snagging or false seams; fragment storm with bounded
memory, visible body counts, bounded queues, continuing static streaming,
converging stale work and no catastrophic synchronous frame task; save while
fragments move, rotate and sleep, then reload (persistent state correctness, not
frame-exact continuation); travel away until everything unloads and return; and
destruction repeated very far from the origin, where ray hit, removed cells,
support classification, fragment extraction, physics and rendering must all refer
to the same global location.

### Diagnostics

HUD/debug telemetry for: destruction edits, cells removed; structural jobs
pending/active, structural cells visited, supported/detached/indeterminate
components, stale structural results; active and sleeping fragments, fragment
cells; fragment mesh bytes, collision boxes, collision bytes; active physics
bodies, static colliders; fragment persistence bytes.

Measure before claiming.

## Acceptance

A large structure spans multiple volumes and regions, supported by several
anchored cells. The player removes enough cells to sever its final support.
Structural analysis runs asynchronously and determines the detached component
without treating unloaded data as empty. The component is atomically removed from
the static world and becomes one or more native volumetric fragments. Those
fragments retain their exact materials, receive greedily compiled render meshes
and collision geometry, fall and collide through the sandbox physics adapter, can
sleep, can leave the streamed area, can be saved, and appear correctly when the
player returns.

Throughout: no cell becomes an ECS entity; no structural flood-fill blocks the
render frame; no stale structural result can mutate newer geometry; memory remains
bounded; global coordinates remain stable; quitting cannot silently lose the
destroyed state.

## What comes after

Not decided now. DROP 0003's measurements should choose between **renderer /
visibility / LOD** (if fragments expose GPU or draw-call limits), **advanced
destruction** (if structural analysis or collision is the limit), or an **editor**
(if the foundation is performant enough and authoring is the highest-leverage
need). Do not lock DROP 0004 before DROP 0003 says what the engine needs.
