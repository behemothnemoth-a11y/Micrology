# Native world format

**Format tag:** `micrology.world` · **Current version:** `3` · **Encoding:** UTF-8 JSON

Three versions exist and all load. **v3** is what the engine writes: a sharded
directory with support. **v2** is the same directory without support. **v1** is a
single file, still readable, and migratable with `engine_io::migrate_v1_to_v2`.

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

## Version 2: a sharded directory

```text
world/
  world.json                  manifest: metadata, materials, region index
  regions/
    r.0.0.0.json
    r.-1.0.2.json
```

v1 was one file holding the whole world, which is fine for a resident demo and
impossible for a streamed one: loading a single region would mean parsing
everything, and saving one edit would mean rewriting everything. Regions are the
residency unit, so they became the storage unit.

### The manifest

| field | notes |
|-------|-------|
| `format` | `micrology.world` |
| `version` | `2` |
| `id` | stable identifier for this world |
| `name` | optional, human-readable |
| `spawn` | `[x, y, z]` in cells, where a camera or player starts |
| `generation` | reserved. A seeded world will eventually record enough here to regenerate untouched terrain instead of storing it; that format is not designed yet |
| `materials` | as v1, ascending by id |
| `regions` | which shards exist, ascending |

### A region shard

| field | notes |
|-------|-------|
| `format` | `micrology.region` |
| `version` | `2` |
| `pos` | the region this shard holds, checked against the file name on load |
| `volumes` | `{pos, palette, cells}` per volume, ascending, same encoding as v1 |

Shards are named `r.<x>.<y>.<z>.json`, negative coordinates included
(`r.-1.0.2.json`). A shard whose `pos` disagrees with the name it was loaded as is
rejected, as is a volume that does not belong to the region claiming it — both
would otherwise corrupt a world quietly.

### What v2 adds beyond sharding

- **Single-region I/O.** `load_region` and `save_region` touch one shard. This is
  the streaming primitive.
- **Dirty-only saves.** `save_dirty_regions` writes just the regions with unsaved
  edits and marks them clean, which is what makes "save before evict" affordable.
- **World metadata**, which v1 had nowhere to put.

Everything v1 guaranteed still holds and is still tested: canonical byte-identical
output for equal worlds, atomic writes via a `.tmp` sibling and a rename, unknown
keys ignored at every level, and unregistered materials rendering as magenta
rather than failing.

## Version 3: support

v3 adds **one optional field per region shard** and changes nothing else:

```json
{
  "format": "micrology.region",
  "version": 3,
  "pos": { "x": 0, "y": 0, "z": 0 },
  "volumes": [ … ],
  "anchors": [
    { "pos": { "x": 0, "y": 0, "z": 0 }, "anchors": [[true, 256], [false, 3840]] },
    { "pos": { "x": 1, "y": 0, "z": 0 }, "anchors": [[true, 4096]] }
  ]
}
```

`anchors` records which cells are **structurally fixed to the world** — see
[`drop-0003.md`](drop-0003.md) for why that is world topology rather than a
material property.

### Migration is a reading rule, not a rewrite

A v2 shard is already a valid v3 shard: one that anchors nothing, which is
exactly what a world written before support existed meant. The reader accepts
both versions and a v2 world comes back with zero anchors. Nothing has to be
converted, and the committed `fixtures/worlds/drop0002_sample` is **frozen at
v2** and never regenerated — a fixture rewritten with the format it is supposed
to outlive proves nothing.

### Why anchors are listed beside volumes, not inside them

A volume can be anchored while holding no cells. "This is where the ground is"
stays true after the ground has been dug out, and a world that nested support
inside volume records would quietly heal its own foundations on reload.

### The encoding

Anchor bits are run-length encoded `(anchored, length)` pairs in the same
canonical index order as cells, and lengths always sum to 4,096. That order runs
`x` fastest and `y` slowest, which fits support unusually well: anchoring follows
the ground, so a volume whose bottom layer is anchored is **two runs**.

A uniform volume is **one run**, which is why persistence needs no separate flag
for the uniform storage tiers — all three collapse into one wire representation
and the format never has to know they exist. The committed v3 fixture is the same
size on disk as the v2 one it extends.

Runs that do not cover exactly 4,096 cells are rejected rather than padded:
silently padding a truncated shard would unanchor part of a structure without
saying so.

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
- No generator metadata. The manifest has a placeholder and nothing writes it.
- No timestamps, and no record of which engine version wrote a world.
- `spawn` is the only float in the format. Everything else is integers, which is
  why determinism is currently easy — keep it that way if you can.
- Migration is one-way: there is no v2 → v1 downgrade, and there should not be.
