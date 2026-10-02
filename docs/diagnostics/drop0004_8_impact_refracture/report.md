# DROP 0004.8 — Impact Coupling + Re-fracture acceptance

Section status: **complete**

Scope accepted here only: damage impulse coupling, native fragment damage/re-fracture, and bounded secondary fragment/world impact damage. DROP 0004.9 was not started.

## Engine behavior

- Damage cause can survive async structural analysis and couple one impulse after detachment admission succeeds.
- Uniform reference mass uses occupied-cell count; coarse angular response uses lever arm and radius-squared inertia.
- Motion changes do not advance fragment geometry revision.
- Native fragment damage is atomic: unchanged, updated, destroyed, or re-fractured.
- Re-fracture uses fresh deterministic fragment IDs from the destruction sequence.
- Child fragments retain the parent's local coordinate frame, pose and velocities.
- Admission refusal leaves the parent store and destruction sequence unchanged.
- Partial fragment-local damage is remapped to surviving child ownership.
- Old persisted parent identity is explicitly tombstoned before committing destroy/re-fracture replacements.
- Secondary damage is bounded by generation depth, pending queue size, and events processed per host step.
- Fragment-vs-world Avian contacts feed the existing DamageEvent -> progressive-damage pipeline.
- Fragment-vs-fragment impact damage is deliberately not automatic in this pass.

## Acceptance evidence

Focused impact/re-fracture tests: **7 / 7 passed**.

Sandbox Clippy with warnings denied passed. Formatter and diff checks passed.

Real Windows 11 / NVIDIA RTX 5050 Laptop GPU / Vulkan acceptance:
- 318 cells detached by the root damage cut
- accepted damage impulse transferred into the new moving fragment
- fragment-local follow-up damage re-fractured the parent into 2 children
- 21 fragment-local cells failed before re-fracture
- 3 fragment/world impacts were sampled
- impacts produced 24 secondary-failed static cells
- 42 total static cells failed including the original 18-cell support cut
- demo exited successfully after the bounded loop completed

## Acceptance finding

The first visual fixture put its anchored floor at Y=0, on a storage-volume boundary. Exact structural snapshot growth followed occupied geometry through the -Y volume face and required absent regions below it. The engine correctly returned inconclusive rather than treating unknown as empty. The fixture was moved to Y=4; no structural rule was weakened.

Avian's transient collision-start flag was also not reliable at the post-physics read point used by this sandbox. Host contact sampling now compares the current touching-contact set with the previous fixed step, preserving the engine-neutral impact contract.

## Visual

`Micrology_DROP0004_8_Impact_Refracture.mp4` is a fresh FFmpeg Desktop Duplication (`ddagrab`) capture of the current Vulkan sandbox. It is trimmed to the live Micrology portion and cropped tightly to the app window.
