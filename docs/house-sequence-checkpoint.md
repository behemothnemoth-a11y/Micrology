# House sequence - integrated continuation

Base: `ecbf421f302afdf536cdd7b155e3413d95cc44f3` (Step 7).
Branch: `chatgpt/house-sequence-20261003`.
The user authorized the whole next sequence in this conversation: house workload,
live house interaction, controlled material failures, and evidence-driven debris
work. Separate verified commits remain useful; the former stop-after-each-step
instruction is superseded for this sequence. No automatic merge or paid compute.

## Section A - static support paths (implemented)

The old full occupancy classifier, full crack classifier, and DROP 0004 support
prepass remain unchanged. New `fracture_witness` is an opt-in fracture-aware
query. A supported root can finish when an exact resident, occupied, carrying
path reaches an authored anchor or a path proved earlier in this same call.
All discovered cells on that search tree are connected to the anchor. No
persistent support certificate or cache survives an edit or worker execution.

Every DETACHED set must still close completely. Unknown neighbor data is checked
before occupancy or old crack state can treat it as empty. An exhausted budget,
unknown loose boundary, or inconclusive occupancy cause check refuses the whole
transaction. A supported witness is not represented as a partial Component.
The result contains only complete detached Components; the ordinary atomic
admission/detach and state ownership paths still create native Fragments.

`FractureTransactionLimits::support_witnesses` defaults to false. The legacy
fragments, fracture coefficients, exact classifiers and old benchmark output
are unchanged. JobSummary now reports topology cells independently from the
existing fracture-propagation work field. Static direct results document the
witness-mode detached-only classification explicitly.

### Same window hit, unchanged output

Five samples per mode on the Windows MSI, optimized development build. Median
wall times are observations, not acceptance thresholds or GPU/FPS claims.

| Metric | Full legacy | Support paths |
|---|---:|---:|
| Crack topology cells | 167,192 | 3,801 |
| Occupancy cross-check cells | 167,192 | 3,917 |
| Local fracture propagation work | 173 | 173 |
| Snapshot occupied volumes | 229 | 229 |
| Snapshot geometry footprint | 1,908,192 bytes | 1,908,192 bytes |
| Capture median | 0.6016 ms | 0.5884 ms |
| Analysis median | 351.8015 ms | 6.2193 ms |
| Commit median | 0.0520 ms | 0.0471 ms |

Both produced the same exact world, fracture records, native fragments and
identity sequence: 64 broken bonds, 37 glass cells in 14 fragments, zero erasure.
The new path completes with the ORIGINAL 65,536-cell structural traversal caps.
The house still needs an explicit finite snapshot allowance of 256 volumes;
geometry bytes remain capped at 4 MiB. This is a bounded house configuration,
NOT a smaller snapshot or a general solution for arbitrary world residency.
Raw benchmark observations: `diagnostics/house_sequence_20261003/workload.json`.

Five focused tests cover exact full-house equality, 80 generated assemblies with
both intact and cracked gates against the unchanged independent classifiers,
unknown/zero budgets, spent budgets hiding roots, and anchor removal across calls.
862 engine all-target tests passed at this section; engine Clippy and formatting
passed. The existing window-hit baseline remained byte-equivalent. The complete
final sequence verification and runtime evidence are recorded below when run.

## Sections B-D

Live runtime, controlled scenarios, and debris diagnostics are not accepted by
Section A's tests. They require their own implementation and evidence.
