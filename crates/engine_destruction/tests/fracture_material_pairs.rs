//! Material-pair response must not depend on coordinate ownership.
//! Synthetic profiles isolate the seam; these are NOT wood/glass tuning.
use engine_core::{Axis, CellPos, GlobalPos, MaterialId, REGION_EDGE_CELLS, RegionPos};
use engine_destruction::{
    AllResident, BOND_INTEGRITY, BaselineFracturePolicy, BondFractureProfile, BondKey, BondMode,
    BondSite, DamageAmount, DamageEvent, DamageEventId, DamageFalloff, DamageSpace, DamageVolume,
    FractureDefer, FractureEvaluation, FractureImpact, FractureLimits, FractureLoad,
    FracturePolicy, FractureProfile, FractureScene, FractureState, Fragment, FragmentId, Residency,
    evaluate_fracture,
};
use engine_world::World;

const A: MaterialId = MaterialId(101);
const B: MaterialId = MaterialId(102);
const UNKNOWN: MaterialId = MaterialId(65534);
struct TwoMaterials;
impl FracturePolicy for TwoMaterials {
    fn profile(&self, material: MaterialId) -> FractureProfile {
        let mut p = FractureProfile::REFERENCE_SOLID;
        if material == A {
            p.toughness_milli = 200;
        } else if material == B {
            p.toughness_milli = 700;
            p.tensile = DamageAmount(600);
            p.compressive = DamageAmount(4000);
            p.shear = DamageAmount(1600);
        }
        p
    }
    fn erase_unbonded(&self) -> bool {
        false
    }
}

fn joint_world(lower: CellPos, axis: Axis, materials: [MaterialId; 2], reverse: bool) -> World {
    let key = BondKey::along(lower, axis).unwrap();
    let mut entries = [(key.lower(), materials[0]), (key.upper(), materials[1])];
    if reverse {
        entries.reverse();
    }
    let mut world = World::new();
    for (cell, material) in entries {
        world.set(cell, Some(material));
    }
    world.take_dirty();
    world
}

fn hit(lower: CellPos, space: DamageSpace, direction: Option<[f64; 3]>) -> FractureImpact {
    let event = DamageEvent::new(
        DamageEventId::new(1, 0),
        space,
        None,
        DamageVolume::Sphere {
            center: GlobalPos::new(
                f64::from(lower.x) + 0.5,
                f64::from(lower.y) + 0.5,
                f64::from(lower.z) + 0.5,
            ),
            radius: 4.0,
            falloff: DamageFalloff::Linear,
        },
        // At 600 energy the B-owned bond falls BELOW the 20/1000 sparse-record
        // reporting floor. 1200 makes both original responses observable, and
        // neither saturates: the uncorrected implementation reports 327 vs 33.
        DamageAmount(1200),
        None,
    )
    .unwrap();
    FractureImpact::from_event(&event, direction).unwrap()
}

fn loaded(evaluation: FractureEvaluation) -> FractureLoad {
    match evaluation {
        FractureEvaluation::Loaded(load) => load,
        other => panic!("expected a loaded evaluation, got {other:?}"),
    }
}

fn joint_load(
    lower: CellPos,
    axis: Axis,
    materials: [MaterialId; 2],
    reverse: bool,
    direction: Option<[f64; 3]>,
    policy: &dyn FracturePolicy,
) -> FractureLoad {
    let world = joint_world(lower, axis, materials, reverse);
    let state = FractureState::new();
    let before = world.clone();
    let load = loaded(
        evaluate_fracture(
            &hit(lower, DamageSpace::StaticWorld, direction),
            FractureScene::static_world(&world, &world, &AllResident),
            policy,
            &state,
            FractureLimits::UNLIMITED,
        )
        .unwrap(),
    );
    assert_eq!(world, before, "analysis must not mutate geometry");
    assert!(state.is_empty(), "analysis must not mutate damage history");
    load
}

#[test]
fn swapping_endpoint_materials_preserves_bond_load_in_all_axes() {
    for axis in Axis::ALL {
        for lower in [
            CellPos::ZERO,
            CellPos::new(-2, -3, -4),
            CellPos::new(
                REGION_EDGE_CELLS - 1,
                REGION_EDGE_CELLS - 1,
                REGION_EDGE_CELLS - 1,
            ),
        ] {
            let ab = joint_load(lower, axis, [A, B], false, None, &TwoMaterials);
            let ba = joint_load(lower, axis, [B, A], false, None, &TwoMaterials);
            assert_eq!(ab.bonds.len(), 1);
            assert_eq!(ba.bonds.len(), 1);
            // The lower material is a record-owner tag, NOT interface strength.
            assert_eq!(ab.bonds[0].bond, ba.bonds[0].bond);
            assert_eq!(ab.bonds[0].mode, ba.bonds[0].mode);
            assert_eq!(
                ab.bonds[0].loss, ba.bonds[0].loss,
                "coordinate-only bias at {lower:?}, {axis:?}"
            );
            assert_eq!(ab.bonds[0].load, ba.bonds[0].load);
            assert!(ab.bonds[0].loss > DamageAmount::ZERO);
            assert!(ab.bonds[0].loss < BOND_INTEGRITY);
        }
    }
}

#[test]
fn symmetry_keeps_actual_impact_direction_separate_from_material_order() {
    for axis in Axis::ALL {
        for sign in [-1.0, 1.0] {
            let mut direction = [0.0; 3];
            direction[axis.index()] = sign;
            let ab = joint_load(
                CellPos::ZERO,
                axis,
                [A, B],
                false,
                Some(direction),
                &TwoMaterials,
            );
            let ba = joint_load(
                CellPos::ZERO,
                axis,
                [B, A],
                false,
                Some(direction),
                &TwoMaterials,
            );
            assert_eq!(ab.bonds.len(), 1);
            assert_eq!(ba.bonds.len(), 1);
            assert_eq!(ab.bonds[0].mode, ba.bonds[0].mode);
            assert_eq!(ab.bonds[0].loss, ba.bonds[0].loss);
            assert_eq!(ab.bonds[0].load, ba.bonds[0].load);
        }
    }
}

#[test]
fn pair_properties_are_symmetric_and_same_profiles_are_unchanged() {
    for a in [A, B, UNKNOWN] {
        for b in [A, B, UNKNOWN] {
            assert_eq!(
                TwoMaterials.bond_profile(a, b),
                TwoMaterials.bond_profile(b, a)
            );
        }
        let single = TwoMaterials.profile(a);
        let pair = TwoMaterials.bond_profile(a, a);
        assert_eq!(pair.load_milli, 1000 - single.toughness_milli);
        for mode in BondMode::ALL {
            assert_eq!(pair.capacity(mode), single.capacity(mode));
        }
    }
    // Unprofiled behavior comes from the caller's existing profile fallback;
    // this new seam does not register materials or invent a second fallback.
    assert_eq!(
        TwoMaterials.profile(UNKNOWN),
        FractureProfile::REFERENCE_SOLID
    );
    let baseline = BaselineFracturePolicy::REFERENCE;
    assert_eq!(
        baseline.bond_profile(A, B),
        baseline.bond_profile(UNKNOWN, UNKNOWN)
    );
}

#[test]
fn weaker_side_is_explicit_mode_by_mode_and_uses_no_cell_crush_or_density() {
    let a = TwoMaterials.profile(A);
    let mut b = TwoMaterials.profile(B);
    b.tensile = DamageAmount(40);
    b.shear = DamageAmount(80);
    let pair = BondFractureProfile::weaker_side(a, b);
    assert_eq!(pair.tensile, DamageAmount(40));
    assert_eq!(pair.compressive, a.compressive);
    assert_eq!(pair.shear, DamageAmount(80));
    assert_eq!(pair.load_milli, 800);
    b.density_milli = i64::MAX;
    b.crush = DamageAmount::ZERO;
    assert_eq!(pair, BondFractureProfile::weaker_side(a, b));
}

#[test]
fn profile_toughness_extremes_cannot_overflow_bond_load_composition() {
    for a_tough in [i64::MIN, -1, 0, 350, 1000, i64::MAX] {
        for b_tough in [i64::MIN, -1, 0, 350, 1000, i64::MAX] {
            let mut a = FractureProfile::REFERENCE_SOLID;
            let mut b = a;
            a.toughness_milli = a_tough;
            b.toughness_milli = b_tough;
            let pair = BondFractureProfile::weaker_side(a, b);
            assert_eq!(pair, BondFractureProfile::weaker_side(b, a));
            assert!((0..=1000).contains(&pair.load_milli));
        }
    }
}

struct PairOverride {
    load_milli: i64,
}
impl FracturePolicy for PairOverride {
    fn profile(&self, m: MaterialId) -> FractureProfile {
        TwoMaterials.profile(m)
    }
    fn bond_profile(&self, _: MaterialId, _: MaterialId) -> BondFractureProfile {
        BondFractureProfile {
            load_milli: self.load_milli,
            tensile: DamageAmount(2000),
            compressive: DamageAmount(2000),
            shear: DamageAmount(2000),
        }
    }
}

#[test]
fn evaluator_uses_pair_override_and_clamps_its_load_fraction() {
    let sample = |value| {
        joint_load(
            CellPos::ZERO,
            Axis::X,
            [A, B],
            false,
            None,
            &PairOverride { load_milli: value },
        )
    };
    let off = sample(-1);
    assert!(off.bonds.is_empty());
    assert_eq!(
        off.cells.len(),
        2,
        "cell energy remains independent of bond loading"
    );
    let full = sample(1000);
    let over = sample(i64::MAX);
    assert_eq!(full, over);
    assert_eq!(full.bonds.len(), 1);
    assert_eq!(full.bonds[0].load, DamageAmount(450));
    assert_eq!(full.bonds[0].loss, DamageAmount(225));
}

#[test]
fn repeated_and_reverse_insertion_evaluations_are_identical() {
    let first = joint_load(CellPos::ZERO, Axis::X, [A, B], false, None, &TwoMaterials);
    for _ in 0..8 {
        assert_eq!(
            first,
            joint_load(CellPos::ZERO, Axis::X, [A, B], true, None, &TwoMaterials)
        );
    }
}

#[test]
fn repeated_damage_accumulates_in_one_shared_bond_record() {
    let lower = CellPos::ZERO;
    let key = BondKey::along(lower, Axis::X).unwrap();
    let site = BondSite {
        space: DamageSpace::StaticWorld,
        bond: key,
    };
    let run = |materials| {
        let world = joint_world(lower, Axis::X, materials, false);
        let before = world.clone();
        let mut state = FractureState::new();
        let mut prior = BOND_INTEGRITY;
        for _ in 0..2 {
            let scene = FractureScene::static_world(&world, &world, &AllResident);
            let load = loaded(
                evaluate_fracture(
                    &hit(lower, DamageSpace::StaticWorld, None),
                    scene,
                    &TwoMaterials,
                    &state,
                    FractureLimits::UNLIMITED,
                )
                .unwrap(),
            );
            let outcome = state
                .apply(&load, scene, &TwoMaterials, FractureLimits::UNLIMITED)
                .unwrap();
            assert!(outcome.failed.is_empty());
            assert_eq!(state.bond_entries(), 1);
            assert!(state.integrity(site) < prior);
            prior = state.integrity(site);
        }
        assert_eq!(world, before);
        prior
    };
    assert_eq!(run([A, B]), run([B, A]));
}

struct OnlyRegion(RegionPos);
impl Residency for OnlyRegion {
    fn is_resident(&self, p: CellPos) -> bool {
        p.region() == self.0
    }
}

#[test]
fn unknown_neighbor_and_exhausted_work_still_produce_no_applicable_load() {
    let lower = CellPos::new(REGION_EDGE_CELLS - 1, 4, 4);
    let world = joint_world(lower, Axis::X, [A, B], false);
    let before = world.clone();
    let state = FractureState::new();
    let residency = OnlyRegion(lower.region());
    let eval = evaluate_fracture(
        &hit(lower, DamageSpace::StaticWorld, None),
        FractureScene::static_world(&world, &world, &residency),
        &TwoMaterials,
        &state,
        FractureLimits::UNLIMITED,
    )
    .unwrap();
    assert!(matches!(eval, FractureEvaluation::Indeterminate { .. }));
    let eval = evaluate_fracture(
        &hit(lower, DamageSpace::StaticWorld, None),
        FractureScene::static_world(&world, &world, &AllResident),
        &TwoMaterials,
        &state,
        FractureLimits {
            max_bonds_considered: 0,
            ..FractureLimits::UNLIMITED
        },
    )
    .unwrap();
    assert!(matches!(
        eval,
        FractureEvaluation::Deferred {
            reason: FractureDefer::BondBudgetSpent
        }
    ));
    assert_eq!(world, before);
    assert!(state.is_empty());
}

#[test]
fn native_fragment_local_analysis_uses_the_same_symmetric_pair_query() {
    let id = FragmentId::new(900, 0);
    let run = |materials| {
        let world = joint_world(CellPos::new(10, 10, 10), Axis::X, materials, false);
        let fragment = Fragment::from_cells(
            id,
            &world,
            &[CellPos::new(10, 10, 10), CellPos::new(11, 10, 10)]
                .into_iter()
                .collect(),
        )
        .unwrap();
        let lower = fragment.occupied_cells().min().unwrap();
        let before = fragment.clone();
        let state = FractureState::new();
        let load = loaded(
            evaluate_fracture(
                &hit(lower, DamageSpace::FragmentLocal(id), None),
                FractureScene::of_fragment(&fragment),
                &TwoMaterials,
                &state,
                FractureLimits::UNLIMITED,
            )
            .unwrap(),
        );
        assert_eq!(fragment, before);
        assert!(state.is_empty());
        assert_eq!(load.space, DamageSpace::FragmentLocal(id));
        assert_eq!(load.bonds.len(), 1);
        (load.bonds[0].mode, load.bonds[0].load, load.bonds[0].loss)
    };
    assert_eq!(run([A, B]), run([B, A]));
}
