# Working on Micrology

Guardrails for anyone — human or agent — changing this repository. They come from
`docs/vision/04_DESIGN_PRINCIPLES.md`; this file is the short, enforceable version.

## What this project is

A reusable standalone volumetric engine for large, destructible, procedurally
generated worlds with very fine local geometry. It must support multiple games. It
is not a game and **not a Minecraft mod**.

Currently at DROP 0001 — World Kernel. See `docs/drop-0001.md` for exact status.

## Hard rules

**No Minecraft in core crates.** No NBT as world state, no Fabric, no Minecraft
block-registry ids as material ids, no Minecraft chunk coordinates as an
unavoidable abstraction, no block entities. Conversion belongs in import/export
adapters that do not exist yet.

**Cells are data, never entities.** A chunk may be an entity. A compiled mesh may
be an entity. An individual cell is a `u16` in an array. Never model a cell as an
ECS entity or a rigid body — not for picking, not for physics, not for lighting.

**Keep the oracle.** `ExactCompiler` is not dead code and is not a slow path to be
deleted. It is the independent reference that makes `GreedyCompiler` trustworthy,
and it is written deliberately without sharing the greedy mesher's slice
machinery. If you optimise geometry further, add the implementation *and* extend
`crates/engine_geometry/tests/oracle.rs`. An oracle that shares code with the
thing it validates is not an oracle.

**Rebuild locally.** Changing one cell must never re-mesh the world. If you touch
`World::set` or the dirty-tracking logic, remember that an edit on a chunk
boundary affects the neighbour's geometry too, and that
`crates/engine_geometry/tests/incremental.rs` is what catches getting it wrong.

**Determinism is a feature.** Meshing, coordinate transforms and serialization
must be reproducible: same input, same bytes, same quad order. Several tests
assert it directly. This is what makes regression fixtures, seeded generation,
multiplayer and reproducible bug reports possible later. Concretely: iterate
`BTreeMap`/`BTreeSet`, never `HashMap`, anywhere output order can be observed; and
never put a timing or a wall-clock reading inside a type that gets compared for
equality.

**Measure before claiming.** Track occupied cells, exposed faces, quads,
vertices, triangles and rebuild time — the sandbox HUD shows all of them. Do not
claim a frame-rate improvement from geometry counts alone.

**Keep headroom for large worlds.** Never assume the whole world is in memory, in
one coordinate space, in one mesh, in one save file, or regenerated on every edit.

**Do not expand scope.** DROP 0001 is deliberately boring and foundational. Do
not start streaming, LOD, destruction, physics, an editor, procedural generation,
scripting, networking, or import/export until the current drop's next step
(named at the bottom of `docs/drop-0001.md`) is done. Twenty half-working
subsystems is the failure mode this file exists to prevent.

**Correctness before cleverness.** Build the exact implementation first, behind a
clean trait, and test it. Then optimise against it.

## Layout and dependency direction

```text
engine_core  <- engine_volume <- engine_world
engine_core  <- engine_geometry
engine_core + engine_volume + engine_world -> engine_io
all of the above -> apps/sandbox
```

`engine_geometry` depends only on `engine_core` and must stay that way: it reads
cells through the `CellSource` trait and so does not care how they are stored.
Bevy belongs in `apps/sandbox` alone, and ideally only in `src/render.rs`.

The sandbox is kept out of `default-members` so `cargo test` does not build Bevy.
Keep it that way; a fast test loop is the point.

## Before you commit

```sh
cargo fmt --all
cargo clippy --workspace --exclude sandbox --all-targets -- -D warnings
cargo test
cargo clippy -p sandbox --all-targets -- -D warnings   # slow: builds Bevy
```

If you change the save format, bump `FORMAT_VERSION`, say what changed in
`docs/native-world-format.md`, and regenerate the fixtures with
`cargo run -p engine_io --example make_fixtures`. CI fails if the committed
fixtures are no longer canonical output, which is the point — a format change
should be a visible, reviewed diff, and old files must still load.

## Conventions

Tests are named as the sentence they assert
(`a_boundary_edit_dirties_the_touching_neighbour`), not `test_set_3`. Comments
explain *why* a non-obvious decision was made, not what the line does; several
existing comments record the reasoning behind a subtle invariant, and those are
worth preserving.
