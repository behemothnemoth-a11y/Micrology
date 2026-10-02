//! Material-aware damage, as new implementations of the DROP 0004.6 contract.
//!
//! This module is the payoff for putting `engine_mechanics` *downstream* of
//! `engine_destruction`. [`DamagePolicy`] and [`DamageFailurePolicy`] are traits
//! defined upstream, so making damage material-aware needs no change to the
//! damage contract, the sparse accumulator or anything else DROP 0004 proved.
//! Both policies here are additions sitting beside the uniform references, not
//! replacements for them.
//!
//! # Toughness is used once
//!
//! It would be easy to apply toughness twice — once reducing incoming damage
//! and again raising the failure threshold — and end up with a material far
//! more durable than its numbers say. The split here is deliberate:
//!
//! * **Threshold** comes from `toughness`, and nothing else does. "How much
//!   accumulated damage breaks this cell" is the question `DamageFailurePolicy`
//!   exists to answer.
//! * **Delivery** is modulated by [`FailureCharacter`], and nothing else.
//!   Brittle material transmits more of a hit than it should and shatters;
//!   ductile material absorbs some of it. That is a different property of the
//!   material, not a second helping of the same one.
//!
//! Together they produce the behaviour the drop is for: glass fails to a light
//! tap that barely marks steel.

use crate::{FailureCharacter, MechanicalRegistry};
use engine_core::MaterialId;
use engine_destruction::{
    DamageAmount, DamageEvent, DamageFailurePolicy, DamagePolicy, DamageTarget, DamageWork,
};

/// Delivery modulation per failure character, as `(numerator, denominator)`.
///
/// Integer ratios rather than floats, so that the same hit on the same material
/// always produces the same number.
const fn delivery_ratio(character: FailureCharacter) -> (u64, u64) {
    match character {
        // Transmits the hit rather than deforming under it.
        FailureCharacter::Brittle => (3, 2),
        // Deforms, and spends part of the hit doing so.
        FailureCharacter::Ductile => (3, 4),
        // Crumbles locally, passing the hit on roughly intact.
        FailureCharacter::Granular => (1, 1),
    }
}

/// How much of an event reaches a cell, given what the cell is made of.
///
/// A material with no profile is delivered the event's own amount, which is
/// exactly what `UniformDamagePolicy` does, so an unprofiled world behaves as
/// it did before DROP 0005.
#[derive(Clone, Copy, Debug)]
pub struct MaterialDamagePolicy<'a> {
    registry: &'a MechanicalRegistry,
}

impl<'a> MaterialDamagePolicy<'a> {
    pub const fn new(registry: &'a MechanicalRegistry) -> Self {
        Self { registry }
    }

    /// The amount that would reach a cell of `material` from a raw `amount`.
    pub fn delivered(&self, material: MaterialId, amount: DamageAmount) -> DamageAmount {
        let Some(profile) = self.registry.get(material) else {
            return amount;
        };
        let (numerator, denominator) = delivery_ratio(profile.failure);
        let scaled = (u64::from(amount.0) * numerator) / denominator;
        DamageAmount(scaled.min(u64::from(u32::MAX)) as u32)
    }
}

impl DamagePolicy for MaterialDamagePolicy<'_> {
    fn evaluate(&self, event: &DamageEvent, target: DamageTarget) -> Option<DamageWork> {
        if event.space != target.space() {
            return None;
        }
        let amount = self.delivered(target.material(), event.amount_at_cell(target.cell())?);
        Some(DamageWork {
            event: event.id,
            target,
            amount,
        })
    }
}

/// How much accumulated damage makes a cell fail, given what it is made of.
///
/// Threshold is `toughness` in whole units, never below one so that a profiled
/// material can always eventually be broken. A material with no profile falls
/// back to the uniform threshold the caller supplies.
#[derive(Clone, Copy, Debug)]
pub struct MaterialFailurePolicy<'a> {
    registry: &'a MechanicalRegistry,
    fallback: DamageAmount,
}

impl<'a> MaterialFailurePolicy<'a> {
    pub const fn new(registry: &'a MechanicalRegistry, fallback: DamageAmount) -> Self {
        Self { registry, fallback }
    }

    /// The threshold for `material`, and whether it came from a profile.
    pub fn threshold_of(&self, material: MaterialId) -> (DamageAmount, bool) {
        match self.registry.get(material) {
            Some(profile) => {
                let whole = profile.toughness.raw() / crate::SCALE;
                let clamped = whole.clamp(1, i64::from(u32::MAX)) as u32;
                (DamageAmount(clamped), true)
            }
            None => (self.fallback, false),
        }
    }
}

impl DamageFailurePolicy for MaterialFailurePolicy<'_> {
    fn threshold(&self, target: DamageTarget) -> DamageAmount {
        self.threshold_of(target.material()).0
    }
}
