# Dynamic structural failure — phase 2 contact-bond classification

This pass answers one narrow question left by phase 1:

> Of the contact-generated broken bonds, how many are actual authored construction joints?

No strength, solver, fracture policy, contact policy, or physics behavior changed.

## Method

The sandbox dump now reconstructs broken fragment-local bond keys back into the
reference house's original source coordinates using each fragment's
`source_origin`. It compares those keys to the canonical
`reference_house_joints::authored_joint_bonds(reference_house::build())` set.

The exact same forced-motion probe from phase 1 was then rerun on the Windows
RTX 5050 Laptop GPU through Vulkan.

## Result

Total contact-broken bonds: **385**

| Fragment | Cells | Broken bonds | Authored-joint breaks | Non-authored breaks |
| --- | ---: | ---: | ---: | ---: |
| f0.0 | 87,188 | 274 | **12** | **262** |
| f0.1 | 400 | 37 | 0 | 37 |
| f0.3 | 320 | 37 | 0 | 37 |
| f0.4 | 320 | 37 | **6** | 31 |
| Static world | - | 0 | 0 | 0 |

Only **18 / 385 (4.7%)** of the contact-generated broken bonds are authored
construction joints.

For the 87,188-cell main body specifically, only **12 / 274 (4.4%)** are
authored construction joints. The other **262 / 274 (95.6%)** are ordinary
local/material bonds.

The earlier static diagnosis measured 10,393 authored joint faces inside this
main body, so this contact event broke roughly **0.12%** of that authored-joint
network.

## Spatial result

The damage is strongly localized to a thin contact patch on the high-X side of
the house.

For f0.0, all 274 broken bonds lie inside source-space bounds:

- X: **121..123**
- Y: **39..49**
- Z: **36..73**

Axis counts:

- X: 4
- Y: 92
- Z: 178

The 12 authored-joint breaks in f0.0 are even more localized. They are all
Z-axis bonds at:

- X = 121..123
- Y = 45 or 46
- Z = 43 or 65

That is exactly 3 x 2 x 2 = 12 authored bonds.

The six authored breaks in f0.4 are also all Z-axis bonds, at:

- X = 122
- Y = 39..44
- Z = 69

## Conclusion

The existing contact-fracture path is behaving like **local impact damage**.
It is not meaningfully transferring impact/bending demand into the authored
construction-joint network.

That explains the phase-1 result:

- motion activates contact fracture,
- hundreds of bonds break,
- but almost all of them are local surface/material bonds,
- so the 131-part assembly retains essentially all of its authored structural
  connectivity,
- and the 87,188-cell body remains one fragment.

This is stronger evidence that simply increasing contact damage or lowering
joint strength would be the wrong next move.

The next smallest architecture question is:

> How should motion, bending, torque, or impact demand be transferred from a
> moving authored part into the authored interfaces that connect it to the rest
> of the assembly?

Do not implement that model yet. First trace the existing authored-part graph
and identify the minimum runtime signals already available for relative part
motion / impulse transfer.

## Evidence

- `docs/diagnostics/dynamic_structural_failure_phase2_20261005/joint_classification_after_180.json`
- `docs/diagnostics/dynamic_structural_failure_phase2_20261005/joint_classification_gpu.log`
