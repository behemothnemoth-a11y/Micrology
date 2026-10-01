# DROP 0002 — Scale Foundation: report

DROP 0001 made a volumetric world correct. DROP 0002 makes it **big**: a world
larger than memory, streamed around a moving camera, meshed off the main thread,
bounded by a memory budget, and persisted a region at a time.

Everything below is a measurement, not an estimate. Where a number appears, the
command that produced it is named.

![The sandbox streaming a 144-region world, seen from above. The pale seam
material marks region boundaries, and the hard edge is where residency ends.](images/streaming.png)

---

## 1. What shipped

| pass | what it is |
|---|---|
| 0002.1 | measurement harness: seven deterministic scenarios, committed counters |
| 0002.2 | spatial-role separation: cell → volume → region → world, and render sections apart from all of it |
| 0002.3 | floating render origin: integer world coordinates, `f64` camera, section-local meshes |
| 0002.4 | the region layer: regions own volumes, two distinct kinds of "dirty" |
| 0002.5 | world format v2: sharded directory, atomic writes, v1 migration |
| 0002.6 | residency state machine with checked transitions |
| 0002.7 | async mesh jobs with fingerprint-based stale-result protection |
| 0002.8 | prioritised, bounded scheduler with starvation promotion |
| 0002.9 | camera-relative streaming, hysteresis, teleport handling |
| 0002.10 | byte-denominated memory budget, eviction, in-flight backpressure |
| 0002.11 | renderer granularity audit — measured, no change justified |
| 0002.12 | measured mesh-memory reductions, −20.4% |
| 0002.13 | torture pass, acceptance tests, shutdown flush |

**309 tests pass.** `cargo fmt --check`, `cargo clippy --workspace --all-targets
-- -D warnings` and `cargo test --workspace` are clean.

| suite | tests | what it covers |
|---|---|---|
| `engine_core` (unit) | 30 | coordinates, spatial roles, render origin |
| `engine_volume` (unit) | 13 | the 16³ leaf and its three storage tiers |
| `engine_world` (unit + `regions`, `picking`) | 49 | regions, editing, dirty tracking, raycast |
| `engine_geometry` (`surface`, `oracle`, `incremental`, `indices`) | 45 | mesher against the exact oracle, index width |
| `engine_io` (`persistence`, `persistence_v2`) | 41 | v1 and v2 formats, migration, atomicity |
| `engine_stream` (unit + `residency`, `mesh_jobs`, `scheduler`, `streamer`, `torture`) | 113 | residency, jobs, scheduling, streaming, adversarial orderings |
| `engine_stress` (unit + `baseline`, `granularity`) | 17 | the harness itself, and the granularity invariants |

(308 test functions plus one doc-test.)

---

## 2. The crate map DROP 0003 inherits

```
crates/engine_core/      coordinates, materials, revisions, CellSource,
                         spatial roles (VolumePos / RegionPos / RenderSectionId),
                         SectionGrid, GlobalPos, RenderOrigin
crates/engine_volume/    the 16³ leaf: palette-indexed cells, three storage tiers
crates/engine_world/     regions own volumes; editing; dirty tracking; raycast
crates/engine_geometry/  ExactCompiler (oracle) + GreedyCompiler + SectionMeshCache
crates/engine_io/        format v1 (single file) and v2 (sharded directory)
crates/engine_stream/    residency, mesh jobs, scheduler, streamer, memory budget
crates/engine_stress/    deterministic scenarios, granularity audit, world generator
apps/sandbox/            the only code that knows a renderer exists
```

The dependency rule has not been broken: **no engine crate depends on Bevy, and
no engine crate knows a renderer exists.** `apps/sandbox` is the single place
engine geometry meets a GPU, and `cargo test` never builds Bevy because `sandbox`
is excluded from the workspace's `default-members`.

### The seams that matter

- **`CellSource`** — anything that can answer "what material is at this cell".
  The mesher takes one of these, which is why a mesh job can own a *snapshot* of
  cells and run on a worker thread without the world.
- **`SurfaceCompiler`** — `ExactCompiler` and `GreedyCompiler` are
  interchangeable, and the exact one is kept permanently as the oracle every
  optimisation is checked against.
- **`SectionGrid`** — the render unit, uncoupled from storage. Proven uncoupled
  by test, not by assertion (see §6).
- **`RegionStreamer`** — decides *what* should happen and returns `StreamAction`s.
  It never performs I/O, never spawns a task, and never touches an entity. The
  host carries the actions out and reports back.
- **`MeshScheduler`** — owns priority, bounds and staleness. Same contract: it
  hands out `MeshJobInput`s and judges `MeshJobResult`s, and knows nothing about
  threads.
- **`MemoryAccount` / `MemoryBudget`** — the host measures, the engine decides.
  The engine cannot see the renderer's memory, so it is *told*.

---

## 3. Stress baseline: what changed and why

`cargo run --release -p engine_stress --bin stress -- --check`

| scenario | volumes | cells | faces | quads | mesh (DROP 0001) | mesh (now) |
|---|---|---|---|---|---|---|
| `dense_solid` | 64 | 262,144 | 24,576 | 96 | 20.3 KiB | **16.2 KiB** |
| `sparse_terrain` | 111 | 259,044 | 49,448 | 9,504 | 1.96 MiB | **1.56 MiB** |
| `checker` | 8 | 16,384 | 98,304 | 98,304 | 20.25 MiB | **16.12 MiB** |
| `material_stress` | 8 | 32,768 | 6,144 | 6,144 | 1.27 MiB | **1.01 MiB** |
| `edit_storm` | 8 | 32,646 | 6,844 | 674 | 142.2 KiB | **113.2 KiB** |
| `boundary_storm` | 8 | 32,644 | 6,708 | 696 | 146.8 KiB | **116.9 KiB** |
| `travel_strip` | 110 | 249,780 | 55,666 | 8,856 | 1.82 MiB | **1.45 MiB** |
| **total** | | | | | **25.60 MiB** | **20.38 MiB** |

Across all thirteen passes the baseline moved exactly **twice**, and both times
for a stated reason:

1. **0002.4** moved one field: `resident_regions`, 0 → real, because regions
   started existing.
2. **0002.12** moved one field: `mesh_bytes`, −20.4%, with every geometry counter
   byte-identical.

Every other pass — the spatial-role split, the floating origin, the region layer,
the v2 format, residency, async jobs, the scheduler, streaming, the budget, the
granularity audit — left the baseline **byte-identical**. That is the harness
earning its keep: a refactor that changes no behaviour should be provably unable
to change behaviour.

---

## 4. Real resident-memory behaviour

`MICROLOGY_BUDGET_MB=24` against a generated 144-region world (57.2M cells,
**153.3 MiB** if ever all resident), flown across the terrain:

| | sample 1 | sample 3 | sample 5 | sample 6 |
|---|---|---|---|---|
| in use | 18.5 MiB | 22.7 MiB | 18.2 MiB | 22.3 MiB |
| pressure | OVER SOFT | OVER SOFT | OVER SOFT | OVER SOFT |
| load radius in use | 1 (configured 2) | 1 | 1 | 1 |
| evicted for budget | 0 | 55 | 102 | 137 |
| loads withheld | 0 | 8,513 | 52,212 | 55,826 |
| resident regions | 175 | 129 | 99 | 149 |
| cell bytes | 8.3 MiB | 9.6 MiB | 7.5 MiB | 9.6 MiB |
| **mesh bytes** | **10.1 MiB** | **13.0 MiB** | **10.7 MiB** | **12.7 MiB** |
| entities | 1,340 | 1,722 | 1,408 | 1,731 |

- Memory stayed between 18.2 and 22.7 MiB against an 18 MiB soft target and a
  24 MiB hard ceiling. **The ceiling was never crossed.**
- The load radius shrank from 2 to 1 under pressure and the engine kept
  streaming. Reaching outward stops before anything resident is given up.
- **Mesh bytes exceeded cell bytes in every sample.** A budget denominated in
  regions, or even in cells, would have been measuring the smaller half.
- Residency oscillated 99–175 and returned. No ratchet.

A flight out of the world entirely ended at cell `-4976 -3167 -5842` with
volumes, entities and mesh bytes all **0**, and the render origin region-aligned
throughout. Nothing leaks behind the camera.

### Why the budget is in bytes

From the very first baseline:

| scenario | mesh bytes per cell |
|---|---|
| `dense_solid` | **0.1 B** |
| `sparse_terrain` | 6.3 B |
| `travel_strip` | 6.1 B |
| `material_stress` | 32.2 B |
| `checker` | **1,032.0 B** |

Four orders of magnitude. Any budget denominated in regions, volumes or cells is
wrong by that factor somewhere.

---

## 5. Async job statistics

From the acceptance run: **3,809 mesh results applied, 1 discarded as stale**, 0
discarded as unwanted, with 1,857 section entities live.

Stale results are rare in ordinary play and must still be impossible to apply.
The guard is `SectionFingerprint`: the revisions of a section's volumes *plus
their six neighbours*, with `Option<Revision>` so an absent volume is
distinguishable from an untouched one. It is deliberately conservative — an edit
anywhere in an adjacent volume invalidates the section, even where it could not
have changed the shared boundary — and that conservatism is documented and tested
rather than discovered later.

Under the deliberately hostile `an_edit_storm_with_scrambled_completion` fixture,
where edits land *after* snapshots are taken and results come back in a seeded
permutation, the test asserts stale results actually occur (or it would be
proving nothing) and that every section still converges on exactly the geometry a
synchronous rebuild would have produced.

Bounds, all tested:

| bound | what it protects |
|---|---|
| `max_active_mesh_jobs` | worker load |
| `max_in_flight_snapshot_bytes` | snapshot memory, which the job count does *not* bound |
| `max_results_applied_per_tick` | the frame spike from uploading geometry |
| `max_pending_mesh_requests` | reports a misbehaving prefetch policy instead of absorbing it |
| `starvation_ticks` | promotes work that has waited, so distance cannot starve it forever |

Stale results do not consume the apply budget: discarding costs nothing, so it
must not crowd out real work.

---

## 6. Renderer granularity

`cargo run --release -p engine_stress --bin granularity`

| grid | volumes/section | sections | total mesh | largest section | bytes/edit | uploads/edit |
|---|---|---|---|---|---|---|
| **1** | 1 | 309 | 25.3 MiB | **2.5 MiB** | **68.8 KiB** | 1.47 |
| 2 | 8 | 44 | 25.3 MiB | 20.2 MiB | 80.7 KiB | 1.00 |
| 4 | 64 | **17** | 25.3 MiB | 20.2 MiB | 80.7 KiB | 1.00 |

**Verdict: keep `SectionGrid = 1`.**

- Coarsening buys **draw calls, and only draw calls** — 309 → 17, an 18×
  reduction. That is a real and potentially large win, and it is a *GPU* win,
  which a software rasteriser cannot measure. It needs real hardware before it
  changes anything.
- Coarsening costs **edit latency, now**: median rebuild 0.99 → 13.49 ms on
  `edit_storm`, 2.12 → 12.59 ms on `boundary_storm`. 13.5 ms for one changed cell
  is a dropped frame at 60 Hz.
- It also raises the largest single buffer 8× and job snapshots 12×.
- After 0002.12 it can even **cost memory**: a section of 65,536+ vertices needs
  32-bit indices, and `checker` crosses that line at grid 2 for +1.13 MiB of
  byte-identical geometry.

The honest counter-argument: coarsening cuts fingerprint invalidations per edit
from 4.00 to 1.00, because neighbours fall inside the same section. Real, but far
smaller than a 13× latency regression.

Granularity stays **configurable and uncoupled**, and that is asserted rather than
claimed. At every audited grid, tests prove the surface is bit-identical (a
coarser grid producing fewer quads would mean the mesher had begun merging across
volumes, which would make incremental rebuilds unsound), that the trade runs the
direction the audit says, and that coarsening the default later fails a test
naming the audit instead of passing as a tweak.

---

## 7. Mesh memory, before and after

| | quads | vertices | indices | mesh bytes |
|---|---|---|---|---|
| DROP 0001 | — | — | — | 25.60 MiB |
| after 0002.12a (no UVs) | unchanged | unchanged | unchanged | 21.81 MiB (−14.8%) |
| after 0002.12b (u16 indices) | unchanged | unchanged | unchanged | **20.38 MiB (−20.4%)** |

−20.4% on every scenario, to the byte, with only `mesh_bytes` moving in the
baseline.

- **UVs**: built in cell units so a merged quad would tile, uploaded, and never
  read. Nothing was lost — a quad's UVs are a pure function of its `du` and `dv`,
  and the non-obvious part (cell units, not normalised) is now a comment on
  `Quad`.
- **Indices**: `U16` or `U32`, chosen from the final vertex count. The whole risk
  is one number — a `u16` addresses `0..=65535`, so the first mesh that cannot use
  one has **65,536** vertices, and an off-by-one there silently truncates indices
  and draws garbage. Tested from both sides at exactly 65,532 and 65,536.

At the shipped granularity the narrow path always wins: the worst a single 16³
volume can produce is a checkerboard at 2,048 cells × 6 faces × 4 unshared
vertices = **49,152**. A test pins that number, because coarsening `SectionGrid`
would invalidate it with nothing else noticing.

---

## 8. Region I/O

`cargo run --release -p engine_stress --bin regionio -- <world>`

| | |
|---|---|
| region | 163 volumes, 511,198 cells |
| resident | 705.3 KiB |
| shard on disk | 707.0 KiB |
| **load** | **median 3.41 ms** (2.64–4.79) |
| **save** | **median 3.45 ms** (3.17–4.13) |

3.4 ms is about a fifth of a 60 Hz frame, which settles whether region I/O
belongs on the main thread. It does not.

The three staleness rules that make async I/O safe, each tested:

- **Load**: a per-region generation token. A load that returns after the camera
  has moved on is discarded rather than inserted.
- **Save**: `SaveTicket.revision`. A completed save of revision N must not mark
  revision N+1 clean — getting this wrong loses a player's work silently.
- **Mesh**: `SectionFingerprint`, as above.

---

## 9. Known remaining bottlenecks

Measured or observed, not speculated:

1. **Draw calls.** 1,400–1,700 entities in the acceptance run, one per section.
   0002.11 says coarsening would cut this ~18× and that the decision needs real
   GPU evidence. **This is the largest known unexploited win.**
2. **`rebuild_sections` compiles absent volumes.** It iterates every volume a
   section covers, present or not. Free at grid 1; most of the 13.5 ms at grid 4.
   Only worth fixing if granularity is ever coarsened.
3. **Vertices are not shared between quads.** Four unshared vertices per quad, so
   a cell's corner is emitted up to twelve times. Index reuse would cut vertex
   memory substantially and was deliberately deferred: it changes geometry, and
   DROP 0002 confined itself to reductions that provably do not.
4. **The world format is JSON.** It round-trips at roughly 1:1 with resident size
   and costs ~3.4 ms a region. Readable and debuggable, which was the right trade
   while the format was still moving; a binary format is an obvious later win.
5. **A region that loads empty still leaves a residency record.** Cheap, but
   `region_count` therefore overstates memory — which is exactly why the budget
   counts bytes.
6. **Palette compaction is not automatic.** A volume emptied by editing keeps its
   allocation until something compacts it; `allocated_volume_count()` reports the
   difference.
7. **No LOD, no occlusion, no frustum culling.** Everything resident is drawn.

---

## 10. What DROP 0003 inherits

A world that is **correct, streamed, bounded and measured**:

- Integer world coordinates with a floating render origin, so distance does not
  degrade precision and a rebase costs one transform per section rather than a
  rebuild.
- Regions as the unit of residency and persistence; volumes as the unit of
  storage; sections as the unit of rendering — three roles that were one concept
  in DROP 0001 and are now separately addressable.
- A streamer that decides and a host that acts, with no engine crate aware of
  Bevy, threads or entities.
- Three independent staleness rules, each tested against forced orderings rather
  than hoped-for ones.
- A memory budget in bytes, with distance setting preference and the budget
  setting what may stay.
- A measurement harness whose counters are committed and CI-asserted, so the next
  drop can tell a refactor from a regression on the first run.
- A shutdown flush, so quitting cannot lose work.

And one habit worth carrying forward: **every claim in this report is a number
somebody can reproduce with a named command.** Where DROP 0002 could not measure
something — the GPU cost of draw calls — it says so and leaves the knob
configurable, rather than guessing and baking the guess in.
