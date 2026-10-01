//! Backend-neutral fragment physics bridge: pass 0003.11.
//!
//! The destruction crate still knows nothing about Bevy or Avian.  It describes
//! what a physics host needs to create a body, and what state the host gives
//! back after simulation.  The backend-specific conversion stays in the app.

use crate::{
    CollisionCompiler, CollisionShape, Fragment, FragmentId, FragmentPose, GreedyCollisionCompiler,
};
use engine_core::CellBounds;

/// Everything a host needs to instantiate one fragment as a rigid body.
///
/// The collider is already compiled in fragment-local coordinates.  Backends do
/// not need to know how Micrology stores cells, and changing physics backends
/// does not require a second collision compiler.
#[derive(Clone, PartialEq, Debug)]
pub struct FragmentPhysicsDescriptor {
    pub id: FragmentId,
    pub pose: FragmentPose,
    pub linear_velocity: [f32; 3],
    pub angular_velocity: [f32; 3],
    pub sleeping: bool,
    pub collider: CollisionShape,
}

impl FragmentPhysicsDescriptor {
    /// Describe a fragment using the shipped greedy collision compiler.
    pub fn from_fragment(fragment: &Fragment) -> Self {
        let bounds = CellBounds::new(fragment.bounds.min, fragment.bounds.max);
        Self {
            id: fragment.id,
            pose: fragment.pose,
            linear_velocity: fragment.linear_velocity,
            angular_velocity: fragment.angular_velocity,
            sleeping: fragment.is_sleeping(),
            collider: GreedyCollisionCompiler.compile(fragment, bounds),
        }
    }

    pub fn collision_boxes(&self) -> usize {
        self.collider.len()
    }

    pub fn collision_bytes(&self) -> u64 {
        self.collider.bytes()
    }
}

/// Backend-neutral state sampled from a live physics body.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FragmentPhysicsState {
    pub pose: FragmentPose,
    pub linear_velocity: [f32; 3],
    pub angular_velocity: [f32; 3],
    pub sleeping: bool,
}

impl FragmentPhysicsState {
    pub fn from_fragment(fragment: &Fragment) -> Self {
        Self {
            pose: fragment.pose,
            linear_velocity: fragment.linear_velocity,
            angular_velocity: fragment.angular_velocity,
            sleeping: fragment.is_sleeping(),
        }
    }

    /// Make the engine-owned fragment agree with the backend observation.
    pub fn apply_to(self, fragment: &mut Fragment) {
        fragment.update_from_physics(
            self.pose,
            self.linear_velocity,
            self.angular_velocity,
            self.sleeping,
        );
    }
}
