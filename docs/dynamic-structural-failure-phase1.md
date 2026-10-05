# Dynamic structural failure — phase 1 motion/contact probe

Branch starts from the accepted static-load diagnosis at `6a2ced3`.

This pass is intentionally narrow. It does **not** add a dynamic load solver,
change authored-joint strength, change the static minimum-cut solver, or claim
the collapse-size target is solved.

## Runtime path traced

The existing live path is already motion-aware at the contact boundary:

1. Avian advances dynamic fragment bodies.
2. `contact_fracture::collect` runs after `PhysicsSystems::Last`.
3. For a newly touching native body pair it samples solved normal impulse and
   pre-solve closing speed.
4. `ContactFracturePolicy` rejects resting support and quantizes a bounded
   fracture budget.
5. The hit enters the normal fracture worker as `Work::Impact`.
6. The existing fracture transaction owns bond damage, connectivity and any
   resulting fragment split.

The contact source is edge-triggered per native body pair. A resting pair does
not repeatedly inject energy.

The fragment physics bridge is also live: reference-house mode uses gravity
`[0, -196.2, 0]` in 20-cells-per-metre units, and dynamic fragments are
spawned as `RigidBody::Dynamic`.

## Why the Step 9 live result sat still

The automatic half-foundation case creates five dynamic fragments, including
the 87,188-cell superstructure fragment, but the recorded Step 9 run leaves
them essentially at their spawn transforms and sleeping. That means the
automatic static failure did not create the relative motion needed to enter
the contact-fracture feedback path.

## Smallest motion probe

To separate "contact fracture is insufficient" from "the scenario never
reaches contact fracture", this probe reuses the exact automatic case and does
not change strength or solver logic.

Replay sequence:

- build the reference house
- pause
- keep contact fracture disabled
- run `foundation_half_auto_joints`
- record the normal post-failure state
- select the largest native fragment
- enable contact fracture
- apply one bounded downward velocity edit of 24 cells/s
- advance 60 fixed ticks
- advance to 180 fixed ticks

The velocity edit is a diagnostic wake-up only. It is not proposed as product
mechanics and does not satisfy the requirement for naturally emerging motion.

The run executed on the Windows RTX 5050 Laptop GPU through Vulkan.

## Result

| Measurement | after failure | 60 ticks | 180 ticks |
| --- | ---: | ---: | ---: |
| static cells | 51,036 | 51,036 | 51,036 |
| fragment cells | 88,508 | 88,508 | 88,508 |
| fragments | 5 | 5 | 5 |
| largest fragment | 87,188 | 87,188 | 87,188 |
| contact samples | 0 | 4 | 4 |
| contact hits processed | 0 | 8 | 8 |
| contact-broken bonds | 0 | 387 | 387 |
| contact-created fragments | 0 | 0 | 0 |
| fragment failed cells | 0 | 0 | 0 |

At 60 ticks the existing contact path has therefore become active: four
contact pairs produced eight participant hits and **387 broken bonds**.
Three sampled contacts were fragment-to-fragment pairs.

However, those bond breaks do not split the 87,188-cell fragment. By 180 ticks
the numbers are unchanged and the largest fragment is sleeping again.

## First conclusion

The existing impact/contact machinery is **partially sufficient**:

- it already converts real solved motion/contact into fracture-state damage;
- the Step 9 zero-contact result was not evidence that the path was dead;
- merely entering that path is not enough to produce a structural cascade.

The next smallest question is now narrower than "build dynamic structural
failure":

> Which fragment/space received the 387 broken bonds, and why did those breaks
> not cross enough interior connectivity to split the 87,188-cell assembly?

The next pass should add attribution/diagnostics for contact-generated bond
breaks (at minimum static vs fragment, preferably by fragment ID) and inspect
whether the large assembly is accumulating only local surface damage or
meaningful interior joint damage.

Do **not** implement a new bending/torque solver until that attribution is
measured.

## Evidence

- `docs/diagnostics/dynamic_structural_failure_phase1_20261005/forced_motion_probe.json`
- `docs/diagnostics/dynamic_structural_failure_phase1_20261005/forced_motion_gpu.log`
- `docs/diagnostics/dynamic_structural_failure_phase1_20261005/after_failure.json`
- `docs/diagnostics/dynamic_structural_failure_phase1_20261005/after_60.json`
- `docs/diagnostics/dynamic_structural_failure_phase1_20261005/after_180.json`
