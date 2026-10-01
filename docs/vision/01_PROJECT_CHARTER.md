# Project Charter

## Working description

A reusable standalone volumetric game engine for large, destructible, procedurally generated worlds with extremely fine local geometry.

The engine must support multiple different games. It is not a single game.

## Core goals

1. **Engine-native volumetric worlds**
   - Microgeometry is a first-class engine concept.
   - The renderer must not inherit Minecraft block boundaries.

2. **Multiple levels of detail**
   - Very fine detail can exist locally.
   - Distant or simple regions must use cheaper representations.
   - Fine resolution should be activated only where useful.

3. **Large-world streaming**
   - Worlds are divided hierarchically into regions/chunks.
   - The engine loads and compiles only what is relevant.

4. **Destruction-ready architecture**
   - Static geometry should be aggressively optimized.
   - Edited/damaged areas can temporarily become higher-cost dynamic regions.
   - Local changes should trigger local recompilation, not whole-world rebuilds.

5. **Procedural generation**
   - Deterministic seeded generation is a long-term core capability.
   - Untouched generated terrain should eventually be regenerable from seed.
   - Saves should primarily persist deltas/changes.

6. **Multiple games**
   - Engine systems and game rules must be separated.
   - Games should become projects/modules on top of the engine.

7. **Editor-first workflow**
   - The eventual editor is a major product surface, not an afterthought.
   - Building, carving, material painting, generation, and playtesting should converge in one tool.

8. **Interoperability**
   - Minecraft/Astra Microblocks/Litematica should be import/export targets.
   - BuildWright-style architecture generation should eventually target the engine directly.
   - Future GIS / terrain / city experiments should feed the same engine world representation.

## Non-goals for the first milestone

Do not try to make:
- a Minecraft replacement
- a full Teardown clone
- a No Man's Sky clone
- a complete editor
- a complete game
- a realistic physics simulation
- an infinite procedural universe

The first milestone is a kernel/proof of architecture.
