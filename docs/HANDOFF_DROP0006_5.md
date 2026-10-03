# Handoff after DROP 0006.5 — state, evidence, and what is actually next

Planning input for the next coding session. Everything below was verified in a
Claude Code cloud Linux container (CPU only, no GPU) unless marked otherwise.

## Branch state

```
main                                    19ee256   DROP 0003
fracture-integration                    a81cdd6   DROP 0006 base (blasts, grab/throw, debris arena)
astra-demolition-wip                    a2c2c5e   preservation snapshot of Astra's uncommitted pass
claude/jolly-ride-rfr55c                a17a9e5   budget-exhaustion safety fix, standalone on a81cdd6
claude/demolition-finish                e4e113e   demolition acceptance + measurements
claude/drop-0006-5-local-fracture-index 410c819   CURRENT HEAD
```

Lineage of the live line:

```
a81cdd6 -> a2c2c5e -> 4da5060 -> e4e113e -> 79bfab1 -> 410c819
```

`a17a9e5` is a side branch; its semantics were hand-reconciled into `4da5060`,
so it does not need merging. Nothing was force-pushed.

**Start the next drop from `410c819`.**

## What was completed, in order

### 1. Budget-exhaustion safety fix (`4da5060`)

Astra's WIP guarded the classifier root loop's `break` on the last component
having deferred. Right intent, insufficient: an unoccupied or isolated root
closes conclusively and costs nothing, so the guard never fired, the loop walked
every remaining root on a spent budget, and the pass still came out entirely
conclusive. That is why
`empty_or_exactly_completed_roots_cannot_hide_budget_exhaustion_from_either_oracle`
failed.

Fixed in both classifiers: charge the root against the budget before looking at
it, defer when nothing is left to spend, and record unexamined roots as
`Deferred { PassBudgetSpent }` instead of dropping them. `cells_visited` now
never exceeds the limit. The two classifiers keep independent bookkeeping, as
`CLAUDE.md` requires.

Deliberately *not* asserted: that no component may be `Detached` inside an
unsettled pass. A fully closed component really is detached; the safety property
is that the *pass* is unsettled, which is where every caller already gates
(`fracture_transaction`, `jobs`, `sim_lab`). Took the demolition suite 10/11 to
11/11.

### 2. Demolition acceptance and measurements (`e4e113e`)

Both replays driven through the real host headlessly;
`tools/verify-demolition-replay.py` passes on both branches. Mass closes at
exactly 1,811 in all ten building dumps. Capacity **off** leaves 1,803 cells
static and zero fragments after both cuts; capacity **on** fails 8 further cells
in one generation for one native 682-cell fragment that keeps all 682 cells
through fall, grab and the 180-step pull; throw adds real collision cracks; the
blast leaves 4 pieces holding 681 of 682; systems-off reproduces structural
state and fracture checksum byte-identically (`f4546c6026930b5a`).

`MEASUREMENTS_PENDING` replaced with real numbers. Engine cost without a
renderer: capture 0.014 ms median, worker 3.164 ms, commit 0.106 ms — analysis
costs about 30x the on-frame commit, which is the architectural claim, measured.

### 3. DROP 0006.5 local fracture indexing (`79bfab1`, `410c819`)

`capture` bounded its world copy to <=27 regions and then copied *every* static
fracture record in the world. Now `snapshot_regions` collects only what those
regions can reach, backed by a region index that lives inside one private type
with the authoritative maps, so it cannot drift.

Two spatial questions, answered differently on purpose: persistence asks who
*saves* a record (exactly one region — a bond by its lower cell — or
extract/restore duplicates or drops it); analysis asks who can *read* one (a
boundary bond is readable from both sides). Fragment-local records are not
indexed; they are owned by exact `FragmentId` and already contiguous.

Measured: **7,813x more damage history costs 2.2x more capture time** (0.0389 ->
0.0863 ms) with the snapshot provably constant at 128 records. Before this, that
rung would have been `JobRefusal::SnapshotBudget` against the default 32,768
ceiling — unrelated history was a denial of service against local destruction.

## Evidence that currently holds

```
cargo test (workspace)                           757 passed, 0 failed
clippy --workspace --all-targets -- -D warnings  exit 0, zero warnings (sandbox incl.)
cargo fmt --all --check / git diff --check       PASS
destruction_bench  --check                       committed fixtures current
destruction_replay --check                       committed fixtures current and valid
interaction_bench                                all 8 checksums unchanged
verify-demolition-replay.py                      PASS, both branches
demolition suite                                 11 passed, 0 failed
```

Canonical demolition evidence: `building 1811 roof 682 fragments 1 failures 16
checksum 4fa4fc10f6b1ba55`. Coherent chunk: 60 of 60 cells retained, 32 pieces.

No committed benchmark result was regenerated at any point.

## Open weaknesses, roughly in priority order

1. **Chunk-size distribution is still suspicious.** The 60-cell canonical chunk
   retains all 60 cells but splits into **32 pieces**. Mass-preserving but
   probably too fragmentary. This is the intended next drop (0006.6).
2. **The stress replay is not a deterministic fixture.** Three runs of the same
   fixture on one unchanged binary gave fragment counts 515 / 559 / 419. Cause
   is in the dumps: contact sampling sheds opportunistic samples at a rate set
   by frame cadence (230 enqueued / 57 capped versus 180 / 76). Mass, the
   authoritative queue's `refused=0 capped=0`, and `stress_18 == stress_off`
   are invariant every time. **Do not read a changed stress checksum as a
   regression signal.** Fixing it means driving contact sampling off the fixed
   step rather than frame cadence — a scenario change, deliberately out of
   0006.5's scope, and a good candidate to fold into 0006.7.
3. **Index memory overhead is ~83% of the record payload.** 40-byte keys against
   48-byte records; 38.15 MiB of keys at 1M records. The cheap fix — ordering the
   authoritative keys volume-major and deleting the second structure — is blocked
   because `FractureState::digest` iterates in key order, so it would move every
   committed fracture checksum. Unblocking it means canonicalising the digest
   independently of storage order first.
4. **`prune_against` still scans the whole state.** By contract, and
   `fracture_ownership.rs:331` asserts `records_examined == 2` exactly, so
   narrowing it changes observable behaviour. Hosts that know their extent
   should call `forget_removed`, which is already local.
5. **The 2.2x capture residual is unattributed.** Small in absolute terms
   (<0.1 ms). The `index_regions` explanation in the doc is a design claim, not
   a measurement.
6. **Windows/GPU work is entirely pending**: frame profile p50/p95/p99/max,
   frames over 33.3 ms, the GPU-frame response latency that would actually test
   the bounded multiple-poll change, and all visual evidence. The 544-fragment
   observation that motivated the poll change (response p95 200.93 ms, p99
   284.18 ms, max 319.63 ms) has never been reproduced or refuted.
7. `forget_space(StaticWorld)` is whole-space by definition; the `FragmentLocal`
   case that actually occurs is bounded.

## Environment notes for whoever runs the next session

A cloud container can do more than expected, but not everything:

* **Bevy needs system deps installed first**, or `cargo clippy -p sandbox` fails
  in a build script: `apt-get update && apt-get install -y libwayland-dev
  libxkbcommon-dev libxkbcommon-x11-0 libasound2-dev libudev-dev`.
* **Headless replay works.** Xvfb plus Mesa lavapipe gives software Vulkan:
  `Xvfb :99`, `DISPLAY=:99`,
  `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json`, then
  `MICROLOGY_REPLAY_SCRIPT=<fixture> MICROLOGY_REPLAY_CAPTURE=1 ./target/debug/sandbox`.
  `MICROLOGY_REPLAY_CAPTURE` auto-advances the replay on a 2s timer, so no
  keypresses are needed. Real engine dumps come out; **frame timings from this
  path are meaningless** and queue-wait/response are inflated by slow frames.
* **Windows capture is impossible here**: no PowerShell, no `ddagrab`/`gdigrab`
  in this ffmpeg, no `/dev/dri`. `tools/capture-demo.ps1 -CheckOnly` cannot run.
* **Watch disk.** The debug sandbox binary is ~2.1 GB and `target/` reached
  15 GB; disk hit 95% mid-session. Avoid a second target directory, and prune
  stale git worktrees.
* Measure response latency separately from FPS. FPS alone is not the acceptance
  metric — that was the original 544-fragment lesson.

## Artefacts to read before planning

```
docs/drop-0006-5.md                                     design, methodology, limits
docs/integrated-demolition-drop.md                      demolition drop, now with real measurements
docs/diagnostics/drop-0006-5-locality.json              scaling ladder raw output
docs/diagnostics/drop-0006-5-replay-reverification.json the three-run non-determinism evidence
docs/diagnostics/demolition-engine-cpu.json             renderer-free engine costs
docs/diagnostics/demolition-replay-evidence.json        distilled dump evidence
crates/engine_stress/src/bin/fracture_locality_bench.rs the scaling benchmark
crates/engine_destruction/tests/fracture_locality.rs    streaming + fragment acceptance
```

## Suggested next drop, and the decision it needs

The stated plan was 0006.6 chunk-size distribution / fracture hierarchy, then
0006.7 collision and mesh/collider rebuild spike control.

One question worth settling before 0006.6 starts: **what is the target chunk-size
distribution, and how is it judged?** "32 pieces is too many" is a visual
impression, and the lesson from this drop's measurement work is that a tuning
target without a deterministic metric turns into taste. 0006.6 probably needs a
measured distribution statistic — piece-count and piece-size histogram per
reference impact, checked into a fixture the way `benchmark-pack.json` is —
before any tuning, or there will be no way to tell improvement from change. The
chunk fixture is already deterministic (unlike the stress replay), so this is
cheap to do properly.

Still one homogeneous material. No multi-material tuning, fluids, fire, joints
or EarthForge integration yet.
