# DROP 0004.4 — Async Fragment Derived Data acceptance

Section status: **complete**

Scope accepted here only: asynchronous fragment mesh/collider compilation. No 0004.5 or later work was added during this acceptance pass.

## What is implemented

- Owned engine worker contracts for fragment mesh and collision compilation.
- Geometry-only fragment fingerprints, separate from motion/persistence revision.
- Mesh compilation bounded to 4 active worker jobs.
- GPU mesh upload bounded to 4 per frame.
- Collision compilation bounded to 8 active worker jobs.
- Avian body creation bounded to 8 per frame.
- Completed worker results are applied in deterministic FragmentId order.
- Geometry-stale worker results are rejected.
- Normal rigid-body motion does not invalidate local mesh/collider results.
- Bevy/Avian/task-pool types remain outside the engine worker contracts.

## Acceptance evidence

Focused engine tests:
- 4 / 4 derived_jobs tests passed.
- Owned mesh/collision inputs and results are Send.
- Mesh/collision compilation is deterministic.
- Physics motion does not stale geometry results.

Static checks:
- workspace Clippy excluding sandbox passed with warnings denied.
- sandbox Clippy passed with warnings denied.
- the shared branch's global cargo fmt check is currently blocked only by later 0004.6 damage.rs formatting; 0004.4 itself was previously rustfmt-closed in commit 98494b5.

Existing section hardware evidence:
- Windows / NVIDIA / Vulkan destruction smoke passed at 0004.4 closure.
- 384 cells detached.
- async mesh/collision results admitted.
- Avian body created.
- physics readback observed fragment Y movement from 16.000 to 15.989.

## Visual

Micrology_DROP0004_4_Async_Fragment_Derived_Data.mp4 is an 18.6 second section recap. It uses real Micrology destruction footage and overlays the async worker behavior because the scheduling change itself is intentionally invisible during ordinary play.
