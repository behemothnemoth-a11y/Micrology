# Teardown interaction drop

2026-10-02. Continuation of `fracture-integration` from `eb9f1b6`, preserving
the completed Claude work and the previous integrated contact-fracture drop.
The remote was fetched before work; no additional remote work needed merging.

## Playable behavior

- **Directional strikes:** accepted wall strikes push the fragments they free.
  Strikes on loose material also change its motion. One strike's momentum
  budget is shared by surviving cell mass, so splitting into more pieces does
  not multiply that budget. Velocity edits reach already installed physics
  bodies before their next step, and wake sleeping bodies.
- **Grab, pull, and throw:** hold the right mouse button over a fragment, move
  the camera to pull it, use the wheel to change reach, release to drop, or
  left-click to throw. A damped point force runs once per fixed step through
  Avian's actual mass and inertia. The material retains collision and rotation.
  Overextended holds, destroyed owners, and changed geometry release safely.
- **Radial blasts:** B deposits a radial fracture field at the pointed surface.
  It weakens bonds, admits actual connected components, and pushes surviving
  fragments outward. Radial pressure can reach disconnected surfaces across
  air. It uses the unchanged single reference material, including its crush and
  bond thresholds. This is a gameplay blast field without shielding, fluid
  pressure, heat, or a shock-wave solver.
- **A debris floor:** G adds 4,096 ordinary anchored cells of the same material.
  The floor can be damaged. It has normal generated geometry and collision;
  the canonical wall fixture itself is unchanged. The command is idempotent
  and refuses overlaps instead of replacing existing material.
- **Replay and evidence:** additive replay commands cover the floor, blast,
  grab, target motion, and release. Dumps now include world centers, rotations,
  tool counters, and bounded timing samples. Original replay commands and
  fixture checksums remain valid.

The lab opens paused. G adds the floor; R supplies a chunk you can aim at and
grab before pressing F6 to start physics.

R still launches a naturally fractured reference chunk; F9 toggles contact
damage; F6 pauses/runs; F7 steps; F8 changes speed; F10 dumps; F11 advances a
replay. All new tools are confined to the opt-in disposable lab. Authored worlds
remain ordinary engine data and do not require these simulation systems.

## Bounded work and correctness

The engine continues to store cells as data. A detached component is a native
volumetric Fragment with one backend body, including the legitimate case where
that component contains a single cell. No per-cell entity/body representation
was introduced. Bevy and Avian remain outside engine crates.

The exact occupancy classifier is unchanged and still cross-checks fracture
separation. Each target's damage, topology, geometry, identity, and storage
admission commit atomically. Unknown residency, exhausted structural work, or
storage refusal preserves that target. A blast is a bounded sequence of these
transactions, not an atomic transaction spanning every object in the scene.

The blast queue holds at most 32 targets: the static world and the nearest 31
existing fragments, with stable ID ordering for equal distances. It processes
two targets per render update; radius is limited to eight cells and input
energy to 24,000. Candidate queries refuse above 4,096 resident fragments.
Deferred fragment targets validate their geometry revision. The same blast
never enqueues the children it creates. Excess targets remain intact and are
counted. Replays wait for the queue before advancing their next command.

Picking rejects fragments outside a conservative ray/sphere bound before
traversing native cells. Static material occludes loose material. Grab reach
is 3–24 cells, acceleration is capped at 80 cells/s², and force at 12,000
reference units. Throw/strike velocity increments cap at 24 cells/s; tool-edited
resulting velocities cap at 80 cells/s. Existing contact generation, body,
collider, memory, and mesh upload budgets remain in place.

## Measured acceptance

The 630-step interaction replay starts with 7,228 authored cells (the immutable
3,132-cell wall plus the optional floor) and spawns one 60-cell canonical chunk.
It lifts, translates, throws, blasts, advances debris motion, then runs 120
steps with contact damage off.

- The same 60 cells and fracture checksum survive both held checkpoints.
  Measured held-point/center errors are 0.274 and 0.268 cells; the small vertical
  offset is the force controller supporting weight under gravity.
- The throw produces real contact fracture. The blast then breaks 348 bonds,
  fails 122 cells, creates 16 fragments, and applies outward motion. There are
  no pending or refused blast targets at the checkpoint.
- After subsequent collisions, 36 fragments retain 106 cells and the static
  world retains 6,845. All 7,288 original-plus-spawned cells are accounted for
  as surviving geometry or explicitly failed cells.
- The damage-off checkpoint preserves geometry, fracture state, and contact
  counters while the native bodies continue through the physics schedule.

`tools/verify-interaction-replay.py` checks these properties against actual
sandbox dumps, including finite poses/velocities, unique IDs, material
conservation, held-point error, and damage-off stability. It does not synthesize
expected physics output.

The separate uncaptured stress run used nine chunks, three blasts, the floor,
and 840 fixed steps. It reached **144 fragments / 127 awake**, processed 196
contact targets including 25 fragment-to-fragment pairs, and applied 58 blast
targets. Five excess blast candidates and four contact events were capped;
40 contact samples/targets were stale. There were zero transaction refusals and
zero pending targets at completion. The resulting 7,768 cells are all accounted
for: 5,254 static + 325 in 136 surviving fragments + 2,189 explicitly failed.

On the same Core 7 240H / RTX 5050 Laptop / 32 GiB system as the preceding drop,
the 55-second uncaptured release run measured:

| Measurement | Median | 95th percentile | 99th percentile | Maximum |
|---|---:|---:|---:|---:|
| Frame interval, 3,002 samples after warmup | 16.68 ms | 18.11 ms | 20.71 ms | 30.62 ms |
| Physics-active frame interval, 840 samples | 16.69 ms | 17.97 ms | 20.24 ms | 21.42 ms |
| Contact transaction batch, 98 samples | 0.54 ms | 10.13 ms | 11.58 ms | 11.58 ms |

There were **zero measured frames above 33.3 ms** and zero above 50 ms. The
largest blast batch was 14.54 ms. VSync is active; these are observed frame
intervals, not uncapped throughput or GPU timings. Renderer capture was off
during this measurement. This is evidence for the measured workload, not a
claim of lag-free arbitrary worlds or guaranteed 60 FPS.

`interaction_bench` adds four radial cases beside the existing destruction pack.
It checks identical geometry, fracture digest, and work results across three
warmups and 31 measured executions per case, with cell conservation throughout.
Timing includes fracture, topology/oracle work, and native fragment creation;
it excludes fixture construction, backend physics, rendering, and digesting.

| Radius / energy | Failed cells | Fragments / surviving cells | Median CPU | p95 CPU |
|---|---:|---:|---:|---:|
| 4 / 10,000 | 35 | 21 / 62 | 4.93 ms | 6.72 ms |
| 6 / 10,000 | 89 | 13 / 44 | 5.82 ms | 6.91 ms |
| 8 / 16,000 | 449 | 16 / 56 | 8.20 ms | 9.24 ms |
| 8 / 24,000 | 597 | 40 / 176 | 9.49 ms | 14.99 ms |

## Research and implementation choices

Teardown's public API exposes dynamic-body impulses at world-space points,
voxel shapes owned by bodies, and separate frame-input and fixed simulation
callbacks. That supports pursuing body-level interaction around native volumes;
the API does not reveal Teardown's internal fracture algorithm. The implementation
here remains Micrology's own cell/bond model.
[Teardown scripting API](https://teardowngame.com/modding/api.html#ApplyBodyImpulse).

Avian distinguishes sustained forces from immediate impulses and supplies
point-force application that produces torque away from the center of mass.
The grab uses that API, verified against the installed 0.7.0 source. Throw and
blast kicks use explicit bounded native velocity changes, synchronized to the
host bodies. Their gameplay energy units are not a claim of physical blast
pressure or material-calibrated joules.
[Avian forces documentation](https://docs.rs/avian3d/latest/avian3d/dynamics/rigid_body/forces/index.html).

Box2D's primary documentation recommends fixed timesteps and explains both the
cost of awake bodies and the advantage of processing body movement events.
These are architectural lessons, not a reason to change Micrology's backend.
Preserve sleep, process only the native body being held, and bound fracture
admission so repeated contacts cannot create unbounded recursive work.
[Box2D simulation documentation](https://box2d.org/documentation/md_simulation.html).

The main remaining frame-time risk is a large connected structure's synchronous
topology/oracle transaction. The measured 14.54 ms blast batch makes the next
performance priority concrete: resumable or worker-based topology jobs with
revision validation and atomic admission. Exact occupancy remains beside that
optimization. Neither dropping geometry under pressure nor making every cell
a body is an acceptable shortcut. Large piles also need long-duration sleep,
wake, and collider-residency measurements beyond this small arena.

Joints, constrained demolition tools, richer structural failure, and blast
occlusion remain future mechanics. This drop does not add more materials or
claim a finished Teardown-equivalent engine.

## Validation and reproduction

- 730 engine/workspace tests and doctests; 11 sandbox tests.
- Workspace/all-target Clippy with warnings denied; format and diff checks.
- All eight existing destruction result cases remain current and accepting.
- Existing replay fixture checks and stress baseline pass; both new replay
  scripts validate. The original crack-separation replay is also rerun against
  its previously accepted checkpoints.
- Fresh renderer-native PNGs and JSON checkpoints under
  `docs/diagnostics/teardown_interaction/`. They show the application itself,
  without moving, minimizing, or recording other desktop windows. The existing
  non-invasive FFmpeg/ddagrab tooling is preserved.

Run the lab interactively from the repository in PowerShell:

```powershell
$env:MICROLOGY_DESTRUCTION_LAB = '1'
cargo run --release -p sandbox
```

For the automatic demonstration, set `MICROLOGY_REPLAY_SCRIPT` to
`fixtures/destruction/replay-blast-grab-throw.json` and
`MICROLOGY_REPLAY_CAPTURE=1` before launching. That variable paces replay commands;
it does not start a desktop recorder. Set `MICROLOGY_NATIVE_CAPTURE=1` only for
renderer screenshots. `MICROLOGY_PROFILE_SECONDS=65` writes a bounded profile and
exits. Keep capture off for performance measurements. The stress script is
`fixtures/destruction/replay-blast-stress.json`, measured here for 55 seconds.

```powershell
python tools/verify-interaction-replay.py
cargo run --release -p engine_stress --bin interaction_bench
```
