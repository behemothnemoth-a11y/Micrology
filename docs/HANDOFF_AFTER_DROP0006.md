# Handoff after DROP 0006 closeout

DROP 0006 — Core Destruction / the Fracture Model — is **closed complete**.

Read first:

```text
docs/drop-0006-closeout.md
docs/HANDOFF_DROP0006_7.md
docs/drop-0006-7.md
```

The older section docs remain historical evidence. When one says a later
section is "future", the closeout document is authoritative about whether it
eventually shipped.

## Current continuation state

Branch:

```text
chatgpt/drop-0006-7-fixedstep-rebuild
```

Latest fully tested code head:

```text
db0fe8310b2d874df3db595b4a617f50504ba9d0
```

GitHub Actions acceptance:

```text
run 199
id 37112865912
engine job PASS
sandbox job PASS
```

Later commits on the branch are documentation-only and intentionally use
`[skip ci]`.

Draft validation PR #8 remains a validation surface for the continuation
lineage; it is not a request to merge the historical stack blindly into the
old main branch.

## DROP 0006 final capability

The one-material destruction subsystem now includes:

```text
persistent sparse cell fracture
persistent sparse bond fracture
directional impact propagation
free-surface response
crack-tip continuation
fatigue floor
fracture-aware connectivity beside exact occupancy connectivity
streaming ownership/extract/restore
fragment-local fracture ownership
native volumetric fragments
re-fracture after detachment
solved-contact secondary destruction
fragment→static and fragment→fragment damage
bounded contact generations/queues
blasts
grab / pull / throw
demolition / structural capacity
background owned fracture jobs
local fracture indexing
coherent fragment grouping policies
deterministic fragment-distribution measurement
fixed-step authoritative collision readiness
bounded async static collider compilation
stale/superseded collision rejection
collision queue telemetry
deterministic contact replay acceptance
```

All of this still uses one homogeneous reference material on purpose.

## Hard invariants preserved

Keep these unless a future design explicitly reopens them:

```text
cells are data
no ECS entity per cell
no rigid body per cell
native volumetric Fragment objects

exact occupancy connectivity remains the permanent topology oracle
fracture-aware connectivity stays beside it
fragment partitioning is not topology authority
spatial fracture index is acceleration/ownership only

unknown != empty
inconclusive != destruction permission
budget exhaustion != destruction permission

fragment admission is atomic
async results are stale-checked
observable topology/order is deterministic

collision derives from cells, not render meshes
Bevy/Avian stay outside engine crates

work budgets and byte budgets stay separate

destruction / stress / physics remain optional
authored fantasy/impossible structures remain valid when policies are disabled
```

## Accepted scheduling regression signal

Do not use the old nondeterministic stress checksum as the primary 0006.7 signal.

Use the dedicated contact-volley gate.

Three identical sandbox runs, six checkpoints each, end identically at:

```text
fixed ticks             300
static cells            2983
fragments                 61
fragment cells            269
structural checksum af9d203e941190b3
fracture checksum   1aece4b9460d1de3

contact samples             8
enqueued                   16
processed                  16
refused                     0
capped                      0
stale                       4
static failed              31
fragment failed           209
fragments created          63
```

## Partition-policy decisions already made

Do not casually reopen these.

### Global default

Still:

```text
FragmentPartitionPolicy::PerCrackComponent
```

`coherent_mean` and `coherent_median` both reduce the real 60-cell fixture
from the singleton-heavy result to the same 16-fragment result, but there is only
one real re-fracture fixture family.

Changing the global default requires broader evidence.

### Static path

Leave static grouping unchanged.

Measured first strong liberation:

```text
190 detached cells
15 fragments
0 singletons
largest 60 cells
occupancy groups 6
```

It does not exhibit the singleton/dust pathology that motivated the fragment
partition layer.

## Existing real-hardware baseline

Do not say "there has never been Windows/GPU evidence."

An earlier integrated run on Core 7 240H / RTX 5050 Laptop GPU measured an
18-chunk sequence:

```text
all frames p99          17.807 ms
all frames max          21.841 ms
physics-active p99      18.033 ms
contact-batch p99        7.050 ms
frames >33.3 ms              0
simultaneous fragments      201
fragments created           239
```

But that predates the final 0006.7 collision scheduler.

The accurate open item is:

> revalidate the **current** scheduler on Windows/real GPU and revisit the older
> 544-fragment response-latency observation.

## Open items that are now post-closeout work

These do not reopen DROP 0006 automatically.

### 1. Current-code Windows/GPU validation

Need p50/p95/p99/max, hitch counts and response latency on the current branch.

### 2. Fracture-index memory optimization

Index key overhead remains large. Do not change key ordering before making
`FractureState::digest()` canonical independent of storage order and proving
checksum equivalence.

### 3. Whole-state maintenance contracts

`prune_against` and `forget_space(StaticWorld)` remain broad by contract.

### 4. Broader fragment-shape research

More deterministic real break fixtures are required before changing the default
grouping policy or adding shape-aware/minimal-cut partitioning.

## Free-only workflow

The user explicitly wants to keep infrastructure free for now.

Use:

```text
GitHub branch edits
GitHub Actions for Linux build/test/headless sandbox validation
Windows PC only when Windows/GPU-specific evidence is needed
```

Do not provision DigitalOcean unless the user later explicitly asks to change
that decision.

At the time of closeout, Desktop Commander monthly capacity had been exhausted;
do not assume remote Windows access is currently available.

## What to do next

Do **not** automatically start 0006.8.

The next conversation should choose a new boundary first.

Useful questions:

1. Is the next priority current-code Windows/GPU validation?
2. Is the fracture-index memory optimization worth doing before more features?
3. Does the user want more destruction-character/fragment-shape work?
4. Should development return to an unfinished broader Micrology roadmap item
   outside DROP 0006?

Only after that choice should a new numbered drop be named.
