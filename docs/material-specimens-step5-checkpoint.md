# Reference material specimens - Step 5 checkpoint

Date: 2026-10-03.
Base: `dfdc11c740b3926bfa1ed960611d2d39836919c5` (owned worker Step 4).
Branch: `chatgpt/material-specimens-step5-20261003`.
Status: implemented; local engine acceptance and sandbox Clippy passed; not merged.

## Scope

An opt-in first set of wood, masonry, concrete and glass IMPACT fracture presets,
plus equal-geometry specimens through the real owned worker. This is measured
engine behavior from authored tuning, NOT physical-material calibration. No
house, new fracture topology, smaller-debris tuning, solver change, density/mass
integration, save schema, visual capture or automatic merge is part of the pass.

Source context: [Step 1 audit](reference-materials-step1-audit.md),
[Teardown-related research](research/teardown-materials-fracture-notes.md), and
[Step 4 worker checkpoint](material-worker-step4-checkpoint.md).
No outside code or numerical material data was imported in this checkpoint.

## Implementation

`engine_mechanics::reference_fracture` supplies explicit optional preset data.
It reuses ReferenceMaterial names, adds no dependency, and does not modify the
existing MechanicalProfile settings. Steel intentionally has no new fracture
preset here. A host still chooses MaterialIds and constructs OwnedFracturePolicy.
No default sandbox/demo policy is switched on by importing these constants.

The preset revision is 1. Values below are the existing fracture model's abstract
energy/capacity units; toughness is the 0..1000 bond-loading control, not the
mechanical accumulated-damage threshold. Density is unused metadata here, NOT
a new runtime body-mass calculation or a real-unit scale conversion.

| Preset | Toughness | Crush | Tension | Compression | Shear |
|---|---:|---:|---:|---:|---:|
| Wood | 750 | 24,000 | 700 | 850 | 360 |
| Masonry | 250 | 8,000 | 90 | 1,100 | 260 |
| Concrete | 500 | 16,000 | 240 | 1,800 | 550 |
| Glass | 50 | 4,500 | 100 | 600 | 140 |

These express a more bond-tolerant wood reference, a tension-weak masonry
reference, a more resistant concrete reference than masonry, and an impact-fragile
glass reference. They do not establish a universal strength ranking. Glass's
impact parameters are deliberately NOT copied from its existing idealized
static-load capacities. Wood does not bend or have grain under this scalar model.

## Specimen and controls

Every material gets the same 12 x 8 x 2 cell slab (192 cells), front-side radial
impact at (9.5, 8.5, 3.5), radius 4 cells, Coherent fallback and unmodified
transaction/grouping budgets. Static slabs have the same explicitly authored
24-cell anchored bottom row. Native slabs start as exact native Fragments with
no support; no physics time step is simulated in either route.

For THIS diagnostic, the display annotation is 100 mm per cell: the slab would
be 1,200 x 800 x 200 mm. This does not choose the engine's global cell size, or
convert force/mass/energy units. In particular a 200 mm glass comparison slab
is NOT a proposed house window. Real pane/stud resolution remains undecided.

The committed matrix contains 64 deterministic rows:
- Six fresh impact energies (100, 600, 2,400, 4,800, 12,000, 24,000), four
  materials, two routes: 48 independent tests of fresh specimens.
- Four sequential 600-energy static hits for each material: 16 observations.
  These keep earlier damage and detached fragments; the next hit still targets
  static geometry, not automatically the detached pieces.

A separate uniform-policy control proves that changing only MaterialId yields
identical numerical responses. Four-preset response curves are compared without
material labels or ID-dependent checksums so identity changes cannot fake a
material-behavior distinction. Native and static curves are NOT expected to be
equal: support differs and Coherent already suppresses the native surface gain.

## Measurements and first observations

First SAMPLED fresh static impact producing native debris with no cell erasure:

| Material | Impact energy | Newly broken bonds | Detached cells | Native pieces | Largest piece, cells |
|---|---:|---:|---:|---:|---:|
| Glass | 2,400 | 138 | 62 | 23 | 9 |
| Masonry | 4,800 | 88 | 30 | 6 | 9 |
| Wood | 12,000 | 64 | 9 | 1 | 9 |
| Concrete | 12,000 | 64 | 9 | 1 | 9 |

These are discrete test points, not exact failure thresholds or physical units.
Wood and concrete overlap at this point; their broader response curves differ.
At the fresh 24,000 static hit, erased cells are respectively wood 1, masonry 46,
concrete 10 and glass 62. Every result closes initial cells = remaining static +
native-fragment cells + cumulative explicitly erased cells. Erased material is
reported, not silently counted as fragments or fabricated dust.

Repeated glass hits show a useful progression: first two 600-energy hits add
damage without broken bonds; hit three breaks 13 bonds but detaches nothing;
hit four breaks another 125 bonds and releases 62 cells into 23 native pieces.
No cells are erased in that four-hit sequence.

Every row records loaded cell/bond counts, deposited energy, peak deposit,
bond load/loss, visited/considered work, fracture-snapshot records/bytes,
new broken bonds, cumulative erasure, native object/creation counts, exact
size lists, median, histogram, shape extents/aspects, physical-size annotations,
material counts and structural/fracture checksums. Loaded records are not the
same thing as work inspected. The extra read-only evaluation used to expose
loading diagnostics is not claimed as worker performance or production overhead.
An unchanged parent counts as one native object, not as a newly created fragment.

## One local preset adjustment, before the FIRST baseline

The first trial used wood crush 10,000. At energy 12,000 it broke 43 bonds and
erased 9 cells but produced no detached piece. Raising ONLY the new wood preset's
crush threshold to 24,000 gave a sampled crack/separate interval: 64 broken bonds,
a 9-cell native fragment, zero erasure. The mechanical wood preset, accepted
homogeneous model, fracture rules and partition settings were not touched.

Trial logs are retained with the verification evidence. No previously accepted
baseline was regenerated. The new CLI's `--write-new` uses create_new and refuses
to overwrite an existing baseline; later revisions require an explicit review.

## What the evidence does NOT solve

This does not produce material-specific shard/splinter shapes. Strong-hit
results contain many one-cell pieces, while the native 4,800 glass hit retains
a 130-cell main remnant and generates smaller pieces around the impact. A large
unstruck remainder is not automatically a fragmentation defect. Size lists and
extents allow us to judge which piece is actually too large at a stated scale.
At this diagnostic scale, even one cell is a 100 mm cube, NOT dust or a fine shard.

No bend/grain model, thin-pane representation, brick/mortar authoring, nails/glue,
material-pair override table, elastic stiffness or realistic mass was added.
The existing symmetric weaker-side pair rule remains an approximation. Mechanical
capacity/collapse calibration for a house is not established by an impact coupon.
New presets are not yet activated in the sandbox UI or old synchronous demos.

## Run and verification

```sh
cargo run --locked -q -p engine_stress --bin material_specimen_bench
cargo run --locked -q -p engine_stress --bin material_specimen_bench -- --json
cargo run --locked -q -p engine_stress --bin material_specimen_bench -- --check
cargo test --locked -p engine_stress --test reference_material_specimens
```

Baseline: `fixtures/destruction/reference-material-specimens-v1.json`.
The ordinary engine test suite checks it, so no new Actions workflow is required.
The 11 new focused tests include pair symmetry, explicit opt-in IDs, matched
geometry/support, a uniform-material control, four distinct numerical curves,
cell accounting/histograms, staged glass damage, a static separation-before-
erasure sample for each material, repeatability and exact baseline comparison.

Executed on the user's Windows MSI; the 11 new tests are INCLUDED in 835.

| Check | Local result |
|---|---|
| New reference-specimen tests | 11 passed |
| Engine default members, all targets | 835 passed |
| Engine doc tests | 1 passed |
| New 64-row specimen baseline check | PASS |
| Formatting / whitespace | PASS |
| Engine and sandbox Clippy, all targets, warnings denied | PASS |
| Existing destruction benchmark and replay | PASS |
| Existing fragment-distribution and fracture-locality checks | PASS |
| Pre-existing goldens unchanged | PASS |
| CLI refusal to overwrite the new baseline | PASS, baseline hash unchanged |

The old locality fixture remains 128 records / 6,144 snapshot bytes at all four
rungs through 1,000,000 unrelated records. This preserves existing behavior; it
is not a new material-scale performance claim. Checks, exit codes, normalized
source/baseline hashes and raw-log hashes are in
`docs/diagnostics/material_specimens_step5_20261003/verification.json`.
Full raw logs remain under `target/material-specimens-step5-verification`.
Source hashes were rechecked before this closeout; code did not change during
verification. The previous baseline SHA plus hashes identify the exact tested
source, including files that were untracked when tests ran.

Local sandbox runtime tests were not executed; remote CI is separate and must
be checked before being called successful.
Sandbox interactive GPU acceptance and new video capture are not local gates here.

## Next small boundary

Review this diagnostic response/scale baseline, then build the static house
fixture for inspection with explicit dimensions and material placement. Do not
assume this slab's 100 mm display scale can resolve a thin pane or every timber
member. House destruction, material-shaped debris and visual acceptance remain
separate reviewed checkpoints.
