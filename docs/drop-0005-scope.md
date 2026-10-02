# DROP 0005 — Material Mechanics and Structural Capacity

DROP 0005 is the first drop in which two cells made of different materials behave
differently.

DROP 0004 closed destruction at scale and applied damage while deliberately
keeping every material identical: one failure threshold, one damage response,
mass approximated as occupied-cell count. That uniformity was the right call —
it let the event, accumulation, detachment and fragment pipeline be proved
without any mechanical model confusing the question. DROP 0005 pays that debt.

The milestone has three ordered goals:

1. give materials mechanical identity (density, capacity, failure character);
2. make the existing damage path material-aware without redesigning it;
3. add a deterministic, bounded, optional model of structural capacity — the
   first answer to "is this connection strong enough?", as distinct from the
   DROP 0003 question "is this connection present?".

The order matters. Material properties are inert data; material-aware damage is
a pure policy substitution; structural capacity is a new analysis with its own
budget, determinism and residency obligations. Each must be shippable and
reviewable before the next is attempted.

## Engine identity remains unchanged

Micrology is a reusable volumetric engine and game-development environment, not
a specific game, not a Minecraft mod, not a Fabric mod. It grew out of lessons
from Astra Microblocks and does not run on it. Material ids are Micrology-native;
importers map foreign registries onto them.

Nothing in this drop may make mechanical realism mandatory for a valid world.

Hard rule, carried forward unchanged:

> No core physical system is required for authored world validity.

A floating island, impossible castle or magical bridge is valid authored data,
and **must not need a fake infinite-strength material to stay up**. DROP 0004.9
verified this directly: an unanchored island round-tripped through save/load and
took local damage while staying exactly where authored, and opt-in analysis
confirmed it *would* detach if a host asked. DROP 0005 adds a second thing a host
may ask for. It does not add a thing a host must accept.

These remain separable and individually optional:

- gravity
- support/anchoring
- connectivity detachment
- applied damage/fracture
- fragment rigid-body physics
- material mechanics
- structural capacity/load analysis

## Inherited invariants

All DROP 0003 and DROP 0004 invariants remain load-bearing:

- cells are data, never ECS entities or per-cell rigid bodies;
- detached connected masses become native volumetric `Fragment`s;
- support/anchoring is not a material property; `SupportSource::is_anchor`
  stays its own concept;
- Bevy and Avian stay in the host;
- collision derives from cell geometry, not render meshes;
- asynchronous work uses owned snapshots/results with stale-result validation;
- unknown/unloaded space is not empty;
- inconclusive analysis never detaches or collapses geometry;
- fragment admission is atomic, and policy refusal leaves the fragment store and
  identity sequence unchanged;
- exact/reference implementations are retained beside optimized paths;
- ordering, identity and externally visible outcomes remain deterministic;
- byte budgets and work budgets are separate.

One further invariant is inherited from `engine_geometry` and applies with full
force to this drop's solver:

> An oracle that shares code with the thing it validates is not an oracle.

`ExactCompiler` is written independently of `GreedyCompiler` for exactly this
reason, and that comparison is the engine's entire correctness story for
optimized meshing. The exact capacity solver must be written independently of
any accelerated one.

## Explicit non-goals

DROP 0005 does **not** implement:

- a finite-element solver;
- per-cell rigid bodies or per-cell physics state;
- soft-body simulation;
- arbitrary elastic deformation;
- full metal plasticity;
- bending or buckling as continuous phenomena;
- a game-specific weapon, player, health or scoring system;
- a construction/engineering game;
- mandatory realism;
- automatic collapse because authored geometry would be impossible in reality.

---

# Resolved design decisions

These are settled here so that the passes below do not re-litigate them.

## A. Mechanical data lives in a new crate, not on `Material`

Mechanical properties do **not** extend `Material` in `engine_core`.

A new crate `engine_mechanics` owns a `MechanicalRegistry` keyed by `MaterialId`,
holding the mechanical profile for materials that have one. Materials without an
entry have no mechanical identity, which is a valid and expected state.

Reasons:

- `engine_core` is the shared vocabulary every crate depends on, and it is
  deliberately dependency-light. Loading it with capacity, density and failure
  modes forces mechanics vocabulary onto meshing, streaming and I/O, none of
  which have any use for it.
- Mechanics must be omittable. A registry that may legitimately be empty
  expresses "this world has no mechanical model" far better than a `Material`
  whose mechanical fields are all defaulted and therefore always present.
- `MaterialId` already travels everywhere it needs to — including on both
  variants of `DamageTarget` — so a side table keyed by id costs nothing in
  reach while keeping identity and mechanics separable.
- It mirrors the existing separation of anchoring from material identity. Making
  strength a property of `Material` is one short step from making support a
  property of `Material`, which is explicitly forbidden.

Naming: **`engine_mechanics`**. Not `engine_stress` — that crate exists and means
the deterministic measurement and chaos harness. Not `engine_structure` — the
word "structural" is already taken by connectivity analysis (`StructureJobInput`,
`structural_candidates`, `StructuralLimits`) and reusing it would make every
future conversation ambiguous.

## B. DROP 0005 is additive by default

`engine_mechanics` sits to the right of `engine_destruction` in the dependency
graph:

```text
engine_core ──┬── engine_volume ──── engine_world ──┬── engine_destruction ──┬── engine_mechanics
              └── engine_geometry ──────────────────┘                        └── engine_io
```

This placement is what makes the drop additive. Because `DamagePolicy` and
`DamageFailurePolicy` are traits defined in `engine_destruction`, a *downstream*
crate may implement them for its own types. Material-aware damage therefore
needs new implementations, not a modified contract. The same applies to mass:
`FragmentImpulse` and `apply_fragment_impulse` are public, and `Fragment`
implements `CellSource`, so a density-aware impulse function can be written
entirely in `engine_mechanics` while `reference_detachment_impulse` remains in
place as the material-neutral reference the invariants require.

**Any pass that proposes modifying `engine_destruction` must justify it in that
pass.** The default expectation is that the 0004.6 damage contract, the 0004.7
accumulator and the 0004.8 coupling path are untouched.

## C. The structural representation is an integer capacity network

Structural capacity is evaluated as an **integer flow problem over the exact
6-neighbour connectivity graph**, not as a per-cell stress field and not as an
iterative relaxation.

- Each occupied participating cell contributes a **demand** equal to its own
  weight, derived from material density.
- Each face-connection between two occupied cells carries a **capacity** derived
  from the two materials' profiles and the load mode across that face.
- Anchors (`SupportSource::is_anchor`) are the sinks.
- A configuration is **mechanically satisfied** when a feasible integer flow
  exists routing all demand to anchors within capacity.
- When no feasible flow exists, the **minimum cut** names the set of connections
  that are overloaded. That set, and nothing else, is what mechanical failure
  acts on.

This model was chosen over the alternatives for specific reasons:

- it is **exact in integer arithmetic**, which answers the determinism problem
  outright rather than mitigating it;
- **redistribution falls out of it**. Flow reroutes around a weakened path
  automatically, which is precisely the "repeated overload redistribution"
  behaviour the chaos list demands, and which a simple downward load-accumulation
  model gets wrong;
- the **min cut is the failure set**, so the model names where it breaks instead
  of merely reporting that it broke;
- it **directly resolves the one-cell support cliff** (see H).

The cost risk is real and is acknowledged: flow on a per-cell graph is far more
expensive than connectivity. This is why 0005.4 and 0005.5 are bounded,
demand-scoped and non-mutating, and why acceleration is deferred to 0005.7
behind the support-witness precedent. **If measurement in 0005.4 shows the exact
per-cell network is not viable even under region-scoped demand, the fallback is a
coarsened network over connected components with per-component aggregate capacity,
keeping the same integer formulation.** That fallback must be decided on measured
evidence, not anticipated.

## D. Determinism is designed, not hoped for

- All mechanical quantities are **integers**. Density, capacity, demand and flow
  are `i64` fixed-point in milli-units. No floating point enters the capacity
  path at any stage.
- Traversal is **canonical**: ascending `CellPos` order, and the six `FaceDir`s
  in their declared order, exactly as the meshers already do.
- Augmenting paths are selected by a **deterministic rule** (lowest canonical
  cell sequence first), never by hash iteration order or by whichever worker
  finished first.
- The augmentation count is **bounded by an explicit ceiling**. Reaching it
  produces `Deferred`, never a decision.
- The result is a **pure comparable value** with no timings in it, so an exact
  and an accelerated solver can be compared for equality — the same property that
  makes `QuadSet` the oracle for meshing.

## E. Direction stays on the mechanics side

Tension, compression and shear need direction. That direction does **not** enter
`DamageEvent` or `DamageWork`.

`DamageWork` carries a scalar amount, and the 0004.7 sparse accumulator, its
atomic transaction semantics and its canonical failure ordering are all built on
that. Threading a direction through it would reach into proven code for no gain,
because applied damage is not where direction is known: it is known at the
connection, from the geometry of the flow across it.

Load mode is therefore computed inside `engine_mechanics` per connection, from
the flow direction relative to the face normal and the gravity axis, and is used
to select which capacity (tensile, compressive, shear) applies. Mechanical
failure then emerges as an ordinary **failed cell set**, which enters the
existing pipeline unchanged.

## F. Mechanical failure composes with topological failure in a fixed order

One host step resolves in this order:

1. applied damage accumulates (0004.7) and produces failed cells;
2. failed cells are removed through the existing `WorldEditBatch` path;
3. connectivity analysis runs and detachment is admitted atomically (0003/0004);
4. **mechanics evaluates the resulting configuration** and may produce a new
   failed cell set;
5. that set re-enters step 2 **on the next generation**, not recursively within
   this one.

Generation depth, pending work and per-step evaluations are bounded
independently, reusing the discipline 0004.8 established for secondary damage. A
collapse must not be able to run unbounded inside a single frame. Mechanics
always evaluates **after** connectivity, never instead of it.

## G. Re-fracture stays material-neutral

Child admission on re-fracture does not become material-aware in this drop.

Re-fracture is a topological operation over `connected_parts`, and admission is
already a host-supplied policy closure — a host that wants material-aware
admission can express it there without the engine learning anything new. Making
identity allocation depend on material data would couple the destruction
sequence to the mechanical registry for no capability gain.

Density-aware mass applies to children automatically, because mass is aggregated
from the cells a child actually holds.

## H. The one-cell support cliff becomes a capacity question

Today a component attached by one cell is supported, and the DROP 0003 chaos
suite records this as `known_last_cell_support_cliff` — correct for binary
topology, inadequate for load-bearing mechanics.

DROP 0005 does **not** change that answer. Exact connectivity remains the
topological oracle and keeps saying "attached". Mechanics adds a second, separate
question: the single connecting face has a finite capacity, and if the demand
routed through it exceeds that capacity, the min cut is that one connection and
it fails mechanically.

The division is strict and must stay strict:

- connectivity may detach geometry; mechanics may not;
- mechanics may produce **cell failures**, which are then removed and fed back
  through connectivity in the normal way;
- mechanics may never declare something attached that connectivity says is
  detached, and may never prevent a detachment connectivity has concluded;
- with mechanics disabled, behaviour is bit-identical to DROP 0004.

---

# Ordered passes

## 0005.0 — mechanics architecture and participation contract

Create `engine_mechanics` with no mechanical behaviour in it yet.

Required:

- the crate, its place in the dependency graph, and its exclusion from any
  mandatory path;
- the deterministic numeric type (`i64` milli-units) and its saturating
  arithmetic rules;
- `MechanicalRegistry` keyed by `MaterialId`, legitimately empty;
- the participation policy: a per-system tri-state (`Inherit` / `Enable` /
  `Disable`) over the seven separable systems, resolvable at four scopes —
  engine/game default, world, region/structure/object, gameplay-supplied
  override — with **most specific wins** and a documented, tested resolution
  order;
- persistence of participation policy and mechanical registry, additively, so
  older worlds load unchanged and default to mechanics disabled.

Acceptance: a world with no mechanical data and no participation policy behaves
identically to DROP 0004, proved by running the existing regression with the new
crate present. Policy resolution is table-tested across all four scopes including
an override that *disables* an inherited enable.

No capacity, no failure, no solver.

## 0005.1 — mechanical material profiles

Add the inert data.

Required per profile: density, tensile capacity, compressive capacity, shear
capacity, damage resistance/toughness, and a failure-character tag (brittle,
ductile, granular) which in this drop only selects reference response curves.

Values are engine-neutral and data-driven. Ship a reference set covering wood,
masonry, concrete, steel and glass as *examples*, clearly marked as reference
data rather than engine truth.

Acceptance: profiles round-trip through persistence; an unregistered material
yields no profile rather than a defaulted one; the reference set is ordered and
deterministic.

## 0005.2 — material-aware damage policies

Implement `MaterialDamagePolicy` and `MaterialFailurePolicy` in
`engine_mechanics`, as new implementations of the existing `engine_destruction`
traits.

Required behaviour: glass fails to a light hit that barely marks steel; toughness
scales incoming damage; threshold derives from the profile; a target whose
material has no profile falls back to the uniform reference response.

Signatures do not change. `DamageEvent`, `DamageWork`, the sparse accumulator and
its atomicity are untouched.

Acceptance: identical events on different materials produce different, repeatable
failed-cell sets; the uniform policies still pass every DROP 0004 test; mixed
material structures damage deterministically.

**Visual:** this is the first visibly material-aware pass — one event, five
materials, five different results.

## 0005.3 — density-aware fragment mass and inertia

Aggregate mass from material density over a fragment's occupied cells, and use it
for linear and angular coupling.

`reference_detachment_impulse` stays as the material-neutral reference. The
density-aware function lives in `engine_mechanics` beside it.

Acceptance: a steel fragment and a wood fragment of identical geometry receive
measurably different motion from the same impulse; mass aggregation is
deterministic and order-independent; a fragment of unprofiled material matches
the DROP 0004 cell-count behaviour exactly.

## 0005.4 — exact load representation, without failure

Build the integer capacity network and compute flow. **Do not act on it.**

Required: demand aggregation from density; capacity derivation per connection
including load-mode selection; anchors as sinks; canonical traversal; bounded
augmentation with `Deferred` on exhaustion; `Indeterminate` when the network
reaches unloaded data, with explicit region demand in the 0004.2 sense.

This pass must report measurements, not just correctness: network size,
augmentation counts and visited cells on the canonical structures, so that the
coarsening fallback in decision C can be judged on evidence.

Acceptance: a known-supported structure is satisfied; a known-overloaded one is
not, and names a min cut; budget exhaustion defers; a network touching an absent
region is indeterminate and requests it; repeated runs are byte-identical; a
**disabled** case produces no analysis and no cost at all.

## 0005.5 — structural capacity evaluation

Turn the flow result into an answer about a specific connection or component:
sufficient, insufficient with a named cut, or inconclusive.

This is where the one-cell support cliff starts being answered: a one-cell
connection is topologically present and may be mechanically overloaded, and the
engine can now say which.

Still no mutation.

Acceptance: a column on a one-cell neck is reported insufficient under a load
that a full-width neck carries; the exact connectivity oracle's answer is
unchanged in both cases; disabled participation returns "not evaluated" rather
than "sufficient".

## 0005.6 — progressive mechanical failure

Mechanical overload may now produce cell failures.

Those failures go through the **existing** path — `WorldEditBatch`, connectivity,
detachment, fragments. There is no separate "mechanical fragment" system and no
second detachment mechanism.

Required: the fixed step order from decision F; independent bounds on generation
depth, pending work and per-step evaluations; refusal leaves world and sequence
unchanged.

Acceptance: an overloaded structure collapses progressively over bounded
generations rather than in one unbounded cascade; the same input collapses
identically every run; with mechanics disabled the identical structure stands
forever; budget exhaustion defers and never collapses.

**Visual:** the first "it fell down because it was too heavy, not because
something hit it" capture.

## 0005.7 — accelerated capacity solver experiment

Only after exact behaviour exists.

Follow the `support_witness` precedent exactly: the fast path may answer
`DefinitelySufficient` or `NeedExact`, and may **never** declare insufficiency.
It is written independently of the exact solver, per the oracle rule.

Acceptance: an oracle comparison across a scenario matrix in which the witness
never contradicts the exact solver, plus a recorded measurement of work saved.
Shipping is optional — a measured negative result is an acceptable outcome, as it
was for 0004.5.

## 0005.8 — participation and fantasy acceptance

Explicit fixtures for the full matrix: full mechanics; damage only; connectivity
only; mechanics disabled; floating-island override; and a mixed structure where
one region participates and an adjacent one does not.

Note that this pass **verifies** the hard rule rather than introducing it: every
pass from 0005.4 onward already carries a disabled case in its own acceptance,
because a rule checked only at the end of a drop is a rule that erodes quietly
in the middle of one.

Acceptance: an unanchored floating island of profiled stone stays exactly where
authored with mechanics disabled, and no fake infinite-strength material appears
anywhere in the fixtures.

## 0005.9 — chaos pass and hardware acceptance

Extend `chaos4` rather than inventing a second chaos framework; the Chaos Pass
rule is already enforced there by the harness itself.

Minimum adversarial cases: massive mixed-material structures; repeated overload
redistribution; capacity networks crossing region boundaries; very low work and
byte budgets; stale async mechanical results; extreme density ratios; mechanical
fracture storms; deterministic replay of a full collapse; and fantasy worlds with
stress disabled.

Acceptance: every chaos case passes with live-target integrity verified; the
DROP 0003 and DROP 0004 chaos suites stay green; a full collapse replays
byte-identically; a mechanics-disabled fantasy world is untouched by the entire
suite; and the report is byte-identical across repeated runs.

Then real Windows/NVIDIA/Vulkan acceptance with a fresh `ddagrab` capture showing
material-differentiated destruction and a load-driven collapse.

---

# Physical-policy direction

DROP 0005 establishes mechanics as the sixth separable system, not as the system
the others defer to. After this drop a game should be able to choose:

- static authored geometry with nothing enabled;
- connectivity destruction only;
- damage and fracture with no continuous capacity analysis;
- full mechanics on a region and none on its neighbour;
- gameplay-supplied support that overrides mechanics entirely.

The last one matters most for the engine's stated purpose. A magical bridge is
not a material problem; it is a gameplay assertion that the bridge is held up.
The participation override must be able to express that without the bridge
pretending to be steel.

Deliberately left for later drops: bending and buckling as continuous phenomena,
plastic deformation, fatigue, thermal effects, and any per-cell mechanical state
that would reintroduce per-cell runtime objects.

---

# Success criteria

DROP 0005 is complete when all of the following are true:

1. mechanical properties exist in `engine_mechanics`, keyed by `MaterialId`, and
   `Material` in `engine_core` is unchanged;
2. different materials demonstrably take damage and fail differently through new
   implementations of the existing policy traits, with the 0004.6 contract
   unmodified;
3. fragment mass and inertia derive from density, with the cell-count reference
   retained;
4. structural capacity is computed exactly, in integer arithmetic, with
   byte-identical results across runs;
5. capacity exhaustion of work or memory produces deferral or inconclusiveness
   and never collapse;
6. a capacity network reaching unloaded data is inconclusive and can demand the
   regions it needs;
7. mechanical failure produces ordinary cell failures that flow through the
   existing damage, connectivity, detachment and fragment pipeline, with no
   parallel system;
8. collapse is bounded by generation, pending work and per-step limits;
9. the one-cell support cliff is answerable as a capacity question without the
   exact connectivity oracle changing its topological answer;
10. with mechanics disabled, behaviour is identical to DROP 0004, and an
    unanchored floating island remains valid authored data needing no
    infinite-strength material;
11. any accelerated solver is conservative, independently written, and compared
    against the exact one;
12. the chaos suite covers mixed materials, redistribution, boundaries, budgets,
    stale results and deterministic replay, with live-target integrity verified;
13. a fresh `ddagrab` capture shows material-differentiated destruction and a
    load-driven collapse on current hardware.
