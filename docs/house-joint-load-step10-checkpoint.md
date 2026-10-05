# Reference house Step 10 — why automatic joint failure leaves one large body

Date: 2026-10-05.
Base: `9127764` (Step 9 automatic finite-strength authored joints).
Branch: `chatgpt/house-joint-load-step9-20261004`.
Status: diagnosis complete; the Step 10 size goal is **not met**, and the
evidence says it cannot be met by tuning the static solver. One correctness bug
found and fixed. No live GPU run performed in this environment.

## The question

Step 9's automatic result detaches a largest body of **87,188 cells**. The
welded control was **43,564** and the explicit all-joints-release control was
**2,274**. The automatic result is therefore *worse* than the welded control it
was meant to improve on. This pass asked why, and whether a bounded fix exists.

The answer is not a tuning constant. Four candidate causes were tested and three
were refuted by measurement.

## What the 87,188-cell body actually is

Instrumenting the solve and the resulting detachment:

| Measurement | Value |
|---|---:|
| graph parts / interfaces / augmentations | 175 / 462 / 249 |
| authored interfaces in the graph | 447 |
| minimum cut: part-pairs / bonds | 54 / 1,048 |
| rigid (non-authored) interfaces in the cut | 0 |
| distinct authored parts inside the largest body | **131** |
| authored joint faces **interior** to that body | **10,393** |
| of those, broken by the cut | **0** |
| distinct authored part-pairs welded inside it | 326 |
| cross-part faces **not** represented as authored joints | **0** |

The cut's 1,048 bonds by material pair: wood/wood 324, wood/glass 486,
masonry/masonry 238. The 486 wood/glass faces are the zero-capacity glazing
interfaces, which carry no load and are free for the cut to pass through, so the
load-bearing part of the cut is 562 faces.

Geometry: the cut is a roughly horizontal surface at y≈4..44; the detached body
is the entire wood+masonry superstructure (67,930 wood + 19,258 masonry), and
the four other fragments are pure glazing panels of 400/320/320/280 cells.

So the large body is **one legitimate static assembly**: 131 authored parts held
together by 10,393 interior authored joints, none of which the cut touched.

## Hypotheses tested

### Refuted: the authored interface representation is incomplete

Hypothesis (B) was that parts stay welded through faces not modelled as
construction joints. Measured: **0** cross-part faces inside the large body are
unrepresented. Every face between two different authored parts is already an
authored joint. The representation is complete.

### Refuted: the monolithic concrete foundation hides where support is

The foundation is a single graph node (`label=1`, 29,568 cells, the only
anchored part; 7,616 world anchors, all concrete, spanning x 0..79). That looked
like a real modelling defect: load from x=127 routes to the same sink as load
from x=0, so the graph has no spatial notion of support.

Decomposing the slab into spatial bays was prototyped at bay sizes 128, 64, 32,
24 and 16 cells, giving 1, 3, 9, 16 and 29 anchored parts respectively. The cut
was **identical in every case**: 54 pairs, 1,048 bonds, 0 rigid. Concrete's own
internal interfaces are not authored joints, so they keep full concrete capacity
and the slab still distributes load horizontally — which is also what a real
slab does. The monolith is not the cause.

### Refuted: one generation is simply not enough

Hypothesis (A). Re-solving the **attached remainder** after the first cut
returns `Satisfied` immediately: the supported half is sound, so there is no
second generation to run. Nothing further fails.

Bounding the failure to a few interfaces per generation instead was prototyped
at 1, 4, 8 and 16 interfaces per generation with a 40-generation ceiling. Every
configuration **stalls after generation 1** and detaches nothing at all:

| interfaces/generation | gen 1 bonds | fragments | outcome |
|---:|---:|---:|---|
| 1 | 24 | 0 | gen 2 holds |
| 4 | 74 | 0 | gen 2 holds |
| 8 | 129 | 0 | gen 2 holds |
| 16 | 287 | 0 | gen 2 holds |
| 54 (all) | 1,048 | 5 | settles, largest 87,188 |

A partial cut neither separates the body topologically — it is still held by the
remaining live interfaces — nor leaves anything for the next generation to name.
Once an interface is dead it has zero capacity, so the minimum cut prefers to
route through it, and the next cut degenerates onto the already-dead set, with
no live authored bonds left to nominate. Progressive generations do not help.

### Refuted: a per-interface sole-load-path test would name interior joints

A tributary-load model was prototyped: for each authored interface, the demand
that loses every anchor path when that interface is removed, compared against
its capacity. A joint that is the only load path for more weight than it can
carry is overloaded.

Result: **0 overloaded interfaces**, on both the intact and the
half-foundation house. The part graph is too redundant for any single joint to
be a sole load path. The shortfall is collective, which is exactly what the
minimum cut already measures.

### Confirmed: the static model cannot express this failure

Minimum cut yields, by construction, exactly **one** bipartition per solve. The
source side becomes one unsupported region; everything in it detaches as a
single body. The 10,393 interior joints are genuinely *not* overloaded while the
assembly is standing and connected — they are carrying their load successfully.

`engine_mechanics/src/collapse.rs` already states the governing doctrine:

> A structure that reaches no anchor at all is *unsupported*, not
> *under-strength* [...] Mechanics may only ever add a failure reason to geometry
> that is genuinely attached — it may not pre-empt, duplicate or second-guess
> the topological answer.

So re-solving the detached body as a free body is ruled out by design, and
correctly: a body in free fall has no internal gravity stress. Its interior
joints are loaded when it **bends, twists and lands** — not while it stands and
not while it falls.

That is why the explicit-release control reaches 2,274 cells. It does not
compute a smaller failure; it pre-breaks all 447 authored interfaces before the
cut. It is a control, not a mechanism.

## Conclusion

The Step 9 solver is doing its job correctly. The 87,188-cell body is the right
static answer to the question asked. The Step 10 size goal is not reachable by
changing part labels, foundation decomposition, generation counts, or the
interface overload test, because **static self-weight routed to an anchor is
structurally incapable of naming more than one failure surface per solve**.

Reaching a materially smaller body needs load that only exists once the body is
in motion — bending and torque in a detached assembly, and impact when it
lands. That is the boundary the Step 9 checkpoint already named as next, and
this pass is the measurement that confirms it is genuinely the blocker rather
than a tuning problem.

Two candidate architectures, neither implemented here, both needing their own
evidence:

1. **Impact-driven interior failure through the existing contact path.** The
   explicit-release live run already produced 3,425 contact-broken bonds and 113
   contact-created fragments, so the mechanism exists. The Step 9 live run
   recorded **zero** contact-fracture jobs because the detached assembly rested
   against the surviving half instead of falling. This needs a live run to test,
   and may be a scenario-geometry question rather than a solver question.
2. **A bounded construction-level bending/torque approximation** for a detached
   assembly, loading its interior authored joints from its own span and support
   footprint rather than from gravity-to-anchor. This is new physics and must
   not be introduced without measurements that interpret it.

Do not reach the number by lowering `AUTO_JOINT_STRENGTH_MILLI` until the intact
house falls down, by pre-releasing interfaces, or by a global fragment-size
rule. All three would produce the metric without the mechanism.

## What was fixed

The solver read the live world's geometry but ignored its fracture state, so an
already-broken authored joint still contributed full capacity and was still
nominated as a failure candidate. `build` now takes the `FractureState`: a face
whose bond has zero integrity contributes no capacity and is not a candidate.

This mattered twice. A house carrying prior damage was reported sounder than it
is. And a second generation could not advance, because it re-nominated spent
bonds — which is how the generational experiment above was able to distinguish
"stalls because the model cannot progress" from "stalls because the solver
forgot what it already broke".

The committed scenario is unchanged, because the intact and half-foundation
cases carry no prior joint damage.

## Invariants held

- No change to normal house startup: still zero seeded fracture records.
- No change to any non-house caller's defaults.
- Exact occupancy and fracture-aware connectivity remain topology authority.
- Budget exhaustion still returns `Deferred`, and `Deferred` is still an error
  at the scenario boundary, never permission to destroy.
- A rigid/non-authored cut interface still refuses.
- Cell accounting still closes exactly.
- No save-schema change. No new fixtures. No Bevy/Avian types in engine crates.
- The explicit catastrophic release remains a comparison control only.

## Not done in this environment

No live GPU validation. This container has no Vulkan device (`/dev/dri` absent),
so the Windows/RTX 5050 run remains a handoff. Nothing in this checkpoint claims
a visual result.
