//! Fragment ownership, lifecycle accounting and budgets: pass 0003.12.
//!
//! Structural analysis reports every exact detached component.  This module is
//! deliberately *after* that exact step: deciding that a piece is rigid-body
//! simulation, lightweight debris, discarded, or game-specific behaviour is a
//! policy choice, not a connectivity result.
//!
//! The second rule is measurement.  Fragment cost is not represented by a
//! fragment count.  0003.7 measured a 20-cell fragment costing a whole 8 KiB
//! storage volume while a 2,000-cell solid fragment can be comparably compact.
//! Budgets therefore track bytes and work units independently.

use crate::{Fragment, FragmentId, FragmentState};
use std::collections::BTreeMap;

/// What a higher-level game/host chooses to do with an exact detached component.
///
/// The destruction engine itself never silently selects one of these during
/// connectivity analysis.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FragmentDisposition {
    /// Full volumetric object with a physics body.
    RigidBody,
    /// Lightweight visual/game debris.  The core does not define how it is
    /// rendered or simulated.
    Debris,
    /// Deliberately omit the fragment after exact analysis.
    Discard,
    /// Game-defined handling.  The numeric tag belongs to the game.
    Custom(u32),
}

/// Derived runtime cost that does not live in the fragment's cell storage.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct FragmentDerivedFootprint {
    pub mesh_bytes: u64,
    pub collision_bytes: u64,
    pub collision_boxes: u64,
    pub physics_bodies: u64,
}

/// One fragment's cost, split by category so no caller has to pretend bytes,
/// collider complexity and body count are interchangeable.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct FragmentFootprint {
    pub cells: u64,
    pub storage_bytes: u64,
    pub mesh_bytes: u64,
    pub collision_bytes: u64,
    pub collision_boxes: u64,
    pub physics_bodies: u64,
}

impl FragmentFootprint {
    pub fn of(fragment: &Fragment, derived: FragmentDerivedFootprint) -> Self {
        Self {
            cells: fragment.cell_count(),
            storage_bytes: fragment.footprint_bytes(),
            mesh_bytes: derived.mesh_bytes,
            collision_bytes: derived.collision_bytes,
            collision_boxes: derived.collision_boxes,
            physics_bodies: derived.physics_bodies,
        }
    }

    pub fn tracked_bytes(self) -> u64 {
        self.storage_bytes
            .saturating_add(self.mesh_bytes)
            .saturating_add(self.collision_bytes)
    }
}

impl std::ops::AddAssign for FragmentFootprint {
    fn add_assign(&mut self, rhs: Self) {
        self.cells = self.cells.saturating_add(rhs.cells);
        self.storage_bytes = self.storage_bytes.saturating_add(rhs.storage_bytes);
        self.mesh_bytes = self.mesh_bytes.saturating_add(rhs.mesh_bytes);
        self.collision_bytes = self.collision_bytes.saturating_add(rhs.collision_bytes);
        self.collision_boxes = self.collision_boxes.saturating_add(rhs.collision_boxes);
        self.physics_bodies = self.physics_bodies.saturating_add(rhs.physics_bodies);
    }
}

/// Aggregate lifecycle counts independent of any renderer or physics backend.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct FragmentStoreStats {
    pub fragments: u64,
    pub dynamic: u64,
    pub sleeping: u64,
    pub cells: u64,
    pub storage_bytes: u64,
}

/// Engine-owned fragment collection.
///
/// It is intentionally just deterministic data.  Bevy can wrap this in a
/// Resource; persistence can serialize it later; neither requirement belongs in
/// this crate.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct FragmentStore {
    fragments: BTreeMap<FragmentId, Fragment>,
}

impl FragmentStore {
    pub fn insert(&mut self, fragment: Fragment) -> Option<Fragment> {
        self.fragments.insert(fragment.id, fragment)
    }

    pub fn remove(&mut self, id: FragmentId) -> Option<Fragment> {
        self.fragments.remove(&id)
    }

    pub fn get(&self, id: FragmentId) -> Option<&Fragment> {
        self.fragments.get(&id)
    }

    pub fn get_mut(&mut self, id: FragmentId) -> Option<&mut Fragment> {
        self.fragments.get_mut(&id)
    }

    pub fn contains(&self, id: FragmentId) -> bool {
        self.fragments.contains_key(&id)
    }

    pub fn len(&self) -> usize {
        self.fragments.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fragments.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (FragmentId, &Fragment)> {
        self.fragments.iter().map(|(id, fragment)| (*id, fragment))
    }

    pub fn ids(&self) -> impl Iterator<Item = FragmentId> + '_ {
        self.fragments.keys().copied()
    }

    pub fn stats(&self) -> FragmentStoreStats {
        let mut stats = FragmentStoreStats {
            fragments: self.fragments.len() as u64,
            ..FragmentStoreStats::default()
        };
        for fragment in self.fragments.values() {
            stats.cells = stats.cells.saturating_add(fragment.cell_count());
            stats.storage_bytes = stats
                .storage_bytes
                .saturating_add(fragment.footprint_bytes());
            match fragment.state {
                FragmentState::Dynamic => stats.dynamic += 1,
                FragmentState::Sleeping => stats.sleeping += 1,
            }
        }
        stats
    }
}

/// All fragment-related cost currently charged to a game/host.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct FragmentAccount {
    pub footprint: FragmentFootprint,
    pub dynamic_fragments: u64,
    pub sleeping_fragments: u64,
}

impl FragmentAccount {
    pub fn from_store(store: &FragmentStore) -> Self {
        let stats = store.stats();
        Self {
            footprint: FragmentFootprint {
                cells: stats.cells,
                storage_bytes: stats.storage_bytes,
                ..FragmentFootprint::default()
            },
            dynamic_fragments: stats.dynamic,
            sleeping_fragments: stats.sleeping,
        }
    }

    /// Add renderer/physics costs measured by the host.
    pub fn add_derived(&mut self, derived: FragmentDerivedFootprint) {
        self.footprint.mesh_bytes = self.footprint.mesh_bytes.saturating_add(derived.mesh_bytes);
        self.footprint.collision_bytes = self
            .footprint
            .collision_bytes
            .saturating_add(derived.collision_bytes);
        self.footprint.collision_boxes = self
            .footprint
            .collision_boxes
            .saturating_add(derived.collision_boxes);
        self.footprint.physics_bodies = self
            .footprint
            .physics_bodies
            .saturating_add(derived.physics_bodies);
    }

    pub fn with_next(mut self, next: FragmentFootprint) -> Self {
        self.footprint += next;
        self
    }
}

/// Which budget boundary an account is on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FragmentPressure {
    Comfortable,
    OverSoft,
    OverHard,
}

/// Byte/work limits for fragment runtime state.
///
/// No default is supplied on purpose.  A default numerical cap would look like
/// an engine truth despite depending on the game, platform and measured
/// workload.  Hosts construct this explicitly and expose it in diagnostics.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FragmentBudget {
    pub soft_bytes: u64,
    pub hard_bytes: u64,
    pub max_physics_bodies: u64,
    pub max_collision_boxes: u64,
}

impl FragmentBudget {
    pub const fn new(
        soft_bytes: u64,
        hard_bytes: u64,
        max_physics_bodies: u64,
        max_collision_boxes: u64,
    ) -> Self {
        Self {
            soft_bytes,
            hard_bytes,
            max_physics_bodies,
            max_collision_boxes,
        }
    }

    pub fn pressure(self, account: FragmentAccount) -> FragmentPressure {
        let bytes = account.footprint.tracked_bytes();
        if bytes > self.hard_bytes
            || account.footprint.physics_bodies > self.max_physics_bodies
            || account.footprint.collision_boxes > self.max_collision_boxes
        {
            FragmentPressure::OverHard
        } else if bytes > self.soft_bytes {
            FragmentPressure::OverSoft
        } else {
            FragmentPressure::Comfortable
        }
    }

    /// Pressure if `next` were admitted, without mutating any store or account.
    pub fn pressure_with(
        self,
        account: FragmentAccount,
        next: FragmentFootprint,
    ) -> FragmentPressure {
        self.pressure(account.with_next(next))
    }
}
