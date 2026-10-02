//! Force coupling, fragment re-fracture and bounded secondary damage: DROP 0004.8.
//!
//! The engine stays backend-neutral. Avian (or another host) may report an
//! impact, but core code owns deterministic damage events, fragment identity and
//! bounded recursion policy. Material-specific mass/strength remains DROP 0005.

use crate::{
    DamageAmount, DamageEvent, DamageEventId, DamageFalloff, DamageImpulse, DamageSpace,
    DamageTarget, DamageVolume, DestructionSequence, DetachOutcome, Fragment, FragmentId,
    FragmentPhysicsState, FragmentStore,
};
use engine_core::GlobalPos;
use std::collections::VecDeque;

/// Backend-neutral velocity change applied to one native fragment.
///
/// These are deltas, not backend impulses. The reference DROP 0004 coupling uses
/// one unit of mass per occupied cell; later material mechanics can replace that
/// policy without changing fragment storage or the damage event contract.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FragmentImpulse {
    pub linear_delta: [f32; 3],
    pub angular_delta: [f32; 3],
}

fn f32_saturated(value: f64) -> f32 {
    value.clamp(f64::from(f32::MIN), f64::from(f32::MAX)) as f32
}
/// Uniform reference coupling for a newly detached fragment.
///
/// New detachments start with identity rotation, so their local center can be
/// used directly for a simple torque estimate without making the engine
/// interpret a backend quaternion. Linear response divides by cell count; the
/// angular response divides by a coarse radius-squared inertia estimate.
pub fn reference_detachment_impulse(
    event: &DamageEvent,
    fragment: &Fragment,
) -> Option<FragmentImpulse> {
    let vector = event.impulse_world?.vector();
    let mass = fragment.cell_count().max(1) as f64;
    let linear = [
        f32_saturated(vector[0] / mass),
        f32_saturated(vector[1] / mass),
        f32_saturated(vector[2] / mass),
    ];

    let center_local = fragment.local_centre();
    let center = GlobalPos::new(
        fragment.pose.translation.x + center_local[0],
        fragment.pose.translation.y + center_local[1],
        fragment.pose.translation.z + center_local[2],
    );
    let source = event.source_world.unwrap_or(center);
    let lever = [
        center.x - source.x,
        center.y - source.y,
        center.z - source.z,
    ];
    let torque = [
        lever[1] * vector[2] - lever[2] * vector[1],
        lever[2] * vector[0] - lever[0] * vector[2],
        lever[0] * vector[1] - lever[1] * vector[0],
    ];
    let inertia = mass * fragment.bounding_radius().powi(2).max(1.0);
    let angular = [
        f32_saturated(torque[0] / inertia),
        f32_saturated(torque[1] / inertia),
        f32_saturated(torque[2] / inertia),
    ];
    Some(FragmentImpulse {
        linear_delta: linear,
        angular_delta: angular,
    })
}
/// Apply a coupling result without changing local geometry.
pub fn apply_fragment_impulse(fragment: &mut Fragment, impulse: FragmentImpulse) {
    let state = FragmentPhysicsState {
        pose: fragment.pose,
        linear_velocity: [
            fragment.linear_velocity[0] + impulse.linear_delta[0],
            fragment.linear_velocity[1] + impulse.linear_delta[1],
            fragment.linear_velocity[2] + impulse.linear_delta[2],
        ],
        angular_velocity: [
            fragment.angular_velocity[0] + impulse.angular_delta[0],
            fragment.angular_velocity[1] + impulse.angular_delta[1],
            fragment.angular_velocity[2] + impulse.angular_delta[2],
        ],
        sleeping: false,
    };
    state.apply_to(fragment);
}

/// Couple one accepted damage event into every fragment produced by a completed
/// detachment transaction. Admission already happened; coupling cannot fail.
pub fn couple_damage_to_detachment(outcome: &mut DetachOutcome, event: &DamageEvent) -> usize {
    let mut coupled = 0usize;
    for fragment in &mut outcome.fragments {
        if let Some(impulse) = reference_detachment_impulse(event, fragment) {
            apply_fragment_impulse(fragment, impulse);
            coupled += 1;
        }
    }
    coupled
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RefractureRefusal {
    ParentMissing,
    StillConnected,
    RejectedByPolicy,
    IdentityCollision,
}
#[derive(Clone, PartialEq, Debug)]
pub struct RefractureOutcome {
    pub parent: FragmentId,
    pub fragments: Vec<Fragment>,
    pub sequence: u64,
}

#[derive(Clone, PartialEq, Debug)]
pub enum FragmentDamageResult {
    Unchanged,
    Updated(Fragment),
    Destroyed { parent: FragmentId },
    Refractured(RefractureOutcome),
}

/// Atomically replace one disconnected fragment with canonical child fragments.
///
/// Child fragments retain the parent's local frame, pose and velocities. This is
/// what makes re-fracture valid even after arbitrary backend rotation: no
/// quaternion interpretation or origin renumbering occurs in engine code.
pub fn refracture_store_if(
    store: &mut FragmentStore,
    sequence: &mut DestructionSequence,
    parent_id: FragmentId,
    admit: impl FnOnce(&[Fragment]) -> bool,
) -> Result<RefractureOutcome, RefractureRefusal> {
    let Some(parent) = store.get(parent_id).cloned() else {
        return Err(RefractureRefusal::ParentMissing);
    };
    let parts = parent.connected_parts();
    if parts.len() <= 1 {
        return Err(RefractureRefusal::StillConnected);
    }

    let seq = sequence.peek();
    let mut children = Vec::with_capacity(parts.len());
    for (index, cells) in parts.iter().enumerate() {
        let id = FragmentId::new(seq, index as u32);
        if store.contains(id) {
            return Err(RefractureRefusal::IdentityCollision);
        }
        if let Some(child) = parent.subfragment_with_id(id, cells) {
            children.push(child);
        }
    }
    if children.len() <= 1 {
        return Err(RefractureRefusal::StillConnected);
    }
    if !admit(&children) {
        return Err(RefractureRefusal::RejectedByPolicy);
    }

    let consumed = sequence.take();
    debug_assert_eq!(consumed, seq);
    store.remove(parent_id);
    for child in &children {
        store.insert(child.clone());
    }
    Ok(RefractureOutcome {
        parent: parent_id,
        fragments: children,
        sequence: seq,
    })
}

/// Apply failed fragment-local cells and atomically reconcile the parent object.
///
/// Connected survivors keep the parent's identity. A fully destroyed fragment is
/// removed. If damage disconnects the object, fresh child IDs are staged and
/// admitted before the parent is replaced, so policy refusal leaves the store
/// and sequence unchanged.
pub fn damage_fragment_store_if(
    store: &mut FragmentStore,
    sequence: &mut DestructionSequence,
    parent_id: FragmentId,
    failed: &[DamageTarget],
    admit: impl FnOnce(&[Fragment]) -> bool,
) -> Result<FragmentDamageResult, RefractureRefusal> {
    let Some(parent) = store.get(parent_id).cloned() else {
        return Err(RefractureRefusal::ParentMissing);
    };
    let Some(updated) = crate::fragment_after_failures(&parent, failed) else {
        store.remove(parent_id);
        return Ok(FragmentDamageResult::Destroyed { parent: parent_id });
    };
    if updated.geometry_revision() == parent.geometry_revision() {
        return Ok(FragmentDamageResult::Unchanged);
    }

    let parts = updated.connected_parts();
    if parts.len() <= 1 {
        store.insert(updated.clone());
        return Ok(FragmentDamageResult::Updated(updated));
    }

    let seq = sequence.peek();
    let mut children = Vec::with_capacity(parts.len());
    for (index, cells) in parts.iter().enumerate() {
        let id = FragmentId::new(seq, index as u32);
        if store.contains(id) {
            return Err(RefractureRefusal::IdentityCollision);
        }
        if let Some(child) = updated.subfragment_with_id(id, cells) {
            children.push(child);
        }
    }
    if children.len() <= 1 {
        store.insert(updated.clone());
        return Ok(FragmentDamageResult::Updated(updated));
    }
    if !admit(&children) {
        return Err(RefractureRefusal::RejectedByPolicy);
    }

    let consumed = sequence.take();
    debug_assert_eq!(consumed, seq);
    store.remove(parent_id);
    for child in &children {
        store.insert(child.clone());
    }
    Ok(FragmentDamageResult::Refractured(RefractureOutcome {
        parent: parent_id,
        fragments: children,
        sequence: seq,
    }))
}

/// One damage event plus its secondary-recursion generation.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SecondaryDamage {
    pub event: DamageEvent,
    pub generation: u8,
}

impl SecondaryDamage {
    pub const fn root(event: DamageEvent) -> Self {
        Self {
            event,
            generation: 0,
        }
    }

    pub fn child(self, event: DamageEvent) -> Self {
        Self {
            event,
            generation: self.generation.saturating_add(1),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SecondaryDamageLimits {
    pub max_pending: usize,
    pub max_events_per_step: usize,
    pub max_generation: u8,
}

impl SecondaryDamageLimits {
    pub const fn new(max_pending: usize, max_events_per_step: usize, max_generation: u8) -> Self {
        Self {
            max_pending,
            max_events_per_step,
            max_generation,
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SecondaryDamageRefusal {
    PendingLimit,
    GenerationLimit,
}

#[derive(Clone, PartialEq, Default, Debug)]
pub struct SecondaryDamageQueue {
    pending: VecDeque<SecondaryDamage>,
    rejected_pending: u64,
    rejected_generation: u64,
}

impl SecondaryDamageQueue {
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn rejected_pending(&self) -> u64 {
        self.rejected_pending
    }

    pub fn rejected_generation(&self) -> u64 {
        self.rejected_generation
    }

    pub fn enqueue(
        &mut self,
        damage: SecondaryDamage,
        limits: SecondaryDamageLimits,
    ) -> Result<(), SecondaryDamageRefusal> {
        if damage.generation > limits.max_generation {
            self.rejected_generation = self.rejected_generation.saturating_add(1);
            return Err(SecondaryDamageRefusal::GenerationLimit);
        }
        if self.pending.len() >= limits.max_pending {
            self.rejected_pending = self.rejected_pending.saturating_add(1);
            return Err(SecondaryDamageRefusal::PendingLimit);
        }
        self.pending.push_back(damage);
        Ok(())
    }
    /// Drain at most one host step's work, preserving enqueue order.
    pub fn drain_step(&mut self, limits: SecondaryDamageLimits) -> Vec<SecondaryDamage> {
        let count = self.pending.len().min(limits.max_events_per_step);
        self.pending.drain(..count).collect()
    }
}

/// Backend-neutral sample from a fragment/world impact.
///
/// The host decides how to obtain these values. Avian can use solved normal
/// contact impulse and its world-space contact point; another backend can supply
/// equivalent data.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FragmentImpact {
    pub fragment: FragmentId,
    pub point_world: GlobalPos,
    pub impulse_world: [f64; 3],
    pub normal_impulse: f64,
}

/// Simple host policy for turning a sufficiently strong impact into static-world
/// damage. It is intentionally uniform and quantized; materials remain outside
/// DROP 0004.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct UniformImpactDamagePolicy {
    pub min_normal_impulse: f64,
    pub radius: f64,
    pub damage_per_impulse: u32,
    pub max_strength: u32,
}

impl UniformImpactDamagePolicy {
    pub fn event_for(self, id: DamageEventId, impact: FragmentImpact) -> Option<DamageEvent> {
        if !impact.normal_impulse.is_finite()
            || impact.normal_impulse < self.min_normal_impulse
            || self.damage_per_impulse == 0
            || self.max_strength == 0
        {
            return None;
        }
        let units = impact.normal_impulse.floor().max(1.0) as u64;
        let strength = units
            .saturating_mul(u64::from(self.damage_per_impulse))
            .min(u64::from(self.max_strength)) as u32;
        let impulse = DamageImpulse::new(impact.impulse_world).ok()?;
        DamageEvent::new(
            id,
            DamageSpace::StaticWorld,
            Some(impact.point_world),
            DamageVolume::Sphere {
                center: impact.point_world,
                radius: self.radius,
                falloff: DamageFalloff::Uniform,
            },
            DamageAmount(strength),
            Some(impulse),
        )
        .ok()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DamageSequence, DamageTarget};
    use engine_core::{CellPos, MaterialId};
    use engine_world::{EditOutcome, World};
    use std::collections::BTreeSet;

    fn line_fragment(id: FragmentId) -> Fragment {
        let mut world = World::new();
        world.fill_box(CellPos::ZERO, CellPos::new(2, 0, 0), Some(MaterialId(1)));
        Fragment::from_cells(
            id,
            &world,
            &BTreeSet::from([CellPos::ZERO, CellPos::new(1, 0, 0), CellPos::new(2, 0, 0)]),
        )
        .unwrap()
    }

    #[test]
    fn damage_impulse_changes_motion_not_geometry() {
        let mut fragment = line_fragment(FragmentId::new(1, 0));
        let geometry = fragment.geometry_revision();
        let event = DamageEvent::new(
            DamageEventId::new(1, 0),
            DamageSpace::StaticWorld,
            Some(GlobalPos::new(-2.0, 2.0, 0.0)),
            DamageVolume::Cell(CellPos::ZERO),
            DamageAmount(1),
            Some(DamageImpulse::new([9.0, 0.0, 0.0]).unwrap()),
        )
        .unwrap();
        let impulse = reference_detachment_impulse(&event, &fragment).unwrap();
        assert_ne!(impulse.linear_delta, [0.0; 3]);
        assert_ne!(impulse.angular_delta, [0.0; 3]);
        apply_fragment_impulse(&mut fragment, impulse);
        assert_ne!(fragment.linear_velocity, [0.0; 3]);
        assert_eq!(fragment.geometry_revision(), geometry);
    }

    #[test]
    fn coupling_updates_a_completed_detachment_outcome() {
        let fragment = line_fragment(FragmentId::new(2, 0));
        let mut outcome = DetachOutcome {
            fragments: vec![fragment],
            edit: EditOutcome::default(),
            sequence: 2,
        };
        let event = DamageEvent::new(
            DamageEventId::new(2, 0),
            DamageSpace::StaticWorld,
            None,
            DamageVolume::Cell(CellPos::ZERO),
            DamageAmount(1),
            Some(DamageImpulse::new([6.0, 0.0, 0.0]).unwrap()),
        )
        .unwrap();
        assert_eq!(couple_damage_to_detachment(&mut outcome, &event), 1);
        assert!(outcome.fragments[0].linear_velocity[0] > 0.0);
    }

    #[test]
    fn disconnected_fragment_refractures_atomically() {
        let parent_id = FragmentId::new(3, 0);
        let parent = line_fragment(parent_id)
            .without_local_cells(&BTreeSet::from([CellPos::new(1, 0, 0)]))
            .unwrap();
        let pose = parent.pose;
        let mut store = FragmentStore::default();
        store.insert(parent);
        let mut sequence = DestructionSequence::new(20);

        let result = refracture_store_if(&mut store, &mut sequence, parent_id, |_| true).unwrap();
        assert_eq!(result.fragments.len(), 2);
        assert_eq!(sequence.peek(), 21);
        assert!(!store.contains(parent_id));
        assert_eq!(store.len(), 2);
        assert!(result.fragments.iter().all(|child| child.pose == pose));
        assert_eq!(
            result
                .fragments
                .iter()
                .map(Fragment::cell_count)
                .sum::<u64>(),
            2
        );
    }

    #[test]
    fn fragment_damage_transaction_can_refracture_the_store() {
        let parent_id = FragmentId::new(40, 0);
        let parent = line_fragment(parent_id);
        let mut store = FragmentStore::default();
        store.insert(parent);
        let mut sequence = DestructionSequence::new(41);
        let failed = [DamageTarget::FragmentCell {
            fragment: parent_id,
            cell: CellPos::new(1, 0, 0),
            material: MaterialId(1),
        }];

        let result =
            damage_fragment_store_if(&mut store, &mut sequence, parent_id, &failed, |_| true)
                .unwrap();
        let FragmentDamageResult::Refractured(result) = result else {
            panic!("expected re-fracture");
        };
        assert_eq!(result.fragments.len(), 2);
        assert!(!store.contains(parent_id));
        assert_eq!(store.len(), 2);
        assert_eq!(sequence.peek(), 42);
    }

    #[test]
    fn refracture_policy_refusal_changes_nothing() {
        let parent_id = FragmentId::new(4, 0);
        let parent = line_fragment(parent_id)
            .without_local_cells(&BTreeSet::from([CellPos::new(1, 0, 0)]))
            .unwrap();
        let mut store = FragmentStore::default();
        store.insert(parent);
        let before = store.clone();
        let mut sequence = DestructionSequence::new(30);

        assert_eq!(
            refracture_store_if(&mut store, &mut sequence, parent_id, |_| false),
            Err(RefractureRefusal::RejectedByPolicy)
        );
        assert_eq!(store, before);
        assert_eq!(sequence.peek(), 30);
    }

    #[test]
    fn secondary_queue_bounds_pending_step_and_generation() {
        let mut ids = DamageSequence::new(1);
        let make = |id| {
            DamageEvent::new(
                id,
                DamageSpace::StaticWorld,
                None,
                DamageVolume::Cell(CellPos::ZERO),
                DamageAmount(1),
                None,
            )
            .unwrap()
        };
        let limits = SecondaryDamageLimits::new(2, 1, 1);
        let root = SecondaryDamage::root(make(ids.next_root()));
        let child = root.child(make(ids.next_root()));
        let grandchild = child.child(make(ids.next_root()));
        let mut queue = SecondaryDamageQueue::default();

        queue.enqueue(root, limits).unwrap();
        queue.enqueue(child, limits).unwrap();
        assert_eq!(
            queue.enqueue(grandchild, limits),
            Err(SecondaryDamageRefusal::GenerationLimit)
        );
        assert_eq!(
            queue.enqueue(SecondaryDamage::root(make(ids.next_root())), limits),
            Err(SecondaryDamageRefusal::PendingLimit)
        );
        assert_eq!(queue.drain_step(limits).len(), 1);
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn impact_policy_quantizes_contact_impulse_into_damage() {
        let policy = UniformImpactDamagePolicy {
            min_normal_impulse: 5.0,
            radius: 2.5,
            damage_per_impulse: 2,
            max_strength: 100,
        };
        let weak = FragmentImpact {
            fragment: FragmentId::new(1, 0),
            point_world: GlobalPos::new(0.5, 0.5, 0.5),
            impulse_world: [2.0, 0.0, 0.0],
            normal_impulse: 4.9,
        };
        assert!(policy.event_for(DamageEventId::new(1, 0), weak).is_none());

        let strong = FragmentImpact {
            normal_impulse: 12.8,
            ..weak
        };
        let event = policy.event_for(DamageEventId::new(1, 1), strong).unwrap();
        assert_eq!(event.strength, DamageAmount(24));
        assert!(matches!(event.volume, DamageVolume::Sphere { .. }));
    }
}
