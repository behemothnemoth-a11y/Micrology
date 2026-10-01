# Design Principles / Guardrails

## 1. Engine first, game later

Anything specific to one future game belongs above the engine layer.

## 2. No Minecraft dependency in core crates

Minecraft conversion belongs under import/export adapters.

Do not use:
- Minecraft NBT structures as native world state
- Fabric APIs in core
- Minecraft block registry IDs as core material IDs
- Minecraft chunk coordinates as an unavoidable engine abstraction

## 3. Microcells are data, not entities

Never model every microcell as an ECS entity.

A chunk/region may be an entity. A compiled mesh may be an entity. Individual cells should normally remain compact data.

## 4. Static geometry should become cheap

If nothing is changing, the engine should be free to compile geometry into an efficient static representation.

## 5. Rebuild locally

Changing one cell must not require remeshing the entire world.

## 6. Determinism matters

Meshing, coordinate transforms, serialization, and generation should become reproducible.

This matters for:
- regression testing
- multiplayer later
- seeded generation later
- reproducible bug reports
- BuildWright integration
- schematic conversion

## 7. Keep an oracle

When adding aggressive geometry optimization, preserve a simple correctness implementation or test oracle.

The existing Astra Microblocks workflow benefited from validating optimized surfaces against exact unit-face topology.

Keep that habit.

## 8. Performance claims need measurements

Track:
- occupied cells
- exposed unit faces
- emitted quads/triangles
- vertices/indices
- mesh build time
- dirty rebuild time
- rendered chunks
- frame timing

Do not claim FPS improvements from geometry counts alone.

## 9. Preserve headroom for large worlds

Avoid assumptions that require the entire world:
- in memory
- in one coordinate space with poor precision
- in one mesh
- in one save file
- regenerated on every edit

## 10. Avoid premature engine bloat

DROP 0001 should be boring, testable, and foundational.

Do not create twenty half-working subsystems.
