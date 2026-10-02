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

---

# DROP 0005.7 — the conservative witness

Re-run of the same harness, now running the cheap path first as a host would.

| case | exact | exact time | witness | witness time | saving |
| --- | --- | ---: | --- | ---: | ---: |
| `column_8x1` | overloaded | 18 µs | need_exact | 10 µs | 2× |
| `slab_16x16x2` | satisfied | 17 ms | **proven** | 0.7 ms | 25× |
| `tower_8x8x16` | overloaded | 11 ms | need_exact | 0.9 ms | 11× |
| `block_16x16x8` | satisfied | 117 ms | **proven** | 1.7 ms | 68× |
| `wall_16x8` | satisfied | 163 ms | **proven** | 2.3 ms | 70× |
| `block_24x24x8` | satisfied | 1.41 s | **proven** | 4.0 ms | **352×** |

## The shape of the result

The saving grows with the structure — 2×, then 25×, then 352× — because the
witness is linear in cells while the exact solver is roughly quadratic. The
largest case is the one that mattered: 1.41 seconds becomes 4 milliseconds, and
the answer is a *proof* rather than an estimate.

Just as important is **which** cases the witness declined. It proved every sound
structure in the set and deferred only on the two the exact solver rejects. That
is the ideal division of labour: the expensive path runs when something is
genuinely suspicious, not on every healthy building in the world.

## Why this is safe

The witness never estimates. It constructs an actual routing of every cell's
weight down to an anchor, and `DefinitelySufficient` means that routing exists
and was exhibited — a feasible flow, which guarantees the exact solver agrees.
`the_witness_never_contradicts_the_exact_solver` asserts exactly that across the
scenario matrix, and `the_witness_never_declares_failure` asserts the other half:
no input, including the ones the exact solver rejects, can make this module
return anything readable as "overloaded".

Its incompleteness is deliberate and is tested as such. A cantilever is sound,
and the witness cannot see it: the sweep only pushes downward, so it finds
nothing beneath the overhanging cell and declines. The exact solver routes that
weight sideways and down. Being bad at finding routes costs a fallback; being
wrong about one would cost a building.
