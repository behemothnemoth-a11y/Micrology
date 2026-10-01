# DROP 0003 — Destruction Foundation: final report

DROP 0003 is complete on the integration/robustness branch.

The milestone is no longer a collection of independent subsystems. The sandbox
now exercises the full path:

```text
live carve
  -> transactional WorldEditBatch
  -> bounded async structural snapshot
  -> exact connectivity classification
  -> stale/inconclusive validation
  -> atomic budget admission
  -> static-world detach
  -> native Fragment
  -> bounded fragment mesh upload
  -> bounded Avian f64 rigid-body creation
  -> physics readback into engine-owned Fragment
  -> fragment + static persistence
```

Engine crates remain free of Bevy/Avian types.

## Final validation

### CI

At commit `1d871067c25d0a89d538a8b758627fd945a41727`, repository CI passed:

- formatting
- workspace Clippy with warnings denied
- engine test suite
- doc tests
- geometry stress baseline
- structural stress baseline
- canonical persistence fixtures
- sandbox/Bevy/Avian Clippy

### Real sandbox destruction smoke

A full release sandbox run was executed on the MSI test machine:

- Windows 11 Home
- Intel Core 7 240H
- 31.7 GiB RAM
- NVIDIA GeForce RTX 5050 Laptop GPU
- Vulkan backend

With `MICROLOGY_DESTRUCTION_SMOKE=1`, the normal sandbox host:

1. created an anchored structure,
2. cut it through `WorldEditBatch`,
3. dispatched the real async structural worker,
4. detached 384 cells,
5. created one native fragment,
6. uploaded its render mesh,
7. created its Avian f64 rigid body,
8. observed physics readback move the fragment from Y 15.999 to Y 15.980,
9. exited successfully,
10. flushed one fragment and one static region.

Process exit code: **0**.

## Adversarial destruction pass

`engine_stress::chaos` was run in release mode on the MSI.

All nine cases passed.

| Case | Result | Finding |
|---|---|---|
| structural matrix | PASS | all 10 canonical scenarios matched |
| stale result | PASS | stale worker result removed nothing and spent no fragment id |
| atomic policy refusal | PASS | budget/policy refusal removed no cells and kept sequence 77 |
| snapshot byte ceiling | PASS | byte-limited job became inconclusive, never detached |
| fragment storm | PASS | 256 components / 5,120 cells; greedy collision reduced 5,120 boxes to 256 |
| integer-edge carve | PASS | sphere at i32::MAX clipped; no wraparound |
| persistence crash windows | PASS | missing index and stale spatial index both recovered |
| repeat determinism | PASS | 32 runs produced the same world/fragment signature |
| last-cell support cliff | PASS as current rule | 3/4 neck cells removed stayed supported; 4/4 detached |

The expensive canonical negative case was `large_supported_structure`: **25,216
cells visited**, about **127 ms** in the direct release-mode classifier on the MSI.
This is diagnostic timing, not a CI threshold.

## Robustness changes made during 0003.14–0003.15

### Live destruction integration

- ordinary sandbox carving now feeds structural analysis
- LMB = one-cell carve
- Shift+LMB = radius-2 deterministic sphere
- Ctrl+LMB = radius-4 deterministic sphere
- structural classification runs on the async compute pool
- stale results are rejected and retried
- inconclusive results never detach geometry
- structural snapshots are limited by both volume count and an explicit byte ceiling
- only one new structural snapshot is materialized per rendered frame
- at most two structural jobs are active
- pending structural requests are bounded and conservatively coalesced
- a request waiting for unloaded regions no longer head-of-line blocks unrelated ready work

### Atomic fragment admission

`detach_if` lets the host inspect exact candidate fragments before static cells
are removed.

If the fragment hard-byte policy would be exceeded:

- static cells remain where they are
- no fragment id is consumed
- no half-detach occurs
- the HUD reports the refusal

### Fragment render / physics backpressure

- fragment render entities are separate from physics entities
- render remains camera-relative f32
- physics remains global f64
- fragment mesh uploads are capped per frame
- fragment rigid-body creation is capped per frame
- hard budget checks happen before admission
- permanently blocked bodies/meshes are not recompiled every frame
- blocked work is retried only after capacity increases
- a physics body is not created for a fragment whose visible mesh was not admitted

### Persistence hardening

- destruction sequence is restored before live detachment
- F5 saves fragments as well as dirty static regions
- F9 tears down old fragment render + physics entities before reload
- shutdown flushes fragment state
- runtime-only smoke fragments are excluded from persistence
- missing fragment index with valid payloads is recoverable
- stale derived spatial index is rebuilt from authoritative fragment payloads on full load
- save order is payloads -> atomic index -> orphan cleanup

### Coordinate-edge hardening

- checked cell-neighbour traversal
- checked volume-neighbour traversal
- connectivity does not wrap at integer limits
- snapshot expansion does not wrap
- sphere carving uses checked integer addition and clips at address-space edges

## Weak points that remain

These are not hidden by the green torture pass.

### P0 — binary support is not load-bearing mechanics

The current exact structural rule is still:

> any face-connected path to an anchor is sufficient support.

The weak-point diagnostic demonstrated the consequence directly: one surviving
cell can hold an arbitrarily large connected mass until that final cell is
removed.

This is correct for DROP 0003 topology and inadequate for advanced destruction.

Do **not** remove the exact topology oracle. A later structural-mechanics layer
should build on top of it with support capacity/load paths/stress/failure.

### P0 — fragment streaming is not yet a bounded-world lifecycle

Fragment persistence and a spatial index exist, but the sandbox still loads the
whole fragment store rather than streaming fragments by region relevance.

That violates the long-term bounded-world goal once fragment populations become
large.

Required follow-up:

- region-driven fragment discovery
- fragment load/unload lifecycle
- save-before-eviction
- moving-fragment spatial-index updates
- render and physics residency radii independent from fragment storage residency

### P0 — structural dependencies cannot yet demand streaming

An inconclusive structural job can name required regions, but the host currently
waits for normal camera streaming to make them resident.

A cut at a streaming boundary can therefore remain conservatively unresolved
until the player moves close enough.

Required follow-up: an explicit structural-demand/pin/request path into region
streaming.

### P1 — snapshot materialization still happens on the main thread

Classification is async, but creating the owned structural snapshot clones world
data before dispatch.

It is now:

- at most one snapshot per frame
- byte capped
- globally in-flight capped

That prevents unbounded damage, but a single near-cap snapshot can still be a
frame-time spike.

Likely follow-up: immutable/COW region-volume snapshots, staged snapshot building,
or another ownership model that can be handed to workers without a large main
thread clone.

### P1 — negative connectivity searches are expensive

The direct classifier visited 25,216 cells in the large supported case.

The current answer is correct and async, but future large structures need an
acceleration layer while retaining the exact classifier as oracle.

Candidates for investigation:

- cached component/support labels
- coarse volume-level structural graph
- incremental invalidation after cuts
- dynamic connectivity structures

Do not replace the exact classifier until a faster implementation is proved
equivalent on local canonical geometry.

### P1 — fragment derived-data builds are bounded but synchronous

Mesh uploads and collider/body creation are spread across frames, but each
individual fragment mesh/collider is still compiled synchronously in the sandbox
host.

A single extremely complex fragment can still spike a frame.

Follow-up: async fragment mesh and collision jobs with revision fingerprints and
stale-result rejection, matching the static mesh architecture.

### P1 — fragment persistence is full-store, not incremental

Correctness is strong, but F5/exit currently persist the complete fragment
collection.

Large dynamic worlds need dirty-fragment revisions and incremental payload/index
updates.

### P2 — retry exhaustion is conservative but crude

Repeated stale structural questions eventually stop retrying and leave geometry
static. That is safe, but future host policy should expose/backoff/re-enqueue
these more deliberately.

### P2 — renderer submission scaling remains measured but unresolved

DROP 0002's one-section submission count remains a known renderer issue.
Destruction did not justify changing the renderer contract yet.

## Recommended next major drop

**DROP 0004 — Dynamic World Residency / Destruction Scale**

Do this before adding realistic stress mechanics.

Primary scope:

1. fragment spatial streaming
2. structural-demand region loading/pinning
3. incremental fragment persistence
4. async fragment mesh/collision compilation
5. structural search acceleration experiments with the exact classifier retained as oracle
6. real-hardware fragment-storm and travel acceptance tests

After that, a later drop can add load-bearing mechanics:

```text
exact connectivity
 -> load path
 -> support capacity
 -> accumulated load/stress
 -> failure
 -> connectivity changes
 -> repeat
```

That keeps correctness topology separate from game-specific mechanical realism.
