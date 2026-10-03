# Material bonds - Step 2 implementation checkpoint

Date: 2026-10-03.
Status: **implemented; local engine acceptance passed; not merged**.

Branch: `chatgpt/material-bonds-step2-20261003`.
Starting point: `b6f8b49877564fe279140e8906d54698c6213f32`.
The bond implementation, tests and this note are committed together. The
separate preceding hygiene commit is `b656366eda3eb1e0c5397df5c5b3df374b97a033`.

## What was reproduced

With a two-cell, two-material synthetic joint and an otherwise identical 1200
energy impact, the original implementation reported bond integrity losses of
**327 versus 33** when only the endpoint material ordering was exchanged.
Both are in the model's 0..1000 integrity units; this is not a performance ratio.
The reproduction is preserved in
[reproduction_before.txt](diagnostics/material_bonds_step2_20261003/reproduction_before.txt).

The earlier 600-energy test was not a valid two-record comparison: one response
fell below the 20/1000 sparse-record floor and produced no bond record. The test
energy was changed, not the production floor, to expose both responses without
saturation. The corrected fixture was observed failing before the code change.

## Implementation

Added `BondFractureProfile` and the `FracturePolicy::bond_profile(a, b)` query.
The evaluator now consults both endpoint materials for a shared face instead of
using the lower-coordinate material's profile as the interface response.

The default pair composition is a deliberately simple isotropic approximation:

- Minimum capacity of the two materials, separately for tension, compression
  and shear.
- Maximum of their existing bond-load fractions; the evaluator clamps an
  override's fraction to 0..1000.
- Actual impact direction and loading mode are still evaluated separately.

Policies may override this composition. It is **not calibrated wood/brick/glass
behavior**, and it does not model nails, mortar, adhesives, grain or elasticity.
For equal profiles it reduces to the previous homogeneous response.

Canonical bond identity remains lower-cell-plus-axis. The lower material also
remains the historical record-owner tag; it no longer chooses interface strength.
No fracture record, save schema, fragment grouping policy or golden was changed.
No third-party code was imported.

## Focused acceptance

The new `crates/engine_destruction/tests/fracture_material_pairs.rs` contains
10 passing tests covering endpoint swaps on every axis, negative coordinates,
region seams, both impact directions, same-profile reduction, explicit
mode-by-mode composition, extreme toughness values, pair overrides/clamping,
repeatability under reversed insertion order, one shared accumulating bond,
unknown/budget conservatism, and native-fragment-local evaluation.

The same test count can cover multiple properties. The 10 focused tests are
included in, not additional to, the 797-test engine all-targets result.
Analysis-only checks assert that world/fragment geometry and fracture state do
not change. Repeated damage uses the ordinary fracture-state apply path.

## Final local verification

Executed on the Windows MSI with `rustc 1.98.1 (48a229cea 2026-09-01)`.
Machine-readable commands, individual exit codes, log hashes and LF-normalized
source hashes are in
[verification.json](diagnostics/material_bonds_step2_20261003/verification.json).
The verifier also checked that source contents did not change during its run.
Its `base_commit` records the pre-commit HEAD; the source hashes identify the
actual modified files tested.

| Check | Result |
|---|---|
| Focused material-pair tests | 10 passed |
| `cargo fmt --all --check` | PASS |
| `cargo test --locked --all-targets` (engine default members) | 797 passed |
| `cargo test --locked --doc` | 1 passed |
| Workspace engine Clippy, all targets, warnings denied | PASS |
| `destruction_bench --check` | PASS |
| `destruction_replay --check` | PASS |
| `fragment_distribution_bench --check` | PASS |
| `fracture_locality_bench` | PASS |
| `git diff --check` | PASS |
| Existing fixture/diagnostic files unchanged at verification | PASS |

Locality retains **128 records / 6144 snapshot payload bytes** at 0, 10,000,
100,000 and 1,000,000 unrelated records. Each rung produces the same local
fracture/structural result. Raw output is in
[fracture_locality.json](diagnostics/material_bonds_step2_20261003/fracture_locality.json).
This is a locality check, not a claimed frame-rate or timing improvement.

The pre-existing workspace-format failure was one line wrap in `engine_editor`.
It was corrected without changing its behavior. Cargo also supplied the missing
lockfile entry for the already-declared local `engine_editor` crate; no external
package versions changed. Both corrections are isolated in the hygiene commit.

**Not run locally in this closeout:** sandbox/Bevy Clippy, sandbox tests, new
visual replay, or Windows/GPU profiling. Local engine acceptance is not a claim
that a new GitHub Actions sandbox run has passed. Check the draft validation PR
for its separate CI status.

## Next boundary - do not silently broaden completion

This checkpoint corrects the pair-response calculation only.

**Still open, next small step:** replacing either endpoint material must safely
invalidate incompatible bond history. The current lower-material record tag
and `prune_against` behavior have not been redesigned or repaired here. Direct
repaints are therefore not newly claimed safe for mixed-material fracture.

**Still open afterward:** carrying explicit four-material fracture policies
through the owned worker (which still selects homogeneous `FractureModel`),
material/physical-scale calibration, then the static house and later destruction
scenarios. A passing synthetic pair test is not four-material house acceptance.

Do not start any of those in this checkpoint. Keep the accepted one-material
baseline and the editor's section numbering intact. No house, finer debris,
video, paid infrastructure, or automatic merge belongs in this pass.
