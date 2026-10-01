# DROP 0002 — Scale Foundation: scope

## Goal

Prove Micrology can move from a small fully resident demo to a **large,
bounded-memory world** that streams, meshes asynchronously, and stays spatially
stable far from the origin.

This is engine work. It exists to make later systems safe to build on.

## Core decision

Scale is the right second pass, but **spatial-role separation comes before the
streaming loop**. Five concepts must stop being assumed identical:

| concept | role |
|---------|------|
| microcell | the authored unit |
| 16³ storage volume | the small editable storage leaf |
| world chunk / addressing unit | how the world is addressed |
| region / persistence unit | residency and disk grouping |
| render / mesh unit | what the renderer is handed |

They may map 1:1 for now. What must disappear is the *architectural assumption*
that one 16³ volume is one world chunk is one save unit is one renderer entity,
forever. The 16³ palette-indexed storage representation itself is good and stays.

This is the most important structural work in the drop.

## Passes

Ordered. Each leaves tests passing and lands as its own commit.

### 0002.1 — Measurement harness

Before any scale architecture changes. A repeatable headless stress harness over
deterministic generated fixtures, measuring: populated storage volumes; occupied
cells; CPU cell-storage bytes; palette bytes; exposed unit faces; greedy quads;
vertices; indices; CPU mesh bytes; full-world compile time; individual dirty
rebuild time; mesh/render unit count; resident regions; queued, completed and
discarded-stale mesh jobs.

Scenarios: **dense solid** (seam elimination, best case) · **sparse terrain**
(lots of exposed surface) · **checker/fragmentation** (hostile, greedy merging
barely helps) · **material stress** (many materials) · **edit storm** (repeated
edits in one area) · **boundary storm** (edits on volume boundaries) ·
**travel world** (long strip, for later streaming tests).

Benchmark timings are **diagnostic, never CI pass/fail**. CI asserts
deterministic counters and correctness. Baseline reports from the current
implementation are committed before major changes.

### 0002.2 — Spatial-role separation

`CellPos` stays the authoritative global authoring address. The 16³ structure
stays the storage leaf. Introduce the layering explicitly:

```
Cell → Volume/Brick → Region → World          (storage and residency)
World cells → MeshSource/RenderSection → Compiled Mesh → Renderer   (rendering)
```

A render section may initially be exactly one 16³ volume. The API must allow a
future section to cover several volumes, part of a region, a generated source, or
an LOD representation **without rewriting the world model**.

No cross-volume greedy merging yet. Cross-boundary hidden-face removal keeps
working exactly as it does. Abstraction work, not a mesher rewrite.

### 0002.3 — Global coordinates and floating render origin

World coordinates stay integer and authoritative; never mutate them to simulate
movement. Rendering computes `global − render origin → small local → f32`. The
origin shifts only on coarse, chunk/region-aligned boundaries, invisibly to the
camera.

Tests: positive and negative coordinates, crossing zero, moving the origin,
relative positions preserved across a shift, and ray/edit conversion before and
after a shift. Must land before physics, gizmos or large-world gameplay depend on
coordinate assumptions.

### 0002.4 — Region layer

A region tier above the storage leaves, with dimensions defined centrally rather
than scattered as magic numbers, and tolerant of being changed later. A region
owns or references a collection of local volumes and is the primary unit for
persistence, load/unload, residency accounting and streaming requests. **It is
not necessarily the render unit** and that distinction stays explicit.

Tests: coordinate conversion, negative coordinates, boundary cells, lookup,
insertion/removal, deterministic iteration, and identical world contents
independent of region load order.

### 0002.5 — Persistence v2

From one file to a sharded directory:

```
world/
  world.json          manifest
  regions/r.<x>.<y>.<z>.json
```

Manifest carries format, version, world id, optional name, spawn/initial camera,
and a generation-metadata placeholder — not a full generation format.

Requirements: deterministic serialization, stable region naming, safe handling of
missing region files, useful corruption errors, v1→v2 migration, round-trip byte
stability. Keep JSON unless measurement proves it dominant; do not invent a
binary format prematurely.

### 0002.6 — Residency state machine

Not a pile of booleans. An explicit, testable lifecycle:

```
Unloaded → Requested → Loading → Resident → Meshing → Ready
Ready → EvictRequested → (Saving if dirty) → Unloaded
```

The engine must distinguish: exists on disk · requested · loaded into CPU memory ·
dirty · mesh requested · mesh ready · renderer uploaded · safe to evict. Testable
with no Bevy and no GPU.

### 0002.7 — Async mesh compilation

Mesh building moves off the main schedule. A job carries enough information to
decide whether its result is still valid on completion:

```
MeshJob { render_section, source_revision, neighbour_revisions, priority }
```

**Stale-result rule:** if geometry changed after a job began, the old result must
never overwrite newer geometry. Compare revisions on completion; discard if
obsolete. Never block waiting.

Tested scenarios: edit during a running job · multiple edits before the first
result · boundary edit changing neighbour requirements · chunk unloaded mid-job ·
region evicted mid-job · same location reloaded at a newer revision · jobs
completing out of order.

### 0002.8 — Mesh scheduler

The ordered dirty set becomes scheduler input. Start small and deterministic.
Priority considers: currently visible/needed area, camera distance, dirty existing
geometry before unseen prefetch, and starvation prevention.

Must deduplicate: editing one chunk 100 times before the mesher reaches it
produces **one** pending rebuild at the newest revision, not 100. Bound active
jobs, queued jobs, and results applied per frame — avoiding frame spikes in both
directions.

### 0002.9 — Camera-relative streaming

Two radii, with the **unload radius larger** so hysteresis prevents churn at the
boundary. Required: loads ahead while travelling · regions behind become evictable
· dirty regions save before eviction · returning restores identical contents ·
memory stops growing under steady travel · rapid direction changes do not corrupt
residency · **teleporting converges on the new location instead of loading every
region in between**.

### 0002.10 — Memory budget

Residency is not defined by distance alone. Account resident cell storage,
palettes, compiled CPU mesh data, queued mesh results, and renderer allocations
where observable, against a configurable budget. Distance sets preference; budget
sets what may remain. Evict preferring distant, not visible, clean, least recently
needed. Establish boundedness — not a sophisticated replacement policy.

### 0002.11 — Renderer granularity firewall

One mesh per render section stays acceptable *as this drop's implementation*. It
is not acceptable as a permanent engine contract. A renderer-facing handle
(`RenderSectionId` / `MeshHandle` / `CompiledSection`) means the world requests
geometry without knowing whether the renderer later combines 1, 8 or 64 volumes
into one submission. Do not speculatively solve 100,000 render entities — make the
relationship replaceable and let profiling choose later.

### 0002.12 — Low-risk mesh memory cleanup

Only after the harness exists, and only where measurement justifies it: remove UV
data if genuinely unused · `u16` indices where the section's vertex count allows ·
section-local positions · avoid duplicate CPU copies after GPU upload. Packed
normals, tight quantisation, meshlets and custom shaders belong to a later
renderer pass unless measurement shows otherwise.

## The exact oracle stays

`ExactCompiler` is kept, with its role slightly narrowed to **LOD0 / source
geometry correctness oracle**. It is not run across enormous streamed worlds in
normal operation. It continues to validate deterministic fixtures, random local
worlds, boundaries, negative coordinates, streamed-in regions, edits, and future
generated cell sources.

When LOD representations arrive they get their own correctness properties. LOD
geometry will not necessarily equal LOD0 geometry, so the unit-face oracle must
not be abused to prove something it was never designed to prove. It remains the
right oracle for canonical fine geometry.

## Explicitly out of scope

LOD · structural destruction · rigid-body fragments · physics · procedural planet
generation · biomes · caves · BuildWright · Minecraft/Litematica import · editor
UI · scripting · networking · multiplayer · cross-section greedy merging ·
meshlets · GPU-driven rendering rewrite · occlusion system · game framework.

## Required test expansion

**Spatial** — global→volume, global→region, negative coordinates, boundaries,
origin shifting.
**Persistence** — v1 migration, region save/load, canonical serialization, missing
regions, malformed regions, dirty-region save.
**Streaming** — enter region, leave region, hysteresis, revisit, teleport, bounded
residency.
**Async** — stale completion, out-of-order completion, unload while compiling,
repeated dirties, neighbour-boundary invalidation.
**Determinism** — equivalent streaming paths run in different request and
completion orders must produce identical world state and canonical geometry.

## Diagnostics

Camera global position · render origin · resident regions · resident volumes ·
loaded cells · dirty sections · queued/active/completed/discarded mesh jobs · CPU
cell bytes · CPU mesh bytes · visible mesh count. Timings informational. Available
independently enough that a future editor can consume them.

## Acceptance tests

1. **Endless travel strip.** A deterministic world far larger than resident
   memory, flown continuously: regions load ahead, old ones leave memory, memory
   settles in a bounded range, no seams, no permanent holes from async ordering,
   coordinates stay correct.
2. **Edit and return.** Modify geometry, travel until its region unloads, return —
   the modification is exact.
3. **Async edit storm.** Edit faster than meshing completes; final displayed
   geometry is the newest revision and no old mesh reappears.
4. **Extreme coordinates.** Load geometry very far from origin; rendering stays
   stable across repeated origin shifts with no visible relative jump.
5. **Deterministic reload.** Save a streamed world, reload — cell contents and
   compiled geometry match.
6. **Boundary torture.** Edit volume edges, corners, region edges and negative
   boundaries — no missing or duplicated faces.

## Performance acceptance

Not a universal FPS number. Success means: main-thread mesh compilation is gone ·
per-frame work is bounded · resident memory is bounded · loading and mesh
completion are rate limited · stale work does not accumulate · real hardware
traverses the stress world without large synchronous meshing stalls.

Record real-hardware measurements once available. **Never replace measurements
with estimates.**

## Invariants to preserve

Core engine crates stay independent of Bevy · cells stay data · `CellSource` stays
storage-independent · `ExactCompiler` stays an independent oracle · greedy output
stays deterministic · boundary edits keep invalidating neighbouring geometry
correctly · Minecraft stays entirely outside core engine architecture.

## Completion target

Micrology can traverse a world far larger than resident memory, asynchronously
load and mesh only what it needs, preserve edits through eviction and reload,
maintain stable rendering far from the global origin, and stay deterministic and
bounded under stress.
