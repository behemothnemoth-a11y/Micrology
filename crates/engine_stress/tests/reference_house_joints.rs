use engine_core::{CellPos, CellSource};
use engine_destruction::{BOND_INTEGRITY, DamageSpace, FractureState};
use engine_stress::{reference_house, reference_house_joints as joints};

#[test]
fn joint_map_is_deterministic_nonempty_and_keeps_concrete_out_of_the_prototype() {
    let a = reference_house::build();
    let b = reference_house::build();
    let ja = joints::authored_joint_bonds(&a);
    let jb = joints::authored_joint_bonds(&b);
    assert_eq!(ja, jb);
    assert!(!ja.is_empty());
    let mut cross_material = 0usize;
    for bond in ja {
        let lower = a.material_at(bond.lower()).unwrap();
        let upper = a.material_at(bond.upper()).unwrap();
        assert_ne!(lower, reference_house::CONCRETE);
        assert_ne!(upper, reference_house::CONCRETE);
        assert_ne!(
            joints::part_label(bond.lower(), lower),
            joints::part_label(bond.upper(), upper)
        );
        cross_material += usize::from(lower != upper);
    }
    assert!(
        cross_material > 0,
        "wood-masonry/glass interfaces are authored joints too"
    );
}

#[test]
fn explicit_release_changes_only_authored_joint_state_and_not_geometry() {
    let world = reference_house::build();
    let before = reference_house::report(&world);
    let mut state = FractureState::new();
    let report = joints::seed(&mut state, &world, 0).unwrap();
    assert_eq!(reference_house::report(&world), before);
    assert_eq!(state.bond_entries(), report.bonds);
    assert_eq!(state.broken_bonds(), report.bonds);
    assert!(
        state
            .bonds_in(DamageSpace::StaticWorld)
            .all(|b| b.integrity.0 == 0)
    );
    assert!(report.by_pair.contains_key("101:102"));
    assert!(report.by_pair.contains_key("101:104"));
}

#[test]
fn ordinary_unseeded_house_still_has_full_integrity_everywhere() {
    let world = reference_house::build();
    let state = FractureState::new();
    for bond in joints::authored_joint_bonds(&world) {
        assert_eq!(
            state.integrity(engine_destruction::BondSite {
                space: DamageSpace::StaticWorld,
                bond
            }),
            BOND_INTEGRITY
        );
    }
}

#[test]
fn semantic_labels_are_local_to_reference_house_construction_zones() {
    let world = reference_house::build();
    for p in [
        CellPos::new(10, 4, 10),
        CellPos::new(20, 54, 20),
        CellPos::new(4, 20, 30),
        CellPos::new(12, 50, 80),
    ] {
        if let Some(m) = world.material_at(p) {
            assert!(joints::part_label(p, m).is_some());
        }
    }
}
