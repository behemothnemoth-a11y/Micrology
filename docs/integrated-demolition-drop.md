# Integrated one-material demolition drop

This drop adds an owned background fracture transaction, crack-weakened load
capacity, a roof-and-column demolition fixture, and a chunk-preserving reference
response. It continues `fracture-integration` from `a81cdd6`; all earlier Claude
and Codex work remains in history. The historical eight-case destruction pack
and its material profile remain frozen and pass without regenerated counters.

## What is playable

Set `MICROLOGY_DEMOLITION_BUILDING=1` and run the sandbox from the repository root.
The disposable scene has a 578-cell roof with an overhang, four two-by-two
columns framing an open doorway, and an anchored landing platform. Its entire
1,811-cell construction uses the reference material. It is a structural test
building, not a finished interior or an Astra export.

- **J:** toggle structural capacity; **1–4:** cut a support cross-section.
- **F9:** toggle collision damage; **F6/F7:** run/pause and single-step.
- **LMB / Shift+LMB:** strike; **B:** blast; hold **RMB** to grab and pull;
  **LMB while holding:** throw. **R:** launch a canonical damaged chunk.
- **F11:** advance the included demolition replay. Existing capture and replay
  controls remain available. Ordinary authored worlds never opt into this lab.

One cut leaves the fixture sound. Two cuts with capacity disabled leave all
1,803 surviving cells in the authored static world. Enabling capacity then
fails eight additional support cells and releases one native 682-cell fragment.
The detached roof retains its upper column geometry. It can land on those
column remnants; it does not automatically disintegrate just because it is free.

The visual replay lifts and pulls that object for 180 fixed steps, throws it
downward, records collision cracks, then blasts it into multiple native pieces.
All surviving material plus explicitly failed cells must equal the starting
1,811 cells. A final 120-step interval with both damage systems disabled changes
poses but preserves geometry and the fracture-state checksum.

## Engine and host boundaries

`engine_destruction::fracture_jobs` owns a bounded snapshot and returns prebuilt
fragments, removed-cell addresses, and a sparse fracture-state patch. Analysis,
both connectivity checks, and fragment construction run on the worker. Commit
validates the original world/damage revisions, parent geometry, identity
sequence, collisions with existing IDs, and aggregate admission before applying
any geometry. Moving fragment bodies retain their current pose and velocity.

The host uses one worker and a FIFO of at most 32 requests. Three non-blocking
poll opportunities, separated by other host work, permit up to three
commits/snapshots per render frame without spinning or waiting for analysis.
Capacity triggers coalesce until the current impact burst settles; a full
impact queue cannot lose a pending capacity assessment. Contact requests retain
both their original fragment-geometry or static-volume revision stamps.
Pausing/single-stepping waits for pending destruction and contact queues. Normal
unpaused play continues physics while a worker analyzes its snapshot.

Snapshot limits are 27 known regions, 128 occupied volumes, 4 MiB of voxel
payload and 32,768 damage records for the target owner. These are conservative
refusal limits, not a claim of constant cost at arbitrary world size. The
static snapshot still copies region payload, and static fracture records are
bounded by owner rather than an impact-local spatial record index. Only the
fully known disposable lab declares empty exterior regions resident; generic
engine callers must supply real residency. Unknown or exhausted work holds all
geometry. Engine crates contain no Bevy or Avian types.

Capacity uses the existing exact integer flow solver, multiplying each face's
capacity by its remaining bond integrity. Occupancy topology remains separate.
The intact-only witness is not used to approve a damaged load network. The
existing conservative minimum-cut failure selection never chooses anchors and
limits collapse to 32 failed cells per generation and eight generations.
Mechanical settings are still optional; this is not a calibrated engineering
stress model.

## Chunk response and historical compatibility

The new `Coherent` response preserves fully disconnected cells as material in
native fragments; broken bonds alone do not erase them. It removes the free-
surface energy bonus for fragment-local hits, raises the one reference solid's
crush threshold from 6,000 to 12,000 and toughness from 350 to 650 thousandths.
The historical response remains available for exact old benchmark/replay
comparisons. These are two experiment versions of one homogeneous material,
not a new multi-material system.

The inherited 60-cell canonical chunk survives the tested 2,500-energy contact
with **60 cells and zero crushed cells**, but splits into **32 pieces**. Its
existing cracks remain real and are never healed for appearance. This is a
clear improvement in mass retention, not a claim that tiny-debris formation is
solved. The roof fixture demonstrates the complementary large-chunk behavior.

## Measurements and acceptance

See the accompanying measured JSON and verification logs. Timing capture is
separate from renderer screenshot capture. Frame measurements are VSync-limited
on this machine; p99 and maximums are reported alongside active physics frames,
queue waits and response latency. No result establishes unlimited lag-free
operation or a cross-machine performance guarantee.

The first 544-fragment stress run exposed one-completion-per-frame queue delay:
response p95 200.93 ms, p99 284.18 ms, maximum 319.63 ms, despite frame p99
20.31 ms and no frame above 33 ms. That observation motivated the three bounded
poll opportunities and coalesced capacity assessment used in the final build.
The first-run profile is preserved as `demolition-single-poll-profile.json`.

MEASUREMENTS_PENDING

Verification covers stale world, damage and parent-geometry results; identity
collision; aggregate admission; zero snapshot/analysis budgets; unknown
residency; moving-parent pose preservation; unchanged historical transaction
results; partial bond damage reducing capacity; overload redistribution;
conserved mass; generation limits; replay fixture binding; bounded FIFO and
system-off cancellation. Existing full engine regressions and sandbox tests,
all-target Clippy, formatting, diff checks, and the historical benchmark pack
also run.

## Research applied and remaining work

Bevy explicitly assigns work that need not finish for the next frame to its
[AsyncComputeTaskPool](https://docs.rs/bevy/0.19.1/bevy/tasks/index.html). This drop
uses that boundary, while keeping sparse authoritative commit on the host.

Dennis Gustafsson's [2024 engine summary](https://blog.voxagon.se/2024/12/29/year-summary.html)
discusses data-parallel task scheduling, sparse voxel objects, and a parallel
physics solver. Its sparse format describes the **new engine**, not a claim
about shipped Teardown internals. Micrology already has volumetric storage;
this drop keeps voxel data out of ECS and avoids per-cell physics construction.

His earlier [Smash Hit fracture article](https://blog.voxagon.se/2014/05/13/cracking-destruction.html)
explains why collision/fracture timing and preserved motion affect the feel of
breakage. That is relevant evidence, not a description of Teardown's current
algorithm. Micrology still samples solved contacts after the physics step;
this drop does not implement capped-impulse substep re-solving. That deeper
physics integration, improved chunk size distribution, impact-local damage
indices, and collision/mesh construction spikes are the next investigations.
[Avian 0.7](https://docs.rs/avian3d/0.7.0/avian3d/) already provides sleeping and
parallel physics features; tuning should follow active-body and contact costs,
not delete debris to manufacture a smooth frame count.

Joints, hinges, fire, fluids, additional materials and a production-scale world
are outside this drop.
