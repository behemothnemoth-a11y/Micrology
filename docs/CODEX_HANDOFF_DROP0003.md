# CODEX HANDOFF — Micrology after DROP 0003

## Start here

Current work is on PR #6 / branch:

`drop-0003-14-15-chaos`

DROP 0003 should be treated as **Destruction Foundation complete pending final
merge/documentation CI**.

Do not recreate the destruction architecture.

Read:

1. `docs/drop-0003.md`
2. `docs/drop-0003-report.md`
3. `apps/sandbox/src/destruction.rs`
4. `apps/sandbox/src/fragment_render.rs`
5. `apps/sandbox/src/physics.rs`
6. `crates/engine_destruction/src/jobs.rs`
7. `crates/engine_destruction/src/detach.rs`
8. `crates/engine_stress/src/bin/chaos.rs`

## Proven current behavior

The live sandbox now performs:

```text
carve
-> WorldEditBatch
-> async StructureJobInput
-> exact classifier
-> stale/inconclusive validation
-> atomic detach admission
-> Fragment
-> render mesh
-> Avian f64 rigid body
-> physics readback
-> save/reload
```

A real release sandbox smoke on the MSI detached 384 cells, rendered/simulated
the fragment, observed it move under Avian, and flushed fragment + static state
before clean exit.

The adversarial runner reports all nine current abuse cases passing.

## Hard invariants — do not regress

- cells remain data, never ECS entities
- engine crates remain independent of Bevy/Avian
- exact structural classifier remains the oracle
- unloaded/unknown is never treated as empty
- stale worker results never mutate newer world state
- policy/budget refusal happens before static-cell removal
- refused detachment spends no fragment sequence
- fragment render and physics entities stay separate
- global physics remains f64
- rendering remains camera-relative
- hard budgets are conservative: hold static rather than delete/guess
- persistence sequence must survive reload
- runtime diagnostic fragments never persist
- coordinate-edge traversal must never wrap
- main-thread work stays bounded per frame

## Do not spend time on these already-fixed problems

- fragment sequence restoration
- missing fragment index crash recovery
- stale spatial index full-load recovery
- F9 stale Avian-body overwrite
- unlimited fragment body creation per frame
- unlimited fragment mesh upload per frame
- permanent recompilation of hard-budget-rejected colliders
- unbounded structural pending queue
- waiting request head-of-line blocking
- sphere-carve integer overflow
- static detachment before fragment-budget admission

## Next required architecture

Plan and implement **DROP 0004 — Dynamic World Residency / Destruction Scale**.

Priority order:

### 1. Stream fragments instead of loading the whole store

Use the existing fragment spatial index.

Need:

- region-driven fragment discovery
- exact-once fragment ownership
- load/unload/save lifecycle
- moving-fragment region overlap updates
- fragment storage residency separate from render residency and physics residency
- no fragment leaks after long travel

### 2. Let structural jobs demand/pin regions

A structural result already reports required regions.

Add an explicit dependency-loading path; do not require the player to move the
camera near a support just to resolve destruction.

A structural dependency must not be evicted while the analysis that needs it is
active.

### 3. Incremental fragment persistence

Track dirty fragment revisions.

Save only changed/new/deleted fragment payloads plus the smallest necessary index
update. Preserve atomic crash behavior already established in 0003.13.

### 4. Async fragment mesh and collision derivation

Current work is bounded across frames but an individual large fragment build is
still synchronous.

Mirror the static mesh architecture:

- owned job input
- fragment revision fingerprint
- bounded scheduler
- stale-result rejection
- separate render/collision results
- memory accounting for in-flight fragment jobs

### 5. Investigate structural-search acceleration

Measured direct negative case:

- `large_supported_structure`
- 25,216 cells visited
- ~127 ms on the MSI release diagnostic

Do not replace the classifier first.

Prototype accelerators against it:

- coarse volume connectivity graph
- component/support caches
- incremental invalidation
- dynamic connectivity

Require equivalence on canonical fine geometry.

## Mechanical realism comes after scale

The current binary topology intentionally has a last-cell cliff.

The diagnostic proves:

- 3/4 cells of a neck removed: still supported
- final cell removed: detached

Do not patch this with arbitrary thresholds inside the topology classifier.

A later advanced-destruction drop should introduce explicit load/capacity/stress
semantics above exact connectivity.

## Validation commands

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

cargo run --release -p engine_stress --bin structural -- --check
cargo run --release -p engine_stress --bin chaos -- docs/diagnostics/chaos/report.json
```

For real host validation:

```text
MICROLOGY_DESTRUCTION_SMOKE=1 cargo run --release -p sandbox
```

Expected smoke behavior: live async detach, one rendered/simulated fragment,
motion observed, process exits success.

## Stop condition for the next implementation session

Do not jump to material strength, explosions, fire, fluids, multiplayer or
procedural planets.

First prove:

> fragments and structural dependencies can travel through a world larger than
> resident memory without leaks, lost edits, duplicate identities, synchronous
> giant derived-data stalls, or treating unknown space as empty.
