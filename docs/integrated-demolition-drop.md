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
poll opportunities and coalesced capacity assessment used in the final build. It
is a recorded observation from the authoring machine: the profile JSON for it was
never committed, so it cannot be re-derived from this repository.

### Where these numbers come from

Everything below was measured in a Claude Code cloud Linux container: CPU only,
no GPU at all (`/dev/dri` absent), rendering through Mesa llvmpipe software
Vulkan under Xvfb. **None of it is a performance result for the authoring
Windows/RTX 5050 machine.** The engine CPU costs are meaningful, because they
never touch the renderer. The frame cadence, queue wait and end-to-end response
numbers are dominated by the software rasteriser and are reported only to say
what was actually observed. Raw artefacts are
`docs/diagnostics/demolition-engine-cpu.json` and
`docs/diagnostics/demolition-replay-evidence.json`.

### Engine cost without a renderer

`cargo run --release -p engine_stress --bin demolition_bench`, the roof blast at
radius 6 and 16,000 energy against the coherent model, 31 samples after 3
warmups. The same final structural digest on all 34 iterations, so the
transaction is deterministic; mass closes at 1,657 static + 100 fragment + 54
failed = 1,811.

| stage | median | p95 | p99 | max |
|---|---|---|---|---|
| snapshot capture | 0.014 ms | 0.026 ms | 0.033 ms | 0.033 ms |
| worker analysis | 3.164 ms | 7.252 ms | 7.362 ms | 7.362 ms |
| main-thread commit | 0.106 ms | 0.125 ms | 0.134 ms | 0.134 ms |

The split the design exists for is the measured part: analysis costs about
thirty times what the authoritative commit costs, and only the commit is on the
frame thread.

### Demolition building replay

`tools/verify-demolition-replay.py` passes against ten real host dumps, one per
label. Every row conserves mass exactly, and the authoritative queue never went
stale, refused or capped.

| dump | static | fragment cells | fragments | failed | total | capacity | capacity failed | contact bonds |
|---|---|---|---|---|---|---|---|---|
| intact | 1811 | 0 | 0 | 0 | 1811 | on, Sound | 0 | 0 |
| one_support | 1807 | 0 | 0 | 4 | 1811 | on, Sound | 0 | 0 |
| overloaded_off | 1803 | 0 | 0 | 8 | 1811 | off | 0 | 0 |
| roof_released | 1113 | 682 | 1 | 16 | 1811 | on | 8 | 0 |
| roof_impact | 1113 | 682 | 1 | 16 | 1811 | on | 8 | 0 |
| lifted | 1113 | 682 | 1 | 16 | 1811 | on | 8 | 0 |
| pulled | 1113 | 682 | 1 | 16 | 1811 | on | 8 | 0 |
| slammed | 1113 | 682 | 1 | 16 | 1811 | on | 8 | 3 |
| settled | 1107 | 681 | 4 | 23 | 1811 | on | 8 | 3 |
| systems_off | 1107 | 681 | 4 | 23 | 1811 | off | 8 | 3 |

One cut leaves the fixture sound. A second cut with capacity disabled leaves
1,803 cells in the authored static world and **zero** fragments: the mechanical
policy, not the cut, is what releases anything. Enabling capacity fails eight
further support cells in one generation and releases exactly one native 682-cell
fragment, which then keeps all 682 cells through the fall, the grab and the
180-step pull. The throw adds real collision cracks (three broken bonds, up from
none). The blast leaves four pieces holding 681 of the 682 cells — seven cells
failed in total across the whole sequence, which is the chunk-preserving
behaviour this fixture exists to show rather than debris confetti. With both
damage systems disabled the final interval holds structural state identical to
`settled` and the fracture checksum byte-identical at `f4546c6026930b5a`,
revision 9, 89 broken bonds.

### Demolition stress replay

The same verifier's stress branch passes. Mass closes against an independently
computed expectation that accounts for every launched chunk: 1,811 + 9x60 =
2,351, then 1,811 + 18x60 = 2,891.

| dump | static | fragment cells | fragments | failed | total | expected |
|---|---|---|---|---|---|---|
| stress_9 | 1134 | 1101 | 305 | 116 | 2351 | 2351 |
| stress_18 | 1024 | 1622 | 515 | 245 | 2891 | 2891 |
| stress_off | 1024 | 1622 | 515 | 245 | 2891 | 2891 |

At `stress_18`: 515 live fragments tracked as bodies, 2,176 contact-broken
bonds, 81 fragment-fragment pairs, fracture state of 3,385 bond and 2,487 cell
entries at revision 294, structural checksum `da31e2d5ba1b4e54` and fracture
checksum `eb7dcb8394550adf`. Turning both damage systems off reproduces both
checksums exactly while bodies keep moving.

Queue behaviour under that load is the part worth reading carefully. The
authoritative fracture FIFO reached its bound — high-water 32 of 32 — and
**capped zero requests**: it filled to the boundary without ever rejecting
authoritative work, and mass conservation above is the proof none was lost or
applied twice. Twenty-eight worker results were rejected as stale, which is the
revision check doing its job in a run where 515 fragments are being edited
concurrently, not a fault. The *opportunistic* queues did shed work, as they are
allowed to: contact sampling capped 53 of 276 enqueued and interaction capped 26.
Shedding a contact sample or a kick changes no geometry; shedding an
authoritative fracture job would, and that count is zero.

### Host latency, and what it does not establish

From the stress replay, 333 samples:

| stage | median | p95 | p99 | max |
|---|---|---|---|---|
| capture | 0.014 ms | 0.664 ms | 0.763 ms | 1.764 ms |
| worker | 0.165 ms | 2.659 ms | 7.249 ms | 19.138 ms |
| commit | 0.060 ms | 0.161 ms | 0.644 ms | 1.581 ms |
| queue wait | 49.985 ms | 281.763 ms | 733.716 ms | 907.964 ms |
| response | 67.758 ms | 323.524 ms | 791.254 ms | 967.806 ms |

The engine's own costs stay small and bounded: worker p99 7.249 ms off-frame,
commit p99 0.644 ms on the frame thread. Response latency is almost entirely
queue wait, and queue wait here is a function of how fast frames retire, because
the queue drains at most three commits per frame. On a software rasteriser frames
are very slow, so these two rows mostly measure llvmpipe.

That has a consequence worth stating plainly: **this run neither confirms nor
refutes the bounded multiple-poll improvement.** The 544-fragment observation
that motivated it (response p95 200.93 ms, p99 284.18 ms, maximum 319.63 ms,
against frame p99 20.31 ms) was taken on the authoring machine with real GPU
frames, and nothing here is comparable to it. The earlier single-poll profile is
described in this document from that session's notes; the JSON for it was never
committed to the repository, so it is a recorded observation rather than a
reproducible artefact.

Still pending, by environment rather than by omission: the Windows frame profile
(p50/p95/p99/max and the count of frames over 33.3 ms), the GPU-frame response
latency that would actually test the poll change, and all visual evidence. This
container has no PowerShell, so `tools/capture-demo.ps1 -CheckOnly` cannot run;
its ffmpeg has neither `ddagrab` nor `gdigrab`; and there is no GPU to capture.
Those must be produced on the Windows machine with the existing non-invasive
capture path.

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
