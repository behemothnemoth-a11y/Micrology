# Micrology

A standalone volumetric game engine for large, destructible, procedurally
generated worlds with very fine local geometry.

**This is an engine, not a game, and not a Minecraft mod.** Minecraft is a future
import/export format and a test ecosystem — never the foundation. No core crate
depends on Minecraft, Fabric, NBT, block entities or a block registry, and none
ever will; that belongs behind import/export adapters.

The engine is at **DROP 0001 — World Kernel**. One milestone is done: the engine
owns, displays, edits, saves and reloads its own volumetric geometry. Everything
in `docs/vision/06_FUTURE_ROADMAP.md` past phase A is deliberately not started.

![The sandbox on its demo scene](docs/images/sandbox.png)

*The demo scene: terraced ground, a hollow brick room, a stepped ramp. Note the
large unbroken faces — greedy meshing collapsed 12,064 exposed cell faces into
1,532 quads, so flat regions have no per-cell seams at all.*

## Quick start

```sh
cargo test              # the engine suite — seconds, no GPU needed
cargo run -p sandbox    # open the sandbox on the demo scene
```

That is the whole setup. A fresh clone needs nothing but a stable Rust toolchain
(and, on Linux, the system libraries listed below).

```sh
cargo run -p sandbox -- fixtures/worlds/drop0001_sample.json   # load a world instead
```

### Sandbox controls

| input | action |
|-------|--------|
| click | capture the mouse |
| `Esc` | release the mouse |
| `W` `A` `S` `D`, `Space`, `Ctrl` | fly |
| `Shift` | fly faster |
| left click | carve the cell under the crosshair |
| right click | place a cell against the face you are looking at |
| `F` | repaint the cell under the crosshair |
| `1`–`5`, mouse wheel | choose material |
| `F5` / `F9` | save / reload `saves/sandbox.world.json` |
| `G` | swap the greedy mesher for the exact oracle |

`G` is a debugging tool worth knowing about: it re-renders the world with the
simple reference extractor instead of the optimised mesher. If a visual artefact
survives the swap, the cell data is wrong, not the meshing.

### Linux system dependencies

Bevy needs these to build the sandbox. The engine crates need none of them.

```sh
sudo apt-get install -y libasound2-dev libudev-dev libwayland-dev \
                        libxkbcommon-dev libxkbcommon-x11-dev
```

`libxkbcommon-x11` is easy to miss: it is loaded at runtime rather than link time,
so leaving it out builds fine and then panics inside winit on startup.

## What exists

```text
crates/
  engine_core/      coordinates, materials, revisions, the CellSource trait
  engine_volume/    one 16^3 volume of cells, palette-indexed
  engine_world/     sparse volume map, editing, dirty tracking
  engine_geometry/  surface extraction, greedy meshing, per-section mesh cache
  engine_io/        versioned save/load
  engine_stress/    deterministic stress scenarios and the measurement harness
apps/
  sandbox/          the desktop app: window, camera, picking, rendering
fixtures/worlds/    committed worlds, used to prove format stability
docs/               architecture, save format, testing, and the vision pack
```

The crates form a strict line: `core <- volume <- world`, with `geometry`
depending only on `core`, and `io` on top. Geometry does not know how cells are
stored and the engine does not know what a renderer is. Bevy appears in exactly
one place — `apps/sandbox` — and `apps/sandbox/src/render.rs` is the only file
where engine geometry meets a graphics API.

### The four ideas that matter

**Cells are data, never entities.** A 16³ volume is `u16` palette indices, 8 KiB
at its most expensive, and nothing at all when it is empty or uniform. Nothing in
the engine will ever model a cell as an ECS entity or a rigid body.

**Volume seams are not render boundaries.** Meshers read cells in *global* space
through one trait, so they sample a cell past the volume they are compiling. A
face shared with a solid neighbour is never emitted. Two solid volumes side by
side lose all 512 faces at their seam.

**There is always an oracle.** `ExactCompiler` emits one quad per exposed face and
is kept permanently. `GreedyCompiler` merges coplanar same-material rectangles — a
solid volume collapses from 1536 faces to 6 quads. The tests expand the merged
output back into unit faces and demand it match the oracle exactly, over seeded
pseudo-random worlds at every density. Optimised geometry is only trustworthy
while something simple stands beside it.

**Rebuilds are local.** Editing one cell re-meshes one render section. An edit on
a volume boundary also re-meshes the touching neighbour, because clearing a seam
cell exposes a face that belongs to the neighbour's mesh — the subtle half of the
promise, and the one that leaves holes at seams when it is missed.

**Storage, residency and rendering are different units.** A 16³ volume is the
storage leaf. A region is what gets loaded, evicted and saved. A render section is
what gets meshed and drawn. They happen to line up today; nothing is allowed to
assume they always will.

## What does not exist yet

Not started, by design: streaming, LOD, destruction, physics, collision,
connectivity/island detection, an editor, procedural generation, scripting,
networking, multiplayer, Minecraft or Litematica import/export, GIS ingestion,
transforms on volume data, and any actual game.

DROP 0002 (scale: regions, streaming, async meshing) is in progress —
`docs/drop-0002.md` tracks it pass by pass.

## Documentation

| document | contents |
|----------|----------|
| [`docs/architecture.md`](docs/architecture.md) | how the crates fit together, and why the boundaries sit where they do |
| [`docs/native-world-format.md`](docs/native-world-format.md) | the save format, field by field, and how to evolve it |
| [`docs/testing.md`](docs/testing.md) | what is covered, and how to add a case |
| [`docs/drop-0001.md`](docs/drop-0001.md) | DROP 0001 status, limitations, verification |
| [`docs/drop-0002-scope.md`](docs/drop-0002-scope.md) | DROP 0002 scope: scale, regions, streaming |
| [`docs/drop-0002.md`](docs/drop-0002.md) | DROP 0002 progress and recorded baselines |
| [`docs/vision/`](docs/vision/) | the original project brief, preserved verbatim |
| [`CLAUDE.md`](CLAUDE.md) | guardrails for anyone — human or agent — changing this repo |

## Lineage

The ideas here were proven in [`astra-microblocks`](https://github.com/behemothnemoth-a11y/astra-microblocks):
a 16³ microcell volume, mixed materials in one volume, native RGB materials,
deterministic greedy face merging, exact geometry preservation, and the habit of
validating an optimised mesher against a unit-face topology oracle. What is *not*
carried over is the Minecraft architecture those ideas were trapped in. This
engine owns its own batching, buffering, cross-volume occlusion, rebuild
scheduling and persistence.

## Licence

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your
option.
