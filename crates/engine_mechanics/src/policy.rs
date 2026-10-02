//! Which physical systems an object participates in.
//!
//! The hard rule is that no core physical system is required for authored world
//! validity. A floating island, impossible castle or magical bridge is valid
//! data and must not need a fake infinite-strength material to stay up. This
//! module is how a host says which systems it is opting a thing into.
//!
//! Two properties make that rule enforceable rather than aspirational:
//!
//! 1. **Participation is per system, not one global switch.** An object can take
//!    weapon damage, shed fragments and obey gravity while never participating
//!    in continuous structural capacity.
//! 2. **The engine default is that nothing participates.** A world that never
//!    mentions policy gets no physical behaviour from this crate at all, which
//!    is exactly what a DROP 0004 world already does.

use serde::de::{Deserializer, MapAccess, Visitor};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// The separable physical systems.
///
/// These are the seven concerns DROP 0004 required stay independent. Adding one
/// here is additive: an older save that has never heard of it simply leaves it
/// inheriting.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum PhysicalSystem {
    Gravity,
    Anchoring,
    ConnectivityDetachment,
    AppliedDamage,
    FragmentPhysics,
    MaterialMechanics,
    StructuralCapacity,
}

impl PhysicalSystem {
    /// Declaration order, which is also the canonical iteration order.
    pub const ALL: [Self; 7] = [
        Self::Gravity,
        Self::Anchoring,
        Self::ConnectivityDetachment,
        Self::AppliedDamage,
        Self::FragmentPhysics,
        Self::MaterialMechanics,
        Self::StructuralCapacity,
    ];

    /// The stable name used in persistence. Never derived from the variant, so
    /// renaming the variant cannot silently invalidate saved worlds.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Gravity => "gravity",
            Self::Anchoring => "anchoring",
            Self::ConnectivityDetachment => "connectivity_detachment",
            Self::AppliedDamage => "applied_damage",
            Self::FragmentPhysics => "fragment_physics",
            Self::MaterialMechanics => "material_mechanics",
            Self::StructuralCapacity => "structural_capacity",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|system| system.name() == name)
    }

    const fn index(self) -> usize {
        match self {
            Self::Gravity => 0,
            Self::Anchoring => 1,
            Self::ConnectivityDetachment => 2,
            Self::AppliedDamage => 3,
            Self::FragmentPhysics => 4,
            Self::MaterialMechanics => 5,
            Self::StructuralCapacity => 6,
        }
    }
}

/// A tri-state so that an override can also turn something *off*.
///
/// Two states would make a more specific scope unable to disable what a broader
/// one enabled, which is the case a fantasy region most needs.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash, Debug, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Participation {
    /// Defer to a less specific scope.
    #[default]
    Inherit,
    Enable,
    Disable,
}

/// Where a decision was expressed. Most specific wins.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum PolicyScope {
    /// The engine/game-wide default.
    Engine,
    /// One world.
    World,
    /// A region, structure or object within a world.
    Object,
    /// A gameplay-supplied override: the magical bridge that is simply held up.
    Gameplay,
}

impl PolicyScope {
    /// Resolution order. The first scope here that expresses a decision wins,
    /// which is what "most specific wins" means operationally.
    pub const MOST_SPECIFIC_FIRST: [Self; 4] =
        [Self::Gameplay, Self::Object, Self::World, Self::Engine];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Engine => "engine",
            Self::World => "world",
            Self::Object => "object",
            Self::Gameplay => "gameplay",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::MOST_SPECIFIC_FIRST
            .into_iter()
            .find(|scope| scope.name() == name)
    }
}

/// One scope's opinion about each system.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct ParticipationSet {
    entries: [Participation; 7],
}

impl ParticipationSet {
    /// Expresses no opinion at all.
    pub const INHERIT_ALL: Self = Self {
        entries: [Participation::Inherit; 7],
    };

    pub const fn new() -> Self {
        Self::INHERIT_ALL
    }

    #[inline]
    pub const fn get(&self, system: PhysicalSystem) -> Participation {
        self.entries[system.index()]
    }

    #[inline]
    pub fn set(&mut self, system: PhysicalSystem, participation: Participation) {
        self.entries[system.index()] = participation;
    }

    /// Builder form, so a scope can be written as one expression.
    #[must_use]
    pub fn with(mut self, system: PhysicalSystem, participation: Participation) -> Self {
        self.set(system, participation);
        self
    }

    #[must_use]
    pub fn enabling(self, system: PhysicalSystem) -> Self {
        self.with(system, Participation::Enable)
    }

    #[must_use]
    pub fn disabling(self, system: PhysicalSystem) -> Self {
        self.with(system, Participation::Disable)
    }

    pub fn is_silent(&self) -> bool {
        self.entries
            .iter()
            .all(|entry| *entry == Participation::Inherit)
    }

    /// Systems with an explicit opinion, in canonical order.
    pub fn stated(&self) -> impl Iterator<Item = (PhysicalSystem, Participation)> + '_ {
        PhysicalSystem::ALL
            .into_iter()
            .map(|system| (system, self.get(system)))
            .filter(|(_, participation)| *participation != Participation::Inherit)
    }
}

impl Serialize for ParticipationSet {
    /// Only stated systems are written, so a silent scope costs an empty map and
    /// a future system is absent rather than wrongly recorded as disabled.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let stated: Vec<_> = self.stated().collect();
        let mut map = serializer.serialize_map(Some(stated.len()))?;
        for (system, participation) in stated {
            map.serialize_entry(system.name(), &participation)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for ParticipationSet {
    /// Unknown system names are ignored rather than rejected: a world written by
    /// a later build must still load, exactly as the cell format ignores unknown
    /// keys.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SetVisitor;

        impl<'de> Visitor<'de> for SetVisitor {
            type Value = ParticipationSet;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a map of physical system name to participation")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut set = ParticipationSet::INHERIT_ALL;
                while let Some((key, value)) = map.next_entry::<String, Participation>()? {
                    if let Some(system) = PhysicalSystem::from_name(&key) {
                        set.set(system, value);
                    }
                }
                Ok(set)
            }
        }

        deserializer.deserialize_map(SetVisitor)
    }
}

/// The resolved answer plus where it came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Resolution {
    pub participates: bool,
    /// `None` when no scope stated an opinion and the engine default applied.
    pub decided_by: Option<PolicyScope>,
}

/// The full policy stack.
///
/// A policy that states nothing resolves every system to "not participating",
/// which is the DROP 0004 world exactly.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct ParticipationPolicy {
    scopes: BTreeMap<PolicyScope, ParticipationSet>,
}

impl ParticipationPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace one scope's opinion.
    pub fn set_scope(&mut self, scope: PolicyScope, set: ParticipationSet) {
        if set.is_silent() {
            self.scopes.remove(&scope);
        } else {
            self.scopes.insert(scope, set);
        }
    }

    #[must_use]
    pub fn with_scope(mut self, scope: PolicyScope, set: ParticipationSet) -> Self {
        self.set_scope(scope, set);
        self
    }

    pub fn scope(&self, scope: PolicyScope) -> ParticipationSet {
        self.scopes
            .get(&scope)
            .copied()
            .unwrap_or(ParticipationSet::INHERIT_ALL)
    }

    pub fn is_silent(&self) -> bool {
        self.scopes.is_empty()
    }

    /// Resolve one system, most specific scope first.
    pub fn resolve_detail(&self, system: PhysicalSystem) -> Resolution {
        for scope in PolicyScope::MOST_SPECIFIC_FIRST {
            match self.scope(scope).get(system) {
                Participation::Inherit => continue,
                Participation::Enable => {
                    return Resolution {
                        participates: true,
                        decided_by: Some(scope),
                    };
                }
                Participation::Disable => {
                    return Resolution {
                        participates: false,
                        decided_by: Some(scope),
                    };
                }
            }
        }
        // Nothing stated an opinion. Nothing participates.
        Resolution {
            participates: false,
            decided_by: None,
        }
    }

    #[inline]
    pub fn participates(&self, system: PhysicalSystem) -> bool {
        self.resolve_detail(system).participates
    }

    /// Every system's answer in canonical order.
    pub fn resolved(&self) -> impl Iterator<Item = (PhysicalSystem, bool)> + '_ {
        PhysicalSystem::ALL
            .into_iter()
            .map(|system| (system, self.participates(system)))
    }
}

impl Serialize for ParticipationPolicy {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.scopes.len()))?;
        for (scope, set) in &self.scopes {
            map.serialize_entry(scope.name(), set)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for ParticipationPolicy {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PolicyVisitor;

        impl<'de> Visitor<'de> for PolicyVisitor {
            type Value = ParticipationPolicy;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a map of policy scope name to participation set")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut policy = ParticipationPolicy::new();
                while let Some((key, value)) = map.next_entry::<String, ParticipationSet>()? {
                    if let Some(scope) = PolicyScope::from_name(&key) {
                        policy.set_scope(scope, value);
                    }
                }
                Ok(policy)
            }
        }

        deserializer.deserialize_map(PolicyVisitor)
    }
}
