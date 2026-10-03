# Working on Micrology

Guardrails for anyone — human or agent — changing this repository. They come from
`docs/vision/04_DESIGN_PRINCIPLES.md`; this file is the short, enforceable version.

## What this project is

A reusable standalone volumetric engine for large, destructible, procedurally
generated worlds with very fine local geometry. It must support multiple games. It
is not a game and **not a Minecraft mod**.

DROP 0001 (world kernel), 0002 (scale and streaming), 0003 (structural
destruction) and 0004 (destruction at scale and applied damage) are complete.
DROP 0005 — material mechanics and structural capacity — is in progress; see
`docs/drop-0005.md` for exact status and `docs/drop-0005-scope.md` for the
ordered passes.

DROP 0006 — the core destruction/fracture model — is in progress alongside it and
is where current work is happening; see `docs/drop-0006.md`. It is developed
against **one** homogeneous baseline material on purpose: with several materials
in the fixture, a fracture-model problem and a tuning problem look identical.

## Hard rules

**No Minecraft in core crates.** No NBT as world state, no Fabric, no Minecraft
block-registry ids as material ids, no Minecraft chunk coordinates as an
unavoidable abstraction, no block entities. Conversion belongs in import/export
adapters that do not exist yet.

**Cells are data, never entities.** A render section may be an entity. A compiled mesh may
be an entity. An individual cell is a `u16` in an array. Never model a cell as an
ECS entity or a rigid body — not for picking, not for physics, not for lighting.

**Keep the oracle.** Twice over. `ExactCompiler` is not dead code and is not a slow path to be
deleted. It is the independent reference that makes `GreedyCompiler` trustworthy,
and it is written deliberately without sharing the greedy mesher's slice
machinery. If you optimise geometry further, add the implementation *and* extend
`crates/engine_geometry/tests/oracle.rs`. An oracle that shares code with the
thing it validates is not an oracle.

The same holds for structure: occupancy connectivity is the **permanent
topological oracle**. `engine_destruction::fracture` records cracks as broken
bonds; recording a broken bond alone does not detach anything. DROP 0006.1's
fracture-aware classifier treats those bonds as cuts and lives *beside* the exact
occupancy classifier, with independent searches tested against each other.
Detachment still requires a settled result and atomic admission. Do not weaken
that invariant to make cracking easier.

**Rebuild locally.** Changing one cell must never re-mesh the world. If you touch
`World::set` or the dirty-tracking logic, remember that an edit on a volume
boundary affects the neighbour's geometry too, and that
`crates/engine_geometry/tests/incremental.rs` is what catches getting it wrong.

**Keep the spatial roles separate.** A 16³ *volume* is the storage leaf. A
*region* is the residency and persistence unit. A *render section* is what the
renderer is handed. They currently line up, and nothing may assume they always
will: go through `SectionGrid` rather than using a volume address as a section
address, and never reintroduce a type that is "a chunk" meaning all three at
once. Undoing that conflation is what DROP 0002 did; do not reintroduce it.

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

**Unknown is not empty.** A search, mesh, load path or analysis that reaches
data the engine does not have must say so — `Indeterminate`, `Inconclusive`,
`Deferred` — and never treat absent regions as air. Running out of budget is not
a conclusion either: conservative failure means geometry stays where it is until
the engine knows better.

**Inconclusive analysis never changes the world.** Detachment, collapse and any
later mechanical failure act only on a settled, current, complete answer.
Admission is atomic: a refusal leaves the world, the fragment store and the
identity sequence exactly as they were.

**No physical system is mandatory for authored world validity.** A floating
island, impossible castle or magical bridge is valid data and must not need a
fake infinite-strength material to stay up. Gravity, anchoring, connectivity
detachment, applied damage, fragment physics, material mechanics and structural
capacity are separable and individually optional. Support and anchoring are
never material properties.

**Current authorized scope (2026-10-02).** The user explicitly requested a large
integrated drop and removed the numbered-section review gates. Complete the
one-material destruction loop and its measured performance work together. Keep
the hard invariants, tests, deterministic benchmarks and fresh visual evidence;
do not stop merely because a numbered section ends. Unrelated engine features
and additional materials remain outside this request.

**Correctness before cleverness.** Build the exact implementation first, behind a
clean trait, and test it. Then optimise against it.

## Layout and dependency direction

See `docs/architecture.md` for the full graph. The arrows only ever point one
way, and these are the parts that are load-bearing:

```text
engine_core        <- engine_volume, engine_geometry
engine_volume      <- engine_world
engine_world       <- engine_destruction, engine_stream, engine_io
engine_destruction <- engine_io, engine_mechanics
```

`engine_geometry` depends only on `engine_core` and must stay that way: it reads
cells through the `CellSource` trait and so does not care how they are stored.

**Bevy and Avian never enter engine crates.** Engine data, editing, geometry,
destruction and mechanics are plain Rust that compiles and tests without a GPU,
a window or a display. A deliberate host/adapter crate that wraps the engine for
a particular backend is fine and is where that wiring belongs; what is forbidden
is an `engine_*` crate learning about a renderer or a physics backend.

Anything that builds Bevy is kept out of `default-members` so `cargo test` does
not build it. Keep it that way; a fast test loop is the point.

`engine_mechanics` is optional by construction. An empty mechanical registry and
a silent participation policy must behave exactly as the drop before it did, and
no mechanical quantity may be floating point — determinism in a solver is bought
by never introducing the problem.

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

If you change engine behaviour in a way that moves the stress counters,
regenerate with `cargo run --release -p engine_stress --bin stress -- --write`
and justify the diff in the commit message. If a refactor was supposed to change
nothing, the baseline not moving is the proof.

## Conventions

Tests are named as the sentence they assert
(`a_boundary_edit_dirties_the_touching_neighbour`), not `test_set_3`. Comments
explain *why* a non-obvious decision was made, not what the line does; several
existing comments record the reasoning behind a subtle invariant, and those are
worth preserving.
