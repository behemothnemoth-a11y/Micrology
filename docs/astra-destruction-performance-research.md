# Astra-scale destruction: research and performance direction

> Update, 2026-10-02: the user superseded section review gates with a large
> integrated drop. Historical measurements below describe the earlier commit;
> see [the integrated drop](integrated-one-material-destruction.md) for current
> contact fracture, byte limits, live profiles, tests and remaining limits.


Research and code inspection: 2026-10-02. Scope: Micrology, one homogeneous
physical material, exact microcell geometry, persistent fracture, native
destructible fragments. This is an engineering direction with one measured
implementation improvement, not a claim that a complete Teardown-scale game is
already running without stalls.

## Decision

Keep the current sparse volumetric engine and independent exact oracles. Finish
the single-material destruction loop, while making every stage local, scheduled,
and measurable. Separate fine visual/cell detail from the number of objects the
physics backend actively solves. Treat color variation as an appearance concern;
it must not require millions of distinct physical response profiles.

The largest immediate risk found in code is work that grows with unrelated
world history. The first repair replaces full fracture-state copies during a
local impact with a transaction containing only affected records. It preserves
the old implementation as a test-only reference and changes no fracture tuning.

"Lag free" becomes an explicit workload and hardware target. An unbounded number
of active bodies, contacts, edited cells, uploads, or impacts cannot fit in a
finite frame budget. Under overload, queue work, preserve geometry, and reduce
optional presentation/active simulation cost. Never turn budget exhaustion into
permission to erase geometry or skip authoritative damage commands.

## What the primary sources actually establish

### Teardown and its successor technology

Dennis Gustafsson's 2024 account distinguishes Teardown's dense per-shape storage
from a **new engine** using sparse 8x8x8 blocks. It also describes data-parallel
task scheduling, a parallel physics solver, contact/broad-phase improvements and
substepping. These are evidence for sparse storage and controlled parallel work,
not proof that Micrology should adopt that block size or rewrite its renderer.
His new hardware-raytracing renderer is separate work; its claimed benefits are
not measurements of Micrology. [Source](https://blog.voxagon.se/2024/12/29/year-summary.html)

The 2026 multiplayer account describes fixed-point destruction and ordered
deterministic structural commands, alongside state synchronization for motion.
It also reports bounded late-join command history because replay itself becomes
expensive. **Inference for Micrology:** retain deterministic topology and command
ordering while treating backend poses separately, and bound replay/history work
as well as live simulation. [Source](https://blog.voxagon.se/2026/03/13/teardown-multiplayer.html)

The spraycan article describes palette-indexed voxel materials and inherited
palettes for broken pieces. It illustrates the memory advantages and the
constraints of tying colors to a small palette. **Inference:** keep Micrology's
local palettes and Astra's exact color goals, but do not copy a 255-color limit
or create a physics material for every paint color.
[Source](https://blog.voxagon.se/2020/12/03/spraycan.html)

### NVIDIA Blast: useful architecture, different geometry contract

Blast models connected support chunks and bonds, detects disconnected islands,
and represents newly disconnected groups as actors. Its documentation describes
a pre-fractured hierarchy. This supports Micrology's component-to-fragment rule,
but replacing editable microcells with a fixed pre-fractured asset would change
the product. Use graph/hierarchy ideas as acceleration structures over native
geometry. [Source](https://docs.omniverse.nvidia.com/kit/docs/blast-sdk/latest/docs/api/introduction.html)

Blast's stress extension works on a support graph with tension, compression and
shear limits. It exposes iteration and graph-complexity tradeoffs and explicitly
requires external physics information. **Inference:** use an optional, coarse
support/stress graph to propose or prove decisions, and keep exact microcell
connectivity as the fallback when a proof is inconclusive. Graph reduction is
not authority to destroy a component the exact model cannot justify.
[Source](https://docs.omniverse.nvidia.com/kit/docs/blast-sdk/latest/docs/api/extensions/ext_stress.html)

### Solver quality, sleeping and frame pacing

Avian 0.7 documents island-based sleeping: an island rests only when all its
connected bodies qualify; changing transforms/velocities or applying forces can
wake it. It offers diagnostic plugins and notes that parallel execution can add
overhead for small scenes. **Inference:** measure contact islands, wake churn,
solver time and actual body counts before changing backend or thread settings.
[Sleeping](https://docs.rs/avian3d/latest/avian3d/dynamics/rigid_body/sleeping/struct.Sleeping.html),
[Avian configuration and diagnostics](https://docs.rs/avian3d/0.7.0/avian3d/index.html)

Erin Catto's Solver2D experiments and the Small Steps paper are useful evidence
for testing substep/iteration tradeoffs. The latter studies XPBD; neither source
establishes a universal best substep count for Micrology's Avian contact piles.
Changing solver settings must be tested for stability, tunneling, sleeping and
CPU cost on the same debris cases.
[Solver2D](https://box2d.org/posts/2024/02/solver2d/),
[Small Steps paper](https://mmacklin.com/smallsteps.pdf)

Glenn Fiedler explains the feedback loop where an expensive simulation falls
behind and tries to run more catch-up steps, worsening the stall. Fixed steps,
headroom, bounded catch-up and interpolation address different parts of that
problem. **Inference:** a slow destruction job must not cause unlimited physics
catch-up, and interpolation must not modify authoritative geometry.
[Source](https://gafferongames.com/post/fix_your_timestep/)

### Meshing and alternative fracture techniques

Binary Greedy Meshing uses occupancy masks, word-wide face culling, greedy
merging and packed quads. It is a concrete candidate for a future compiler
beside Micrology's exact compiler. Its published timings use different hardware,
chunk size and workload; they are not transferable performance promises.
Material boundaries, partial regions and cross-volume faces still need oracle
tests. [Source](https://github.com/cgerikj/binary-greedy-meshing)

Breaking Good precomputes preferred fracture modes and projects impacts onto
them, avoiding online propagation for that representation. This may suit
reusable authored props. **Inference:** continuously carved microcell structures
would need mode invalidation/recomputation, making it a poor replacement for the
current general fracture model. Keep it as a possible optional asset accelerator.
[Paper](https://arxiv.org/abs/2111.05249)

NVIDIA's Nsight Systems provides CPU/GPU timeline profiling. Use a timeline to
separate CPU stalls, GPU work, uploads and synchronization before claiming a
renderer or compute rewrite would help.
[Source](https://developer.nvidia.com/nsight-systems)

## Findings in Astra and Micrology

These are repository findings, distinct from external research.

Astra's `docs/render-performance-0.7.0.md` records that the 13,436-host Huracan
and 12,805-host Chiron overwhelmed the per-host renderer even with greedy mesh
caching. Moving static work into chunk compilation removed the observed lag in
that test. The lesson is to remove recurring submission overhead, not simply
reduce triangles. I inspected the existing Astra checkout read-only; its
uncommitted development work was preserved. Its historical observations are not
a fresh frame-time measurement from this session.

Micrology already has several correct foundations:

- 16-cubed storage volumes, region residency, and independently configurable
  render sections; empty/uniform/indexed volume representations;
- exact and greedy geometry compilers, local dirty tracking and worker meshing;
- native cell-based fragment geometry, cell-derived collision, independent
  render/physics residency, asynchronous fragment collision jobs;
- fragment memory/body/collider admission, deterministic damage work ceilings,
  native fragment identities, replay and reference benchmarks;
- optional mechanics, with no renderer or physics backend in engine crates.

The following risks deserve measured work:

| Code path | Finding | Consequence / next action |
| --- | --- | --- |
| `FractureState::apply` | Copies both complete damage maps for each local hit | Measured and repaired in this pass |
| `restore_region` | Also clones both maps | Repair separately for streaming bursts, using the same atomic semantics |
| `forget_space`, ownership searches | Some scan all stored records | Index by ownership or bound ranges before fragment storms |
| Replay `apply_benchmark_case` | Evaluation, fracture connectivity and exact cross-check run synchronously | A large reachable component can block a frame despite finite work limits |
| Structural limits | Default allows up to 1,048,576 cells per component and 4,194,304 per pass | A work ceiling is not a millisecond deadline; use incremental jobs and bounded commits |
| `apply_mesh_results` | Scheduler already caps result count per tick | Integrated drop adds a 2 MiB soft byte budget and bounded stale-result inspection; one oversized result can progress alone |
| Physics host | Limits rebuild/body work; retains exact fragment data | Good starting point, but contact count and awake islands also need measurement |
| Impact host | Still uses the older scalar secondary-damage route and ignores fragment-to-fragment pairs | 0006.4 must connect bounded contacts to the same fracture model |
| Small detached piece | Existing benchmark fails 54 of 60 cells on its next hit | 0006.5 must correct free-surface over-fragility without adding materials |

## A performance contract for this machine

Observed hardware: Intel Core 7 240H (10 cores / 16 logical processors), roughly
32 GiB RAM, RTX 5050 Laptop GPU with 8,151 MiB reported VRAM, driver 581.34.
Laptop thermals, power mode, other applications and resolution affect timing.
No system power settings or drivers were changed.

Initial **proposed targets**, not achieved claims:

| Measure | Target / interpretation |
| --- | --- |
| Reference presentation | 60 FPS at 1920x1080; 120 FPS is a separate later profile |
| Frame budget | 16.67 ms total; record p50/p95/p99/max and hitch counts |
| CPU application work | Aim below 8 ms p95 to leave scheduling/driver headroom |
| GPU frame | Aim below 12 ms p95 on the reference camera/workload |
| Main-thread destruction commit | Aim below 1 ms p95 for ordinary local hits; large commits must be staged safely |
| Hitches | Investigate every gameplay frame above 33.3 ms; no unexplained repeated 50+ ms stalls |
| Interaction latency | Record command-to-commit and commit-to-visible latency separately from FPS |
| Memory | Resident cells + fracture + fragment geometry + meshes + collision + snapshots + queued work |

Measure release builds, warmed assets, identical replay/camera/resolution, and
both idle and destruction intervals. Separate uncaptured profiling runs from
FFmpeg evidence runs. Capture and encoding consume resources and are not neutral
performance instruments. Record background activity rather than manufacturing a
"60 FPS" claim from a VSync-capped average.

The acceptance ladder should grow from the unchanged shared wall to independently
versioned fixtures: repeated local hits; many separated walls; a large attached
structure; 32/128/512 active fragment piles; settled piles; damaged region reload;
high-color geometry; and sustained impact chains. Expand only after the previous
scale has measurements. Counts are test workloads, not promises of capacity.

## Architecture decisions ranked by likely payoff

1. **Local transactions and bounded ownership work.** Stage changed keys, avoid
   copying or scanning unrelated history, keep failure atomic and observable.
2. **A scheduled fracture pipeline.** Capture bounded immutable inputs, evaluate
   and classify on workers, validate geometry plus fracture revisions, and
   commit in canonical command order. Inconclusive work remains queued. An
   arbitrary worker completion order or wall-clock cutoff must never decide
   topology. Persist applied command order/tick if replay depends on timing.
3. **Connectivity acceleration beside the oracle.** Cache volume-local connected
   components and boundary bonds; invalidate edited volumes and neighbors. Use
   a coarse graph/support witness to prove cheap cases, then exact searches when
   needed. Fragment membership remains exact; a coarse graph cannot invent a
   cut or mistake unloaded space for air.
4. **Budget derived work separately.** Bound mesh uploads, collider creation,
   snapshots and job completions per update, with queue age/fairness metrics.
   Do not let an async completion burst move an unbounded stall onto the main
   thread. Keep the last valid render until its replacement is ready.
5. **Control active physics.** Sleep stable islands, restrict collision residency,
   combine adjacent occupied cells into collider boxes, cap spawned bodies and
   measure contacts. Keep every detached component as its own native `Fragment`.
   Distant or budget-held fragments may lack an active backend body under an
   explicit policy; their authoritative geometry must remain available.
6. **Separate visual batching from physical identity.** Render many sleeping or
   distant fragments in batches without merging their native identities. Dust
   and particles can be cheap visual effects; they cannot substitute for an
   existing structural fragment or quietly delete its cells.
7. **Keep static rendering cheap.** Continue region/section batching and local
   mesh invalidation; then evaluate occupancy masks and packed quads against
   the exact surface oracle. Use frustum/distance/occlusion policies and visual
   LOD only where measurements identify a bottleneck.
8. **Use GPU compute selectively.** Rendering, culling and cosmetic effects are
   strong candidates. Moving mutable authoritative fracture to the GPU adds
   synchronization, readback and deterministic-order problems. Profile before
   choosing it. A raytracer or a new physics backend is not a prerequisite for
   the one-material destruction loop.

## Measured implementation: local atomic damage commits

`FractureState::apply` previously cloned every stored cell and bond record before
changing a local subset. It now reads unchanged records in place and stages
insertions, replacements and deletion tombstones. Entry admission checks the
final map lengths before either live map changes. Once admitted, only staged
keys are committed. An empty map takes ownership of the staged tree, avoiding
a second allocation/insertion pass. Ordered maps preserve result order.

Additional staging storage is proportional to touched keys, rather than total
history. Access still has ordered-map lookup cost; this is not a constant-time
claim. Restore/streaming transactions are not changed by this patch.

### Method and result

Release build, 31 measured trials after three warmups per workload on the
hardware above. Baseline was `dc30b64` plus the same diagnostic timing harness;
the final implementation was measured after the local transaction change. No
fracture coefficients, benchmark targets, occupancy oracle or fixture changed.
Runs were sequential, not a statistically controlled randomized A/B experiment.

The same weak hit loads 497 cells and 1,840 bonds. Synthetic unrelated records
grow the stored history without changing that hit or its world. Each row below
counts cell and bond records together: the largest uses 500,000 of each.

| Unrelated records | Before commit median | After commit median | Before / after p95 |
| ---: | ---: | ---: | ---: |
| 0 | 0.326 ms | 0.394 ms | 0.434 / 0.498 ms |
| 20,000 | 1.476 ms | 0.605 ms | 1.785 / 0.794 ms |
| 200,000 | 11.733 ms | 0.619 ms | 12.924 / 0.704 ms |
| 1,000,000 | 62.160 ms | 0.645 ms | 65.230 / 0.741 ms |

The largest case improves about **96 times** for the commit stage, or 98.96%
less time. The empty-history median regresses by 0.068 ms (about 21%); that
small absolute cost is the present tradeoff for avoiding history-sized copies.
Evaluation remains roughly 1.06-1.29 ms median in the final run and is a
separate stage. These are CPU diagnostic timings, not whole-frame/FPS claims.
Reset, checksums, renderer, physics, connectivity, IO and setup are excluded.

Result checksums match the original in every row: `ed45741ae0b089ef`,
`50fc1177b9b06f4f`, `4967ff76e000a03f`, `6a378e6bd2d02baf` respectively.
The committed `fracture_perf` diagnostic allows rerunning with:

```powershell
cargo run --release -p engine_stress --bin fracture_perf -- 31
```

Do not make variable timings deterministic CI thresholds. Keep the raw before
and after reports with the dated diagnostic evidence, separate from canonical
benchmark results. The test-only `reference_commit` retains the old full-copy
algorithm independently: repeated shared impacts, atomic entry refusals and
256 seeded adversarial loads compare complete states, revisions and outcomes.
A separate storage test proves that editing three keys over 100,000 existing
records stages only those three keys.

### Validation and fresh replay evidence

- All 712 engine tests and all five sandbox tests pass, including the three new
  local-transaction tests and the existing independent connectivity tests.
- Workspace Clippy with warnings denied, formatting and diff checks pass.
- The destruction pack and all seven implemented cases match committed results;
  replay validation, baseline digest and the general stress baseline pass.
  `chunk_into_wall` is still unimplemented, not silently counted as a pass.
- The release sandbox was rebuilt and the existing two-strong-hit, pause and
  single-step script was captured through non-invasive FFmpeg/ddagrab. The
  30-second, 2586x1512 video and four fresh dumps are under
  `docs/diagnostics/drop0006_perf_local_commit/`.
- All four fresh dumps match the previous accepted capture's structural,
  fracture and separation fields. After the second hit: 2,867 static cells,
  190 cells in 15 native fragments, 190 cells freed by cracks, zero freed by the
  occupancy classifier. After 120 steps the same geometry remains and all 15
  fragment translations have changed. Rendered frames were visually inspected.

This capture proves preserved behavior, not performance under large debris
loads. Recording itself affects frame time. The HUD's legacy DROP 0004/brick
labels do not identify this test's fracture version or introduce another
material. No Bevy/Avian dependency entered an engine crate; the occupancy
oracle, authored-world validity and optional simulation contracts are unchanged.

## Continued direction

The user has explicitly requested a large integrated drop, superseding the
previous per-section stops. Contact fracture, atomic topology admission,
interactive tools and performance measurements now ship together; the linked
integrated report replaces the old proposed section order. Keep one material.

Secondary impacts must use a canonical contact representative, explicit units
and clamped finite energy. Contact repetition is not permission to inject energy
every tick indefinitely. Refusal must preserve state, and a retry must not apply
the same event twice. Validate energy/direction and generation limits with
headless fixtures, then compare actual replay visuals.

Micrology remains the reusable engine; EarthForge remains a future application
and data pipeline. Recreating Earth requires streaming, provenance and scale
work beyond this destruction subsystem. Fine local cells do not imply loading
or simulating all of Earth at microcell resolution.
