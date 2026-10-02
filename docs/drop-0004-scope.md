# DROP 0004 — Destruction at Scale + Applied Damage

DROP 0004 is the bridge between the exact destruction foundation proven in DROP
0003 and the richer material/stress mechanics planned for DROP 0005.

The milestone has two ordered goals:

1. make dynamic destruction obey Micrology's bounded-world rules at real scale;
2. add force/damage/progressive breakage on top of that bounded foundation.

The order is deliberate. DROP 0003 proved that connectivity, detachment, fragment
physics and persistence are correct. Its closeout also exposed that fragment
residency, persistence and derived-data compilation are not yet scalable enough
to safely multiply destruction events. Force-driven destruction must not create
an unbounded fragment system faster than the engine can stream, persist or
compile it.

## Engine identity remains unchanged

Micrology is a reusable game engine/environment, not a specific game.

Nothing in this drop may make a realistic destruction simulation mandatory for a
valid Micrology world. Different games must be able to choose different physical
rules, including impossible/fantastical worlds.

Hard rule:

> No core physical system is required for authored world validity.

A floating island, unsupported castle or magical bridge is valid authored data.
A game may opt that object or region into gravity, connectivity destruction,
damage, later stress mechanics, all of them, or none of them.

The eventual game-facing physical policy must keep these concepts separable:

- gravity
- support/anchoring
- connectivity detachment
- applied damage/fracture
- fragment rigid-body physics
- material mechanics
- structural stress/load paths

DROP 0004 implements and exercises the first scalable damage path. DROP 0005 may
add material stress, but it must remain optional and independently switchable.

## Inherited invariants

All DROP 0003 invariants remain load-bearing:

- cells are data, never ECS entities or per-cell rigid bodies;
- detached connected masses become native volumetric Fragments;
- support/anchoring is not a Material property;
- engine crates own world/destruction truth;
- Bevy/Avian stay in the host;
- static collision and fragment collision derive from cells, not render meshes;
- asynchronous work uses owned snapshots/results and stale-result validation;
- inconclusive structural results never detach geometry;
- admission/refusal is atomic;
- byte budgets and work budgets remain separate;
- exact/reference implementations are retained when optimized paths are added;
- output-visible ordering remains deterministic.

## Explicit non-goals

DROP 0004 does **not** implement:

- realistic material strength differences;
- tension/compression/shear stress propagation;
- load-bearing capacity;
- bending/plastic deformation;
- steel-versus-wood structural behavior;
- automatic collapse because an authored structure is physically implausible;
- a game-specific weapon, player, health or scoring system;
- per-cell rigid bodies;
- a full finite-element simulation.

Those belong to later layers. DROP 0004 supplies the scalable event/damage
foundation they can use.

---

# Ordered passes

## 0004.0 — scope, acceptance contract and baselines

Lock this document and record the post-DROP-0003 baseline.

Add acceptance scenarios before implementation expands:

- long-distance travel through a world containing persisted fragments;
- large fragment populations outside the camera area;
- structural damage at a streaming boundary;
- repeated impacts against one structure;
- explosion/radial damage determinism;
- fragment-on-structure secondary impact;
- re-fracture of an already detached fragment;
- fantasy-policy fixture where unsupported geometry is intentionally retained.

No new mechanic is considered complete without a deterministic scenario that
states exactly what changed.

## 0004.1 — fragment spatial residency

Fragments must stop being "load the whole store".

Required behavior:

- discover fragment ids from the existing spatial index by relevant regions;
- load fragment payloads only when storage residency requires them;
- unload fragments that leave the storage residency window;
- save dirty fragments before eviction;
- update spatial membership as fragments move;
- keep storage residency distinct from render and physics residency;
- prevent one fragment overlapping multiple regions from being loaded twice;
- maintain deterministic load/evict order;
- preserve FragmentId and destruction-sequence continuity.

A sleeping fragment is still a fragment. Residency does not voxelize it back into
terrain.

## 0004.2 — structural-demand region loading and pins

A structural analysis that needs unloaded world data must be able to request it.

Required behavior:

- an inconclusive structural result may name required regions;
- the destruction host can issue bounded structural-demand requests;
- demanded regions may be temporarily pinned against normal camera eviction;
- demand has explicit count/byte/work limits;
- unrelated ready destruction work must not head-of-line block;
- demand is released when the question resolves, expires or is cancelled;
- the system remains conservative when demand cannot be admitted.

Camera movement must no longer be required to resolve a valid structural question.

## 0004.3 — incremental fragment persistence

Full-store fragment rewrites are not acceptable once damage becomes frequent.

Required behavior:

- dirty tracking per fragment;
- write only dirty/new fragment payloads;
- remove only explicitly deleted/orphaned payloads;
- update the index atomically after authoritative payload writes;
- retain crash-window recovery behavior proven in DROP 0003;
- persist moving fragments without rewriting static terrain;
- deterministic canonical bytes for unchanged state.

## 0004.4 — asynchronous fragment derived data

One complex fragment must not monopolize the frame while compiling mesh/collision.

Move fragment mesh and collider compilation behind bounded asynchronous jobs with:

- immutable/owned job input;
- fragment revision fingerprints;
- stale-result rejection;
- bounded active jobs;
- bounded completed-result application per frame;
- separate render and collision queues/budgets;
- no Bevy/Avian types in engine jobs.

Exact/reference compilation remains available for validation.

## 0004.5 — structural search acceleration experiment

The exact connectivity classifier remains the oracle.

Investigate a faster layer for large supported structures, likely one or more of:

- coarse volume-level component/support graph;
- cached support/component labels;
- incremental invalidation around edits;
- memoized proven-supported regions.

Any optimized answer must be checked against the existing exact classifier across
the canonical structural matrix and seeded adversarial worlds.

If an optimization cannot prove equivalence, it does not replace the oracle.

## 0004.6 — engine-neutral applied-damage contract

Introduce a game-independent damage event representation without embedding weapon
or gameplay concepts in core crates.

The contract needs to express, at minimum:

- world-space source/origin where relevant;
- affected shape/volume;
- scalar event strength/energy;
- optional directional impulse;
- deterministic event identity/ordering;
- falloff for radial events;
- a policy hook that maps an event + target cell/fragment state to damage work.

For this drop, mechanical resistance may be deliberately uniform/defaulted.
Material-specific strength belongs to DROP 0005.

The event contract must work on both static world cells and native fragments.

## 0004.7 — progressive local damage

Damage must become cumulative rather than synonymous with "delete this cell now".

Add a compact damage state/accumulator that can produce deterministic local
failure while remaining sparse: untouched world cells must not acquire large
per-cell runtime objects.

Required behavior:

- sub-threshold hits can accumulate;
- threshold crossing produces a normal world edit/removal transaction;
- one event can affect multiple nearby cells deterministically;
- radial events have deterministic distance falloff;
- edits continue through the existing dirty/revision/destruction pipeline;
- damage state is bounded and evictable/persistable as required by world rules;
- repeated identical input produces identical failed-cell sets.

Do not put realistic steel/wood thresholds here. Use a simple default/reference
response so the pipeline can be proved independently of material mechanics.

## 0004.8 — force-to-fragment coupling and secondary destruction

When damage detaches a fragment, the causing event should be able to influence
its initial motion.

Required behavior:

- accepted detachment can receive deterministic initial linear/angular impulse;
- impulse is applied once, after atomic fragment admission;
- detached fragments can themselves receive damage;
- an already detached fragment may re-fracture into smaller native fragments;
- sufficiently energetic fragment/world collisions can emit bounded secondary
  damage events through host policy;
- secondary events are budgeted and cannot recurse without bounds;
- collision response remains host/backend policy, not engine truth.

This is the first pass intended to create the visible "hit it, crack it, pieces
move, pieces can break more things" loop.

## 0004.9 — chaos pass and hardware acceptance

Extend the destruction chaos suite to try to break the new system.

Minimum adversarial cases:

- fragment population much larger than active residency;
- travel away/back with moving and sleeping fragments;
- dirty-fragment eviction during motion;
- crash windows during incremental fragment save/index update;
- structural demand at region boundaries;
- many simultaneous damage events;
- repeated sub-threshold damage;
- very large radial event clipped at integer/world limits;
- fragment re-fracture storm;
- secondary-impact recursion/budget exhaustion;
- stale async fragment mesh/collider results;
- deterministic replay;
- fantasy mode with structural/stress participation disabled.

The original/live target must remain intact after any destructive diagnostic run;
chaos work runs in an isolated disposable world/copy and integrity is verified
afterward.

---

# Physical-policy direction

DROP 0004 should establish the seam that later games can control, but should not
over-design DROP 0005 today.

The intended direction is that a game/world/object can independently choose
participation such as:

- static authored geometry: no gravity, no auto-collapse;
- connectivity destruction only;
- damage + fracture, but no continuous structural stress;
- full fragment physics;
- later full material stress/load-path mechanics;
- custom/fantasy support supplied by gameplay.

A floating island must not need to masquerade as "infinitely strong steel".
Suspension/support rules and material strength are different concerns.

Overrides should eventually be possible at more than one scope (game/world,
region/structure/object, and gameplay-defined custom policy), with a clear
precedence rule. DROP 0004 only needs enough policy plumbing to prove that the
damage path is not mandatory.

---

# Success criteria

DROP 0004 is complete when all of the following are true:

1. fragment storage is spatially streamed rather than globally loaded;
2. structural analysis can explicitly demand bounded region residency;
3. fragment persistence is incremental and crash-safe;
4. fragment mesh/collision builds are bounded async work;
5. any structural acceleration is oracle-checked against exact connectivity;
6. an engine-neutral force/damage event can damage static geometry and fragments;
7. damage can accumulate before failure;
8. accepted detachment receives event impulse without violating atomic admission;
9. fragments can be damaged/re-fractured;
10. bounded secondary destruction is possible;
11. fantasy/disabled-physics authored geometry remains valid;
12. the expanded chaos suite and real-hardware acceptance pass;
13. no core crate acquires Bevy, Avian, Minecraft, or game-specific weapon/player
    concepts.

The visual target is not "perfect Teardown" in this drop. The target is the
first robust Micrology loop where force can become progressive local failure,
failure becomes native moving fragments, and those fragments participate in
further bounded destruction — without sacrificing the large-world engine
architecture needed for future games.
