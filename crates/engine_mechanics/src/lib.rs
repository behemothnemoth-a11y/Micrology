//! Optional material mechanics and structural capacity.
//!
//! This crate is the sixth separable physical system, not the system the others
//! defer to. It sits downstream of `engine_destruction`, and that placement is
//! deliberate: because [`DamagePolicy`] and [`DamageFailurePolicy`] are traits
//! defined upstream, material-aware damage is a matter of writing new
//! implementations here rather than modifying the DROP 0004.6 contract.
//!
//! [`DamagePolicy`]: engine_destruction::DamagePolicy
//! [`DamageFailurePolicy`]: engine_destruction::DamageFailurePolicy
//!
//! # The rule this crate exists to not break
//!
//! > No core physical system is required for authored world validity.
//!
//! A world that never mentions this crate gets nothing from it. A world that
//! carries an empty [`MechanicalRegistry`] and a silent [`ParticipationPolicy`]
//! behaves exactly as it did in DROP 0004. A floating island stays where it was
//! authored without pretending to be infinitely strong steel.
//!
//! # Determinism
//!
//! Every mechanical quantity is [`Milli`], a signed fixed-point integer. No
//! floating point enters the capacity path at any stage, because a solver whose
//! answer depends on float association order cannot be replayed.
//!
//! # DROP 0005.0 scope
//!
//! This pass establishes vocabulary only: the numeric type, the registry and the
//! participation contract. There is no capacity model, no load path and no
//! failure here yet.

pub mod capacity;
pub mod damage;
pub mod mass;
pub mod policy;
pub mod profile;
pub mod units;

pub use capacity::{
    CapacityDefer, CapacityLimits, CapacityMeasurement, CapacityOutcome, OverloadedConnection,
    evaluate_capacity, load_mode,
};
pub use damage::{MaterialDamagePolicy, MaterialFailurePolicy};
pub use mass::{UNPROFILED_CELL_MASS, density_detachment_impulse, fragment_mass};
pub use policy::{
    Participation, ParticipationPolicy, ParticipationSet, PhysicalSystem, PolicyScope, Resolution,
};
pub use profile::{
    FailureCharacter, LoadMode, MechanicalProfile, MechanicalRegistry, ReferenceMaterial,
    reference_registry,
};
pub use units::{Milli, SCALE};

use serde::{Deserialize, Serialize};

/// Everything this crate persists for one world, as a sidecar document.
///
/// It is a sidecar rather than part of the world file so that DROP 0005 changes
/// no existing format. A world with no sidecar loads exactly as before and gets
/// [`MechanicalSettings::INERT`].
#[derive(Clone, PartialEq, Eq, Default, Debug, Serialize, Deserialize)]
pub struct MechanicalSettings {
    #[serde(default)]
    pub registry: MechanicalRegistry,
    #[serde(default)]
    pub policy: ParticipationPolicy,
}

impl MechanicalSettings {
    /// No profiles, no stated policy: nothing participates in anything.
    pub fn inert() -> Self {
        Self::default()
    }

    /// Whether this world has opted into nothing at all, in which case the
    /// mechanics crate can be skipped entirely rather than run over an empty
    /// table.
    pub fn is_inert(&self) -> bool {
        self.registry.is_empty() && self.policy.is_silent()
    }
}

/// Serialize settings as pretty JSON, matching the engine's transparent,
/// diffable format convention.
pub fn settings_to_json(settings: &MechanicalSettings) -> Result<String, serde_json::Error> {
    let mut json = serde_json::to_string_pretty(settings)?;
    json.push('\n');
    Ok(json)
}

/// Parse settings. Unknown physical systems and unknown policy scopes are
/// ignored rather than rejected, so a world written by a later build still
/// loads.
pub fn settings_from_json(json: &str) -> Result<MechanicalSettings, serde_json::Error> {
    serde_json::from_str(json)
}
