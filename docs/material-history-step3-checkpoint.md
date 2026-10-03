# Material history - Step 3 checkpoint

Date: 2026-10-03.
Status: implemented; local engine acceptance passed; not merged.
Branch: `chatgpt/material-history-step3-20261003`.
Base: `94e5267202d1578b730533ced949a685ae15567b` (Step 2 material-pair response).
This checkpoint is material-replacement cleanup only, not four-material tuning,
worker policy integration, the house, or finer debris.

## Reproduction

A two-cell joint was saved in memory with zero bond integrity. Only the upper
endpoint was replaced with a different MaterialId. The legacy `prune_against`
path retained that broken bond: observed integrity 0, expected fresh integrity
1000. The failing test was run before the implementation and its output is in
`diagnostics/material_history_step3_20261003/reproduction_before.txt`.

The lower-material-only record cannot reconstruct what the upper material used
to be after an untracked edit. This pass deliberately uses exact edit
notifications instead of changing the record/save schema or scanning history.

## Implementation

`World::apply` now reports `EditOutcome::repainted_cells`: only actual occupied-
to-occupied MaterialId changes, not no-op writes. Adds/removals keep their
existing lists. Support, revisions, dirty tracking and occupancy analysis remain
on the normal world path. No destruction dependency was added to engine_world.

`FractureState::forget_edit(space, outcome)` invalidates records for the accepted
edit's added, removed, and repainted cells. For each changed cell it uses the
existing indexed-removal path for that cell and its at most six incident bonds,
including bonds canonically owned by a negative-face neighbor. It does not scan
world geometry or unrelated fracture records. The former `forget_cell` API still
uses that same removal path, with unchanged revision behavior.

`FractureState::apply_static_edit(world, batch)` packages the ordinary world edit
and cleanup into one synchronous call for hosts. It returns both the world
`EditOutcome` and `FractureEditCleanup` (removed-record count and affected static
sidecar owner regions). The sandbox's main SimulationLab fracture state is wired
through this boundary for manual carve/place/repaint actions.

The new cleanup report separates fracture persistence ownership from world
geometry dirtiness. When the upper endpoint is across a region seam, the removed
bond can belong to the unchanged lower region. A save host must retain these
owners in its pending dirty set and flush their current `region_fracture`, even
when it is empty, retrying if I/O fails. Saving only currently nonempty fracture
regions can leave an obsolete sidecar on disk. No global save sweep was added.

## What remains unchanged

- Step 2 symmetric material-pair response and the homogeneous fracture policy.
- Bond/cell record layout, the save schema, and accepted golden files.
- Cell mass, fracture coefficients, fragment grouping and physical scale.
- Optional mechanics/destruction participation and native Fragment ownership.
- Neighboring cells' own accumulated energy and unrelated bonds/materials.

Replacing a material defines a new cell/interface. It does not restore the old
material's cracks if that MaterialId is later placed back. Writing the same ID
without a change preserves its exact old damage and revision.

## Verification

Executed on the user's Windows MSI, without launching a visual demo.
Machine-readable commands, exit codes, elapsed observations, source hashes and
log hashes are in `diagnostics/material_history_step3_20261003/verification.json`.
The source hashes identify the tested working tree and were unchanged during
verification. The 14 new tests below are INCLUDED in the 811 total.

| Check | Local result |
|---|---|
| Focused material edit cleanup | 12 passed |
| Focused fracture sidecar save/reload | 2 passed |
| Engine default members, all targets | 811 passed |
| Engine doc tests | 1 passed |
| Workspace formatting | PASS |
| Engine workspace Clippy, warnings denied | PASS |
| Destruction benchmark/replay golden checks | PASS |
| Fragment-distribution golden check | PASS |
| Fracture-locality benchmark | PASS |
| Whitespace / pre-existing goldens unchanged | PASS |

The cleanup tests cover both endpoints, partial/broken bonds, all axes,
negative coordinates and region seams, no-op/overlapping writes, repeated
notifications before new damage, normal support/dirty tracking, all six faces,
static versus exact FragmentId spaces, repeated region extraction/restoration,
100,000 unrelated cell records preserved, a fresh subsequent impact, and stale
worker-commit refusal after repaint.

The IO tests use disposable directories only. They verify that flushing the
reported lower-region owner deletes the old empty sidecar, that save/reload does
not resurrect the crack, and that a distant sidecar stays byte-identical. A
same-material no-op produces no cleanup owners and preserves the saved damage.

Sandbox Clippy: PASS (all targets, warnings denied); see the separate sandbox_clippy.json evidence.
Sandbox tests, visual interaction acceptance and GPU profiling are not part of
the local engine results above; consult the draft PR's independent CI run.

## Caller contract and limits

This is an explicit edit boundary, not a global interception of all mutations.
Raw `World::set` or callers that discard edit notifications still require local
invalidation. The legacy `prune_against` limitation is now documented; it was NOT
upgraded to infer an unrecorded prior upper material. Call `forget_edit`
immediately after an accepted mutation, before new analysis/save; do not replay
old edit notifications after subsequent damage.

Detached-fragment transfers must retain their existing adopt/remap ordering;
this authored-edit helper must not erase damage before ownership transfer.
For a fragment-local edit outcome, pass the exact FragmentId space. No new
fragment repaint UI was added. Bare editor commands expose the new repaint list;
a future editor host must consume it with its optional fracture state.

The main sandbox currently has no static-fracture sidecar flush wired to manual
edits; this pass supplies and tests the per-region persistence contract rather
than claiming a new whole-world save system. Automated demos with private state
are not converted to new authoring systems here.

## Next small checkpoint

Carry an explicitly owned material-aware fracture policy through the existing
worker path, preserving homogeneous defaults, stale checks and byte/work budgets.
Only then use small material specimens to choose four reference responses.
No house, finer fragments, new solver, new video or automatic merge was started.
