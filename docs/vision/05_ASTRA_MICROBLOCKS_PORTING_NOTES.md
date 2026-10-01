# Astra Microblocks — Concepts Worth Porting

Reference repository:
`behemothnemoth-a11y/astra-microblocks`

These are concepts to study and port conceptually, not Minecraft dependencies to copy wholesale.

## Proven/useful concepts

### Local volume
- 16x16x16 = 4096 cells
- compact occupancy/material representation
- palette-based material storage

### Surface optimization
Existing Astra work demonstrated deterministic greedy rectangle merging for coplanar contiguous faces of the same material.

A solid local volume can collapse to only its exterior surfaces.

Important future improvement in the standalone engine:
- remove hidden surfaces across neighboring local-volume boundaries
- compile at chunk/world level rather than treating every Minecraft host as a separate renderer

### Correctness testing
Astra kept a simple unit-face builder as an independent topology oracle.

Reuse this philosophy:
- exact reference surface extraction
- optimized mesher
- compare expanded optimized output to reference topology

### RGB materials
Astra supports arbitrary 24-bit RGB while keeping material identity compact.

The standalone engine should initially use a much simpler native material definition.

### Transform behavior
Rotation and mirroring should operate on native geometry/data, not renderer-only state.

### Persistence
Astra already treats geometry/material state as persistent content.

The new engine should make persistence native and versioned.

### Cached physical representation
Astra identified repeated collision/shape reconstruction as an expensive path and moved toward cached occupancy-derived shapes.

Standalone engine lesson:
- do not repeatedly rebuild expensive derived data when the source has not changed
- cache by dirty/revision state

## Do not blindly port

Do not carry forward Minecraft-specific restrictions such as:

- block entities
- Minecraft render distance logic
- vanilla atlas UV assumptions
- block-state registry dependence
- one render boundary per host block
- NBT as the engine's primary save model
- Minecraft collision classes
- Litematica transform APIs

Use adapters later.

## Existing performance lesson

The Minecraft implementation still pays for Minecraft's rendering architecture.

The standalone engine's first major advantage should be owning:
- batching
- buffering
- cross-volume occlusion
- dirty rebuild scheduling
- world streaming
