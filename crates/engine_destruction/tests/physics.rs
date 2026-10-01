use engine_core::{CellPos, MaterialId, MaterialRegistry, Rgb};
use engine_destruction::{
    Fragment, FragmentId, FragmentPhysicsDescriptor, FragmentPhysicsState, FragmentState,
};
use engine_world::World;
use std::collections::BTreeSet;

const STONE: MaterialId = MaterialId(1);

fn fragment() -> Fragment {
    let mut materials = MaterialRegistry::new();
    materials.define(1, Rgb::new(128, 128, 128), "stone");
    let mut world = World::with_materials(materials);
    world.fill_box(
        CellPos::new(100, 40, -20),
        CellPos::new(107, 43, -13),
        Some(STONE),
    );
    world.take_dirty();

    let cells: BTreeSet<_> = (100..=107)
        .flat_map(|x| (40..=43).flat_map(move |y| (-20..=-13).map(move |z| CellPos::new(x, y, z))))
        .collect();
    Fragment::from_cells(FragmentId::new(9, 2), &world, &cells).expect("fragment")
}

#[test]
fn descriptor_keeps_engine_state_and_compiles_local_collision() {
    let mut fragment = fragment();
    fragment.linear_velocity = [1.5, -2.0, 0.25];
    fragment.angular_velocity = [0.0, 0.5, -0.25];

    let descriptor = FragmentPhysicsDescriptor::from_fragment(&fragment);
    assert_eq!(descriptor.id, FragmentId::new(9, 2));
    assert_eq!(descriptor.pose, fragment.pose);
    assert_eq!(descriptor.linear_velocity, [1.5, -2.0, 0.25]);
    assert_eq!(descriptor.angular_velocity, [0.0, 0.5, -0.25]);
    assert!(!descriptor.sleeping);
    assert_eq!(descriptor.collision_boxes(), 1, "solid fragment is one box");
    assert_eq!(descriptor.collider.covered_cells(), fragment.cell_count());

    let b = descriptor.collider.bounds().expect("bounds");
    assert_eq!(b.min, fragment.bounds.min);
    assert_eq!(b.max, fragment.bounds.max);
}

#[test]
fn backend_state_round_trips_without_backend_types() {
    let mut fragment = fragment();
    let before = fragment.revision;
    let mut state = FragmentPhysicsState::from_fragment(&fragment);
    state.pose.translation.x += 12.5;
    state.linear_velocity = [3.0, 4.0, 5.0];
    state.angular_velocity = [-1.0, 0.0, 1.0];
    state.sleeping = true;
    state.apply_to(&mut fragment);

    assert_eq!(fragment.pose, state.pose);
    assert_eq!(fragment.linear_velocity, state.linear_velocity);
    assert_eq!(fragment.angular_velocity, state.angular_velocity);
    assert_eq!(fragment.state, FragmentState::Sleeping);
    assert!(fragment.revision > before);
}

#[test]
fn descriptor_preserves_sleeping_state() {
    let mut fragment = fragment();
    FragmentPhysicsState {
        pose: fragment.pose,
        linear_velocity: [0.0; 3],
        angular_velocity: [0.0; 3],
        sleeping: true,
    }
    .apply_to(&mut fragment);

    let descriptor = FragmentPhysicsDescriptor::from_fragment(&fragment);
    assert!(descriptor.sleeping);
}
