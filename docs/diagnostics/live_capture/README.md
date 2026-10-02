# DROP 0003 live capture

Captured from merged `main` commit `19ee256e7b07834984ffd54ee691058abcb3df16` on the MSI test machine.

This is the normal sandbox input path, not the old offline weak-point renderer:

1. temporary anchored capture world loaded through normal v3 streaming,
2. HUD hidden with `H`,
3. player input performs a normal click then `Ctrl+LMB` radius-4 carve,
4. live `WorldEditBatch` feeds async structural analysis,
5. accepted result detaches into a native fragment,
6. fragment gets a render mesh and Avian f64 rigid body,
7. HUD restored to show final engine counters,
8. `Q` performs the normal clean flush.

The capture world/helper were local-only and are not part of this branch. Only the resulting diagnostic media is committed here.
