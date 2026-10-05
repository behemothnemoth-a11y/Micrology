# Reference house Step 9 — automatic finite-strength authored joints

Date: 2026-10-04.
Base: `665d8ecfc4ac2cd6168c88963ca303a144e01181`.
Branch: `chatgpt/house-joint-load-step9-20261004`.
Status: implementation complete and independently re-verified headlessly; not merged.

## Goal

Replace the explicit catastrophic joint-release control with the first bounded,
automatic construction-joint failure path.

This checkpoint is deliberately narrow:

- static authored construction parts carry their own gravity-like demand;
- authored interfaces have finite capacity;
- a conclusive overload names exact authored `BondKey` interfaces;
- those bonds are broken atomically with the triggering geometry edit;
- the existing fracture-aware topology and detachment transaction decide what
  becomes a native fragment.

It does **not** add a dynamic beam solver, bending/torque stress inside moving
fragments, real fastener calibration, or a second topology system.

## Bounded construction-level capacity

`reference_house_joint_load` builds an exact deterministic graph over the
reference house's authored part labels rather than over all 167,192 cells.

The current intact graph is:

- **175 parts**
- **472 part interfaces**
- bounded by 512 parts / 4,096 interfaces / 16,384 augmentations.

Every part accumulates demand from its occupied material cells. Concrete, wood,
masonry and glass keep their existing reference mechanical identities, but glass
is treated as non-load-bearing in this diagnostic so a window cannot secretly
become a structural brace.

The 50 mm house cell annotation is made explicit. Under geometric similarity,
weight scales with length cubed while one face's capacity scales with length
squared, so the demand/capacity ratio scales with cell edge length. This pass
therefore uses **50/1000 of the reference one-unit-cell demand**. This is a
dimensional comparison convention, not SI calibration.

Authored interface capacity is the weaker endpoint mechanical capacity multiplied
by the number of contacting faces and an effective authored-joint factor.

Current provisional joint factor: **350 / 1000**.

That value is a diagnostic threshold, not a nail, screw, mortar or glue property.

## Threshold evidence

At 350/1000:

| Geometry state | Result |
|---|---|
| intact house | satisfied |
| one-quarter foundation removed | satisfied |
| three-eighths foundation removed | satisfied |
| one-half foundation removed | **overloaded** |

The intact case carries all 11,924,200 raw demand units.

The half-foundation case carries 8,422,740 of 8,606,440 raw demand units.
Its exact minimum cut contains:

- **54 authored part-pair interfaces**
- **1,048 exact fracture bonds**
- **0 non-authored / rigid cut interfaces**

The solver therefore does not need to invent a failure through monolithic
foundation or ordinary same-part occupancy.

The broader strength sweep is preserved in
`docs/diagnostics/house_joint_load_step9_20261004/capacity_sweep.txt`.

## Failure transaction

The existing owned fracture worker now has
`run_removals_with_bond_breaks`.

It stages the triggering cell removals and a bounded set of exact bond failures
inside the worker snapshot. The live world, fragment store, fracture state and
destruction sequence remain untouched until the existing commit boundary checks:

- geometry/support revision staleness;
- current residency;
- identity;
- fragment-storage admission.

Admission refusal or stale authority rejects the entire combined result.

The mechanical cut does **not** delete an arbitrary cell to represent a failed
joint. It writes zero-integrity bond records into the staged fracture state,
then uses the existing crack-aware connectivity and ordinary detachment path.

A separate bond-work ceiling is enforced before analysis. The reference case
requires 1,048 bond failures; a 1,047-bond ceiling refuses without changing
authority.

## Headless automatic result

The half-foundation scenario removes 27,648 concrete foundation cells and the
capacity solver automatically names 1,048 authored joint bonds.

The combined worker result is:

| Measurement | Result |
|---|---:|
| foundation cells removed | 27,648 |
| authored joint bonds broken | 1,048 |
| native fragments created | 5 |
| fragment cells | 88,508 |
| remaining static cells | 51,036 |
| largest fragment | 87,188 cells |

Cell accounting closes exactly.

After ownership separates, the final static fracture state reports zero broken
bonds for this cut. That is expected: a broken face whose endpoints now belong
to different native owners is a topology boundary, not one static-world bond
record that can remain meaningful across both owners. The worker summary is the
transaction evidence that the 1,048 joints broke.

## Independent headless re-verification

The structural claims in this document were re-run from a clean checkout of
`81de6a1` on Linux (Ubuntu 24.04, rustc 1.97.0), with no shared target
directory and no GPU, and reproduce exactly:

| Measurement | Documented | Re-run |
|---|---:|---:|
| authored joint bonds broken | 1,048 | 1,048 |
| foundation cells removed | 27,648 | 27,648 |
| native fragments | 5 | 5 |
| fragment cells | 88,508 | 88,508 |
| largest fragment | 87,188 | 87,188 |
| remaining static cells | 51,036 | 51,036 |

Cell accounting closes: 51,036 + 88,508 = 139,544 = 167,192 - 27,648.

The **live Windows/Vulkan numbers below are the authoring machine's own
measurements.** They were not re-verified here, because this environment has no
Vulkan device at all. Treat them as the Windows run's evidence, not as a second
independent confirmation.

## Live Windows/Vulkan result

The canonical replay
`fixtures/destruction/replay-reference-house-auto-joints.json`
runs the same automatic half-foundation scenario through the real background
worker and fixed-step physics host.

Fresh run on the user's MSI:

- NVIDIA GeForce RTX 5050 Laptop GPU
- NVIDIA driver 581.34
- Vulkan
- worker accepted: 1
- stale: 0
- refused: 0
- capture: 0.294 ms
- worker analysis: 166.7292 ms
- commit: 26.1961 ms
- end-to-end response: 175.3638 ms

The live structural result matches headless exactly: 5 fragments / 88,508
fragment cells / 87,188-cell largest fragment.

After 120 fixed ticks the fragment IDs remain present and their poses change
slightly, proving they entered the existing physics/readback path. The largest
measured same-ID displacement among the five pieces is about 0.0055 cell.

There are no contact-fracture jobs in this run because the detached upper
assemblies remain physically resting against the surviving half of the
structure. This checkpoint therefore proves **automatic static overload and
detachment**, not a dramatic dynamic collapse.

## Relationship to Step 8

Step 8's explicit joint-release control remains useful:

- welded support-band control: largest piece 43,564 cells;
- explicit all-joints release: largest piece 2,274 cells;
- automatic half-foundation overload here: largest piece 87,188 cells.

Those are different experiments.

The automatic result is larger because mechanics breaks only the conclusive
minimum-cut interfaces required by the current overload. It does not pre-shatter
all construction boundaries. That is the intended behavior for this checkpoint.

The next problem is internal dynamic load after a detached assembly starts
moving: bending, torque and impacts need a way to load remaining authored joints
inside native fragments. Do not solve that by globally weakening every joint or
by lowering a universal fragment-size target.

## Follow-up applied during review

Four changes were made on top of `81de6a1`; none alter engine behaviour, and
every structural number above is unchanged after them.

- **The replay fixture is now generated, not hand-authored.**
  `reference_house_auto_joints_replay()` joins the window, weak-repeat and
  damaged-area generators, and the test asserts committed bytes equal canonical
  output. Previously the fixture was valid and its embedded checksum was
  correct, but nothing pinned its bytes, so a `ReplayScript` serialization
  change would have let it drift silently while the other three failed loudly.
  The regenerated file is semantically identical to the authored one.
- **Four solver tests added** for rebuild-identical graphs, an exhausted
  interface budget deferring (alongside parts and augmentations), canonical
  sorted overloaded pairs and bonds, and no breakable interface ever naming a
  concrete foundation bond.
- **A dropped invariant comment was restored** in `fracture_jobs.rs`, above the
  support-witness branch, recording that supported paths are not partial
  component sets and must not be handed to detachment as such.
- **Evidence and tooling were separated from the repository root.** The three
  `patch_*.py` scripts were one-shot source mutators carrying absolute paths
  into the authoring worktree; their edits are already in the committed source
  and they would now fail their own `assert s.count(old) == count`, so they are
  removed (recoverable from `81de6a1`). The empty `autoj-live.stdout.log` and
  the `house-auto-joint-live.json` duplicate of the committed fixture are also
  removed, and the live harness now reads the canonical fixture directly. The
  live run helpers moved to `tools/` beside the existing capture and verify
  scripts, and the live logs to this checkpoint's evidence folder.

## Invariants

- Normal house startup has no seeded fracture damage.
- Existing material/fracture/default partition behavior remains unchanged for
  non-house callers.
- Exact occupancy and fracture-aware connectivity remain topology authority.
- Assembly capacity can only name authored interfaces as breakable.
- A rigid/non-authored minimum-cut interface makes the automatic planner refuse.
- Unknown or budget exhaustion never becomes permission to fail.
- Cell-removal and bond-failure budgets remain separate.
- Worker analysis is non-authoritative until commit.
- Stale/admission refusal is atomic.
- No new save schema.
- No Bevy/Avian types enter engine crates.

## Next boundary

After this checkpoint is accepted, the next mechanics pass should address
**dynamic joint loading inside native fragments**, using the authored part graph
or another bounded exact representation.

That work needs evidence for bending/torque/contact transfer. It should not
silently promote the 350/1000 diagnostic threshold into a universal fastener law.
