use engine_stress::house_scenarios::{self, HouseScenario};

#[test]
fn controlled_house_scenarios_keep_their_measured_material_character() {
    let expected = [
        (HouseScenario::Window, 64, 0, 14, 37, 8),
        (HouseScenario::FrameJoint, 41, 1, 14, 14, 1),
        (HouseScenario::WallBreach, 415, 56, 77, 139, 5),
        (HouseScenario::Chimney, 227, 42, 65, 97, 6),
        (HouseScenario::RoofHit, 97, 0, 0, 0, 0),
        (HouseScenario::SupportBand, 0, 3256, 11, 43636, 43576),
        (
            HouseScenario::SupportBandReleasedJoints,
            0,
            3256,
            72,
            46302,
            2274,
        ),
    ];
    for (scenario, broken, erased, objects, fragment_cells, largest) in expected {
        let (_, report) = house_scenarios::run(scenario).unwrap();
        assert_eq!(report.broken, broken, "{scenario:?}");
        assert_eq!(report.erased, erased, "{scenario:?}");
        assert_eq!(report.objects, objects, "{scenario:?}");
        assert_eq!(report.fragment_cells, fragment_cells, "{scenario:?}");
        assert_eq!(
            report.sizes.last().copied().unwrap_or(0),
            largest,
            "{scenario:?}"
        );
        assert_eq!(
            report.initial,
            report.static_cells + report.fragment_cells + report.erased,
            "cell accounting must close for {scenario:?}"
        );
        assert!(report.pieces.iter().all(|piece| piece.parts == 1));
    }
}

#[test]
fn boundary_and_material_specific_cases_touch_only_the_expected_materials() {
    let (_, window) = house_scenarios::run(HouseScenario::Window).unwrap();
    assert_eq!(
        window.touched_materials.keys().copied().collect::<Vec<_>>(),
        vec![104]
    );
    assert_eq!(
        window
            .loaded_material_pairs
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["104:104"]
    );

    let (_, joint) = house_scenarios::run(HouseScenario::FrameJoint).unwrap();
    assert!(joint.loaded_material_pairs.contains_key("101:104"));
    assert_eq!(
        joint.touched_materials.keys().copied().collect::<Vec<_>>(),
        vec![104]
    );

    for scenario in [HouseScenario::WallBreach, HouseScenario::Chimney] {
        let (_, report) = house_scenarios::run(scenario).unwrap();
        assert_eq!(
            report.touched_materials.keys().copied().collect::<Vec<_>>(),
            vec![102]
        );
    }

    let (_, roof) = house_scenarios::run(HouseScenario::RoofHit).unwrap();
    assert!(roof.loaded_material_pairs.contains_key("101:101"));
    assert!(roof.touched_materials.is_empty());
    assert_eq!(roof.objects, 0);
}

#[test]
fn support_cut_is_an_explicit_large_component_diagnostic_not_fake_pulverization() {
    let (_, report) = house_scenarios::run(HouseScenario::SupportBand).unwrap();
    assert_eq!(report.objects, 11);
    assert_eq!(report.sizes.last().copied(), Some(43_576));
    assert!(report.touched_materials.contains_key(&101));
    assert!(report.touched_materials.contains_key(&102));
    assert_eq!(report.broken, 0, "this scenario cuts occupancy explicitly");
    assert!(report.crack_topology_cells > 0);
    assert!(report.occupancy_topology_cells > 0);
}

#[test]
fn explicit_authored_joint_release_reduces_the_catastrophic_weld_without_changing_the_control() {
    let (_, welded) = house_scenarios::run(HouseScenario::SupportBand).unwrap();
    let (_, jointed) = house_scenarios::run(HouseScenario::SupportBandReleasedJoints).unwrap();
    assert_eq!(welded.sizes.last().copied(), Some(43_576));
    assert_eq!(jointed.sizes.last().copied(), Some(2_274));
    assert_eq!(jointed.objects, 72);
    assert_eq!(jointed.erased, welded.erased);
    assert_eq!(
        jointed.broken, 0,
        "explicit authored release predates the occupancy cut"
    );
    assert!(
        jointed.fragment_cells > welded.fragment_cells,
        "joint cuts expose more unsupported construction"
    );
}

#[test]
fn independent_runs_are_identical_for_each_controlled_house_case() {
    for scenario in HouseScenario::ALL {
        let (_, a) = house_scenarios::run(scenario).unwrap();
        let (_, b) = house_scenarios::run(scenario).unwrap();
        assert_eq!(a, b, "{scenario:?}");
    }
}
