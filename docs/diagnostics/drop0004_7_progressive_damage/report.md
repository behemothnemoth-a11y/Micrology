# DROP 0004.7 — Progressive Local Damage acceptance

Section status: **complete**

Scope accepted here only: sparse accumulated damage and local cell failure. No force-to-fragment coupling, fragment splitting policy, or secondary collision damage from 0004.8 was added.

## Engine behavior

- Untouched cells allocate no damage state.
- Repeated sub-threshold DamageWork accumulates per cell.
- Threshold crossing emits one canonical failed target and clears its partial record.
- Sparse state has a hard entry cap; over-cap transactions are refused atomically.
- UniformFailurePolicy keeps all materials equivalent in DROP 0004.
- Static failures convert to WorldEditBatch.
- Fragment-local failures return a new native Fragment with geometry + persistence revisions advanced.
- Destroying every fragment cell returns None instead of an empty fragment.
- Partial damage can be extracted/restored by static region or fragment identity for streamed ownership.

## Acceptance evidence

Focused progressive damage tests: **8 / 8 passed**.

Sandbox Clippy with warnings denied passed.

Fresh Windows/NVIDIA/Vulkan visual acceptance:
- real Micrology sandbox window
- captured with FFmpeg Desktop Duplication (ddagrab), not recycled footage
- 12 weak radial pulses
- 3 target areas
- 63 cells failed
- 0 partial entries remained at completion

## Visual

Micrology_DROP0004_7_Progressive_Damage.mp4 is a fresh ~10.1 second GPU capture of the current sandbox. The HUD shows the live 0004.7 pulse/failure state and the final wall contains three newly-created damage holes.
