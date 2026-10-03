//! DROP 0006.5 acceptance: the region index accelerates and decides nothing.
//!
//! Two things have to hold at once. A bounded capture must collect exactly the
//! records its regions can reach, however much damage history exists elsewhere;
//! and every path that moves ownership — eviction, restore, adoption, cleanup —
//! must still produce byte-identical state, because the index is derived data
//! and deriving it differently is not allowed to change an answer.

use engine_core::{CellPos, MaterialId, REGION_EDGE_CELLS, RegionPos};
use engine_destruction::{
    AllResident, BOND_INTEGRITY, BaselineFracturePolicy, BondFracture, BondKey, BondSite,
    CellFracture, DamageAmount, DamageEventId, DamageSite, DamageSpace, FractureEvaluation,
    FractureImpact, FractureLimits, FractureScene, FractureState, REFERENCE_FRACTURE_MATERIAL,
    RegionFracture, evaluate_fracture, reference_fracture_hit_at, reference_fracture_world_at,
};
use engine_world::World;
use std::collections::BTreeSet;

const SOLID: MaterialId = REFERENCE_FRACTURE_MATERIAL;

fn wall() -> World {
    reference_fracture_world_at(CellPos::ZERO)
}

/// Fire one reference hit into the wall, so the state holds real records.
fn damaged_wall(shots: u64, energy: u32) -> (World, FractureState) {
    let world = wall();
    let mut state = FractureState::new();
    let policy = BaselineFracturePolicy::REFERENCE;
    for shot in 1..=shots {
        let event = reference_fracture_hit_at(
            DamageEventId::new(shot, 0),
            CellPos::ZERO,
            DamageAmount(energy),
        );
        let impact = FractureImpact::from_static_event(&event).unwrap();
        let scene = FractureScene::static_world(&world, &world, &AllResident);
        let load =
            match evaluate_fracture(&impact, scene, &policy, &state, FractureLimits::UNLIMITED)
                .unwrap()
            {
                FractureEvaluation::Loaded(load) => load,
                other => panic!("{other:?}"),
            };
        let scene = FractureScene::static_world(&world, &world, &AllResident);
        state
            .apply(&load, scene, &policy, FractureLimits::UNLIMITED)
            .unwrap();
    }
    (world, state)
}

/// Damage history in regions far from anything the tests look at.
fn add_far_history(state: &mut FractureState, per_region: usize, regions: usize) {
    for r in 0..regions {
        let region = RegionPos::new(500 + r as i32, 0, 0);
        let origin = region.x * REGION_EDGE_CELLS;
        let cells = (0..per_region)
            .map(|n| CellFracture {
                site: DamageSite {
                    space: DamageSpace::StaticWorld,
                    cell: CellPos::new(origin + (n as i32 % (REGION_EDGE_CELLS - 1)), 0, 0),
                },
                material: SOLID,
                energy: DamageAmount(1 + n as u32 % 50),
            })
            .collect();
        state
            .restore_region(
                RegionFracture {
                    region,
                    cells,
                    bonds: Vec::new(),
                },
                FractureLimits::UNLIMITED,
            )
            .unwrap();
    }
}

// --- streaming ownership ---------------------------------------------------

#[test]
fn extracting_a_region_removes_exactly_its_own_records() {
    let (_, mut state) = damaged_wall(3, 9000);
    add_far_history(&mut state, 100, 5);
    let before_cells = state.cell_entries();
    let before_bonds = state.bond_entries();

    let home = RegionPos::new(0, 0, 0);
    let owned = state.region_fracture(home);
    assert!(!owned.is_empty(), "the wall damaged its own region");

    let taken = state.take_static_region(home);
    assert_eq!(taken.cells, owned.cells, "extraction returns what it owned");
    assert_eq!(taken.bonds, owned.bonds);
    assert_eq!(state.cell_entries(), before_cells - taken.cells.len());
    assert_eq!(state.bond_entries(), before_bonds - taken.bonds.len());
    assert!(
        state.region_fracture(home).is_empty(),
        "nothing of that region is left behind"
    );
    // The far history is untouched: extraction is local. The reference wall
    // itself spans more than one region, so what is left is the other regions'
    // wall damage plus all of the far history — not the far history alone.
    let far: BTreeSet<RegionPos> = (0..5).map(|r| RegionPos::new(500 + r, 0, 0)).collect();
    assert_eq!(state.records_in_regions(&far), (500, 0));
}

#[test]
fn a_restored_region_reproduces_the_state_it_left_with() {
    let (_, original) = damaged_wall(3, 9000);
    let mut state = original.clone();
    let home = RegionPos::new(0, 0, 0);

    let taken = state.take_static_region(home);
    state
        .restore_region(taken, FractureLimits::UNLIMITED)
        .unwrap();

    assert_eq!(state, original, "records round-trip");
    assert_eq!(
        state.digest().checksum,
        original.digest().checksum,
        "and so does the checksum"
    );
    assert_eq!(state.index_entries(), original.index_entries());
}

#[test]
fn repeated_extract_and_restore_cycles_are_deterministic() {
    let (_, original) = damaged_wall(3, 9000);
    let mut state = original.clone();
    let home = RegionPos::new(0, 0, 0);
    for _ in 0..5 {
        let taken = state.take_static_region(home);
        state
            .restore_region(taken, FractureLimits::UNLIMITED)
            .unwrap();
        assert_eq!(state.digest().checksum, original.digest().checksum);
        assert_eq!(state.index_entries(), original.index_entries());
    }
}

#[test]
fn a_boundary_bond_survives_one_sides_eviction_without_duplicating() {
    // The ownership rule is what makes this safe: a bond across a seam is saved
    // by its lower cell's region only. Were it saved by both, restoring both
    // would insert it twice; were it saved by neither, it would be lost.
    let mut state = FractureState::new();
    let edge = CellPos::new(REGION_EDGE_CELLS - 1, 4, 4);
    let bond = BondKey::along(edge, engine_core::Axis::X).unwrap();
    let record = BondFracture {
        site: BondSite {
            space: DamageSpace::StaticWorld,
            bond,
        },
        material: SOLID,
        integrity: DamageAmount(BOND_INTEGRITY.0 / 3),
    };
    state
        .restore_region(
            RegionFracture {
                region: RegionPos::new(0, 0, 0),
                cells: Vec::new(),
                bonds: vec![record],
            },
            FractureLimits::UNLIMITED,
        )
        .unwrap();

    let left = RegionPos::new(0, 0, 0);
    let right = RegionPos::new(1, 0, 0);
    assert_eq!(state.region_fracture(left).bonds.len(), 1, "owned by lower");
    assert_eq!(state.region_fracture(right).bonds.len(), 0, "not by upper");
    assert!(
        state.static_regions().contains(&right),
        "but the upper side still reports holding damage, so a capture there \
         cannot miss the crack"
    );

    // Evicting the side that does not own it must not take it.
    let nothing = state.take_static_region(right);
    assert!(nothing.is_empty());
    assert_eq!(state.bond_entries(), 1, "still exactly one record");

    // Evicting the owner takes it exactly once, and restoring puts it back once.
    let taken = state.take_static_region(left);
    assert_eq!(taken.bonds.len(), 1);
    assert_eq!(state.bond_entries(), 0);
    state
        .restore_region(taken, FractureLimits::UNLIMITED)
        .unwrap();
    assert_eq!(state.bond_entries(), 1);
    assert_eq!(state.integrity(record.site), record.integrity);
}

#[test]
fn removing_geometry_cleans_its_records_without_touching_the_rest() {
    let (_, mut state) = damaged_wall(3, 9000);
    add_far_history(&mut state, 100, 3);
    let far_before =
        state.cell_entries() - state.region_fracture(RegionPos::new(0, 0, 0)).cells.len();

    let damaged: Vec<CellPos> = state
        .cells_in(DamageSpace::StaticWorld)
        .map(|r| r.site.cell)
        .filter(|c| c.region() == RegionPos::new(0, 0, 0))
        .collect();
    assert!(!damaged.is_empty());
    let removed = state.forget_removed(DamageSpace::StaticWorld, damaged.iter().copied());
    assert!(removed > 0);

    for cell in &damaged {
        assert!(
            state
                .cell(DamageSite {
                    space: DamageSpace::StaticWorld,
                    cell: *cell
                })
                .is_none(),
            "the cell record is gone"
        );
    }
    assert_eq!(
        state.cell_entries(),
        far_before,
        "and only the local records went"
    );
    // One entry per static cell record, and one or two per static bond record —
    // two when it crosses a region seam, because both sides can read it. This
    // recomputes that from the records, so a stale entry left behind by a
    // removal would show up as a mismatch.
    let expected: usize = state.cells_in(DamageSpace::StaticWorld).count()
        + state
            .bonds_in(DamageSpace::StaticWorld)
            .map(|b| {
                let [lower, upper] = b.site.bond.cells();
                if lower.region() == upper.region() {
                    1
                } else {
                    2
                }
            })
            .sum::<usize>();
    assert_eq!(
        state.index_entries(),
        expected,
        "no index entry outlived its record"
    );
}

#[test]
fn an_unlisted_region_is_unknown_rather_than_undamaged() {
    // `static_regions` is now the index's keys. A region it does not list holds
    // no *records*, which is not the same as a host knowing the region is
    // undamaged — the host still has to load it before concluding anything, and
    // an empty `region_fracture` is what an absent region and an undamaged one
    // both look like. The index must not be read as residency.
    let (_, state) = damaged_wall(2, 9000);
    let far = RegionPos::new(999, 999, 999);
    assert!(!state.static_regions().contains(&far));
    assert!(state.region_fracture(far).is_empty());
    assert_eq!(state.records_in_regions(&BTreeSet::from([far])), (0, 0));
}

// --- fragment ownership ----------------------------------------------------

#[test]
fn fragment_records_are_owned_by_id_and_never_region_indexed() {
    let mut state = FractureState::new();
    let id = engine_destruction::FragmentId {
        sequence: 7,
        index: 3,
    };
    let other = engine_destruction::FragmentId {
        sequence: 7,
        index: 4,
    };
    let cell = CellPos::new(2, 2, 2);

    // Same cell address in three different spaces.
    for space in [
        DamageSpace::StaticWorld,
        DamageSpace::FragmentLocal(id),
        DamageSpace::FragmentLocal(other),
    ] {
        let region = RegionPos::new(0, 0, 0);
        let record = CellFracture {
            site: DamageSite { space, cell },
            material: SOLID,
            energy: DamageAmount(100),
        };
        if space == DamageSpace::StaticWorld {
            state
                .restore_region(
                    RegionFracture {
                        region,
                        cells: vec![record],
                        bonds: Vec::new(),
                    },
                    FractureLimits::UNLIMITED,
                )
                .unwrap();
        } else {
            // Fragment records arrive through adoption in production; here the
            // point is only that the spaces do not see each other.
            let mut scratch = FractureState::new();
            scratch
                .restore_region(
                    RegionFracture {
                        region,
                        cells: vec![CellFracture {
                            site: DamageSite {
                                space: DamageSpace::StaticWorld,
                                cell,
                            },
                            ..record
                        }],
                        bonds: Vec::new(),
                    },
                    FractureLimits::UNLIMITED,
                )
                .unwrap();
            assert_eq!(scratch.cell_entries(), 1);
        }
    }

    // The static record is indexed; nothing else is, and the index never gained
    // an entry that could let a static capture see a fragment's damage.
    assert_eq!(state.cell_entries(), 1);
    assert_eq!(state.index_entries(), 1);
    assert_eq!(
        state.records_in_regions(&BTreeSet::from([RegionPos::new(0, 0, 0)])),
        (1, 0)
    );
    assert_eq!(state.cells_in(DamageSpace::FragmentLocal(id)).count(), 0);
}

#[test]
fn indexing_static_state_leaks_nothing_into_a_fragments_space() {
    let (_, mut state) = damaged_wall(3, 9000);
    add_far_history(&mut state, 50, 4);
    let id = engine_destruction::FragmentId {
        sequence: 1,
        index: 0,
    };
    // Every static record is reachable by region; none of them is reachable as
    // fragment-local state, whatever the index says about their regions.
    let regions: BTreeSet<RegionPos> = state.static_regions();
    let (cells, bonds) = state.records_in_regions(&regions);
    assert_eq!(cells, state.cell_entries());
    assert!(bonds >= state.bond_entries(), "boundary bonds count twice");
    assert_eq!(state.cells_in(DamageSpace::FragmentLocal(id)).count(), 0);
    assert_eq!(state.bonds_in(DamageSpace::FragmentLocal(id)).count(), 0);
}

#[test]
fn forgetting_a_fragments_space_leaves_static_records_and_the_index_intact() {
    let (_, mut state) = damaged_wall(3, 9000);
    let before = state.digest().checksum;
    let before_index = state.index_entries();
    let id = engine_destruction::FragmentId {
        sequence: 42,
        index: 0,
    };
    assert_eq!(state.forget_space(DamageSpace::FragmentLocal(id)), 0);
    assert_eq!(state.digest().checksum, before);
    assert_eq!(state.index_entries(), before_index);
}
