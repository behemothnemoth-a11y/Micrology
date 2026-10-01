# Architecture Direction

## High-level layout

```text
Engine
├── Core
├── Math / Coordinates
├── World
│   ├── Regions
│   ├── Chunks
│   ├── Streaming
│   └── Persistence
├── Volume
│   ├── Cells
│   ├── Materials
│   ├── Palettes
│   ├── Editing
│   └── Transforms
├── Geometry
│   ├── Surface Extraction
│   ├── Greedy Meshing
│   ├── Cross-Volume Occlusion
│   ├── Mesh Cache
│   └── Dirty-Region Rebuilds
├── Renderer
├── Physics
├── Procedural
├── Scripting
├── Editor
├── Assets
└── ImportExport
    ├── Astra Microblocks
    ├── Litematica
    ├── Generic voxel/volume formats
    └── future GIS / glTF / etc.

Projects
├── Sandbox
├── Earth experiment
├── Procedural planet game
├── Destruction game
└── future games
```

## Fundamental rule

Do **not** make a Minecraft block the engine's root spatial primitive.

The engine should think in engine-native cells/volumes/chunks.

A 16x16x16 microcell volume is useful as:
- a compatibility size
- a local editing container
- a test fixture shape

But renderer behavior must be free to optimize across those boundaries.

## Suggested language / framework

Preferred starting direction:

- **Rust**
- **Bevy** for windowing, ECS, rendering integration, input, and fast iteration

However, keep the engine's world/volume/meshing logic as independent Rust crates where reasonable so core data and tests do not become inseparable from Bevy.

## Suggested workspace split

```text
crates/
  engine_core/
  engine_world/
  engine_volume/
  engine_geometry/
  engine_render/
  engine_io/

apps/
  sandbox/

fixtures/
  volumes/
  worlds/

docs/
```

Do not over-fragment early. These boundaries are conceptual; merge crates if needed to keep DROP 0001 simple.

## Representation strategy

Long term the engine should support multiple representations:

```text
coarse world / terrain
    ↓
chunk geometry
    ↓
fine local volume
    ↓
micro-detail only where needed
```

The authoring resolution and display resolution are not required to be identical.

## Rendering strategy

The renderer should eventually use proven techniques such as:

- hidden-face removal
- greedy surface meshing
- merging across local-volume boundaries
- chunk batching
- indexed geometry
- GPU buffering
- frustum culling
- occlusion culling
- material batching
- asynchronous mesh compilation
- LOD
- repeated-object instancing
- mesh clusters / meshlets where useful later

DROP 0001 only needs the minimum subset required to establish the architecture.

## Editing model

An edit should:

1. modify engine-native volume data
2. mark the smallest affected region dirty
3. rebuild only affected geometry
4. update the rendered mesh
5. persist correctly

The first editing operations can be very simple:
- add one cell
- remove one cell
- replace material

## Future destruction model

Do not implement this yet, but preserve the path:

```text
optimized static region
→ local damage
→ affected volume becomes editable
→ local topology changes
→ local mesh + collision rebuild
→ disconnected pieces may later become physics bodies
```

Avoid any architecture that requires every microcell to be its own entity or rigid body.
