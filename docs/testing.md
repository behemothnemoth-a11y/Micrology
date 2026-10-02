# Testing

```sh
cargo test                                             # the engine suite, ~1s, no GPU
cargo test -p engine_geometry --test oracle            # just the equivalence checks
cargo clippy --workspace --exclude sandbox --all-targets -- -D warnings
```

The sandbox is outside `default-members`, so the common commands never build Bevy.
A fast test loop is the point; keep it that way.

## Where things live

| file | covers |
|------|--------|
| `crates/engine_core/src/coords.rs` | coordinate splitting across the origin, index round-trips, face bases |
| `crates/engine_volume/src/lib.rs` | storage tier promotion/demotion, occupancy counting, palette canonicalisation, run encoding |
| `crates/engine_world/src/lib.rs` | sparse volumes, dirty tracking including boundary neighbours, compaction |
| `crates/engine_geometry/tests/surface.rs` | the scope's fixture cases, winding, mesh conversion |
| `crates/engine_geometry/tests/oracle.rs` | greedy-vs-exact equivalence, determinism |
| `crates/engine_geometry/tests/incremental.rs` | edit → dirty → local rebuild; volume/section separation |
| `crates/engine_destruction/tests/fracture.rs` | the destruction wall under one repeated impact: sparse fracture state, bond canonicalisation, determinism, and every conservative path |
| `crates/engine_destruction/tests/fracture_connectivity.rs` | fracture-aware connectivity against the occupancy oracle on seven shapes, and crack-driven separation through the existing detach transaction |
| `crates/engine_io/tests/persistence.rs` | round trips, byte determinism, version handling, format stability |
| `crates/engine_stress/tests/baseline.rs` | stress-scenario counters against the committed baseline |

## The scope's required cases

Every case `03_DROP_0001_SCOPE.md` lists, and where it is:

| case | test |
|------|------|
| empty volume | `an_empty_volume_compiles_to_nothing` |
| one occupied cell | `a_single_cell_emits_six_faces` |
| solid volume | `a_solid_volume_emits_no_internal_faces` |
| two adjacent cells | `two_adjacent_cells_drop_their_shared_faces` |
| hollow cavity | `a_hollow_cavity_is_surfaced_from_the_inside` |
| mixed-material boundary | `a_material_boundary_blocks_merging_but_not_occlusion` |
| cells crossing a volume boundary | `an_object_spanning_a_volume_boundary_has_a_continuous_surface` |
| edit → dirty mark → rebuild | `an_interior_edit_rebuilds_exactly_one_section` |
| save → reload exact equality | `save_and_reload_are_exactly_equal` |
| independent topology oracle preserved | all of `tests/oracle.rs` |

## The three habits worth keeping

**Assert against the oracle, not against a number you recorded.** The greedy
mesher is checked by expanding its merged quads back into unit faces and comparing
the set with what `ExactCompiler` produced — over seeded pseudo-random worlds at
densities from 1% to 100%, with 3 materials and with 64, and around negative
coordinates. A hand-copied expected quad count proves only that the output has not
changed; the oracle proves it is *right*.

**Name the test as the sentence it asserts.** `a_boundary_edit_dirties_the_touching_neighbour`
says what broke when it fails. `test_set_3` does not.

**Seed anything random.** `tests/common/mod.rs` has a SplitMix64 `Rng` with an
explicit seed. A failure must be replayable; a flaky geometry test is worse than
no test. There is no `rand` dependency and there should not be one.

## Adding a case

A new geometry case belongs in `tests/surface.rs` if it is a specific shape, and in
`tests/oracle.rs` if it is a class of shapes. For a shape, prefer asserting a count
you derived by hand *and* oracle agreement — the count catches a silent change, the
oracle catches a wrong one:

```rust
#[test]
fn an_l_shaped_bar_keeps_its_inner_corner() {
    let mut w = world();
    w.fill_box(CellPos::new(0, 0, 0), CellPos::new(3, 0, 0), Some(STONE));
    w.fill_box(CellPos::new(0, 1, 0), CellPos::new(0, 3, 0), Some(STONE));

    let e = exact(&w, VolumePos::ZERO);
    let g = greedy(&w, VolumePos::ZERO);
    assert_eq!(g.unit_faces(), e.unit_faces());
    assert_eq!(e.stats.exposed_faces, 7 * 6 - 2 * 2);
}
```

If you add a meshing optimisation, the oracle test is not optional — it is the only
reason to trust the optimisation.

## What is not tested

Honest gaps, so nobody mistakes green for complete:

- **The sandbox app has no automated coverage.** No window opens in CI; the
  `sandbox` job only type-checks and lints it. The app has been run and observed
  once (see `drop-0001.md`), but nothing re-checks it. A headless smoke test under
  Xvfb with a software Vulkan driver is feasible — that is how the first run was
  done — and would be worth adding before the renderer grows.
- **`raycast` in `apps/sandbox/src/edit.rs` is untested.** It is the one piece of
  real algorithmic work outside the engine crates, and it should move into an
  engine crate with proper tests — it is named as the next step in
  `drop-0001.md`.
- **No performance *budgets*.** The stress harness records timings but nothing
  asserts them, by design — hardware timings are diagnostic, not correctness. What
  CI does assert is the deterministic counters.
- **No GPU is tested.** Everything here runs headless. The one question 0002.11
  could not answer — what draw-call count actually costs — needs real hardware,
  which is why `SectionGrid` stayed configurable instead of being chosen.
- **The renderer is not tested.** `apps/sandbox` has no tests; it is verified by
  running it and looking, which is recorded in `docs/drop-0002.md` with
  screenshots and HUD readings rather than asserted in CI.

## The stress harness

```sh
cargo run --release -p engine_stress --bin stress            # measure and print
cargo run --release -p engine_stress --bin stress -- --check # compare to baseline
cargo run --release -p engine_stress --bin stress -- --write # regenerate it
```

Seven deterministic scenarios, measured on counters that are identical on every
machine. Those counters are committed to `fixtures/stress/baseline.json` and
asserted by `cargo test`; a diff means engine behaviour changed and should be
reviewed, not waved through.

Timings are printed but never committed and never asserted. They vary with
hardware, and a value that varies between runs must not live inside anything
compared for equality — the same reasoning that removed build time from
`CompileStats`.

Scenarios exist to stop one friendly benchmark standing in for "performance".
`dense_solid` is the best case for merging; `checker` is the case where merging
can achieve literally nothing; `material_stress` blocks merging without changing
the shape; `boundary_storm` is the one that forces neighbouring sections to
rebuild. When a change makes `dense_solid` faster, check what it did to `checker`.

## DROP 0006 destruction benchmark pack

The permanent homogeneous destruction wall and its ordered acceptance protocol
live in `engine_stress::destruction_benchmark`, with the committed canonical spec
at `fixtures/destruction/benchmark-pack.json`. This pack contains no fracture
implementation; it exists so fracture algorithms are judged against a stable
fixture rather than moving the wall every time behavior changes.

```sh
cargo run -p engine_stress --bin destruction_bench
cargo run -p engine_stress --bin destruction_bench -- --check
cargo run -p engine_stress --bin destruction_bench -- --template
```

Fracture-specific result fields use `null` until an implementation can actually
measure them. Never rewrite missing data as zero. See
`docs/drop-0006-destruction-benchmarks.md` for the eight ordered cases and their
acceptance rules.

## The other harness tools

```sh
# Renderer granularity: every scenario at 1, 2 and 4 volumes per section edge.
cargo run --release -p engine_stress --bin granularity

# A deterministic world far larger than memory, written a region at a time.
cargo run --release -p engine_stress --bin worldgen -- /tmp/world --regions 12

# What one region of real terrain costs to read and write.
cargo run --release -p engine_stress --bin regionio -- /tmp/world
```

None of these commit anything. They answer a question when it is being asked:
whether to coarsen render sections, whether streaming holds up at scale, whether
region I/O belongs on the main thread. Their findings live in
`docs/drop-0002-report.md`, with the command that produced each number.

A generated world is never committed — one big enough to be interesting is far
too big for a repository.

## The torture suite

`crates/engine_stream/tests/torture.rs` covers the orderings that only happen
when a player is unlucky: a save landing after the edit it should have captured,
a mesh job returning long after the world moved on, a region evicted mid-flight,
storage that simply fails.

**Every ordering is forced, never raced.** A test that reproduces a bug one run in
fifty is not a test, so the host is a `BTreeMap` and completion order is a seeded
permutation that replays identically on any machine.

Two traps it is worth knowing about, because both produced a false failure first:

- **The fake disk must model real storage.** `dirty` means "differs from what is
  on disk" — an in-memory property that storage has no concept of. A `BTreeMap`
  of cloned `Region`s carries the flag across the round trip and makes a reloaded
  region read as unsaved, which `v2::load_region` never does.
- **A fixture has to weigh something.** A uniform volume costs *four bytes* — one
  palette entry, no cell array — so a hundred and seventy-five of them will not
  trouble any budget. Give each volume a second material and it lands in the
  indexed tier at 8,200 bytes. Region count is not memory.
