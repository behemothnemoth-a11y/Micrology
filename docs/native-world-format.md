# Native world format

**Format tag:** `micrology.world` · **Current version:** `1` · **Encoding:** UTF-8 JSON

The engine's own save format. Not NBT, not a schematic, not anything Minecraft
reads. Import and export of foreign formats will be adapters that translate into
this, never the other way around.

## Why JSON, for now

The DROP 0001 scope asks for a transparent first format and says not to optimise
file size prematurely. JSON is readable in any editor, diffable in review, and
hand-editable while the data model is still moving — all of which matter more right
now than bytes on disk. A binary encoding is a later, additive version bump.

Cells are run-length encoded, which is not premature optimisation but the opposite:
without it a single chunk would be a 4096-element array that no human could read.
With it, a solid chunk is one line.

## Example

```json
{
  "format": "micrology.world",
  "version": 1,
  "materials": [
    { "id": 1, "color": { "r": 130, "g": 130, "b": 136 }, "name": "stone" },
    { "id": 3, "color": { "r": 96, "g": 150, "b": 72 }, "name": "turf" }
  ],
  "chunks": [
    {
      "pos": { "x": 1, "y": 0, "z": 0 },
      "palette": [1, 3],
      "cells": [[1, 768], [2, 256], [0, 3072]]
    }
  ]
}
```

That chunk is three stone layers, one turf layer, then empty: 768 cells of palette
slot 1 (stone), 256 of slot 2 (turf), 3072 empty.

## Fields

### Top level

| field | type | notes |
|-------|------|-------|
| `format` | string | Must be `micrology.world`. Anything else is rejected rather than guessed at. |
| `version` | integer | Must be `1`. A version this build does not know is a clean error, never a misparse. |
| `materials` | array | Material definitions, ascending by `id`. Omitted when empty. |
| `chunks` | array | Non-empty chunks, ascending by position. Omitted when empty. |

### Material

| field | type | notes |
|-------|------|-------|
| `id` | integer | The engine's own stable id. Not a foreign registry id. |
| `color` | `{r, g, b}` | 24-bit sRGB, components `0`–`255`. |
| `name` | string | Optional. For editors and debugging only; never semantic. |

A cell referencing an unregistered material still loads, and renders as magenta
(`Rgb::MISSING`) so a data bug is loud rather than invisible.

### Chunk

| field | type | notes |
|-------|------|-------|
| `pos` | `{x, y, z}` | Chunk address, signed. Chunk `-1` covers cells `-16..-1`. |
| `palette` | array of integers | Material ids. Cell value `n ≥ 1` means `palette[n - 1]`. Ascending. |
| `cells` | array of `[value, length]` | Run-length encoded cells. Value `0` is an empty cell. |

Runs are in the engine's canonical index order: `x` fastest, then `z`, then `y`
(`index = x + 16 * (z + 16 * y)`). Lengths must sum to exactly 4096; anything else
is rejected and the error names the offending chunk.

Palettes are **per chunk**, so the same material can occupy different slots in
different chunks. In the example above turf is slot 2 because that chunk contains
no dirt.

## Guarantees

**Canonical output.** Equal worlds serialize to identical bytes, whatever order
they were built or edited in. Chunks ascend by position, materials by id, and each
chunk's palette is sorted and garbage-collected before writing. Tested directly:
a world built forwards and the same world built by a noisy route of paints,
repaints and undos produce the same string.

**Nothing is lost.** Save, reload and the world compares equal — including carved
cavities, negative coordinates and chunks that were emptied. Compiled geometry
after a reload is identical quad-for-quad to the geometry before it.

**Emptiness is not stored.** Chunks that exist in memory but hold no cells are not
written, and are indistinguishable from chunks that never existed.

**Atomic writes.** A save goes to a `.tmp` sibling and is renamed into place, so an
interrupted write cannot leave a half-written world.

**Forward tolerance.** Unknown fields are ignored on load, at every level. A newer
writer may add generator seeds, lighting data or PBR parameters and this build will
still read the file. That is the "room for future metadata" the scope asks for, and
it means additive changes need no version bump.

## Changing the format

Additive and optional? No bump — unknown keys are already ignored and missing ones
default.

Anything a version-1 reader would misread? Then:

1. Bump `FORMAT_VERSION` in `crates/engine_io/src/format.rs`.
2. Record what changed in this file, under a `## Version history` heading.
3. Keep loading old versions. The point of an explicit version is migration, not
   refusal — the current single-version check is the floor, not the ceiling.
4. Regenerate fixtures: `cargo run -p engine_io --example make_fixtures`.
5. Add the *old* file to `fixtures/worlds/` as a permanent regression case, so the
   migration path stays tested.

CI fails if the committed fixtures are no longer this build's canonical output.
That is deliberate: a format change should be a visible, reviewed diff.

## Known limitations

- No compression, and no intention of adding it before a binary version.
- No world metadata yet: no name, no seed, no spawn point, no timestamps. The
  schema has room; nothing writes them.
- Everything is in one file. Phase B's streaming work needs per-region files, and
  that will be a version bump.
- Floating point does not appear in the format at all, which is why determinism is
  currently easy. Keep it that way if you can.
