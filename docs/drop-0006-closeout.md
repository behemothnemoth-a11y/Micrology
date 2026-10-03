# DROP 0006 closeout — Core Destruction / Fracture Model

Status: **COMPLETE**

DROP 0006 is closed as the completed one-material destruction architecture line.

This closeout does not mean Micrology destruction is "finished forever", and it
does not claim Teardown-scale production performance. It means the architectural
questions DROP 0006 set out to answer now have working, tested engine answers:

- persistent damage exists between intact and absent;
- cracks are engine state rather than decoration;
- cracks can change structural connectivity without replacing the exact
  occupancy oracle;
- fracture state survives streaming ownership;
- native detached fragments retain their damage;
- native fragments can be fractured again;
- solved contacts can feed secondary destruction back into the same model;
- large structural collapse can release native volumetric fragments;
- local fracture analysis no longer pays for unrelated world damage history;
- fragment distribution is an explicit policy layer rather than an accidental
  one-component-one-body consequence;
- scripted contact destruction is deterministic with respect to authoritative
  fixed-step state;
- static collider compilation is bounded, local, asynchronous and stale-checked.

Still one homogeneous reference material, deliberately.

## Scope reviewed

This closeout reviews the full live DROP 0006 line, including:

```text
0006.0  baseline persistent fracture model
0006.1  fracture-aware connectivity beside the occupancy oracle
0006.2  fracture-state ownership across streaming
0006.3  fragment-local fracture / true re-destruction
0006.4  contact coupling / secondary destruction
        + integrated one-material interaction/performance delivery
        + demolition / capacity acceptance
0006.5  local fracture indexing / scalable ownership
0006.6  coherent fragment-distribution layer
0006.7  fixed-step collision readiness / bounded collision rebuilds
```

The original numbered documents contain historical review gates and statements
such as "future 0006.4" that were true when written. This closeout is the current
authority for whether those sections eventually shipped.

## Section status

| section | closeout status | accepted result |
|---|---|---|
| 0006.0 | complete | sparse persistent cell/bond fracture state; deterministic directional impact model |
| 0006.1 | complete | crack-aware structural classifier beside unchanged exact occupancy oracle |
| 0006.2 | complete | region ownership/extract/restore; damaged-area continuation |
| 0006.3 | complete | fracture state moves into exact FragmentId space; native fragments re-fracture |
| 0006.4 | complete | bounded solved-contact coupling; fragment→static and fragment→fragment secondary damage; all 8 benchmark cases executable |
| demolition integration | complete | capacity-driven roof release, native moving roof fragment, mass-conserving replay |
| 0006.5 | complete | local static-fracture snapshots remain bounded against unrelated history |
| 0006.6 | complete | deterministic fragment-distribution measurement and explicit grouping policies |
| 0006.7 | complete | authoritative fixed-step collision readiness; bounded async static collider rebuilds; deterministic contact replay |

## What DROP 0006 proved

### 1. Damage is persistent state, not a deletion effect

The canonical weak impact can leave hundreds of cells and bonds damaged while
removing no geometry. A later impact reads that history and behaves differently.

The permanent one-material wall remains the reference fixture.

### 2. Occupancy truth and fracture truth remain separate

Exact occupancy connectivity is still the permanent topology oracle.

Fracture-aware connectivity sits beside it and answers the different question:
"what is still structurally connected if broken bonds are cuts?"

Unknown, deferred and exhausted-budget results remain conservative and cannot
become permission to destroy or detach material.

### 3. Fracture survives ownership changes

Static fracture state has deterministic region ownership and extract/restore.
Boundary-bond read locality and persistence ownership are intentionally
different questions.

Fragment-local fracture state is owned by exact `FragmentId`, remaps during
re-fracture, and does not turn moving fragments into pristine debris.

### 4. Detached material remains native volumetric material

Fragments remain cell-backed engine objects:

```text
no entity per cell
no rigid body per cell
collision from cells
render meshes not physical truth
```

A fragment can move, collide, take another impact, split again or be destroyed
through the same destruction stack.

### 5. Secondary destruction is real

The integrated contact-fracture delivery completed the historical 0006.4 goal.

The permanent benchmark's `chunk_into_wall` case is executable. The headless
representative contact touches 681 cells and 1,508 bonds, records 104 broken
bonds and fails 61 cells across wall and already-damaged chunk.

All eight destruction benchmark cases are measured.

Solved host contacts are bounded by native owner pair, finite energy,
generation depth and queue capacity. Avian remains outside engine crates.

### 6. Structural demolition is part of the same system

The 1,811-cell demolition fixture demonstrates crack-weakened load capacity and
large structural fragment release.

Accepted replay facts include:

```text
one support cut          structure remains sound
two cuts, capacity off   1,803 surviving cells remain static
capacity on              8 additional cells fail
roof release             one 682-cell native fragment
mass                     closes exactly at 1,811
```

The roof remains a native destructible fragment rather than converting into a
special cinematic object.

### 7. Local destruction scales with local damage state

DROP 0006.5 removed the history-sized static fracture copy from local job
capture.

Against the same local impact:

```text
unrelated fracture records: 0 → 1,000,000
snapshot records:           128 → 128
capture time:              ~0.039 ms → ~0.086 ms
```

That is roughly 7,813× more unrelated history for about 2.2× measured capture
time in the recorded environment, rather than copying/refusing on all
1,000,128 records.

The spatial index is acceleration/ownership support only. It is not topology
authority.

### 8. Fragment distribution became an explicit policy question

The canonical 60-cell re-fracture exposed an actual character problem:

```text
per_crack_component at 2,500:
32 fragments
16 singletons
mean 1.875 cells
largest share 5%
```

DROP 0006.6 added deterministic distribution measurement and a grouping layer
that cannot split topology components, invent occupancy connectivity or consult
global history.

At the same impact:

```text
coherent mean:
16 fragments
0 singletons
mean 3.75 cells
largest share 18.3%
20 cracks retained
```

A median-derived alternative was added during 0006.7. On the real canonical
fixture mean and median produce identical membership at 1,200 / 2,500 / 6,000
energy.

**Closeout decision:** the global default remains
`PerCrackComponent`.

There is not yet enough real fixture breadth to change destruction character
globally. The coherent policies remain available for explicit use and future
evaluation.

### 9. The static path does not currently need the same grouping policy

0006.7 measured static fracture before changing it.

First accepted strong-center liberation, hit 2:

```text
190 detached cells
15 fragments
0 singletons
sizes:
3,3,3,3,5,5,5,5,9,9,20,20,20,20,60

occupancy groups   6
largest share      31.5%
top 3 share        52.6%
top 5 share        73.6%
```

This is crack-separated, but it does not show the tiny-fragment/singleton
pathology that motivated the fragment re-fracture partition layer.

**Closeout decision:** leave `fracture_static_if` grouping unchanged until a
real static-path failure demonstrates otherwise.

### 10. Fixed-step destruction now has an authoritative regression fixture

The historical stress replay was not deterministic because fixed physics could
observe different derived collision freshness depending on rendered Update
progress.

0006.7 did not simply "move contact sampling to fixed step"; contact collection
was already after physics.

Instead, scripted fixed steps now wait until current collision derived from live
cells/fragments is installed. Rendering progress is not part of that decision.

GitHub Actions run 199 repeated the contact-volley sandbox scenario three times,
six authoritative checkpoints per run.

All 18 checkpoint comparisons matched.

Final state every run:

```text
fixed ticks                  300

structural:
  static cells               2983
  fragments                    61
  fragment cells              269
  checksum        af9d203e941190b3

fracture:
  cell entries                1262
  bond entries                3352
  broken bonds                 680
  checksum        1aece4b9460d1de3

contacts:
  samples                        8
  enqueued                      16
  processed                     16
  refused                        0
  capped                         0
  stale                          4
  static failed                 31
  fragment failed              209
  fragments created             63
```

This is the current authoritative scheduling regression fixture.

### 11. Static collision compilation no longer belongs on the synchronous hot path

One static storage volume now becomes an owned worker snapshot.

Results are keyed by `VolumePos`, revision-checked, coalesced, and published in
deterministic nearest-first order.

Current host bounds:

```text
maximum active static collision workers       8
maximum static collider publications/frame    8
```

A superseded or stale job cannot publish.

Fragment collision remains keyed to exact fragment geometry revision. Motion
does not invalidate local geometry. A real fragment body temporarily leaves the
physics backend while nearby static collision is incomplete, preserving
engine-owned pose/velocity instead of simulating through known-missing collision.

The existing render-mesh scheduler already had the corresponding async,
deduplicated, byte-bounded, stale-rejecting architecture, so 0006.7 did not
rewrite it for cosmetic symmetry.

## Performance evidence: what exists and what changed afterward

There **is** real Windows/GPU-era evidence from the integrated one-material
delivery, before 0006.7's later scheduler changes.

Recorded hardware:

```text
Core 7 240H
32 GiB RAM
RTX 5050 Laptop GPU
driver 581.34
```

Accepted 18-chunk uncaptured run:

| interval | samples | median | p95 | p99 | max |
|---|---:|---:|---:|---:|---:|
| all post-warmup frames | 2,401 | 16.666 ms | 17.220 ms | 17.807 ms | 21.841 ms |
| physics-active frames | 660 | 16.659 ms | 17.197 ms | 18.033 ms | 21.382 ms |
| contact batches | 57 | 0.081 ms | 5.871 ms | 7.050 ms | 7.050 ms |

No recorded interval exceeded 33.3 or 50 ms. That run reached 201 simultaneous
native fragments and created 239 through the chain.

This evidence remains useful as a historical real-hardware baseline.

It does **not** validate the post-0006.7 collision scheduler change on Windows,
and it does not answer the older 544-fragment response-latency observation.
That revalidation remains pending.

## Final acceptance state

The tested 0006.7 code head `db0fe8310b2d874df3db595b4a617f50504ba9d0`
passed GitHub Actions run 199:

```text
cargo fmt --all --check                         PASS
git diff --check                                PASS
engine clippy -D warnings                       PASS
engine tests                                    PASS
doc tests                                       PASS
destruction_bench --check                       PASS
destruction_replay --check                      PASS
fragment_distribution_bench --check             PASS
static fracture distribution probe              PASS
fracture_locality_bench                         PASS
stress baseline                                 PASS
structural baseline                             PASS
canonical IO fixture regeneration               PASS

sandbox clippy                                  PASS
sandbox tests                                   PASS
3-run / 6-checkpoint contact determinism        PASS
```

The continuation branch contains later documentation-only commits whose code
content is unchanged from that tested head.

## What does NOT block closing DROP 0006

The following are real work, but they are not unfinished core-destruction
correctness and therefore do not keep DROP 0006 open.

### A. Post-0006.7 Windows/GPU revalidation

Need to re-run the current scheduler on Windows/real GPU when that execution
path is available:

```text
frame p50 / p95 / p99 / max
frames over 33.3 ms
response latency
old 544-fragment response observation
```

This is validation debt against the current implementation, not a missing engine
model.

### B. Fracture-index memory layout

The local static-fracture index costs roughly 40-byte keys against 48-byte
records in the million-record measurement — about 83% key overhead relative to
record payload.

The obvious storage rewrite is blocked by `FractureState::digest()` observing
key iteration order.

Correct independent optimization sequence:

```text
canonical digest independent of storage order
        ↓
equivalence proof
        ↓
storage/index layout rewrite
```

This should not be smuggled into destruction-character work.

### C. Contractual whole-state operations

Known, intentionally not changed here:

- `prune_against` scans the complete state by contract;
- `forget_space(StaticWorld)` is whole-space by definition;
- the small ~2.2× capture residual with a million unrelated records is not
  attributed by the benchmark.

These are not evidence that local fracture capture has regressed.

### D. More fragment-distribution fixtures / shape policy

The real fragment re-fracture policy comparison still has one canonical fixture
family.

The static probe reports eight sliver-shaped fragments, but no singleton spray.

Before changing the global partition default or introducing shape-aware/minimal-
cut grouping, add more deterministic real break shapes first.

That belongs to future modeling/refinement, not closure of the core
architecture.

## Final DROP 0006 boundary

DROP 0006 is complete because its core promise is now true:

> one homogeneous Micrology solid has persistent, directional, structurally
> meaningful, replayable fracture state that survives streaming and detachment,
> produces native destructible fragments, participates in bounded secondary
> destruction, scales local analysis to local damage, and has deterministic
> scheduling acceptance.

Do not reinterpret "complete" as:

```text
all materials complete
all performance optimization complete
all future fragment aesthetics complete
Teardown-scale production game complete
```

Those were never the DROP 0006 contract.

## Next-step rule

There is **no automatic 0006.8**.

Before starting another numbered drop, choose the next project boundary
deliberately from the broader Micrology roadmap. Any of these may become future
work, but this closeout does not choose one:

- current-code Windows/GPU validation;
- canonical-digest / index-memory optimization;
- broader fragment distribution and shape research;
- return to unfinished material/fantasy participation work outside DROP 0006;
- another Micrology engine subsystem.

The next numbered drop should be defined only after that choice.
