# DROP 0001 — World Kernel

## Purpose

Prove that the new engine owns its world representation instead of borrowing Minecraft's.

## Deliverable

A standalone desktop sandbox that can:

1. open a native window
2. move a free camera through a simple scene
3. create/load a small engine-native volumetric world
4. store cells and materials independently of Minecraft
5. compile visible surfaces into renderable geometry
6. avoid drawing internal faces
7. render the result
8. edit a small local region at runtime
9. rebuild only the affected local geometry
10. save and reload the world
11. run automated tests on core world and geometry behavior

## Strong preference

Include support for a local `16x16x16` volume fixture because it maps naturally to existing Astra Microblocks research and makes later fixture porting easier.

But:
- do not make 16x16x16 a permanent global world limitation
- do not make each local volume a render entity forever
- do not preserve hidden faces at neighboring volume boundaries

## Minimal material system

DROP 0001 only needs:
- material ID
- RGB/base color
- optional human-readable name

No Minecraft texture atlas.
No hundreds of Minecraft states.
No animation.
No glass.
No PBR requirement yet.

Keep the material interface extensible.

## Basic runtime editing

Provide only enough interaction to prove incremental rebuilds.

Acceptable:
- keyboard/mouse ray pick
- remove selected cell
- add adjacent cell
- cycle a few test materials

Do not build a polished editor UI yet.

## Save format

Create a simple versioned native format.

Requirements:
- explicit format version
- deterministic serialization where practical
- world/chunk coordinates
- palette/material references
- occupancy/cell data
- room for future metadata

Prefer a transparent first format (for example serde-backed RON/JSON) unless a binary format is clearly simpler.

Do not optimize file size prematurely.

## Geometry requirements

At minimum:

- no internal faces between neighboring occupied cells
- consistent winding
- deterministic output
- material boundaries preserved
- empty/full/simple patterns covered by tests

If practical within DROP 0001:
- greedy merge coplanar same-material faces
- cross-local-volume hidden-face removal

If those make the first drop unstable, implement exact naive surface extraction first behind a clean trait/API, then add optimized meshing immediately after.

Correctness before cleverness.

## Tests

Add fixtures/tests for:

- empty volume
- one occupied cell
- solid volume
- two adjacent cells
- hollow cavity
- mixed-material boundary
- cells crossing a local-volume/chunk boundary
- edit → dirty mark → rebuild
- save → reload exact equality

Preserve an independent simple topology oracle if an optimized mesher is added.

## Completion definition

DROP 0001 is complete when a fresh clone can run one documented command and open the standalone sandbox with a rendered editable volume, and automated tests prove the native data survives editing and save/reload.

Do not expand scope before that works.
