# Reference materials — Step 1 audit and behavior definition

Status: **audit complete; mixed-material fracture integration not implemented**.

Reviewed source: `59bc84ae3faa3ddb01a5e8d031b2da719bad1f1b`, the current
`chatgpt/drop-0007-editor-foundation` tip verified through draft PR #12.
This is a source inspection, not a new runtime or performance acceptance run.

## Scope and stop boundary

The user authorized only the audit/definition of **wood, masonry/brick,
concrete, and glass**, followed by a stop. No house, destruction scenarios,
fragment-size tuning, engine changes, or new video belong in this step.

This note does not claim the editor branch is fully accepted. It does not
rename the editor's existing 0007.1 selection section. The earlier conversational
suggestion to call the house work "0007.1" conflicts with that scope; this audit
therefore has no new numbered drop assignment.

## Main finding

**The four materials already exist as mechanical profiles. The inspected
persistent-fracture worker still selects a homogeneous fracture model.**

These are two distinct paths from the same event, not consecutive stages:

```text
DamageEvent -> MaterialDamagePolicy / MaterialFailurePolicy
            -> ProgressiveDamageStore -> failed cells

DamageEvent -> FractureImpact -> FracturePolicy
            -> persistent cell energy / bond integrity -> fracture transaction
```

Adding or recoloring four materials is not sufficient to give the second path
four behaviors. Conversely, replacing the existing mechanical profiles would
throw away useful work that is already present.

Evidence: [mechanical profiles](../crates/engine_mechanics/src/profile.rs),
[material damage policies](../crates/engine_mechanics/src/damage.rs), and
[fracture model and policy](../crates/engine_destruction/src/fracture.rs).

## Existing values — retain for the initial reference set

The table reproduces **raw `Milli` integers** from `ReferenceMaterial::values`.
These are existing example settings, not newly selected or calibrated physical
measurements. `MechanicalProfile::density` contributes mass per occupied cell;
comments describing familiar physical magnitudes do not establish a complete
meter/area/volume conversion for a higher-resolution house.

| Material | Density | Tensile | Compressive | Shear | Toughness | Failure tag |
|---|---:|---:|---:|---:|---:|---|
| Wood | 600 | 40,000 | 30,000 | 7,000 | 20,000 | Ductile |
| Masonry | 2,000 | 400 | 20,000 | 1,200 | 8,000 | Granular |
| Concrete | 2,400 | 3,000 | 35,000 | 5,000 | 12,000 | Granular |
| Glass | 2,500 | 50,000 | 300,000 | 35,000 | 2,000 | Brittle |

Material IDs remain caller-assigned through `reference_registry`. This audit
does not reserve new IDs, duplicate registries, or remove the existing steel
reference; steel is simply outside the requested four-material fixture set.

### What those profiles currently do in applied damage

`MaterialDamagePolicy` scales incoming scalar damage by failure character;
`MaterialFailurePolicy` takes its threshold from toughness in whole units.

| Material | Delivered scalar damage | Accumulated failure threshold |
|---|---|---:|
| Wood | floor(raw amount × 3/4) | 20 |
| Masonry | raw amount | 8 |
| Concrete | raw amount | 12 |
| Glass | floor(raw amount × 3/2), saturated to u32 | 2 |

This table is derived from the inspected formulas, **not a freshly executed
benchmark**. Integer rounding matters: a raw amount of 1 delivers zero to the
current ductile policy. Do not promise that every arbitrarily small hit must
accumulate damage.

The existing [material-damage tests](../crates/engine_mechanics/tests/material_damage.rs)
exercise glass/steel/masonry contrasts, delivery ratios, uniform fallbacks, and
repeatable mixed-material scalar damage. They were read, not rerun in this step.
They do not prove the proposed house's four-material persistent-fracture behavior.

## Minimal behavior definitions for the future fixture

These are **design targets**, not claims that new shard or deformation behavior
has shipped.

| Material | Intended use | Initial behavior target | Explicit limit |
|---|---|---|---|
| Wood | Frame, floor/roof members | Relatively light; tolerate more scalar damage than masonry; localized failure followed by structural detachment | The Ductile tag is not an elastic-bending solver, grain direction, or a splinter generator |
| Masonry/brick | Chimney and selected wall sections | Weak in tension relative to compression; damageable joints/material rather than instant removal of the entire structure | The Granular tag does not author individual bricks, mortar, or brick-shaped debris |
| Concrete | Slab, foundation, steps | Higher existing scalar-damage threshold and mode capacities than masonry; damageable base with explicit authored support | It is not indestructible, automatically anchored, or universally strongest in every mode |
| Glass | Window panes | Low scalar-damage failure threshold; break locally without automatically destroying the surrounding frame | Small shards, pane thickness, transparency, and collision treatment are separate requirements, not consequences of the Brittle tag |

In particular, the existing glass profile has higher static capacity numbers
than concrete despite much lower applied-damage toughness. "Strongest" is not
one global ranking. Keep load capacity, impact response, support, appearance,
and fragment shape separate.

## Confirmed integration gaps

### 1. The worker interface is still homogeneous

`FractureJobInput::run` takes a concrete `FractureModel`, not a supplied
material-aware policy. Both `FractureModel` variants ignore the material ID in
`profile`. Historical uses `REFERENCE_SOLID`; Coherent changes crush to 12,000
and toughness_milli to 650, but still applies the same profile to every material.

The lower-level transactions already accept a fracture policy. The missing
bridge is an opt-in material-aware policy plus an owned worker representation
that carries it consistently through static impacts and fragment re-fracture.
Do not make `engine_destruction` depend on downstream `engine_mechanics`.

Evidence: [owned fracture jobs](../crates/engine_destruction/src/fracture_jobs.rs)
and `FracturePolicy` / `FractureModel` in
[fracture.rs](../crates/engine_destruction/src/fracture.rs).

### 2. A mixed-material bond currently reads only its lower endpoint's profile

The bond-loading pass obtains the material at `BondKey::lower()` and uses that
one profile for brittleness and capacity. This is harmless for the accepted
one-material fixtures. With different per-material profiles, merely swapping
which material occupies the lower endpoint could change the joint response.

**Design requirement:** canonical bond identity may remain lower-cell-plus-axis,
but material-pair response must consult both endpoint materials. For an otherwise
equivalent setup, exchanging the endpoints must not change joint properties
because of coordinate ownership alone. Preserve actual load direction/mode.

A symmetric weaker-side capacity rule is a candidate, not a validated new
physical law. Pair composition, including toughness, must be explicit and tested
before the house's joints are described as working. Nails, mortar, adhesives,
and other connection types are deferred, not silently invented by adjacency.

Evidence: `evaluate_fracture`, bond-loading pass in
[fracture.rs](../crates/engine_destruction/src/fracture.rs).

### 3. Bond-history invalidation also assumes one recorded material

`BondFracture` stores the lower endpoint's material. `prune_against` checks that
material still matches and that the upper endpoint remains occupied; it does not
check an upper-endpoint material replacement.

**Design requirement:** changing either endpoint's material must not silently
reuse an incompatible mixed-material bond history. Before implementation,
choose between explicit two-endpoint metadata and guaranteed local invalidation
of incident bonds on material replacement. The existing local `forget_cell` /
`forget_removed` machinery provides a path to investigate without a global scan.
A persistence/schema rewrite is not authorized by this audit.

This is a source-identified mixed-material hazard; no failing runtime test was
created in Step 1.

### 4. Similar field names do not mean compatible units

`MechanicalProfile::toughness` becomes a scalar-damage threshold.
`FractureProfile::toughness_milli` is an energy-to-bond-load control whose
complement is clamped in 0..1000. Blindly copying raw mechanical toughness
(2,000–20,000 for these materials) into that fracture field would clamp the
bond-loading factor to zero.

`FractureProfile::density_milli` is explicitly not read by fracture analysis.
Changing it alone does not establish material-dependent runtime body mass.
The eventual host/mass integration needs its own check; it was not audited here.

**Definition:** keep explicit fracture tuning separate from mechanical example
values. Reuse the existing fracture fields first: crush, tensile, compressive,
shear, toughness_milli, and the density metadata. No new per-cell material fields
are required merely to distinguish four scalar fracture responses. New numeric
fracture profiles are intentionally not guessed in this source-audit step.

## Granularity remains an open measurement, not a hidden tuning change

Nothing inspected here establishes a physical cell size for the proposed house.
The prior conversational suggestion of 8–16 cells per meter was not a measured
selection. At that illustrative range, one cell would be 12.5–6.25 cm, so it must
not be assumed to resolve every stud or thin pane with several cells.

Before judging debris scale, the fixture will need explicit physical dimensions,
cell scale, and comparisons at a consistent camera distance. Fragment counts
alone cannot distinguish coarse cells from coarse grouping. Changing resolution
also changes per-cell mass and load interpretation unless scaling is defined.

Grain orientation, stiffness/plastic deformation, fracture length scale, a
material-pair connection type, and specialized glass debris are **future candidate
capabilities**. None is added now. The first four profiles must not be advertised
as already implementing them.

## Outcome and next boundary

Step 1 is complete as an audit and behavior definition:

- Reuse the existing four mechanical references, unchanged.
- Preserve the accepted homogeneous fracture policy and all old golden output.
- Record the worker-policy, two-endpoint bond-response, and bond-history gaps.
- Keep fracture tuning independent from mechanical thresholds and cell scale.
- Keep editor numbering unchanged.

No production code, fixtures, golden output, or save format changed. No builds,
benchmarks, hardware capture, or runtime tests were run for this documentation
step; no new pass counts are claimed.

The next user-approved step remains **the static house fixture only**, for visual
inspection before destructive scenarios. A static multi-material house is not
proof that mixed-material fracture is integrated. The gaps above must be resolved
before later destruction results can be used to judge those material behaviors.

**Stop here.** Do not build that house or implement these gaps during this step.
