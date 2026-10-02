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
//! DROP 0005.0 establishes the table. The quantities — density, tensile,
//! compressive and shear capacity, toughness and failure character — arrive in
//! 0005.1, additively.

use engine_core::MaterialId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The mechanical description of one material.
///
/// Deliberately carries no quantities yet. 0005.1 adds them as new fields; the
/// persisted form is a map, so an older file simply lacks the newer keys.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MechanicalProfile {
    pub material: MaterialId,
}

impl MechanicalProfile {
    pub const fn new(material: MaterialId) -> Self {
        Self { material }
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
