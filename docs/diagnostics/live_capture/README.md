# DROP 0003 verified live capture

Captured from merged `main` commit `19ee256e7b07834984ffd54ee691058abcb3df16` on the MSI test machine.

This uses the normal sandbox player-input path, not the old offline weak-point renderer:

1. a local-only anchored capture world is loaded through normal v3 streaming,
2. the capture structure uses a 2x2 weak neck so the cut is intentionally severable,
3. HUD is hidden with `H`,
4. player input performs a normal click then `Ctrl+LMB` radius-4 carve,
5. live `WorldEditBatch` feeds bounded async structural analysis,
6. accepted result passes atomic fragment-budget admission and detaches,
7. the fragment receives its normal render mesh and Avian f64 rigid body,
8. physics motion is visible in the video,
9. HUD is restored before the normal `Q` clean flush.

Verified final HUD from this take:

- 365 cells detached
- 1 native fragment
- 1 fragment render entity
- 1 Avian physics body
- 1 fragment collision box
- no stale/inconclusive/budget-held result on the successful detach

The temporary capture-world generator and generated world are deliberately not committed. Only these diagnostic media files are stored on this branch.
