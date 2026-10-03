# DROP 0004.5 — Structural Search Acceleration acceptance

Section status: **complete as an experiment**

Scope accepted here only: a conservative support-witness prepass for the expensive supported/no-detachment case. The live destruction host still uses the exact classifier.

## Safety contract

The exact connectivity classifier remains structural truth.

The witness is allowed only two outcomes:

- AllRootsSupported — every occupied damage root has a concrete resident face-connected path to an anchor or a previously proven-supported cell.
- NeedExact — anything uncertain falls back to the exact classifier.

The witness never declares geometry detached. Unknown residency, a closed unanchored component, and witness-budget exhaustion all return NeedExact.

## Acceptance evidence

Focused engine unit cases: **5 / 5 passed**
- anchored path proves support
- closed unanchored component never claims support
- unknown space is not treated as empty/supported
- successful-root memoization works
- budget exhaustion falls back instead of guessing

Oracle comparison:
- all ten canonical structural scenarios were checked against the unlimited exact classifier
- no witness-supported result hid an exact detachment or unsettled result

Large supported structure measurement:
- exact classifier: **25,216 cells visited**
- support witness: **3,614 cells visited**
- reduction: **85.7% fewer visited cells**

This is a work-count measurement, not a frame-time or production speed claim.

Static validation:
- engine_destruction + engine_stress Clippy passed with warnings denied
- the 0004.5 implementation was previously rustfmt-closed in commit 02f8ded
- the shared branch currently contains later 0004.6 work; no later-section code was modified during this acceptance pass

## Production decision

The witness remains **disabled in the live host**.

A witness that fails to prove support adds work before exact classification, so enabling it in production should wait for a measured bounded witness budget across a broader workload. The successful experiment is kept as an optional engine primitive and oracle-checked reference.

## Visual

Micrology_DROP0004_5_Support_Witness.mp4 is a 16-second section recap using real Micrology destruction footage plus the reproduced exact-vs-witness cell-visit measurement.
