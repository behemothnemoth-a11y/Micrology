# ChatGPT Handoff — scoping DROP 0005 (material mechanics)

## What this document is for

Produce `docs/drop-0005-scope.md`: the ordered, section-by-section scope and
acceptance contract for DROP 0005. **This is a scoping task, not an
implementation task.** Do not write engine code from this handoff.

Mirror the shape of `docs/drop-0004-scope.md`, which is the model to follow:
milestone statement, engine identity, inherited invariants, explicit non-goals,
then `# Ordered passes` with one `## 0005.N` heading per section, each with its
own acceptance criteria, then forward direction and success criteria.

Justin will review the scope before any code is written.

---

## Resume point

- Repo: `behemothnemoth-a11y/Micrology`
- Branch: `drop-0004-destruction-scale-damage`
- Head: `a7c2210` — `0004.9: add DROP 4 chaos pass with enforced isolation`
- Prior: `40d4d94` handoff doc, `78990e7` 0004.8, `ac7d5aa` 0004.7

Read before scoping: `docs/drop-0004-scope.md` (especially `# Physical-policy
direction`), `docs/drop-0004.md` (what actually shipped), and
`docs/architecture.md`.

---

## Project identity

Micrology is a **standalone reusable volumetric game engine and
game-development environment**. It is not a specific game and not a Minecraft
mod. The goal is engine technology that can later support several games:
realistic destruction, fantasy worlds, horror, building/engineering.

Material ids are the engine's own. They are never foreign block-registry ids;
import adapters map onto them.

---

## Load-bearing invariants

These are architectural, not stylistic. A DROP 0005 scope that violates one is
wrong regardless of how good the mechanics are.

- Cells are data. Never one ECS entity or rigid body per cell.
- One detached connected component becomes one native volumetric `Fragment`.
- **Support/anchoring is separate from material identity.** It is its own trait,
  `SupportSource::is_anchor` in `crates/engine_core/src/source.rs`. Do not merge
  anchoring into material mechanics.
- Bevy and Avian stay outside engine crates.
- Static and fragment collision derive from cell data, not render meshes.
- Async work uses owned snapshots/results with stale-result validation.
- Unknown/unloaded world space is not empty.
- Inconclusive structural analysis never detaches geometry.
- Fragment admission is atomic; refusal leaves store and sequence unchanged.
- Exact/reference implementations remain available alongside optimizations.
- Outputs, ordering and identity remain deterministic.
- Byte budgets and work budgets are separate.

### The hard rule

> No core physical system is required for authored world validity.

A floating island, impossible castle or magical bridge is valid authored data
and **must not need a fake infinite-strength material** to stay up. Gravity,
support/anchoring, connectivity detachment, applied damage, fragment physics,
material mechanics and structural stress must all remain separable and
individually optional.

DROP 0004.9 verified this directly: an unanchored 320-cell floating island
round-trips through save/load and takes local damage while staying exactly where
authored, and opt-in analysis confirms it *would* detach if a host asked. DROP
0005 must not erode that.

---

## Where DROP 0004 ended

| Section | State |
| --- | --- |
| 0004.0 scope/acceptance contract | complete |
| 0004.1 fragment spatial residency | complete |
| 0004.2 structural-demand region loading | complete |
| 0004.3 incremental fragment persistence | complete |
| 0004.4 async fragment derived data | complete |
| 0004.5 structural-search acceleration | complete (as an experiment) |
| 0004.6 engine-neutral applied-damage contract | complete |
| 0004.7 progressive local damage | complete |
| 0004.8 force coupling + secondary destruction | complete |
| 0004.9 chaos pass | complete — 14/14, integrity verified |
| 0004.9 hardware/visual acceptance | **outstanding** — needs a Windows/NVIDIA/Vulkan `ddagrab` capture |

Current engine regression: 518 passed, 0 failed. Chaos: `chaos4` 14/14,
DROP 3 `chaos` 9/9. Clippy clean with warnings denied across engine crates and
the sandbox.

The outstanding hardware acceptance does not block scoping DROP 0005, but DROP
0005 should not be *started* until it is run.

---

## The seams DROP 0005 plugs into

These already exist. Scope DROP 0005 around them rather than inventing parallel
machinery.

### Material identity — `crates/engine_core/src/material.rs`

A `Material` today is: `MaterialId(u32)`, an `Rgb` colour, and an optional name.
That is all. New fields are explicitly designed to be additive, and the save
format ignores unknown keys so older files keep loading.

**Open question for the scope:** do mechanical properties extend `Material` in
`engine_core`, or live in a separate registry in their own crate? `engine_core`
is the shared, dependency-light bottom of the stack; loading it up with
mechanics may be wrong. Decide explicitly and justify it.

### Damage response — `crates/engine_destruction/src/damage.rs`

```rust
pub trait DamagePolicy: Send + Sync {
    fn evaluate(&self, event: &DamageEvent, target: DamageTarget) -> Option<DamageWork>;
}
```

Reference implementation: `UniformDamagePolicy`. This decides **how much of an
event reaches a given cell**.

### Failure threshold — `crates/engine_destruction/src/progressive_damage.rs`

```rust
pub trait DamageFailurePolicy: Send + Sync {
    fn threshold(&self, target: DamageTarget) -> DamageAmount;
}
```

Reference implementation: `UniformFailurePolicy`. This decides **how much
accumulated damage makes that cell fail**. It is deliberately a separate trait
from `DamagePolicy`.

**Important and easy to miss:** `DamageTarget` already carries
`material: MaterialId` on both its `StaticCell` and `FragmentCell` variants. Both
policy traits therefore already receive material identity. Making damage
material-aware needs **no signature change** — only new implementations of two
existing traits. Scope accordingly; do not propose reworking the damage contract.

### Mass and inertia — `crates/engine_destruction/src/impact.rs`

`reference_detachment_impulse` currently approximates mass as occupied cell
count and inertia as `mass * bounding_radius²`. Density is the obvious DROP 0005
substitution, and 0004.8 explicitly deferred it here.

### Structural truth — `crates/engine_destruction/src/connectivity.rs`

Exact 6-neighbour **face** connectivity. Edge and corner contact are not
support. `Classification::Indeterminate` and `::Deferred` exist so that unknown
data and budget exhaustion can never be read as detachment. A component attached
by **one cell** is currently supported, and that is deliberate — the DROP 3
chaos suite records this as `known_last_cell_support_cliff`, an acknowledged
model weakness that load-bearing mechanics are expected to address.

### Conservative acceleration precedent — `support_witness.rs` (0004.5)

The support witness may only answer `AllRootsSupported` or `NeedExact`. It may
never declare detachment. Exact connectivity remains the oracle. Any accelerated
stress solver in DROP 0005 should follow the same discipline: a fast path that
can only ever return "definitely fine" or "ask the exact path".

---

## What DROP 0004 deliberately left uniform

These are the debts DROP 0005 is being created to pay:

- every material fails at the same threshold;
- every material takes damage identically;
- mass is cell count; there is no density;
- there is no tension, compression or shear;
- there is no load-bearing capacity or load path;
- there is no bending or plastic deformation;
- `DamageWork` carries a scalar `amount` and no direction.

---

## Constraints on the DROP 0005 design

1. **Structural stress must be optional and disableable**, per world, and
   ideally per region/structure/object, with a stated precedence rule. Fantasy
   worlds must not need fake materials.
2. **Anchoring stays separate.** Do not fold support into material strength.
3. **Determinism is non-negotiable.** This is the sharpest tension in the whole
   drop: stress solving is usually iterative and floating-point sensitive, while
   Micrology commits to deterministic outputs, ordering and identity. The scope
   must say exactly how a stress result stays deterministic — fixed iteration
   counts, integer/fixed-point accumulation, canonical traversal order, or an
   explicitly bounded-and-conservative answer. Do not leave this implicit.
4. **Bounded work and bounded bytes, separately.** Stress analysis must obey the
   same budget discipline as structural analysis, and exhausting a budget must
   produce a conservative deferral, never a collapse.
5. **Unknown is not empty** applies to stress too. A load path that leaves
   resident data must be inconclusive, not assumed free.
6. **Keep an exact reference** beside any accelerated solver.
7. **No per-cell rigid bodies.** Ever.

---

## Naming traps and anti-goals

- **`crates/engine_stress` is NOT mechanical stress.** It is the deterministic
  measurement and stress-*testing* harness (scenarios, counters, baselines, the
  `chaos` and `chaos4` binaries). Mechanical/structural stress must not be put
  there. If DROP 0005 needs a home for load-path mechanics, name it something
  unambiguous and say so in the scope.
- Do not implement a full finite-element simulation.
- Do not add a game-specific weapon, player, health or scoring system.
- Do not make an authored structure collapse merely because it is physically
  implausible. That is the hard rule, restated.

---

## Open design questions the scope must answer

These are the genuinely hard calls. The scope is good if it resolves them
explicitly and bad if it hand-waves them.

1. Where do mechanical properties live, and why there?
2. Is stress a per-cell field, a per-component analysis, or a load-path graph?
   How is it bounded and streamed under the same rules as everything else?
3. How is determinism preserved through a solver? (See constraint 3.)
4. What is the precedence rule when game, world, region and object each express
   a participation policy?
5. Does the damage pipeline need **direction**, not just a scalar amount, to
   express tension vs compression vs shear? If so, where does that enter —
   `DamageEvent`, `DamageWork`, or a new stress-side concept? Changing
   `DamageWork` touches the sparse accumulator, so justify it.
6. Mechanical failure vs topological failure: today failure means a cell is
   removed and connectivity decides the rest. Stress adds a second failure mode.
   How do they compose, and which runs first?
7. Does re-fracture need material-aware child admission, or does it stay
   material-neutral?
8. How does the one-cell support cliff get addressed without breaking the exact
   connectivity oracle that everything else rests on?

---

## Workflow rules

Justin works **one section at a time**. The scope must therefore be genuinely
ordered and independently shippable, with each section's definition of done:

1. implement only that section;
2. run focused tests;
3. run relevant regression, Clippy and format checks;
4. update DROP documentation;
5. produce a short visual showing that section;
6. stop and report.

He dislikes assistants stalling on huge redundant rebuilds, and he does not
accept a claimed passing test that was not actually run.

Visual capture, when a section needs one: FFmpeg Desktop Duplication
(`ddagrab`), not `gdigrab` — `gdigrab` finds the window but records the Vulkan
surface as blank white. Capture the desktop, foreground and maximize the
Micrology window, crop afterwards.

Chaos/abuse testing follows the Chaos Pass rule, now enforced by the harness
itself in `chaos4`: never abuse the live target, use an isolated disposable
copy, capture a baseline, abuse it, collect evidence, tear down, and verify the
original's integrity afterward. A chaos run is not complete until that integrity
check passes.
