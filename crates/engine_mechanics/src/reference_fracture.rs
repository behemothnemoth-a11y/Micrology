//! Opt-in impact-fracture example data, separate from static-load mechanics.
//!
//! Revision 1 is a diagnostic starting point, NOT measured material science.
//! These are effective scalar impact settings in existing engine energy units;
//! no MPa/Joule conversion, bending, grain, mortar, or shatter pattern is implied.
//! A host supplies its own MaterialIds and explicitly constructs an owned policy.
use crate::ReferenceMaterial;
use engine_destruction::{DamageAmount, FractureProfile};

pub const REFERENCE_FRACTURE_REVISION: u32 = 1;
pub const REFERENCE_FRACTURE_SET: [ReferenceMaterial; 4] = [
    ReferenceMaterial::Wood,
    ReferenceMaterial::Masonry,
    ReferenceMaterial::Concrete,
    ReferenceMaterial::Glass,
];

/// Reference impact response, not a reinterpretation of MechanicalProfile.
/// Steel deliberately has no new preset in this four-material checkpoint.
/// Density mirrors existing example metadata; fracture does not consume it,
/// and this does not change runtime body mass or establish physical cell scale.
pub const fn reference_fracture_profile(material: ReferenceMaterial) -> Option<FractureProfile> {
    let (density, toughness, crush, tension, compression, shear) = match material {
        // Retains bonds longer; no claim of actual elastic bending or splinters.
        ReferenceMaterial::Wood => (600, 750, 24_000, 700, 850, 360),
        // Low tension/shear relative to compression; can crack before crushing.
        ReferenceMaterial::Masonry => (2_000, 250, 8_000, 90, 1_100, 260),
        // More resistant than masonry in all three impact-loading modes.
        ReferenceMaterial::Concrete => (2_400, 500, 16_000, 240, 1_800, 550),
        // Impact-fragile, with a gap between bond failure and cell erasure.
        // These are NOT the high ideal static-load capacities of intact glass.
        ReferenceMaterial::Glass => (2_500, 50, 4_500, 100, 600, 140),
        ReferenceMaterial::Steel => return None,
    };
    Some(FractureProfile {
        density_milli: density,
        toughness_milli: toughness,
        crush: DamageAmount(crush),
        tensile: DamageAmount(tension),
        compressive: DamageAmount(compression),
        shear: DamageAmount(shear),
    })
}
