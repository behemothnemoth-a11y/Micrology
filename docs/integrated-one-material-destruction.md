# Integrated one-material destruction drop

2026-10-02. The user explicitly superseded the earlier numbered-section review
gates and requested one substantial integrated drop toward Astra-style teardown.
This delivery stays on `fracture-integration`, retaining Claude's `f8f19a5`,
`3c04ca6` and `cb6e283`, the acceptance work in `dc30b64`, and the performance
repair in `2327383`. No prior completed work was reset or discarded.

## What is now playable

The disposable benchmark lab has manual strikes, strong strikes, native chunk
launching, and contact-driven fracture. Wall and fragment participants can both
take damage; fragments can strike other fragments and split again. All use the
same single reference material and persistent cell/bond damage model.

`R` launches a 60-cell chunk actually liberated from the shared wall by its
canonical strong-hit sequence, including that chunk's existing damage. It is
not a hand-built substitute. Left click targets authoritative static or rotated
fragment cells; Shift increases the strike energy. `F9` toggles contact damage,
`F6` pauses/runs physics, `F7` single-steps, `F8` changes speed, `F10` dumps state,
and `F11` advances a replay. Camera movement remains available while paused.
See the root README for launch commands.

Two added scripts use the existing replay lab:

- `replay-contact-volley.json`: two three-chunk volleys, intermediate snapshots,
  300 fixed steps, and a final interval with contact damage disabled.
- `replay-contact-stress.json`: two nine-chunk volleys, 660 fixed steps, and
  snapshots before and after disabling contact damage.

The lab no longer participates in normal fragment payload streaming. Visual
inspection found that deleted projectile identities could otherwise be offered
to the disk loader. Lab startup also bypasses authored-world loading/generation;
the normal sandbox and its storage behavior remain available outside lab mode.

## Engine and host changes

**Atomic fracture transactions.** New static and fragment APIs stage damage and
classify the proposed geometry before committing. Admission sees candidate
native fragments. Unknown residency, inconclusive topology, state limits,
identity collisions, exhausted IDs or storage refusal leave geometry, damage,
ownership and the destruction sequence unchanged. The static view masks proposed
failures without cloning the world. A fragment transaction stages only its
parent, rather than the complete fragment store.

**Independent connectivity.** Broken bonds gate the fracture-aware traversal;
exact occupancy connectivity remains beside it as the permanent oracle. Native
fragments are exact connected components. Cells remain volume data. No per-cell
ECS or rigid-body representation has been introduced.

**Local ownership and restoration.** Damage restore now stages changed keys.
Fragment-specific cell/bond queries use ordered space ranges. Remapping a split
builds a direct cell-to-child lookup rather than searching every child for each
record. These paths avoid copying/scanning unrelated damage history. Region
enumeration and other existing global maintenance paths are not all indexed by
this change.

**Bounded contact policy.** Avian stays in the host. It supplies solved impulses,
pre-solve closing speeds and world contact geometry. The strongest finite
contact supplies both position and normal, with a complete deterministic tie
break. Local-frame conversion handles rotated fragments before quantization.
Native owner pairs, rather than collider boxes, define contact beginnings.
Resting contacts cannot add damage every solver tick. The event's clamped energy
budget is shared between both participants, not duplicated per contact point.

The host admits at most 32 queued targets, processes at most two targets per
fixed step, and limits secondary generations to three. It checks installed
collider geometry and queued target revisions. Refused, capped or stale work is
counted and skipped; it does not erase geometry. The current queue does not
automatically retry refused contact events. Frame clocks never decide cuts.

**Derived-work limits.** Static mesh application already had a result-count cap.
This drop adds a 2 MiB soft byte budget and bounded stale-result inspection.
Fragment uploads have a separate 2 MiB soft budget beside their four-upload
limit. One oversized first result can progress alone to avoid starvation; it
is counted, so these byte limits are not a strict maximum upload size. Existing
worker, collider, body, residency and tracked-memory limits remain active.

**Telemetry.** Bounded frame and contact-batch sample buffers report median,
p95, p99, maximum and hitch counts. Physics-active frame intervals are separated
from paused replay intervals. The frame-start interval is attributed to the
previous frame's work. Timing never enters deterministic acceptance checksums.

## Benchmarks and validation

The shared fixture checksum remains `704df76eee29b211`. All seven earlier cases
retain every existing result field. `chunk_into_wall` now executes a headless
representative contact against the wall and the canonical damaged chunk, plus
a perpendicular-direction control. It touches 681 cells and 1,508 bonds, records
104 broken bonds, and fails 61 cells: one in the wall and all 60 in this already
damaged chunk. The live projectile contacts use the same policy but receive
their actual positions and impulses from physics, so their counts differ.

Secondary-damage acceptance is now checked, rather than left unused. All eight
cases are measured and pass. Results document version 2 adds the secondary
observation; the benchmark geometry/spec version and native save format are
unchanged.

Validation includes the complete engine regression (724 tests), sandbox tests (6 tests), workspace
Clippy with warnings denied, formatting/diff checks, canonical destruction
results, the general stress baseline, and fresh renderer snapshots tied to replay dumps. New focused
checks cover atomic refusal, unknown residency, topology exhaustion, identity
exhaustion, deterministic transactions, negative-coordinate ownership ranges,
bounded/shared contact energy, rotated local contact conversion, deferred/stale
mesh uploads and launch-command validation.

## Measured performance

Hardware: Core 7 240H, 32 GiB RAM, RTX 5050 Laptop GPU, driver 581.34. Release
builds use the default sandbox window and AutoVsync. The accepted renderer snapshots are 1600x900
from a separate compact capture window. These are observed desktop frame intervals, not GPU
timer results or an uncapped throughput claim. Profiles exclude five seconds
of startup. Recording runs are separate from uncaptured profiling runs.

Final uncaptured eighteen-chunk run, 45 seconds total:

| Interval | Samples | Median | p95 | p99 | Maximum |
| --- | ---: | ---: | ---: | ---: | ---: |
| All post-warmup frames | 2,401 | 16.666 ms | 17.220 ms | 17.807 ms | 21.841 ms |
| Physics-active frames | 660 | 16.659 ms | 17.197 ms | 18.033 ms | 21.382 ms |
| Contact-processing batches | 57 | 0.081 ms | 5.871 ms | 7.050 ms | 7.050 ms |

No recorded interval exceeded 33.3 ms or 50 ms. The low contact median includes
small fragment updates and skipped stale targets; p95/max better describe the
larger batches. The run reached 201 simultaneous native fragments and created
239 across the chain. It processed 114 queued targets, with 25 sampled
fragment-fragment pairs, 11 capped chains and 38 stale observations/targets.
It left 199 fragments holding 683 cells, plus 2,333 static cells. There were
376 failed static cells and 820 failed projectile/fragment cells; fracture
refusals and pending targets were zero at completion.

After contact damage was disabled, 120 further physics steps changed fragment
motion without changing the geometry, damage digest or contact counters. The
same final geometry and damage counts were reproduced in the preceding
uncaptured run; live backend determinism is still not guaranteed.

The final sparse-history diagnostic uses 31 samples after three warmups. With
one million unrelated records, state-commit median is 0.781 ms and p95 is
1.017 ms; the original full-copy median was 62.160 ms, about 80 times slower.
The empty-history median is 0.458 ms versus the original 0.326 ms. All four
history-size checksums remain unchanged. These timings exclude connectivity,
physics, rendering, IO, reset and digest calculation; they are not frame time.
The raw measurements are preserved with the delivery evidence.

## Evidence and practical limits

Fresh native renderer frames and matching JSON snapshots are under
`docs/diagnostics/integrated_contact_volley/` and
`docs/diagnostics/integrated_crack_regression/`. They show the intact fixture,
launched chunks, the first breakup, secondary wall damage and damage-off state.
The crack replay preserves the accepted 2,867 static cells and 190 cells in 15
native fragments: 190 cells freed through cracks, zero freed by occupancy.
All four structural/fracture/separation records match the previously accepted
replay; all 15 fragment translations change after the scripted physics steps.

The existing non-invasive FFmpeg/ddagrab recorder was exercised. Other desktop
windows obscured the recaptures, so those videos were rejected and removed.
The accepted images are saved directly from Bevy's render target with
`MICROLOGY_NATIVE_CAPTURE=1`, alongside each replay dump. Only one readback may
be outstanding; capture is optional and is off during performance runs. This
produces actual rendered scene evidence without recording other applications.
The `-Compact` recorder option configures only a newly created capture window;
it never rearranges existing desktop windows.

The uncaptured stress snapshots, raw timings, canonical benchmark results and
verification logs are in the accompanying evidence archive. The authored save
files remained byte-for-byte unchanged across the final capture checks, and
no disposable lab world was written to disk.

This is a substantial playable destruction-lab delivery, not a finished
Teardown-scale game. The research and priorities are in
[`astra-destruction-performance-research.md`](astra-destruction-performance-research.md),
including primary sources on Teardown, Blast, dynamic connectivity, voxel
meshing, physics sleeping and CPU/GPU profiling.

The current contact-fracture integration is opt-in in the disposable one-material
lab. Authored-world persistence/streaming still uses its existing host paths;
shipping the new contact policy in arbitrary streamed worlds requires a
residency-aware persistence integration. Engine crates remain backend-free.

Headless fracture results and scripted strike replay checksums are deterministic.
Live contact chains also depend on the solver and asynchronous collider readiness;
this drop does not promise identical live contact chains across runs, machines
or backend versions. The counts in each evidence snapshot describe that run.

Zero lag for arbitrary scenes is not established. Contact evaluation and exact
topology still run synchronously inside each bounded transaction. Larger attached
structures can cost more than this wall. The next scaling work is immutable
worker snapshots with ordered revision-checked commits, local connectivity
caches checked against the oracle, contact-pile/settled-pile benchmarks, and
GPU/interaction-latency measurements. Secondary caps preserve geometry under
overload but can reduce how much additional damage is simulated. This drop
does not retune the reference material or claim finished fracture aesthetics.
