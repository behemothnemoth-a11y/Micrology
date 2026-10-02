# Claude Code Handoff — Continue Micrology Core Destruction

> **2026-10-02 follow-up:** 0006.1 acceptance was completed in `dc30b64`,
> preserving Claude's 0006.1-0006.3 work. The user then requested deep research
> and progress toward full Astra-style, one-material destruction with a focus
> on eliminating stalls. See `astra-destruction-performance-research.md` for
> the evidence and priorities. The follow-up pass repairs history-sized copies
> during a local fracture commit without changing the fracture model. Keep
> the section/checks/evidence/review workflow. 0006.4 remains the next proposed
> behavioral section; `chunk_into_wall` is still unimplemented.

> **2026-10-02 takeover override:** the user has replaced continuous progression
> with one-section review gates. Preserve the remote 0006.1 (`f8f19a5`), 0006.2
> (`3c04ca6`), and 0006.3 (`cb6e283`) commits. Current work is acceptance and
> narrowly scoped fixes for **0006.1 only**, then stop for user review. The older
> instructions below to continue automatically do not apply to this takeover.

## Intent

Keep working continuously on **the same one-material fracture/destruction system** and refine it toward Micrology's version of Teardown-style destruction.

Do not branch into multi-material tuning, editor work, procedural generation, particles-as-a-substitute-for-physics, or unrelated engine features. The current priority is still:

> make one homogeneous material destroy extremely well first.

Work in **small coherent sections/commits**, run the acceptance suite after each section, and continue to the next section without waiting for a new prompt unless an architectural decision would violate a hard invariant or require changing the benchmark target.

## Start here

```powershell
git fetch origin
git switch fracture-integration
git pull --ff-only
git status --short --branch
git log --oneline -12
```

The integration branch combines:

- DROP 0005 mechanics baseline;
- Claude's `047951c` 0006.0 sparse fracture model;
- the permanent destruction benchmark pack;
- deterministic replay + pause/slow-motion/single-step lab;
- non-invasive FFmpeg/ddagrab capture preflight;
- one shared reference fixture used by tests, example, sandbox demo, benchmark and replay.

Do **not** restart or re-scope 0006.0.

## What 0006.0 already proves

`engine_destruction::fracture` is real persistent engine state, not visual decoration:

- sparse per-cell absorbed energy;
- sparse per-bond integrity;
- deterministic integer/fixed-point impact evaluation after one quantisation boundary;
- directional propagation;
- free-surface spall response;
- crack-tip concentration;
- fatigue floor;
- stale-result rejection;
- separate work and entry/memory bounds;
- unknown space remains conservative;
- ordinary `DamageTarget -> WorldEditBatch -> structural pipeline` remains the only geometry-removal route.

The canonical seven-hit reference run remains:

| hit | cracks opened | removed | damaged cells | loaded bonds | broken bonds |
|---:|---:|---:|---:|---:|---:|
| 1 | 0 | 0 | 497 | 1840 | 0 |
| 2 | 4 | 1 | 496 | 1835 | 4 |
| 3 | 28 | 0 | 496 | 1835 | 32 |
| 4 | 184 | 5 | 491 | 1814 | 208 |
| 5 | 308 | 4 | 487 | 1814 | 508 |
| 6 | 201 | 33 | 454 | 1721 | 621 |
| 7 | 180 | 21 | 433 | 1652 | 756 |

After seven hits: 64 of 3,132 fixture cells removed; hole half-width 4; crack half-width 7.

The first hit is the most important acceptance fact: **497 cells are damaged, 1,840 bonds carry persistent state, and zero cells disappear.**

## One canonical fixture — do not fork it

The shared fixture now lives in:

`crates/engine_destruction/src/reference_fixture.rs`

All consumers must use it rather than rebuilding their own version:

- `crates/engine_destruction/tests/fracture.rs`
- `crates/engine_destruction/examples/fracture_wall.rs`
- `apps/sandbox/src/fracture_demo.rs`
- `crates/engine_stress/src/destruction_benchmark.rs`
- `apps/sandbox/src/sim_lab.rs`

Shape:

- 32 x 18 x 5 destruction wall = 2,880 cells;
- wider 36 x 1 x 7 anchored footing = 252 cells;
- total fixture = 3,132 cells;
- one fictional baseline material;
- translated into positive coordinates entirely inside one residency region for benchmark/demo use;
- tests may instantiate the exact same shape at another translation when coordinate-specific cases need it.

Do not change fixture geometry or the canonical weak hit merely because a new algorithm performs poorly. A deliberate fixture change requires updating the benchmark version and explaining why.

## Benchmark and replay tools

Permanent protocol:

```powershell
cargo run -p engine_stress --bin destruction_bench
cargo run -p engine_stress --bin destruction_bench -- --check
cargo run -p engine_stress --bin destruction_bench -- --json
cargo run -p engine_stress --bin destruction_bench -- --template
```

Replay plan:

```powershell
cargo run -p engine_stress --bin destruction_replay
cargo run -p engine_stress --bin destruction_replay -- --check
cargo run -p engine_stress --bin destruction_replay -- --digest-baseline
```

Interactive lab:

```powershell
$env:MICROLOGY_DESTRUCTION_LAB='1'
cargo run -p sandbox
```

Lab controls:

- `F6` — run/pause virtual simulation time;
- `F7` — advance exactly one fixed step while paused;
- `F8` — cycle 0.1x / 0.25x / 0.5x / 1.0x;
- `F10` — dump structural + fracture + fragment diagnostics to `target/diagnostics/destruction_lab/`;
- `F11` — execute the next deterministic replay command.

The current `weak_repeat` replay is connected to the **real 0006.0 fracture evaluator**, not a stub. Future benchmark cases should be wired into the same replay mechanism as they become implemented.

Lab state is disposable. Normal edit/save/reload and shutdown persistence are disabled while the lab is active. Never let benchmark debris overwrite a real world save.

## Capture

Before any visual capture:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\tools\capture-demo.ps1 -CheckOnly
```

If this fails, fix FFmpeg first. Do not debug Micrology because a capture dependency is missing.

DROP 0006 fracture capture:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\tools\capture-demo.ps1 -Demo fracture -Section drop0006_0_fracture
```

or double-click:

`tools\capture-drop6.bat`

Rules:

- Vulkan capture must use FFmpeg `ddagrab`;
- `gdigrab` records the Vulkan surface as blank white and is invalid;
- do **not** minimize, move, resize, restore, or rearrange Justin's other windows;
- the capture tool is intentionally non-invasive;
- keep Micrology unobstructed while recording;
- visual overlays must reflect real engine fracture state, not decorative fake cracks.

## Hard invariants

Do not violate these while improving destruction:

- cells are data, never one ECS entity or rigid body per cell;
- one detached connected component becomes one native volumetric `Fragment`;
- support/anchoring remains separate from material identity;
- Bevy/Avian stay outside engine crates;
- static and fragment collision derive from cells, never render meshes;
- unknown/unloaded space is not empty;
- budget exhaustion or inconclusive analysis never destroys/detaches geometry;
- fragment admission remains atomic;
- deterministic observable order is canonical;
- exact/reference implementations stay beside optimisations;
- byte budgets and work budgets stay separate;
- **no core physical system is required for authored world validity**;
- fantasy/impossible structures remain legal authored data when relevant simulation policies are disabled.

Occupancy connectivity remains the permanent exact topological oracle. New fracture-aware connectivity may sit beside it; do not silently redefine the old oracle.

## Continue the system in this order

The section numbers below are the working continuation plan. Keep each section scoped and independently green, but continue forward automatically after committing it.

### 0006.1 — fracture-aware connectivity + directional benchmark expansion

Primary goal: a broken bond can become a real cut for a fracture-aware structural query, so a cracked plug can separate **without requiring the engine to delete an artificial shell of cells first**.

Requirements:

- add a fracture-aware classifier beside exact occupancy connectivity;
- never mutate/replace `classify_from_roots` as the occupancy oracle;
- unknown/budget semantics remain conservative;
- deterministic ordering and bounded work;
- a structure can be occupancy-connected yet fracture-disconnected, and that difference must be explicit;
- route settled detached components through the existing atomic fragment admission pipeline;
- make the benchmark's strong-center, edge-hit and angled-hit cases executable and measurable;
- use the replay lab for step-by-step comparison;
- visually inspect whether chunks separate along crack state rather than clean deletion masks.

### 0006.2 — fracture-state ownership across streaming + damaged-area continuation

Primary goal: damage must remain real when world ownership moves.

Requirements:

- region-granular extract/restore for static fracture state;
- no stale cell/bond state after carve/removal/repaint/detach;
- dirty/clean ownership semantics compatible with world streaming;
- persisted/reloaded damage reproduces the same deterministic checksum;
- benchmark `previously_damaged_area` must differ from the same hit on a fresh wall;
- replay/dumps must expose that difference.

### 0006.3 — fragment-local fracture / true re-destruction

Primary goal: detached chunks use the **same fracture model**, not the old scalar path as a special case.

Requirements:

- fracture state can belong to an exact `FragmentId` / fragment-local coordinate space;
- hit a detached chunk and continue its existing damage;
- re-fracture remains atomic and deterministic;
- child fragments receive/remap relevant fracture state correctly;
- benchmark `detached_chunk_hit` becomes executable;
- no indestructible rigid-body debris objects.

### 0006.4 — fragment impact coupling and secondary destruction

Primary goal: falling debris becomes part of the same destruction loop.

Requirements:

- fragment-to-static impact feeds bounded energy into fracture;
- fragment-to-fragment impacts become supported with generation/work caps;
- both participants may take damage;
- no recursive unbounded chain reaction;
- benchmark `chunk_into_wall` becomes executable;
- falling chunks should be able to break the structure below them and themselves re-break.

### 0006.5 — refine break character of the one material

Do **not** introduce multiple materials yet.

Use the one material to improve:

- irregular break surfaces;
- local spall/chipping;
- believable chunk-size distribution;
- repeated-hit crack growth;
- edge/free-surface response;
- oblique/directional response;
- avoiding obviously spherical/cubic holes;
- keeping small damage sparse rather than turning the entire reachable area into stored state.

This is algorithm/tuning work on the same material. Preserve the benchmark history so improvements/regressions are visible.

### 0006.6 — destruction scale / fragment hierarchy

Only after the core break loop feels good:

- distinguish structural volumetric fragments from cheap physical debris and purely visual dust;
- full fragments remain destructible;
- budgets decide simulation detail, not topology truth;
- sleeping/collision/derived-data work must survive fragment storms;
- no one-rigid-body-per-cell fallback.

### 0006.7+ — keep measuring and refining

Continue profiling, chaos-testing, replaying and visually refining the same system. Do not jump to material diversity until the homogeneous benchmark is convincingly fun to destroy.

## Continuous work loop

For every coherent change:

1. inspect the current benchmark/replay output;
2. implement one narrow improvement;
3. add/update focused tests;
4. run relevant full regressions;
5. run Clippy with warnings denied;
6. run `cargo fmt --all --check` and `git diff --check`;
7. run benchmark/replay and record deterministic counters/checksums;
8. capture fresh visual evidence when visible behavior changed and FFmpeg preflight passes;
9. compare behavior to the previous section, not just to intuition;
10. update `docs/drop-0006.md` with measured behavior and known limitations;
11. commit and push the coherent section;
12. continue to the next improvement.

Do not stop merely because a section compiles. The goal is a destruction system that becomes progressively more convincing under the same permanent abuse cases.

## Required gates

At minimum keep these green:

```powershell
cargo test -p engine_destruction --test fracture
cargo test -p engine_destruction
cargo test -p engine_stress
cargo clippy -p engine_destruction --all-targets -- -D warnings
cargo clippy -p engine_stress --all-targets -- -D warnings
cargo clippy -p sandbox --all-targets -- -D warnings
cargo fmt --all --check
git diff --check
cargo run -q -p engine_stress --bin destruction_bench -- --check
cargo run -q -p engine_stress --bin destruction_replay -- --check
```

Do not weaken an assertion just to get a new model through the gate. If a benchmark expectation truly needs to change, document the design reason and preserve before/after measurements.

## Definition of direction

The target is not "more particles" and not "copy Teardown exactly."

Micrology's destruction should eventually combine:

- persistent volumetric damage;
- directional fracture;
- structurally meaningful cracks;
- native destructible fragments;
- re-destruction after detachment;
- secondary destruction from debris;
- deterministic/replayable decisions;
- streaming/persistence at large-world scale;
- optional physics so fantasy authored worlds remain valid.

Keep pushing **this one system** toward that target until Justin tells you to change direction.
