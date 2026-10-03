//! Density-aware fragment mass and impulse coupling.
//!
//! DROP 0004.8 approximated a fragment's mass as its occupied-cell count and
//! said so in a comment: material density was deferred to this drop. The
//! reference function it shipped,
//! [`reference_detachment_impulse`](engine_destruction::reference_detachment_impulse),
//! stays exactly where it is and keeps that behaviour. This module adds the
//! density-aware version beside it rather than replacing it, which is what the
//! "exact/reference implementations are retained" invariant asks for.
//!
//! # Why mass is `Milli` but the impulse is not
//!
//! Mass aggregation is integer, so a fragment's mass is the same number
//! regardless of what order its cells were visited in. The impulse arithmetic
//! that follows is `f64`, because `DamageImpulse` has carried an `f64` vector
//! since 0004.6 and motion is not the capacity path. The determinism rule this
//! drop is strict about applies to capacity decisions; a velocity delta is not
//! one.

use crate::{MechanicalRegistry, Milli};
use engine_core::{CellSource, GlobalPos};
use engine_destruction::{DamageEvent, Fragment, FragmentImpulse};

/// Mass contributed by one cell whose material has no mechanical profile.
///
/// One whole unit per cell is precisely the DROP 0004 approximation, so a
/// fragment made entirely of unprofiled material aggregates to its cell count
/// and behaves exactly as it did before this drop.
pub const UNPROFILED_CELL_MASS: Milli = Milli::ONE;

/// Aggregate a fragment's mass from the density of what it is made of.
///
/// Deterministic and order-independent: addition saturates in integers, and
/// `occupied_cells` already yields canonical order.
pub fn fragment_mass(fragment: &Fragment, registry: &MechanicalRegistry) -> Milli {
    fragment
        .occupied_cells()
        .map(|cell| match fragment.material_at(cell) {
            Some(material) => registry
                .get(material)
                .map(|profile| profile.density)
                .unwrap_or(UNPROFILED_CELL_MASS),
            None => Milli::ZERO,
        })
        .fold(Milli::ZERO, Milli::saturating_add)
}

/// Mass as the impulse arithmetic needs it, never below one cell's worth.
///
/// A fragment of weightless material would otherwise divide by zero and launch
/// itself at infinity; clamping keeps a decorative material inert rather than
/// explosive.
fn effective_mass(fragment: &Fragment, registry: &MechanicalRegistry) -> f64 {
    let mass = fragment_mass(fragment, registry);
    if mass.is_positive() {
        mass.to_f64_lossy()
    } else {
        UNPROFILED_CELL_MASS.to_f64_lossy()
    }
}

/// Density-aware coupling for a newly detached fragment.
///
/// Mirrors `reference_detachment_impulse` exactly except for where mass comes
/// from: new detachments start with identity rotation, so the local centre can
/// be used directly for a torque estimate without the engine interpreting a
/// backend quaternion.
pub fn density_detachment_impulse(
    event: &DamageEvent,
    fragment: &Fragment,
    registry: &MechanicalRegistry,
) -> Option<FragmentImpulse> {
    let vector = event.impulse_world?.vector();
    let mass = effective_mass(fragment, registry);

    let linear = [
        saturate_f32(vector[0] / mass),
        saturate_f32(vector[1] / mass),
        saturate_f32(vector[2] / mass),
    ];

    let centre_local = fragment.local_centre();
    let centre = GlobalPos::new(
        fragment.pose.translation.x + centre_local[0],
        fragment.pose.translation.y + centre_local[1],
        fragment.pose.translation.z + centre_local[2],
    );
    let source = event.source_world.unwrap_or(centre);
    let lever = [
        centre.x - source.x,
        centre.y - source.y,
        centre.z - source.z,
    ];
    let torque = [
        lever[1] * vector[2] - lever[2] * vector[1],
        lever[2] * vector[0] - lever[0] * vector[2],
        lever[0] * vector[1] - lever[1] * vector[0],
    ];
    let inertia = mass * fragment.bounding_radius().powi(2).max(1.0);
    let angular = [
        saturate_f32(torque[0] / inertia),
        saturate_f32(torque[1] / inertia),
        saturate_f32(torque[2] / inertia),
    ];

    Some(FragmentImpulse {
        linear_delta: linear,
        angular_delta: angular,
    })
}

fn saturate_f32(value: f64) -> f32 {
    value.clamp(f64::from(f32::MIN), f64::from(f32::MAX)) as f32
}
