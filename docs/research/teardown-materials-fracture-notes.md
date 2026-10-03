# Teardown-related research: materials, connections, and fragment scale

Research date: 2026-10-03.
Status: **focused public-source research only; no implementation or benchmark run**.

Context: [Step 1 material audit](../reference-materials-step1-audit.md). The user
wants small checkpoints, not a materials/house/physics rewrite in one pass.
The immediate engineering target remains a tiny material-pair bond fixture.

## Source boundaries

The sources below are public developer explanations, official modding APIs,
and explicitly licensed independent code. They do not provide a verified copy
of Teardown's private engine implementation or its internal material-pair law.
No game assets, game-install scripts, workshop downloads, binaries, or external
source files were imported into Micrology. No external code was executed.

Distinguish three different things: shipped Teardown documentation; Dennis
Gustafsson's other/next-engine work; unrelated destruction implementations.
An idea from Smash Hit or NVIDIA Blast is not evidence that Teardown uses it.

## 1. Physical scale: do not equate a chunky result with insufficient resolution

**Source fact:** Teardown's `GetShapeSize` API reports voxel scale in meters;
its documented default is 0.1 m. `MakeHole` specifies separate radii for soft,
medium, and hard material categories. These are API behavior controls, not
published constitutive fracture equations. [S1]

**Micrology implication (proposal):** measure physical dimensions, impacted
volume, surviving cells, and fragment membership separately. A higher voxel
resolution may help thin features, but it does not automatically produce smaller
connected fragments. Establish a scale reference before making resolution the
explanation for the large pieces in the current clips.

**Additional source fact:** the API exposes particle lifetime, radius, gravity,
and collision controls separately from voxel shapes/bodies. [S1]

**Boundary:** this does not establish which individual debris effects in a
Teardown video are particles. Cosmetic chips must not secretly replace
Micrology's authoritative cell mass or repair a failed destruction algorithm.

## 2. Mixed-material authoring has known failure cases

**Source facts:** Teardown's modding guide requires face-connected voxels and
warns that soft material trapped between harder pieces can cause physics trouble.
Its brick-wall example recommends brick material with a different grout color
instead of soft plaster between bricks. It also recommends splitting a house
into walls/floors rather than one large hollow object. [S2]

**Micrology implication (proposal):** later add a thin soft insert trapped between
hard sides as an adversarial fixture. Do not initially make every visible mortar
line a separate weak physical material. Distinguish color, material identity,
authored assemblies, and physical connections.

**Do not copy blindly:** the house-object advice reflects Teardown's storage
tradeoffs. It is not an instruction to replace Micrology's volume/region system
or make every construction brick an entity or a rigid body.

## 3. A useful explanation of finer breakage comes from Smash Hit

**Source facts:** in his 2014 Smash Hit write-up, Gustafsson describes isolating
an impact-local volume, subdividing that volume into smaller pieces, then
checking the remaining object's connectivity. He also discusses limiting contact
impulses during breakage so pieces retain motion, with bounded repeated solves.
This is convex-shape glass fracture in **Smash Hit**, not Teardown voxel source.
The article explicitly accepts a gameplay simplification: breakage near impact.
[S3]

**Micrology implication (proposal):** separate creating valid crack boundaries
from grouping material after those boundaries exist. A distribution layer cannot
make finer legitimate fragments merely by returning a different count. Any new
cuts must originate in an explicit fracture/damage policy and pass the existing
crack-aware classifier; partitioning stays non-authoritative.

Do not port its convex-mesh representation, all-pairs connectivity algorithm,
or solver loop into this checkpoint. Contact timing is a later investigation,
not permission to replace Avian or undo the current fixed-step work.

## 4. Teardown architecture is a reference, not a specification to reproduce

**Source facts:** Gustafsson's 2024 summary describes Teardown's dense per-shape
voxel grids, then distinguishes a **new engine** using sparse 8x8x8 voxel chunks.
The same post describes newer physics experiments and parallelism. These are
not all claims about the shipped Teardown implementation. [S4]

His 2020 material write-up says broken pieces inherit their original object's
palette, avoiding a palette increase from each break. [S5]

**Micrology implication (proposal):** retain compact material identity through
fragment creation and re-fracture. Review before adding duplicate per-fragment
profile tables. Keep our existing sparse ownership and cell-derived collision;
no storage, renderer, or physics-backend migration is justified by this research.

## 5. Actual code lead: NVIDIA Blast

**What was inspected:** official Blast introduction/stress documentation,
`blast/include/lowlevel/NvBlastTypes.h` lines 1-255, and the repository license.
The header defines an interface bond and a support graph with both directional
adjacency references pointing to shared bond indices. It carries interface
normal, area, and centroid. [S6, S7]

The stress extension supports separate tensile, compressive, and shear limits.
Blast's documented hierarchy and support graph distinguish chunk subdivision
from grouping connected chunks into actors. [S8, S9]

**Reuse assessment:** the inspected header and root license carry BSD-3-Clause
terms. It is a legitimate code-study/adaptation candidate with retained notices;
check the exact files and their dependencies before copying. No Blast source was
vendored or linked. [S7, S10]

**Immediate usefulness:** compare shared bond identity and endpoint traversal
with our own canonical bond keys. This supports a useful representation pattern,
not a prescribed wood/brick strength formula. There is no evidence here that
Teardown uses Blast.

**Do not do:** do not substitute pre-authored Blast chunks for native volumetric
Fragments, or treat a new graph as authority over exact occupancy. A C++ solver
with floating-point inputs is not a drop-in deterministic Rust implementation.

## 6. Actual code lead: independent Teardown-inspired voxel prototype

Repository: `TimonVenninckx/Teardown-voxel-path-tracer`.
Inspected commit: `0b8f52675d03787509b5031e5db17376e9973d65`.
File: `source/VoxelVolume.cpp`; especially `VoxelVolume::Fracture`, around
lines 379-608. The function places impact-oriented Voronoi seeds, removes boundary
voxels, and discovers remaining connected islands with six-neighbor flood fill.
It uses a process-wide random generator and unordered traversal containers. [S11]

**Reuse assessment:** the repository's LICENSE is MIT. Its README separately
says the voxel models came from elsewhere; do not treat those assets as cleared
for reuse. [S12, S13]

**Useful idea:** impact-local candidate fracture patterns.
**Not suitable unchanged:** boundary-voxel erasure spends material to create
seams, and randomness is not isolated by impact identity. Both conflict with
Micrology's intended evidence and locality contracts unless deliberately changed.
This is a small independent prototype, not Teardown's source or production proof.
No source code or assets were copied.

## Proposed next checkpoint — tiny two-material joint only

The following is our design/test proposal, not a recovered Teardown algorithm.

1. A material-pair property query must be symmetric in endpoint material order
   for the isotropic reference materials. Keep actual load mode/direction and
   geometric normals distinct from the order used for property lookup.
2. Same-material pairs must reduce to the accepted homogeneous response.
3. Both endpoints must refer to the same canonical bond history, not two
   independently accumulated records.
4. Querying pair properties must not mutate geometry, bond state, or identity.
5. Preserve unprofiled/disabled behavior deliberately; do not add accidental
   strength by default.

A weaker-side rule is still only a candidate approximation. The inspected
Teardown sources do not validate a numeric joint-strength formula for our four
materials. Endpoint material-change invalidation remains the following separate
checkpoint, followed by worker/profile integration. New fragment-scale policy
and house construction remain deferred.

## Source register

All URLs checked or file contents inspected on 2026-10-03. Web documents are
living sources unless their publication date is stated. Blob hashes identify the
inspected code content; no claim of exhaustive library audit is made.

- **S1 — Teardown official Lua API:** https://teardowngame.com/modding/api.html
  (`GetShapeSize`, `MakeHole`, particle controls, shapes/bodies).
- **S2 — Teardown official modding guide:** https://teardowngame.com/modding/
  (Voxel Connectivity; Mixing Hard and Soft Materials; Empty Space in Objects).
- **S3 — Dennis Gustafsson, Cracking destruction, 2014-05-13:**
  https://blog.voxagon.se/2014/05/13/cracking-destruction.html
- **S4 — Dennis Gustafsson, Year summary, 2024-12-29:**
  https://blog.voxagon.se/2024/12/29/year-summary.html
- **S5 — Dennis Gustafsson, The Spraycan, 2020-12-03:**
  https://blog.voxagon.se/2020/12/03/spraycan.html
- **S6 — NVIDIA Blast introduction, documentation 5.0.6:**
  https://docs.omniverse.nvidia.com/kit/docs/blast-sdk/latest/docs/api/introduction.html
- **S7 — NVIDIA Blast types header:**
  https://github.com/NVIDIA-Omniverse/PhysX/blob/main/blast/include/lowlevel/NvBlastTypes.h
  Inspected blob `8726f94fd99a32ca5c6abd0230e1b366ff81a5f1`.
- **S8 — NVIDIA Blast stress extension, documentation 5.0.6:**
  https://docs.omniverse.nvidia.com/kit/docs/blast-sdk/latest/docs/api/extensions/ext_stress.html
- **S9 — NVIDIA Blast ExtStressSolver class, documentation 5.0.6:**
  https://docs.omniverse.nvidia.com/kit/docs/blast-sdk/latest/_build/docs/blast-sdk/latest/class_nv_1_1_blast_1_1_ext_stress_solver.html
- **S10 — NVIDIA repository license:**
  https://github.com/NVIDIA-Omniverse/PhysX/blob/main/LICENSE.md
  Inspected blob `2388f6f9a26a42277e0dd0bf4b7b55af5aee29e5`.
- **S11 — Independent prototype source:**
  https://github.com/TimonVenninckx/Teardown-voxel-path-tracer/blob/0b8f52675d03787509b5031e5db17376e9973d65/source/VoxelVolume.cpp
  Inspected blob `2bbac2a394c5d8002adb233df7bb2e134c9b89e6`.
- **S12 — Prototype license:**
  https://github.com/TimonVenninckx/Teardown-voxel-path-tracer/blob/main/LICENSE
  Inspected blob `fcbeba2e048b0db391d892feff1c99d70aa67922`.
- **S13 — Prototype README / asset disclaimer:**
  https://github.com/TimonVenninckx/Teardown-voxel-path-tracer

## Finish state

Research note only. No engine edits, third-party dependency additions, material
tuning, test execution, house generation, paid compute, or video capture.
No benchmark improvements or implemented fixes are claimed. Stop here.
