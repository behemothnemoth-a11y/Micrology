//! Pure fragment-derived-data jobs for DROP 0004.4.
//!
//! Mesh and collision compilation can be expensive for one complex fragment.
//! The host may run these owned inputs on any worker pool and later validate the
//! result against the fragment's geometry revision before applying it. No Bevy,
//! Avian, renderer or task-pool type appears here.

use crate::{
    CollisionCompiler, CollisionShape, Fragment, FragmentId, GreedyCollisionCompiler,
};
use engine_core::{CellPos, MaterialRegistry, Revision};
use engine_geometry::{GreedyCompiler, MeshData, QuadSet, SurfaceCompiler};

/// Exact cache identity for data derived from a fragment's local cells.
///
/// Fragment identity plus the geometry-only revision is enough because a
/// fragment id denotes one engine object. Motion/state revisions are
/// intentionally excluded: translating a rigid piece does not change its mesh
/// or collider.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FragmentGeometryFingerprint {
    pub id: FragmentId,
    pub revision: Revision,
}

impl FragmentGeometryFingerprint {
    pub fn of(fragment: &Fragment) -> Self {
        Self {
            id: fragment.id,
            revision: fragment.geometry_revision(),
        }
    }

    pub fn matches(self, fragment: &Fragment) -> bool {
        self == Self::of(fragment)
    }
}

/// Owned worker input for a complete fragment CPU mesh build.
#[derive(Clone, Debug)]
pub struct FragmentMeshJobInput {
    fingerprint: FragmentGeometryFingerprint,
    fragment: Fragment,
    materials: MaterialRegistry,
}

impl FragmentMeshJobInput {
    pub fn new(fragment: &Fragment, materials: &MaterialRegistry) -> Self {
        Self {
            fingerprint: FragmentGeometryFingerprint::of(fragment),
            fragment: fragment.clone(),
            materials: materials.clone(),
        }
    }

    pub fn fingerprint(&self) -> FragmentGeometryFingerprint {
        self.fingerprint
    }

    pub fn run(self) -> FragmentMeshJobResult {
        let mut quads = QuadSet::default();
        for volume in self.fragment.volume_positions() {
            quads.extend(GreedyCompiler.compile(&self.fragment, volume));
        }
        FragmentMeshJobResult {
            fingerprint: self.fingerprint,
            mesh: MeshData::from_quads(&quads, &self.materials, CellPos::ZERO),
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct FragmentMeshJobResult {
    pub fingerprint: FragmentGeometryFingerprint,
    pub mesh: MeshData,
}

impl FragmentMeshJobResult {
    pub fn is_current(&self, fragment: &Fragment) -> bool {
        self.fingerprint.matches(fragment)
    }
}

/// Owned worker input for fragment-local collision compilation.
#[derive(Clone, Debug)]
pub struct FragmentCollisionJobInput {
    fingerprint: FragmentGeometryFingerprint,
    fragment: Fragment,
}

impl FragmentCollisionJobInput {
    pub fn new(fragment: &Fragment) -> Self {
        Self {
            fingerprint: FragmentGeometryFingerprint::of(fragment),
            fragment: fragment.clone(),
        }
    }

    pub fn fingerprint(&self) -> FragmentGeometryFingerprint {
        self.fingerprint
    }

    pub fn run(self) -> FragmentCollisionJobResult {
        let bounds = self.fragment.bounds;
        FragmentCollisionJobResult {
            fingerprint: self.fingerprint,
            collider: GreedyCollisionCompiler.compile(&self.fragment, bounds),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FragmentCollisionJobResult {
    pub fingerprint: FragmentGeometryFingerprint,
    pub collider: CollisionShape,
}

impl FragmentCollisionJobResult {
    pub fn is_current(&self, fragment: &Fragment) -> bool {
        self.fingerprint.matches(fragment)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FragmentPhysicsState, FragmentPose, Rotation};
    use engine_core::{CellPos, GlobalPos, MaterialId};
    use engine_world::World;
    use std::collections::BTreeSet;

    fn fragment() -> Fragment {
        let mut world = World::new();
        world.fill_box(
            CellPos::new(0, 0, 0),
            CellPos::new(7, 5, 3),
            Some(MaterialId(1)),
        );
        world.take_dirty();
        let cells: BTreeSet<_> = (0..6)
            .flat_map(|y| {
                (0..4).flat_map(move |z| (0..8).map(move |x| CellPos::new(x, y, z)))
            })
            .collect();
        Fragment::from_cells(FragmentId::new(7, 0), &world, &cells).unwrap()
    }

    fn assert_send<T: Send>() {}

    #[test]
    fn job_inputs_and_results_can_cross_worker_threads() {
        assert_send::<FragmentMeshJobInput>();
        assert_send::<FragmentMeshJobResult>();
        assert_send::<FragmentCollisionJobInput>();
        assert_send::<FragmentCollisionJobResult>();
    }

    #[test]
    fn mesh_job_is_owned_and_deterministic() {
        let f = fragment();
        let materials = MaterialRegistry::default();
        let first = FragmentMeshJobInput::new(&f, &materials).run();
        let second = FragmentMeshJobInput::new(&f, &materials).run();
        assert_eq!(first, second);
        assert!(!first.mesh.is_empty());
        assert!(first.is_current(&f));
    }

    #[test]
    fn collision_job_is_owned_and_deterministic() {
        let f = fragment();
        let first = FragmentCollisionJobInput::new(&f).run();
        let second = FragmentCollisionJobInput::new(&f).run();
        assert_eq!(first, second);
        assert!(!first.collider.is_empty());
        assert!(first.is_current(&f));
    }

    #[test]
    fn physics_motion_does_not_stale_geometry_results() {
        let mut f = fragment();
        let mesh = FragmentMeshJobInput::new(&f, &MaterialRegistry::default()).run();
        let collision = FragmentCollisionJobInput::new(&f).run();
        let geometry_revision = f.geometry_revision();

        FragmentPhysicsState {
            pose: FragmentPose {
                translation: GlobalPos::new(50.0, 80.0, -30.0),
                rotation: Rotation::default(),
            },
            linear_velocity: [1.0, 2.0, 3.0],
            angular_velocity: [0.1, 0.2, 0.3],
            sleeping: false,
        }
        .apply_to(&mut f);

        assert_eq!(f.geometry_revision(), geometry_revision);
        assert!(mesh.is_current(&f));
        assert!(collision.is_current(&f));
    }
}
