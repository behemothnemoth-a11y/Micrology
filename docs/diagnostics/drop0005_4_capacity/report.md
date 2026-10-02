# DROP 0005.4 — exact capacity network cost

Harness: `cargo run --release -p engine_stress --bin capacity`.
Counters in `report.json` are deterministic and committed. The timings below are
diagnostic, vary with the machine, and are not committed.

## The question this answers

Decision C in `docs/drop-0005-scope.md` chose an exact per-cell integer flow
network and named its risk in the same breath: flow is far more expensive than
connectivity, and a coarsened per-component fallback was to be decided **on
measured evidence rather than anticipation**. This is that evidence.

## Measurements

| case | outcome | cells | connections | augmentations | time |
| --- | --- | ---: | ---: | ---: | ---: |
| `column_8x1` | overloaded | 9 | 16 | 6 | 20 µs |
| `slab_16x16x2` | satisfied | 768 | 3,904 | 768 | 11 ms |
| `tower_8x8x16` | overloaded | 1,088 | 5,856 | 384 | 9 ms |
| `wall_16x8` | satisfied | 2,304 | 12,736 | 2,304 | 122 ms |
| `block_16x16x8` | satisfied | 2,304 | 12,736 | 2,304 | 115 ms |
| `block_24x24x8` | satisfied | 5,184 | 29,088 | 5,184 | 1.31 s |

## Finding: the exact per-cell network cannot be the live path

Going from 2,304 to 5,184 cells — 2.25× the geometry — costs **11× the time**.
That is the expected shape: augmentations scale with cell count because each
cell's weight is its own unit of demand, and each augmenting search scans the
whole edge set, so the solve is roughly quadratic in cells.

The absolute numbers settle it. `block_24x24x8` is a 24×24 footprint eight cells
tall — a small building, not a landscape — and it takes **1.3 seconds**. A
structure a player would actually build is far larger, and this is the *exact*
solver answering one question once.

So the fallback named in decision C is now chosen, on evidence: **a coarsened
network is required, not optional.** What changes is less than it sounds:

- The exact solver stays exactly as it is, as the **oracle**. It is simple
  enough to be obviously correct, which is the property that makes it worth
  keeping, and the engine already works this way — `ExactCompiler` is permanent
  beside `GreedyCompiler` for the same reason.
- 0005.7 is therefore **load-bearing rather than optional**. It was scoped as an
  experiment that could legitimately ship a measured negative result, as 0004.5
  did. It cannot be that any more: without an accelerated path there is no live
  capacity analysis.
- The integer formulation does not change. Coarsening aggregates demand and
  capacity over connected components; it does not reintroduce floating point,
  and the accelerated path still may only answer "definitely sufficient" or
  "ask the exact one", per the `support_witness` precedent.

## What is already right

The bounds work, which is why this is a finding rather than a hazard. A host
that sets `CapacityLimits` gets `Deferred` instead of a 1.3-second stall, and
`Deferred` is not a verdict — it cannot fail a cell or collapse a structure. The
measurements above were taken with `CapacityLimits::UNLIMITED` precisely to find
the ceiling; no live path should ever run that way.

Two further properties hold at every size measured: repeated solves are
identical, and the answer does not depend on which cell the caller asked about.
