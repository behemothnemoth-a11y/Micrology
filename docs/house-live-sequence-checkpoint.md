# Reference house sequence — live physics and mixed-material checkpoint

Date: 2026-10-04.
Branch: `chatgpt/house-sequence-20261003`.
Base: house impact Step 7 (`ecbf421`), with workload witness checkpoint `c5deb77`.
Status: local Windows engine/sandbox acceptance passed; not merged.

## Scope

This checkpoint carries the four-material reference house from headless fracture
into the existing Windows/Vulkan sandbox and exercises a bounded scenario matrix.
It does **not** claim a finished physical calibration, thin glass, realistic
construction joints, structural engineering, or final debris behavior.

Normal sandbox behavior remains the default. The house path is opt-in through
`MICROLOGY_REFERENCE_HOUSE`.

## House-sized workload

The Step 7 local window hit proved that copying the 229-volume fixture was within
the existing 4 MiB geometry-byte ceiling but that the old structural classifier
could spend its 65,536-cell budget traversing an otherwise-supported building.

The opt-in static support witness keeps the exact occupancy and fracture-aware
classifiers as the oracle/fallback. It may cheaply prove that a root reaches
authored support; it may not declare a partial component detached. Unknown space
and spent work still refuse action.

For the same window result:

| path | crack topology cells | occupancy cross-check cells |
|---|---:|---:|
| old full-component search | 167,192 | 167,192 |
| support-path witness | 3,801 | 3,917 |

The detached cells, identities, fracture records, and resulting fragments are
unchanged. The witness is explicitly enabled for the house worker; the default
transaction remains off.

## Opt-in physical convention

The reference house has a 50 mm/cell **fixture annotation**. The sandbox therefore
uses the following only while `MICROLOGY_REFERENCE_HOUSE` is present:

- Avian internal length scale: 20 cell-units per metre.
- User-space gravity: 196.2 cell-units/s² = 9.81 m/s² at that scale.
- Fixed collision focus: centre (64,40,48), radius 160 cells, independent of the
  overview camera.
- Fragment collider density: average occupied material density converted into
  kg per cell-unit³ with 0.05³. Reference densities are wood 600, masonry 2000,
  concrete 2400, glass 2500 kg/m³.

This gives useful total mass and Earth-like gravity for the diagnostic. It is
**not** a complete heterogeneous inertia model or a conversion of abstract
fracture energy/capacity values into SI units.

## Live window replay

A canonical `ReferenceHouse` replay uses the same Step 7 window pulse and the
Step 5 four-material worker policy. It ran on the user's Windows MSI with:

- NVIDIA GeForce RTX 5050 Laptop GPU
- NVIDIA driver 581.34
- Vulkan backend

The first live transaction produced the same ownership result as the headless
fixture: 14 native fragments containing 37 glass cells, largest 8 cells, with
167,155 cells remaining static. The background worker accepted one result with
zero refusal/stale counts.

The replay then advanced 60 authoritative fixed steps only after current static
and fragment collision had published. Native fragment poses and velocities
changed, proving that the pieces entered Avian and were read back into Micrology.
Most fragments remained clustered close to the pane; the largest same-id
displacement after 60 ticks was below one cell. This is evidence of dense
newly-separated pieces contacting/jamming, not evidence that physics was skipped.

## Controlled fresh-house matrix

All cases below start from the same intact fixture and use the real worker.
`support_band` is an explicitly labelled occupancy cut, not a claim that one
ordinary member failure naturally produces this collapse.

| case | broken bonds | erased cells | native pieces | fragment cells | largest piece |
|---|---:|---:|---:|---:|---:|
| window | 64 | 0 | 14 | 37 | 8 |
| frame joint | 41 | 1 | 14 | 14 | 1 |
| masonry wall breach | 415 | 56 | 77 | 139 | 5 |
| chimney | 227 | 42 | 65 | 97 | 6 |
| roof hit | 97 | 0 | 0 | 0 | 0 |
| support band | 0 | 3,256 | 11 | 43,636 | 43,576 |

The frame-joint case loads a real wood/glass material pair. Wall/chimney results
touch masonry as intended. The roof hit records wood cracks without inventing a
detachment.

## Live support-collapse result

The support-band replay enabled contact fracture and advanced 120 fixed steps.

Immediately after the cut:
- 11 fragments / 43,636 cells.
- largest fragment: **43,576 cells**.

After 25 fixed ticks:
- contact fracture had processed two jobs;
- 220 contact-broken bonds were reported by the contact host;
- 12 additional fragments were created;
- total native fragments became 22;
- largest fragment was still **43,564 cells**.

After 120 ticks the count and largest piece were unchanged. Smaller debris fell
toward the foundation/floor, while the giant component settled as one body.

This is the current highest-value finding: **contact fracture can chip/refracture
the falling house assembly, but ordinary cell adjacency still makes authored
construction behave like a welded continuous solid.** Raising impact energy or
globally shrinking fragment targets would attack the symptom rather than the
missing connection model.

The next evidence-driven change should therefore prototype bounded authored
connection/joint information while preserving material profiles, exact occupancy,
and the existing fracture policy defaults. Same-material wood members need a way
to meet without becoming indistinguishable from one continuous timber block.

## Replay/scenario integration

The existing replay host now understands an opt-in `ReferenceHouse` fixture and
bounded `HouseScenario` commands. It does not introduce a second simulation
path. House impacts/removals still enter the same background fracture worker,
fragment storage, collision build, fixed-step readiness barrier, contact fracture,
and engine-owned physics readback.

A canonical window replay is committed as
`fixtures/destruction/replay-reference-house-window.json`.

The capture script also accepts `-Demo house`; its normal modes are unchanged.
Preflight passed with Rust, FFmpeg 8.1.1, ffprobe, and Vulkan-safe `ddagrab`.

## Verification

Local Windows verification after the live/scenario integration:

- 4 controlled house-scenario tests passed.
- 5 house support-witness tests passed.
- **868 engine all-target tests passed**, including the new tests.
- 1 engine documentation test passed.
- **13 sandbox tests passed**.
- Engine Clippy and sandbox Clippy passed with warnings denied.
- Formatting and whitespace checks passed.
- Existing material specimen and Step 7 window baselines passed unchanged.
- Existing destruction benchmark/replay, fragment distribution and fracture
  locality checks passed unchanged.
- All pre-existing fixtures stayed byte-unchanged; the only new fixture is the
  canonical reference-house replay.

Compact measured evidence is in
`docs/diagnostics/house_live_sequence_20261004/summary.json`. Raw live dumps and
logs remain in the isolated worktree's `target/` directory and are not treated
as canonical fixtures.

## Next boundary

Prototype authored construction bonds/joints **without changing default
FracturePolicy behavior**. First prove the representation on the reference house
and the giant support-collapse component. Do not yet invent a global joint
strength law, save schema, or final debris system. If that makes the house break
at member/panel boundaries, then rerun the live collapse and use the resulting
piece distribution to decide whether material-specific fine debris is still
needed.
