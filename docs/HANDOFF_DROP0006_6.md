# Handoff after DROP 0006.6 — state, evidence, and what is actually next

Planning input for the next coding session. Everything below was verified in a
Claude Code cloud Linux container (CPU only, no GPU) unless marked otherwise.
This supersedes `docs/HANDOFF_DROP0006_5.md`, whose open items are carried
forward below rather than repeated.

## Branch state

```
main                                    19ee256   DROP 0003
fracture-integration                    a81cdd6   DROP 0006 base
astra-demolition-wip                    a2c2c5e   preservation snapshot of Astra's pass
claude/jolly-ride-rfr55c                a17a9e5   budget fix, side branch, already reconciled
claude/demolition-finish                e4e113e   demolition acceptance + measurements
claude/drop-0006-5-local-fracture-index b605009   local fracture indexing
claude/drop-0006-6-coherent-fragments   14fb6cc   CURRENT HEAD
```

Live lineage:

```
a81cdd6 -> a2c2c5e -> 4da5060 -> e4e113e -> 79bfab1 -> 410c819 -> b605009 -> 48868bd -> 14fb6cc
```

**Start the next drop from `14fb6cc`.** Nothing has been force-pushed. Other
branches on the remote (`continuous-visual-replay`, `video/*`, `diagnostic/*`,
the `drop-0003-*` series) are earlier or side work and are not in this lineage.

## What DROP 0006.6 did

Two commits: `48868bd` measured, `14fb6cc` changed.

### Phase 1 — the baseline instrument

`crates/engine_stress/src/fragment_distribution.rs` and
`fragment_distribution_bench` measure how a break distributes material. The
bench runs each case twice and asserts the measurements are identical, so the
fixture proves its own determinism rather than assuming it. `--check` against
`fixtures/destruction/fragment-distribution.json` makes a changed distribution a
reviewed diff.

What the accepted implementation did, at 2,500 energy: 0 cells failed, 44 bonds
affected, **32 fragments**, 16 of them singletons, mean 1.88 cells, largest 3
cells holding 5.0% of the mass — and `occupancy_groups = 1`, meaning all 32
pieces' cells form a single face-connected lump. The material was cut apart by
cracks and was never separated in space.

### Phase 2/3 — the partitioning layer

`engine_destruction::fragment_partition` sits between fracture-aware topology
and `Fragment` creation. It takes `&[Component]` and a policy and returns groups
of cells. It is handed no `World`, no `FractureState`, no store, no residency and
no bond gate, so it cannot compute connectivity, consult damage history, or scan
anything global — DROP 0006.5's locality is safe here by construction.

The policy is `FragmentPartitionPolicy`, with the threshold derived from the
mean component size **of this break** and nothing else. The shipped default is
`PerCrackComponent`: the pre-0006.6 behaviour, unchanged.

### The two findings that shaped it

**Crack retention inverts the obvious worry about grouping.**
`FractureState::remap_fragment` drops any bond whose endpoints land in different
children, because fragment coordinates are per-fragment and such a bond has
nowhere to live. So splitting into 32 fragments retains **zero** separating
cracks, while grouping into 16 retains **twenty**. Grouping keeps *more*
information than splitting, and a merged group is cracked rather than welded:
the bond record still says broken, the fracture-aware classifier still sees two
components inside the fragment, and a later impact still separates them along
the same surface.

**Per-cut strength signals cannot work in this model.** Two fracture-aware
components are separate precisely because *every* face between them is a broken
bond, so every cut is identical by that measure. This was tried first and
discarded on the evidence. The scale has to come from the break's own size
distribution.

## Measured variants (2,500 energy)

Damage topology identical in every row — cells failed 0, occupancy groups 1,
same cells surviving. Only grouping differs.

| policy | fragments | singletons | mean | largest share | cracks retained |
|---|---|---|---|---|---|
| per_crack_component (default) | 32 | 16 | 1.88 | 5.0% | 0 |
| coherent_half_mean | 32 | 16 | 1.88 | 5.0% | 0 |
| **coherent_mean** | **16** | **0** | **3.75** | **18.3%** | **20** |
| coherent_twice_mean | 1 | 0 | 60.00 | 100% | 72 |

The full table across three energies is in `docs/drop-0006-6.md` and the raw
output in `docs/diagnostics/drop-0006-6-fragment-distribution.json`.

## Evidence that currently holds

```
cargo test                                      769 passed, 0 failed
clippy --workspace --all-targets -- -D warnings exit 0, zero warnings (sandbox incl.)
cargo fmt --all --check / git diff --check      PASS
destruction_bench  --check                      committed fixtures current
destruction_replay --check                      committed fixtures current
fragment_distribution_bench --check             current
fracture_locality_bench                         snapshot constant at 128 records
demolition suite                                11 passed
```

Canonical demolition evidence, unchanged through both drops: `building 1811 roof
682 fragments 1 failures 16 checksum 4fa4fc10f6b1ba55`, chunk 60 of 60 cells in
32 pieces.

**No accepted checksum has moved in either drop.** The regenerated distribution
baseline's `per_crack_component` rows are numerically identical to the pre-drop
file on every field including both checksums; the file grew only by variant rows.

## Decisions the next session should make first

1. **Should the default partition policy change?** `coherent_mean` removes every
   singleton, halves the fragment count, triples the largest-fragment share and
   retains 20 cracks that are currently thrown away — with damage topology
   untouched. It was *not* made the default because that is a destruction
   character decision and the brief said character must not change merely to
   look better. The table is the input to that decision. This is a one-line
   change to `FragmentPartitionPolicy::default()` plus regenerating the
   distribution baseline, and it will change the visible character of contact
   destruction.
2. **Mean or median for the scale?** The mean is tail-sensitive: one large
   component among many tiny ones raises the threshold and merges more
   aggressively than a median would. Untested, and cheap to test with the
   existing bench.
3. **Should the static path be wired?** `fracture_static_if` still routes
   through `cracked_structure_result` and `detach_if` — one fragment per
   occupancy component — and is untouched because **its fragmentation has never
   been measured**. Measure it with the Phase 1 instrument before changing it;
   the demolition building's roof release already produces one large fragment,
   so a new fixture is probably needed.

## Open weaknesses, in priority order

### New in 0006.6

* **One fixture.** The canonical 60-cell chunk at three energies. Nothing else
  exercises the partitioner.
* **Merging is greedy, not minimal-cut.** Each under-scale group joins its
  largest neighbour, iterated, deterministic and bounded — not claimed optimal.
* **Shape metrics are measured but unused.** Aspect ratio and
  surface-to-volume are reported and do not influence grouping, so the policy
  can produce a coherent mass that is a poor shape.
* **`coherent_twice_mean` degenerates to a single fragment.** Useful as a table
  upper bound, not a usable policy.
* **Partition time is not isolated.** The bench reports analysis-and-partition
  together (~0.6–1.0 ms for 60 cells on this container). No threshold asserted.

### Carried forward from 0006.5, still open

* **The stress replay is not a deterministic fixture.** Three runs of one
  unchanged binary gave fragment counts 515 / 559 / 419, because contact
  sampling sheds opportunistic samples at a rate set by frame cadence. Mass, the
  authoritative queue's `refused=0 capped=0`, and `stress_18 == stress_off` are
  invariant every time. **Do not read a changed stress checksum as a regression
  signal.** Use the deterministic fixtures. Fixing it means driving contact
  sampling off the fixed step rather than frame cadence — a scenario change, and
  a natural fold into 0006.7.
* **Fracture index memory is ~83% of the record payload** — 40-byte keys against
  48-byte records, 38 MiB at a million records. The cheap fix (volume-major keys,
  no second structure) is blocked because `FractureState::digest` iterates in key
  order, so it would move every committed fracture checksum. Unblocking means
  canonicalising the digest independently of storage order *first*, as an
  isolated change with its own equivalence proof.
* **`prune_against` still scans the whole state**, by contract, and
  `fracture_ownership.rs:331` asserts `records_examined == 2` exactly, so
  narrowing it changes observable behaviour. Hosts that know their extent should
  call `forget_removed`.
* **`forget_space(StaticWorld)`** is whole-space by definition; the
  `FragmentLocal` case that occurs is bounded.
* **The 2.2x capture residual is unattributed.** Under 0.1 ms absolute. The
  `index_regions` explanation in the doc is a design claim, not a measurement.
* **Windows/GPU work is entirely pending**: frame profile p50/p95/p99/max,
  frames over 33.3 ms, and the GPU-frame response latency that would actually
  test the bounded multiple-poll change. The 544-fragment observation that
  motivated it (response p95 200.93 ms, p99 284.18 ms, max 319.63 ms) has never
  been reproduced or refuted.

## Environment notes

* **Bevy needs system deps installed first**, or `cargo clippy -p sandbox` fails
  in a build script: `apt-get update && apt-get install -y libwayland-dev
  libxkbcommon-dev libxkbcommon-x11-0 libasound2-dev libudev-dev`.
* **Headless replay works.** Xvfb plus Mesa lavapipe gives software Vulkan:
  `Xvfb :99`, `DISPLAY=:99`,
  `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json`, then
  `MICROLOGY_REPLAY_SCRIPT=<fixture> MICROLOGY_REPLAY_CAPTURE=1 ./target/debug/sandbox`.
  `MICROLOGY_REPLAY_CAPTURE` auto-advances on a 2s timer, so no keypresses are
  needed. Real engine dumps come out; **frame timings from this path are
  meaningless** and queue-wait/response are inflated by slow frames.
* **Windows capture is impossible here**: no PowerShell, no `ddagrab`/`gdigrab`
  in this ffmpeg, no `/dev/dri`.
* **Watch disk.** The debug sandbox binary is ~2.1 GB and `target/` reaches
  15 GB; this session hit 95% and needed a stale worktree pruned. Avoid a second
  target directory, or clean it up afterwards.
* A release build can fail where a debug check passed: a `#[cfg(debug_assertions)]`
  function referenced inside `debug_assert!` still type-checks in release. Bit
  this drop once.

## Artefacts to read before planning

```
docs/drop-0006-6.md                                     this drop: baseline, variants, limits
docs/drop-0006-5.md                                     indexing design and limits
docs/integrated-demolition-drop.md                      demolition drop with real measurements
docs/diagnostics/drop-0006-6-fragment-distribution.json variant raw output
docs/diagnostics/drop-0006-5-locality.json              locality ladder
docs/diagnostics/drop-0006-5-replay-reverification.json the three-run non-determinism evidence
crates/engine_destruction/src/fragment_partition.rs     the layer, and why it is not an authority
crates/engine_destruction/tests/fragment_partition.rs   pure-layer acceptance
crates/engine_stress/tests/fragment_partition.rs        transaction-level acceptance
crates/engine_stress/src/fragment_distribution.rs       the measurement definitions
```

## Suggested next drop

The stated plan is 0006.7 — collision and mesh/collider rebuild spike control.
Two things make it a good fit now: it is the remaining piece of the original
0006.x sequence, and the stress replay's frame-cadence non-determinism belongs
there, since both are about work that is currently paced by how fast frames
retire rather than by the fixed step.

If 0006.7 is taken, decide items 1 and 2 above first — they are cheap, they are
already measured, and leaving the partitioner at its pre-0006.6 default means
the drop's benefit is available but not switched on.

Still one homogeneous material. No multi-material tuning, fluids, fire, joints
or EarthForge integration yet, and no new video.
