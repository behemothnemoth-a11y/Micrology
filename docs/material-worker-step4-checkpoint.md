# Material worker - Step 4 checkpoint

Date: 2026-10-03.
Status: implemented; local engine acceptance and sandbox Clippy passed; not merged.
Branch: `chatgpt/material-worker-step4-20261003`.
Base: `623800fc03444c3ce0bd694c54d90236b84f66b3` (material-history Step 3).

Scope: carry explicit, immutable material profiles through the owned fracture
worker. No four-material calibration, house, granularity tuning, new video,
physics replacement, save-format change, or automatic merge.

## Implementation

`fracture_policy::OwnedFracturePolicy` owns a canonical, sorted material/profile
table and the existing Historical or Coherent fallback model. It stores fracture
profiles as supplied; it does not reinterpret mechanical toughness as fracture
toughness. Missing material IDs retain the selected model's existing fallback.
The same Step 2 symmetric bond composition uses both endpoint profiles. Authored
nails/mortar/glue, anisotropy and pair-override tables are not implemented here.

Construction refuses duplicate material IDs and checks two explicit ceilings
before allocating the table: profile count (sorting/equality work) and payload
bytes. Default limits are 64 profiles and 16 KiB. Immutable Arc-backed copies
share the table rather than copying a registry for each impact. These are policy
construction budgets, separate from existing world-snapshot, fracture-record
and transaction-work budgets. Reported payload is not total allocator usage.

`FractureJobInput::run_with_policy` moves this policy into worker execution and
uses it for both static impacts and native-fragment re-fracture. Legacy
`run(impact, FractureModel)` remains available and uses the same transaction
implementation without changing its accepted models.

The returned `PolicyFractureJobResult` privately contains the legacy result and
its policy. Its commit requires the current policy and exact content equality
before existing geometry/history/residency/identity/admission checks. It exposes
no conversion to a legacy result that would accidentally bypass that guard.
Reordered tables with identical contents are valid; changed profiles or fallback
models refuse as stale before mutation or admission. No hash-based identity is
used for policy acceptance.

The sandbox background worker now has an optional `impact_policy`. None keeps
the accepted Coherent default. Queue entries take the current policy when they
launch; active impacts validate the current policy when they return. Cuts and
capacity-removal work retain the original geometry-job route and participation
gates. This is an opt-in background-worker connection, not a claim that every
synchronous demo now uses a new material table.

## Direct-versus-worker mismatch investigated

The resumed test compared a raw direct fragment transaction with an admitted
worker result. Those boundaries execute different bookkeeping operations:

- Existing worker admission reapplies current parent motion onto each child.
  `FragmentPhysicsState::apply_to` calls `Fragment::update_from_physics`, which
  advances the persistent fragment revision even when motion values are equal.
  It does not advance the geometry-only revision.
- Direct fracture and remapping can advance fracture-state revision twice,
  whereas admitting the worker's sparse patch advances it once. FractureState's
  existing PartialEq deliberately compares records, not transaction counts.

The four-material synthetic test produced the same five fragments, exact cell
and bond records, material IDs, geometry and identity sequence. Their persistent
fragment revisions were 0 in the direct path and 1 after worker admission.
No production revision semantics, fracture coefficients or golden results were
changed to make the test pass.

The revised test applies the documented motion bridge to an expected CLONE of
each direct fragment, then checks full Fragment equality, including both
revision fields and all storage/motion/provenance fields. Explicit cell/bond
record comparisons also remain. A separate control runs both older homogeneous
worker models: legacy and owned workers must be fully equal, while direct
transactions have the same documented motion-bridge difference. Refusal tests
still compare authority and revisions before/after without normalization.

## Focused coverage

The 13 engine integration tests use four intentionally synthetic profiles, NOT
calibrated wood/masonry/concrete/glass settings. They cover immutable ownership,
Send/Sync and real thread execution, canonical input order, duplicate rejection,
exact count/byte boundaries, fallback behavior, static direct/worker equivalence,
both legacy-model defaults for static and fragment jobs, changed-policy refusal,
equal-content replacement acceptance, existing atomic stale/admission guards,
unknown/budget refusal, exact native-fragment material results, continued parent
motion and unrelated-fragment isolation, and eight repeatable independent runs.

On this Windows x64 build, policy data payload is 184 bytes for four profiles,
versus 24 bytes for a homogeneous policy (160 additional table bytes). These
figures include the table descriptor and data, but exclude Arc reference counters
and allocator overhead. Cloning a policy shares the existing table. These are
not serialized byte counts or a claim of universal ABI sizes.

## Verification

Executed on the Windows MSI. All 13 new focused tests are INCLUDED in the 824
engine total, not additional to it.

| Check | Local result |
|---|---|
| Focused material-policy worker tests | 13 passed |
| Engine default members, all targets | 824 passed |
| Engine doc tests | 1 passed |
| Workspace formatting and whitespace | PASS |
| Engine Clippy, all targets, warnings denied | PASS |
| Sandbox Clippy, all targets, warnings denied | PASS |
| Destruction benchmark and replay | PASS |
| Fragment-distribution baseline | PASS |
| Fracture-locality benchmark | PASS |
| Pre-existing goldens unchanged | PASS |

The existing homogeneous locality fixture still captures 128 records / 6144
fracture payload bytes at 0, 10,000, 100,000 and 1,000,000 unrelated records,
with the same structural/fracture result at every rung. This is baseline
preservation, not a claim of a new multi-material scale benchmark or speedup.

Commands, exit codes, source hashes and log hashes are in
[verification.json](diagnostics/material_worker_step4_20261003/verification.json).
[policy_tests.txt](diagnostics/material_worker_step4_20261003/policy_tests.txt)
contains the compact comparison diagnostics and payload measurement;
[fracture_locality.json](diagnostics/material_worker_step4_20261003/fracture_locality.json)
and [sandbox_clippy.txt](diagnostics/material_worker_step4_20261003/sandbox_clippy.txt)
retain the corresponding evidence. The verifier confirmed that tested source
contents did not change during the run or before evidence preparation. The
pre-commit base SHA plus LF-normalized hashes identify the exact tested code.
Full original logs remain in the isolated worktree's
`target/material-worker-step4-verification` directory.

No golden files were regenerated. Sandbox runtime tests, new visual replay and
Windows/GPU performance acceptance were NOT run locally. The draft PR's Actions
run provides separate sandbox runtime validation; do not mark it passed until
its actual result is checked.

## Limits and next boundary

The default sandbox still uses the homogeneous model unless a host explicitly
supplies a table. Four synthetic transport profiles are not the four reference
material calibration. This pass does not reinterpret already-recorded damage
when a host retunes profiles; existing damage remains data, and active results
from incompatible tuning are rejected. Arbitrary pair-specific policy callbacks
cannot be represented by this table-only snapshot; adding those is separate.

Next: small, same-geometry reference specimens to choose and document initial
wood, masonry, concrete and glass fracture responses using this worker route.
Keep mechanical versus fracture tuning and physical cell scale explicit. Review
those specimens before the static house, finer-debris work, or new footage.
