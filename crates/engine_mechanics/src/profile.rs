//! Mechanical identity, kept separate from material identity.
//!
//! `Material` in `engine_core` is an id, a colour and a name. It stays that way.
//! Mechanical properties live here, in a side table keyed by [`MaterialId`], for
//! three reasons:
//!
//! * `engine_core` is the vocabulary every crate shares and is deliberately
//!   dependency-light. Meshing, streaming and I/O have no use for capacity.
//! * Mechanics must be omittable. A registry that may legitimately be empty says
//!   "this world has no mechanical model" far better than a `Material` whose
//!   mechanical fields are all defaulted and therefore always present.
//! * Making strength a property of `Material` is one short step from making
//!   support a property of `Material`, which is explicitly forbidden.
//!
//! DROP 0005.0 established the table; 0005.1 added the quantities. Everything
//! here is still inert: nothing in this module decides anything. Capacity is
//! *consulted* starting in 0005.4.

use crate::Milli;
use engine_core::MaterialId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How a material gives way once its capacity is exceeded.
///
/// In this drop the tag only selects a reference response. It is recorded now
/// because it is authored data, and authored data is cheaper to carry from the
/// start than to retrofit into saved worlds later.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash, Debug, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum FailureCharacter {
    /// Fails abruptly at capacity with no warning: glass.
    #[default]
    Brittle,
    /// Deforms before it fails: steel, and wood in bending.
    Ductile,
    /// Loses cohesion progressively: masonry, concrete, rubble.
    Granular,
}

/// Which capacity a load is tested against.
///
/// Direction is resolved on the mechanics side, from flow geometry relative to
/// the face normal, which is why no direction had to be threaded through
/// `DamageEvent` or `DamageWork`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadMode {
    Tensile,
    Compressive,
    Shear,
}

impl LoadMode {
    pub const ALL: [Self; 3] = [Self::Tensile, Self::Compressive, Self::Shear];
}

/// The mechanical description of one material.
///
/// Every field defaults to zero so that a profile written by an older build
/// loads without inventing values. A profile whose capacities are all zero is a
/// material that carries nothing — a legitimate authored state for decoration,
/// and *not* the same as having no profile at all.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MechanicalProfile {
    pub material: MaterialId,
    /// Mass contributed by one occupied cell.
    #[serde(default)]
    pub density: Milli,
    /// Capacity in pure tension, where masonry is weakest.
    #[serde(default)]
    pub tensile: Milli,
    /// Capacity in pure compression, where masonry is strongest.
    #[serde(default)]
    pub compressive: Milli,
    /// Capacity across a face.
    #[serde(default)]
    pub shear: Milli,
    /// Resistance to applied damage, consumed by 0005.2's damage policy. This
    /// is deliberately not a capacity: taking a hit and carrying a load are
    /// different questions about the same material.
    #[serde(default)]
    pub toughness: Milli,
    #[serde(default)]
    pub failure: FailureCharacter,
}

impl MechanicalProfile {
    /// A profile that exists but carries and resists nothing.
    pub const fn inert(material: MaterialId) -> Self {
        Self {
            material,
            density: Milli::ZERO,
            tensile: Milli::ZERO,
            compressive: Milli::ZERO,
            shear: Milli::ZERO,
            toughness: Milli::ZERO,
            failure: FailureCharacter::Brittle,
        }
    }

    /// The capacity this material offers against one load mode.
    pub const fn capacity(&self, mode: LoadMode) -> Milli {
        match mode {
            LoadMode::Tensile => self.tensile,
            LoadMode::Compressive => self.compressive,
            LoadMode::Shear => self.shear,
        }
    }

    /// Whether this profile can carry anything at all in any mode.
    pub fn carries_load(&self) -> bool {
        LoadMode::ALL
            .into_iter()
            .any(|mode| self.capacity(mode).is_positive())
    }
}

/// The mechanical profiles a world knows about.
///
/// Being empty is a valid, expected and fully supported state: it is what every
/// world authored before DROP 0005 has, and what a fantasy world may keep
/// forever. A material with no profile has no mechanical identity, which is not
/// the same as having a defaulted one.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct MechanicalRegistry {
    profiles: BTreeMap<MaterialId, MechanicalProfile>,
}

impl MechanicalRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace, returning the previous profile.
    pub fn insert(&mut self, profile: MechanicalProfile) -> Option<MechanicalProfile> {
        self.profiles.insert(profile.material, profile)
    }

    /// The profile for `material`, or `None` if it has no mechanical identity.
    ///
    /// Callers must treat `None` as "does not participate mechanically", never
    /// as "participates with default values".
    pub fn get(&self, material: MaterialId) -> Option<MechanicalProfile> {
        self.profiles.get(&material).copied()
    }

    pub fn contains(&self, material: MaterialId) -> bool {
        self.profiles.contains_key(&material)
    }

    pub fn remove(&mut self, material: MaterialId) -> Option<MechanicalProfile> {
        self.profiles.remove(&material)
    }

    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    /// Profiles in ascending material id order.
    pub fn iter(&self) -> impl Iterator<Item = MechanicalProfile> + '_ {
        self.profiles.values().copied()
    }

    /// Material ids in ascending order.
    pub fn materials(&self) -> impl Iterator<Item = MaterialId> + '_ {
        self.profiles.keys().copied()
    }
}

impl Serialize for MechanicalRegistry {
    /// Written as a sorted list, matching how `MaterialRegistry` is persisted,
    /// so that keys never have to be stringified.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.profiles.values())
    }
}

impl<'de> Deserialize<'de> for MechanicalRegistry {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let profiles = Vec::<MechanicalProfile>::deserialize(deserializer)?;
        let mut registry = Self::new();
        for profile in profiles {
            registry.insert(profile);
        }
        Ok(registry)
    }
}

/// Illustrative mechanical profiles for familiar materials.
///
/// **This is example data, not engine truth.** The numbers are plausible
/// real-world magnitudes — density in g/cm³, capacities in MPa — chosen so that
/// the relationships between materials are recognisable: masonry is an order of
/// magnitude weaker in tension than in compression, glass is strong until it is
/// not, steel carries far more than anything else and weighs accordingly.
///
/// A game is expected to supply its own values. Nothing in the engine reads
/// this set unless a caller asks for it, and no engine behaviour depends on any
/// particular number in it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ReferenceMaterial {
    Wood,
    Masonry,
    Concrete,
    Steel,
    Glass,
}

impl ReferenceMaterial {
    /// Declaration order, which is also canonical iteration order.
    pub const ALL: [Self; 5] = [
        Self::Wood,
        Self::Masonry,
        Self::Concrete,
        Self::Steel,
        Self::Glass,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Wood => "wood",
            Self::Masonry => "masonry",
            Self::Concrete => "concrete",
            Self::Steel => "steel",
            Self::Glass => "glass",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|material| material.name() == name)
    }

    /// `(density, tensile, compressive, shear, toughness, failure)`.
    const fn values(self) -> (i64, i64, i64, i64, i64, FailureCharacter) {
        match self {
            // Light, and far better in tension than masonry despite weighing a
            // third as much.
            Self::Wood => (
                600,
                40_000,
                30_000,
                7_000,
                20_000,
                FailureCharacter::Ductile,
            ),
            // The classic: enormous in compression, almost nothing in tension.
            // An arch stands; a masonry beam does not.
            Self::Masonry => (2_000, 400, 20_000, 1_200, 8_000, FailureCharacter::Granular),
            Self::Concrete => (
                2_400,
                3_000,
                35_000,
                5_000,
                12_000,
                FailureCharacter::Granular,
            ),
            // Carries an order of magnitude more than anything else here, and
            // is three times the density of masonry.
            Self::Steel => (
                7_850,
                400_000,
                400_000,
                250_000,
                100_000,
                FailureCharacter::Ductile,
            ),
            // Strong on paper, but it takes a hit once. Toughness is what
            // separates it from concrete, not capacity.
            Self::Glass => (
                2_500,
                50_000,
                300_000,
                35_000,
                2_000,
                FailureCharacter::Brittle,
            ),
        }
    }

    /// The reference profile for this material, bound to a caller-chosen id.
    ///
    /// Ids are world data, so the engine never picks one. A game maps its own
    /// `MaterialId`s onto whichever of these it wants as a starting point.
    pub fn profile(self, material: MaterialId) -> MechanicalProfile {
        let (density, tensile, compressive, shear, toughness, failure) = self.values();
        MechanicalProfile {
            material,
            density: Milli(density),
            tensile: Milli(tensile),
            compressive: Milli(compressive),
            shear: Milli(shear),
            toughness: Milli(toughness),
            failure,
        }
    }
}

/// Build a registry from caller-supplied id assignments.
///
/// Later entries win, so a caller may override a reference profile by inserting
/// its own afterwards.
pub fn reference_registry(
    assignments: impl IntoIterator<Item = (ReferenceMaterial, MaterialId)>,
) -> MechanicalRegistry {
    let mut registry = MechanicalRegistry::new();
    for (reference, id) in assignments {
        registry.insert(reference.profile(id));
    }
    registry
}
