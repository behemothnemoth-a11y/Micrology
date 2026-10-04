# Reference house sequence — authored joints and debris checkpoint

Date: 2026-10-04.
Branch: `chatgpt/house-sequence-20261003`.
Base: `60d49c9` (live four-material house physics/scenario checkpoint).
Status: implemented and locally verified; not merged.

## Scope

This checkpoint follows the live reference-house collapse evidence rather than
changing fragment size globally. It adds a **reference-house-only authored
construction-joint prototype**, an explicitly named catastrophic joint-release
scenario, conservative fragment grouping for the opt-in house path, and
material-resolved live debris diagnostics.

It does **not** claim automatic load-driven joint failure, real fastening
hardware, a new save schema, final physical calibration, thin glass, or a
universal construction system. Normal sandbox behavior and ordinary reference
house startup remain intact.

## Why this pass exists

The live support-band control produced one 43,564-cell body after contact
fracture. That was not a generic "fragments are too large" result: ordinary cell
adjacency made separately authored framing/panels behave as one welded solid.

A temporary partial-integrity experiment proved that weakened internal bonds are
read by the normal fracture path, but simply starting joints partially damaged
did not make the falling body separate. The external contact points did not
provide a structural bending/load model for remote internal seams.

The useful question was therefore narrower: **does representing authored
construction interfaces as fracture bonds remove the giant welded component
without globally shrinking every fragment?**

## Authored joint prototype

`reference_house_joints` assigns deterministic semantic part labels to the
existing diagnostic house. The labels are intentionally coarse panels/bays,
not claims about studs, nails, mortar, glue, or building-code construction.

A face becomes a prototype construction joint when adjacent occupied cells
belong to different authored part labels. The prototype includes same-material
wood/wood and masonry/masonry interfaces plus wood/masonry and wood/glass
interfaces. Concrete remains the monolithic foundation/reference support and is
not weakened by this prototype.

The representation reuses the existing `BondKey` / `FractureState` path.
There is no second topology system and no per-cell entity. The exact occupancy
and fracture-aware classifiers remain authoritative.

Ordinary house startup has **zero seeded fracture records**. The diagnostic
`support_band_released_joints` scenario explicitly maps its authored
interfaces to zero-integrity fracture bonds before applying the already-labelled
support-band cut. That is a catastrophic test control, not automatic physics.

## Conservative grouping

The opt-in reference-house fracture jobs use the existing
`FragmentPartitionPolicy::CONSERVATIVE` grouping policy during fragment
re-fracture. This does not decide what is detached and cannot invent a cut.
It only groups already-separated, face-adjacent crack components while keeping
their internal broken bonds.

No global default was changed.

## Live result

The same Windows/NVIDIA/Vulkan live host from the previous checkpoint was used:
RTX 5050 Laptop GPU, NVIDIA 581.34. Fixed steps waited for current cell-derived
collision before advancing.

### Welded control after 120 ticks

- 22 native fragments.
- 43,646 fragment cells.
- largest fragment: **43,564 cells**.
- contact jobs processed: 2.
- contact-broken bonds reported: 220.

### Explicit authored-joint release

Immediately after the cut, before physics steps:

- 72 native fragments.
- 46,302 fragment cells.
- largest fragment: **2,274 cells**.
- median fragment: 546 cells.
- no one-cell fragments.

After 25 fixed ticks:

- 180 fragments.
- 61,139 fragment cells.
- largest fragment unchanged at **2,274 cells**.
- contact fracture processed 142 jobs.

After 120 fixed ticks:

- **183 fragments**.
- **61,142 fragment cells**.
- largest fragment: **2,274 cells**.
- 160 contact jobs processed.
- 3,425 contact-broken bonds reported.
- 113 fragments created by contact re-fracture.
- 62 contact samples were conservatively capped and 11 stale results were
  rejected rather than applied.

The largest body fell from 43,564 to 2,274 cells: a **94.8% reduction** without
lowering a global fragment-size target.

## What the remaining small debris is made of

The live dump now records material counts for every native fragment.

At tick 120:

| material | pieces | cells | one-cell pieces |
|---|---:|---:|---:|
| wood | 79 | 41,000 | 5 |
| masonry | 43 | 19,030 | 18 |
| glass | 61 | 1,112 | 44 |
| concrete | 0 | 0 | 0 |

No final fragment mixes materials.

There are 67 one-cell pieces total; **62 are glass or masonry** and only 5 are
wood. That is materially different from a generic pulverization problem.
A global "merge all tiny fragments" or lower-resolution rule would erase useful
brittle behavior to solve a problem wood mostly does not have.

At this 50 mm/cell fixture annotation, even a one-cell glass object is still a
50 mm voxel and is not a realistic thin shard. Thin representation remains a
separate problem.

## Safety / invariants

- Normal house startup: no pre-damaged joints.
- All non-house callers retain their existing fracture and partition defaults.
- Authored joints reuse existing persistent fracture bonds; no parallel
  detachment mechanism was added.
- Concrete support remains authored separately from material identity.
- Unknown space and exhausted budgets still refuse action.
- Existing stale/admission/identity checks remain in force.
- The support-path witness remains an acceleration proof only; exact classifiers
  are still the fallback/oracle.
- The catastrophic joint release is explicitly named and cannot be confused
  with automatic structural failure.
- No old fixture was regenerated.

## Verification

Executed on the user's Windows MSI after the final source changes.

| Check | Result |
|---|---|
| New joint representation tests | 4 passed |
| House scenario tests | 5 passed |
| Engine all-target tests | **873 passed** |
| Engine documentation tests | 1 passed |
| Sandbox tests | **13 passed** |
| Engine Clippy, warnings denied | PASS |
| Sandbox Clippy, warnings denied | PASS |
| Formatting | PASS |
| Material specimen baseline | PASS |
| Step 7 window baseline | PASS |
| Destruction benchmark/replay | PASS |
| Fragment distribution baseline | PASS |
| Fracture locality benchmark | PASS |
| Existing fixtures vs `60d49c9` | unchanged |

`docs/diagnostics/house_joint_sequence_20261004/verification.json` records
commands, exit codes, source hashes and log hashes.
`summary.json` contains the compact welded/released live measurements and
material-resolved distribution. Selected normalized logs are retained beside it;
full raw logs remain under the isolated worktree's `target/`.

## What remains unresolved

The authored-joint **representation** is now proved useful, but the catastrophic
scenario deliberately releases those joints. A normal partially weakened joint
does not yet fail because a falling monolithic fragment has no internal
stress/strain model that turns gravity, bending, or torque into remote joint
load.

The next mechanics problem is therefore **automatic finite-strength joint
failure**, not another global debris-size pass. The existing mechanics stack
already has an exact per-face capacity seam, but using it for a house requires a
bounded construction-level workload and a policy for failing a connection
without deleting an arbitrary cell.

Do not promote the explicit joint-release scenario into ordinary gameplay.
Use it as the measured control for the next joint-strength implementation.
