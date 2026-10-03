//! Immutable, bounded material-profile snapshots for owned fracture workers.
//! No mechanical registry, renderer, borrowed callback, or mutable host state.
//! Values here are fracture tuning, not conversions from mechanical toughness.
use crate::{DamageSpace, FracturePolicy, FractureProfile, fracture::FractureModel};
use engine_core::MaterialId;
use std::sync::Arc;

/// Construction-work and stored-payload ceilings, checked before table allocation.
#[derive(Clone, Copy, Debug)]
pub struct PolicySnapshotLimits {
    pub max_profiles: usize,
    pub max_bytes: usize,
}
impl Default for PolicySnapshotLimits {
    fn default() -> Self {
        Self {
            max_profiles: 64,
            max_bytes: 16 << 10,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicySnapshotRefusal {
    ProfileBudget,
    ByteBudget,
    DuplicateMaterial(MaterialId),
}
#[derive(Debug, PartialEq, Eq)]
struct PolicyData {
    model: FractureModel,
    profiles: Box<[(MaterialId, FractureProfile)]>,
}

/// Canonical immutable value snapshot; clones share owned read-only storage.
/// A host replaces the entire value to change tuning. Exact content equality,
/// not a hash or pointer address, is used to reject obsolete worker results.
/// Missing materials keep the selected homogeneous model's existing fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedFracturePolicy(Arc<PolicyData>);
impl OwnedFracturePolicy {
    /// One constant-size homogeneous configuration, with no material table.
    pub fn homogeneous(model: FractureModel) -> Self {
        Self(Arc::new(PolicyData {
            model,
            profiles: Box::new([]),
        }))
    }

    /// Copy caller-supplied values only after both ceilings pass. Duplicate IDs
    /// refuse rather than allowing input order to choose a material's response.
    /// Entry count bounds sorting/equality work independently of the byte cap.
    pub fn capture(
        model: FractureModel,
        profiles: &[(MaterialId, FractureProfile)],
        limits: PolicySnapshotLimits,
    ) -> Result<Self, PolicySnapshotRefusal> {
        if profiles.len() > limits.max_profiles {
            return Err(PolicySnapshotRefusal::ProfileBudget);
        }
        let bytes = Self::bytes_for(profiles.len()).ok_or(PolicySnapshotRefusal::ByteBudget)?;
        if bytes > limits.max_bytes {
            return Err(PolicySnapshotRefusal::ByteBudget);
        }
        let mut profiles = profiles.to_vec();
        profiles.sort_unstable_by_key(|entry| entry.0);
        for pair in profiles.windows(2) {
            if pair[0].0 == pair[1].0 {
                return Err(PolicySnapshotRefusal::DuplicateMaterial(pair[0].0));
            }
        }
        Ok(Self(Arc::new(PolicyData {
            model,
            profiles: profiles.into_boxed_slice(),
        })))
    }

    fn bytes_for(count: usize) -> Option<usize> {
        count
            .checked_mul(size_of::<(MaterialId, FractureProfile)>())?
            .checked_add(size_of::<PolicyData>())
    }

    /// Stored data payload, including the table descriptor but excluding Arc
    /// reference counters and allocator overhead. This is not total heap usage.
    /// Shared clones do not allocate another table. No serialization ABI implied.
    pub fn payload_bytes(&self) -> usize {
        Self::bytes_for(self.0.profiles.len()).expect("bounded at construction")
    }
    pub fn profile_count(&self) -> usize {
        self.0.profiles.len()
    }
    pub fn model(&self) -> FractureModel {
        self.0.model
    }
}
impl FracturePolicy for OwnedFracturePolicy {
    fn profile(&self, material: MaterialId) -> FractureProfile {
        match self
            .0
            .profiles
            .binary_search_by_key(&material, |entry| entry.0)
        {
            Ok(i) => self.0.profiles[i].1,
            Err(_) => self.0.model.profile(material),
        }
    }
    // Shared bonds deliberately reuse Step 2's symmetric default composition.
    // Separate authored connection types / pair override tables are not added.
    fn surface_gain_milli(&self, space: DamageSpace) -> i64 {
        self.0.model.surface_gain_milli(space)
    }
    fn erase_unbonded(&self) -> bool {
        self.0.model.erase_unbonded()
    }
}
