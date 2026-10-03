# Handoff after DROP 0006.7 — deterministic scheduling and bounded collision rebuilds

This supersedes `docs/HANDOFF_DROP0006_6.md`.

## Start here

Current continuation branch:

```text
chatgpt/drop-0006-7-fixedstep-rebuild
```

The fully CI-proven **code** head is:

```text
db0fe8310b2d874df3db595b4a617f50504ba9d0
```

The branch HEAD containing this handoff is a docs-only `[skip ci]` descendant
of that tested code. Start from the branch HEAD, not from `main`.

The draft PR is **#8**, “WIP validation: DROP 0006.7 fixed-step collision
scheduling”. It exists to run Actions against the continuation lineage. It is
not a request to merge the entire continuation stack into the old `main`
branch.

GitHub Actions run 199 (`37112865912`) is the acceptance run for the tested
code head: engine job PASS, sandbox job PASS.

## What is now complete

Do not redo these unless a later regression specifically requires it:

```text
continuous visual replay
demolition replay
stress/progress video capture

DROP 0006.5 local fracture indexing / scalable ownership
DROP 0006.6 coherent fragment partition layer
DROP 0006.7 fixed-step collision readiness
DROP 0006.7 async static collider compilation
DROP 0006.7 collision queue/coalescing telemetry
DROP 0006.7 authoritative contact replay determinism gate
```

The one-material destruction stack still remains an optional Micrology subsystem,
not the engine itself.

## The most important 0006.7 finding

The contact collector was already in `FixedPostUpdate` after Avian physics.

Do **not** describe this drop as “move contact sampling to fixed step”.

The actual problem was that collider/body freshness and publication were paced
by rendered `Update` frames. Fixed physics therefore could observe different
derived collision state depending on frame retirement.

The fix is:

```text
authoritative cells/fragments
        ↓
bounded local derived collision work
        ↓
current/stale validation
        ↓
authoritative collision readiness
        ↓
scripted fixed step may advance
```

Render mesh progress is not an input to this barrier.

## Determinism is now a real gate

The old stress replay warning from 0006.5 is superseded for the new dedicated
contact-volley fixture.

Run 199 repeated the same sandbox contact replay three times under Xvfb/lavapipe
and compared six authoritative checkpoints in every run.

Result:

```text
PASS
runs                  3
checkpoints/run       6
final fixed ticks   300
```

Every run ended at:

```text
structural checksum  af9d203e941190b3
fracture checksum    1aece4b9460d1de3
static cells         2983
fragments              61
fragment cells         269

contacts sampled        8
enqueued                16
processed               16
refused                  0
capped                   0
stale                    4
static failed           31
fragment failed        209
fragments created       63
```

Use this deterministic fixture for 0006.7 scheduling regressions.

Do not resurrect the old “stress checksum changed, therefore regression”
reasoning for unrelated legacy stress captures.

## Collision scheduling state

### Static world

Before 0006.7, static collision CPU compilation was synchronous inside the host
update.

Now:

```text
one VolumePos
  → owned volume snapshot
  → bounded async GreedyCollisionCompiler job
  → BTreeMap ready queue
  → revision check
  → deterministic nearest-first bounded Avian publication
```

Bounds:

```text
max active static collision jobs          8
max static collider publications/frame    8
```

A newer edit supersedes an old job. A stale result cannot publish. Duplicate
work coalesces by `VolumePos`.

### Native fragments

Fragment collision jobs remain local to exact `FragmentId` and geometry
revision. Pose/velocity changes do not stale the collider.

Real fragment bodies temporarily leave backend simulation while nearby static
collision is incomplete. Pose and velocities remain in the engine-owned
`Fragment`, so the object freezes conservatively and resumes from authoritative
motion state when collision becomes current.

### Rendering

Do not rewrite the render mesh scheduler as part of this work. It already has:

```text
section-level deduplication
bounded active workers
snapshot byte budget
stale result rejection + requeue
bounded apply count
bounded upload bytes
deterministic priority ordering
```

Collision remains derived from cells, not render meshes.

## 0006.6 decisions closed during this drop

### Mean vs median

A `CoherentMedianScale` / `MEDIAN` policy now exists.

A synthetic tail-heavy test proves it behaves differently from the mean when
one large component would distort the scale.

On the real canonical 60-cell break, mean and median are identical at all three
measured energies:

```text
1200: 16 fragments / 0 singletons
2500: 16 fragments / 0 singletons
6000: 16 fragments / 0 singletons
```

Therefore the default remains:

```text
FragmentPartitionPolicy::PerCrackComponent
```

Do not switch the default merely because coherent grouping looks better on the
single existing real fixture family.

### Static partition wiring

Measured before changing.

First static liberation on accepted strong-center hit 2:

```text
190 detached cells
15 fragments
0 singletons
sizes:
  3,3,3,3,
  5,5,5,5,
  9,9,
  20,20,20,20,
  60

occupancy groups    6
largest share     31.5%
top 3 share       52.6%
top 5 share       73.6%
slivers              8
fragment checksum 247ab77930357453
fracture checksum 7e325a7a43b09bfa
```

Decision: **static path stays as-is**.

It is crack-separated but does not show the 0006.6 re-fracture singleton/dust
pathology. Do not wire the fragment partitioner into `fracture_static_if`
without new evidence.

## Evidence that currently holds

The acceptance code head passed:

```text
cargo fmt --all --check                    PASS
git diff --check                           PASS
engine clippy -D warnings                  PASS
engine tests                               PASS
doc tests                                  PASS
destruction_bench --check                  PASS
destruction_replay --check                 PASS
fragment_distribution_bench --check        PASS
static fracture distribution probe         PASS
fracture_locality_bench                    PASS
stress baseline                            PASS
structural baseline                        PASS
canonical IO fixtures                      PASS

sandbox clippy                             PASS
sandbox tests                              PASS
3-run contact replay determinism           PASS
```

DROP 0006.5 locality remains bounded: the accepted fracture locality snapshot is
still 128 relevant records even with large unrelated history.

No accepted destruction golden was changed to make 0006.7 pass.

## Important open work — do not confuse with failed 0006.7

### Windows / GPU validation

Still genuinely pending.

GitHub Actions uses Linux software Vulkan. It cannot establish:

```text
Windows frame p50/p95/p99/max
frames over 33.3 ms
real GPU response latency
whether the old 544-fragment 200–319 ms response observation still reproduces
```

Do this only when a Windows/GPU execution path is available. Do not report
lavapipe timings as production GPU evidence.

### Fracture index memory

Still ~83% key overhead versus record payload in the million-record measurement.

Cheap storage-layout improvement remains blocked by
`FractureState::digest()` depending on key iteration order.

Correct order for that future work:

```text
make digest canonical independent of storage order
        ↓
prove checksum equivalence
        ↓
only then rewrite volume-major index storage
```

Keep this isolated from destruction-character changes.

### Whole-state contractual operations

Still open/known:

- `prune_against` scans the state by contract;
- `forget_space(StaticWorld)` is whole-space by definition;
- the old ~2.2x locality capture residual is still unattributed and small in
  absolute terms.

### Fragment distribution breadth / shape

Still only one real fragment re-fracture fixture family.

The static probe also reports eight sliver-shaped pieces, but no singleton
pathology. Shape-aware grouping, a percentile/median default decision, or
minimal-cut partitioning needs more fixtures first.

## Workflow / environment

The user wants to keep this free for now.

Current execution strategy:

```text
GitHub branch edits
        ↓
draft validation PR
        ↓
GitHub Actions for Linux engine/sandbox acceptance
        ↓
Windows PC only when Windows/GPU-specific evidence is actually needed
```

Do **not** provision DigitalOcean unless the user later explicitly changes that
decision.

At handoff time Desktop Commander monthly remote-call capacity was exhausted.
Do not assume local-PC access is available from a new chat.

## Scope invariants remain

Preserve:

```text
cells are data
no entity per cell
no rigid body per cell
native volumetric Fragments
exact occupancy oracle
fracture-aware connectivity independent
fragment partitioner not topology authority
spatial fracture index acceleration/ownership only
Bevy/Avian outside engine crates
collision from cells, not render meshes
unknown != empty
inconclusive != destruction permission
atomic fragment admission
stale-check async work
deterministic observable topology
separate work and byte budgets
optional destruction/stress/physics
authored/fantasy worlds valid with those systems disabled
one homogeneous reference material for this development line
```

Do not add multiple materials, fluids, fire, joints, EarthForge integration or
new gameplay tools as incidental follow-up.

## What should happen next

**Stop before another major subsystem.**

0006.7 was the last stated section of the original 0006.x sequence.

The next session should first do a DROP 0006 closeout/review:

1. review 0006.5, 0006.6 and 0006.7 together;
2. distinguish architecture-complete items from hardware-only evidence still
   pending;
3. decide whether canonical-digest/index-memory work belongs before closing the
   drop or as an independent optimization;
4. decide whether more fragment-distribution fixtures are required before
   changing the default partition policy;
5. only then define the next numbered drop.

Do not automatically start “0006.8”.
