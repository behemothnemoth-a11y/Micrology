# DROP 0004.6 — Engine-Neutral Damage Contract acceptance

Section status: **complete**

Scope accepted here only: deterministic, gameplay-independent damage events and bounded per-cell evaluation. No 0004.7 accumulation/failure behavior was added.

## What is implemented

- Stable ordered DamageEventId / DamageSequence identity.
- Static-world and exact fragment-local coordinate spaces.
- Cell, inclusive box and radial sphere damage volumes.
- Uniform and linear radial falloff.
- Abstract integer DamageAmount.
- Optional validated world-space source and impulse.
- DamagePolicy seam plus material-agnostic UniformDamagePolicy.
- The same evaluator works against World or Fragment through CellSource.
- Explicit source identity prevents a FragmentLocal(A) event from being evaluated against fragment B.
- Explicit per-event cell-work ceiling.
- Truncated evaluation is never actionable as a partial result.
- Sphere bounds clip safely at current i32 cell address limits.

## Acceptance fixes made

Two contract holes were closed during this section review:

1. DamageImpulse no longer exposes a public raw tuple field, so callers cannot bypass finite-value validation. A vector() accessor returns the validated value.
2. Damage evaluation now carries an explicit DamageSource coordinate domain/fragment identity and rejects event/source mismatches with DamageEvaluationError::SpaceMismatch.

## Acceptance evidence

Focused damage contract tests: **10 / 10 passed**.

Coverage includes:
- deterministic event sequencing
- uniform and linear sphere falloff
- static/fragment-local separation
- wrong-fragment identity rejection
- material-agnostic DROP 0004 reference response
- shared world/fragment evaluator
- bounded all-or-nothing evaluation
- invalid floating-point input rejection
- integer-address-edge clipping

Regression:
- full engine_destruction crate tests passed
- engine_destruction Clippy passed with warnings denied
- cargo fmt --all --check passed
- git diff --check passed

## Deliberate non-goals

This section does not make damage live in the sandbox yet and does not accumulate damage state. It also does not assign different resistance to wood, steel, glass or concrete. Those remain later layers: 0004.7 for sparse accumulated damage/failure and DROP 0005 for material mechanics.

## Visual

Micrology_DROP0004_6_Damage_Contract.mp4 is a ~16 second section recap. It briefly shows existing Micrology destruction output, then diagrams the new DamageEvent -> bounded evaluation -> DamageWork contract and clearly notes that live accumulated damage comes next.
