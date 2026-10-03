# Claude Handoff — Micrology after DROP 0004.8

Prepared after a Claude context reset.

## Resume point

- Repository: `behemothnemoth-a11y/Micrology`
- Active branch: `drop-0004-destruction-scale-damage`
- Pushed head before this handoff: **`78990e7 — 0004.8: complete impact coupling and re-fracture`**
- Isolated worktree: `C:\Users\behem\.chatgpt-worktrees\Micrology-drop4`
- DROP 4 narrative: `docs/drop-0004.md`
- Locked scope: `docs/drop-0004-scope.md`

**Do not work in the original local clone.**  
`C:\Users\behem\Projects\Micrology` is intentionally dirty and behind remote main. At handoff time it has modified/untracked DROP-3-era sandbox files. Leave it untouched. Use the isolated DROP-4 worktree.

## Project identity

Micrology is a standalone reusable volumetric **game engine / game-development environment**. It is not one specific game and is not a Minecraft mod.

Hard invariants:
- cells are engine data, never ECS entities or one rigid body per cell;
- one connected detached mass becomes one native volumetric `Fragment`;
- support/anchoring is separate from `Material`;
- Bevy/Avian remain host-side;
- unknown/unloaded space is **not empty**;
- budget exhaustion is **not detachment**;
- async results validate staleness before mutation;
- fragment admission/refusal is atomic;
- exact/reference implementations remain as oracles;
- byte budgets and work budgets stay separate;
- visible ordering and identity remain deterministic;
- no core physical system is mandatory for authored world validity.

Gravity, connectivity, damage, fragment physics, material mechanics and structural stress are separate participation choices. Fantasy structures such as floating islands must remain valid. Structural stress must later be switchable without fake infinite-strength materials.

## User workflow

The user explicitly wants **one DROP section at a time**.

For each section:
1. implement only that section;
2. run focused tests and relevant regression/Clippy/format checks;
3. update docs/status;
4. make a short **fresh** visual;
5. stop and report;
6. do not automatically start the next section.

Do not attempt a whole DROP in one pass.

## DROP 0004 status

- 0004.0 scope/fantasy-physics contract — complete
- 0004.1 fragment spatial residency — complete
- 0004.2 structural-demand loading/pinning — complete
- 0004.3 incremental crash-safe fragment persistence — complete
- 0004.4 bounded async fragment mesh/collider derivation — complete
- 0004.5 support-witness acceleration experiment — complete; exact live path retained
- 0004.6 engine-neutral `DamageEvent -> DamageWork` contract — complete
- 0004.7 sparse progressive local damage — complete
- 0004.8 impulse coupling, fragment re-fracture, bounded secondary impact damage — complete
- **0004.9 chaos pass + hardware acceptance — NEXT; NOT STARTED**

Do not start DROP 0005 without the user.

## 0004.5 decision

The exact connectivity classifier remains structural truth and the live path. The support witness may only prove support or fall back to exact; it never declares detachment.

Measured expensive supported case:
- exact: 25,216 cells visited
- witness: 3,614
- ~85.7% fewer visits

The optimization remains disabled live pending broader bounded measurements.

## 0004.6 / 0004.7

`crates/engine_destruction/src/damage.rs` owns deterministic, engine-neutral damage input:
- ordered event IDs;
- static-world and exact fragment-local spaces;
- cell/box/sphere volumes;
- uniform/linear falloff;
- validated source + impulse;
- bounded all-or-nothing evaluation;
- material-agnostic policy seam.

`crates/engine_destruction/src/progressive_damage.rs` owns sparse accumulated damage:
- untouched cells allocate nothing;
- weak hits accumulate;
- bounded sparse entry count;
- deterministic threshold failures;
- static failures use `WorldEditBatch`;
- fragment failures change native fragment geometry;
- partial damage can transfer with streamed ownership;
- partial damage can remap to re-fracture children.

Do not add realistic material differences here. Wood/steel/glass/concrete mechanics belong to DROP 0005.

## 0004.8 engine state

New engine module: `crates/engine_destruction/src/impact.rs`

Key API:
- `FragmentImpulse`
- `reference_detachment_impulse`
- `apply_fragment_impulse`
- `couple_damage_to_detachment`
- `FragmentDamageResult`
- `RefractureOutcome` / `RefractureRefusal`
- `refracture_store_if`
- `damage_fragment_store_if`
- `SecondaryDamage`
- `SecondaryDamageLimits`
- `SecondaryDamageQueue`
- `FragmentImpact`
- `UniformImpactDamagePolicy`

Reference impulse coupling is intentionally material-neutral: occupied-cell count is the coarse mass proxy and fragment radius supplies a simple inertia estimate. Motion changes do not bump geometry revision.

Re-fracture:
- connected survivor keeps parent identity;
- total destruction removes parent;
- disconnected survivor gets canonical fresh child IDs from destruction sequence;
- children retain the parent's local coordinate frame, pose and velocities;
- admission happens before replacement;
- partial damage remaps by child local-cell ownership.

This preserves rotated fragments without engine-side quaternion interpretation.

## 0004.8 host state

New sandbox module: `apps/sandbox/src/impact.rs`

Sandbox secondary limits:
- max pending: 32
- max processed/step: 4
- max generation: 3
- progressive entries: 16,384
- uniform failure threshold: 18

Normal reference impact defaults:
- min solved normal impulse: 20
- radius: 1.75
- damage scale: 1
- max strength: 48

Diagnostic overrides:
- `MICROLOGY_IMPACT_MIN`
- `MICROLOGY_IMPACT_DAMAGE_SCALE`

Avian solved fragment-vs-world contacts become bounded secondary `DamageEvent`s. Contact starts are detected by comparing current touching pairs with the prior fixed step; the transient Avian collision-start flag was not reliable at this schedule point.

Fragment-vs-fragment impact damage is deliberately not automatic.

Damage-caused static cuts preserve their cause through async structural analysis. After admission succeeds, the event impulse is coupled once. If several distinct causes are coalesced, cause becomes unknown and no impulse is guessed.

Persistence:
`apps/sandbox/src/fragment_streaming.rs` has explicit authoritative parent deletion for destroy/re-fracture. Ordinary storage eviction is still never deletion. In-flight saves are finished before a persisted parent is tombstoned.

## 0004.8 acceptance

Focused impact/re-fracture tests: **7/7 passed**.  
Sandbox Clippy with warnings denied passed.  
Formatter and diff checks passed.

Real Windows 11 / NVIDIA RTX 5050 Laptop GPU / Vulkan fixture:
- 318 cells detached;
- 1 parent re-fractured into 2 children;
- 21 fragment-local cells failed before split;
- 3 fragment/world impacts sampled;
- impacts caused 24 secondary-failed static cells;
- 42 total static failed cells including the original 18-cell support cut.

Visual:
`docs/diagnostics/drop0004_8_impact_refracture/Micrology_DROP0004_8_Impact_Refracture.mp4`

Report:
`docs/diagnostics/drop0004_8_impact_refracture/report.md`

Visual fixture:
`apps/sandbox/src/impact_demo.rs` with `MICROLOGY_IMPACT_DEMO=1`.

Important acceptance finding: the first fixture put an anchored floor at Y=0 on a storage-volume face. Exact analysis followed occupied geometry into absent -Y regions and returned inconclusive. That was correct: **unknown != empty**. The fixture was moved to Y=4; engine semantics were not weakened.

## Fresh capture workflow

Do not reuse old `weakpoint_break.mp4`.

FFmpeg:
`C:\Users\behem\AppData\Local\Microsoft\WinGet\Packages\Gyan.FFmpeg.Essentials_Microsoft.Winget.Source_8wekyb3d8bbwe\ffmpeg-8.1.1-essentials_build\bin\ffmpeg.exe`

`gdigrab` sees the Vulkan surface as blank white. Use **Desktop Duplication**:
`ddagrab=framerate=30:draw_mouse=0,hwdownload,format=bgra`

Workflow: build → launch opt-in automated demo → foreground/maximize if possible → capture with `ddagrab` → trim/crop to Micrology → extract thumbnail → store video/thumbnail/report in section diagnostics.

## NEXT: 0004.9 only

0004.9 is chaos + hardware acceptance. Do not jump to DROP 0005.

Minimum adversarial cases:
- fragment population far larger than active residency;
- travel away/back with moving + sleeping fragments;
- dirty-fragment eviction during motion;
- crash windows during fragment payload/index persistence;
- structural demand at region boundaries;
- many simultaneous damage events;
- repeated sub-threshold damage;
- huge radial event near integer limits;
- re-fracture storm;
- secondary recursion/pending/per-step budget exhaustion;
- stale async mesh/collider results;
- deterministic replay;
- fantasy/disabled-physics authored geometry validity.

Also attack 0004.8 seams:
- coalesced causes must never guess an impulse;
- persistence tombstones must not race in-flight saves;
- partial damage must remap to the correct child only;
- admission refusal must leave parent + sequence unchanged;
- contact storms must remain bounded;
- generation limits must stop recursive cascades.

## Chaos Pass invariant

Chaos testing is observation/documentation only. The original/live target must remain unchanged after destructive diagnostics.

Run mutating abuse only in a disposable copy/test world/snapshot/clone/sandbox/temp profile/VM/container or equivalent.

Every chaos run:
1. capture pre-run baseline/manifest;
2. do a conventional expert baseline pass first;
3. run aggressive tests only in isolation;
4. collect logs/screenshots/video/reproduction steps;
5. tear down/restore disposable state;
6. verify original/live target against baseline;
7. do not call the run complete until integrity verification passes;
8. produce a full report of what survived, degraded, broke, and how to reproduce it.

The user often asks for a “Josh from Let's Game It Out” abuse pass. Be aggressive inside the disposable target, but never mutate the live/original target as collateral.

At the end of 0004.9: run tests + Clippy + formatter, real hardware acceptance, make a fresh visual, update docs, push, **STOP and report**.
