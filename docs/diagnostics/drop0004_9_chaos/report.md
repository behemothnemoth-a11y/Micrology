# DROP 0004.9 — Chaos pass acceptance

Section status: **chaos pass complete; hardware/visual acceptance outstanding**

Scope accepted here: the adversarial sweep over the complete DROP 0004 system.
The GPU capture half of 0004.9 has not been run (see *Outstanding* below). DROP
0005 was not started.

Harness: `cargo run -p engine_stress --bin chaos4`, which writes `report.json`
beside this file and exits non-zero if any case fails.

## Chaos Pass rule

Every destructive case runs against isolated disposable state under the system
temp directory. The live target is digested file-by-file before the run and
re-verified afterwards; the run reports `integrity_verified` separately from
case results and `all_ok` requires both. The `isolation_and_integrity` case
proves the harness itself obeys this: it builds a pristine world plus persisted
fragment store, digests it, copies it, deletes every persisted fragment and
wipes the entire world **in the copy**, confirms the copy diverged, and then
re-verifies that the live target is byte-identical.

## Cases

All **14 / 14** passed, integrity verified.

| Case | What it attacks |
| --- | --- |
| `population_beyond_residency` | 512 persisted fragments, 352 in the busiest region, 4-load cap |
| `travel_away_and_back` | moving vs sleeping fragments across eviction and return |
| `dirty_eviction_during_motion` | a write in flight while the fragment keeps moving |
| `incremental_persistence_crash_windows` | index-first deletion, orphan payloads, pruning |
| `structural_demand_at_boundary` | support across an absent region |
| `many_simultaneous_damage_events` | 256 events in one transaction, sparse-entry budget |
| `repeated_subthreshold_damage` | weak hits accumulating to exactly one failure |
| `radial_event_at_integer_limits` | 1e18 radius, and a sphere centred on `i32::MAX` |
| `refracture_storm` | 63 consecutive re-fractures from one parent |
| `secondary_recursion_exhaustion` | queue, per-step and generation ceilings |
| `stale_derived_results` | motion vs geometry change vs re-fracture |
| `deterministic_replay` | 24 identical runs of the whole loop |
| `fantasy_mode_structural_disabled` | an unanchored floating island |
| `isolation_and_integrity` | the Chaos Pass rule itself |

## Findings worth keeping

**The sparse-entry budget bounds storage, not work.** A cell that crosses its
failure threshold is removed from the sparse map in the same transaction, so it
never occupies an entry. 256 simultaneous above-threshold events therefore
complete under an entry limit of 4, while the same 256 events at sub-threshold
amounts are refused atomically and leave the store empty. This is correct and
deliberate, but it means "entry limit" is not a limit on event volume. The first
draft of this harness asserted the opposite and failed; the expectation was
wrong, not the engine.

**Cell conservation across a re-fracture storm.** 63 successive middle-cell
failures on a 129-cell parent produced 64 live fragments with unique ids, no
empty fragments, and `66 live + 63 removed = 129`. The destruction sequence
advanced monotonically to 1064 with no identity collisions.

**Recursion terminates on generation, not exhaustion.** A self-feeding secondary
chain stopped after exactly 3 generations under `max_generation = 3`, independent
of queue space. Pending and per-step ceilings were separately confirmed (8
accepted / 56 refused, drained 2 per step in enqueue order).

**Fantasy mode holds.** An unanchored floating island of 320 cells round-tripped
through save/load, took local damage (31 cells failed) and stayed exactly where
it was authored. Opt-in structural analysis confirms the island *would* detach as
one component if a host asked — the point is that no host is obliged to ask. No
physical system is mandatory for world validity.

**`unknown != empty` still holds under demand.** A structure whose support lies
across an absent region returned `Inconclusive` with 1 required region, and
`detach_if` refused with `Err(Inconclusive)` leaving both the world and the
destruction sequence untouched.

## Acceptance evidence

- chaos4: **14 / 14** cases, `integrity_verified: true`, exit 0
- DROP 0003 chaos regression (`--bin chaos`): **9 / 9**, still green
- full engine regression: **518 passed, 0 failed**
- `cargo clippy -p engine_stress --all-targets -- -D warnings`: clean
- formatter clean
- `report.json` is byte-identical across repeated runs

The harness is demonstrably not vacuous: three defects in its own assertions
(a wrong budget expectation, an uncrowded residency region, and a tautological
fantasy-mode comparison) were caught and reported as failures before being
corrected, and `clippy` rejected a fourth assertion that was always true.

## Outstanding

The hardware-acceptance half of 0004.9 has **not** been run. It needs a real
Windows/NVIDIA/Vulkan session with `ddagrab` capture, which this headless Linux
environment cannot provide. Nothing in the chaos pass depends on it, and no claim
above rests on a GPU run.
