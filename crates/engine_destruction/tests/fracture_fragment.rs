//! DROP 0006.3 acceptance: a detached chunk is destructible by the same model.
//!
//! Before this pass a plug's cracks were forgotten the moment it came loose, so
//! every fragment arrived pristine however hard it had been hit, and re-damage
//! ran on the DROP 0004 scalar path as a special case. Both are gone.

use engine_core::{Axis, CellPos, CellSource, FaceDir, MaterialId};
use engine_destruction::{
    AllResident, BOND_INTEGRITY, BaselineFracturePolicy, BondKey, BondLoad, BondMode, BondSite,
    CellDeposit, DamageAmount, DamageEventId, DamageSite, DamageSpace, DamageTarget,
    DestructionSequence, FractureEvaluation, FractureImpact, FractureLimits, FractureLoad,
    FractureMeasurement, FractureScene, FractureState, Fragment, FragmentDamageResult, FragmentId,
    FragmentStore, REFERENCE_FRACTURE_MATERIAL, RefractureRefusal, StructuralLimits,
    damage_fragment_store_if, damage_fragment_store_with_parts, evaluate_fracture,
    fracture_hit_toward_in, fragment_parts_through_cracks, reference_fracture_materials,
    reference_impact_direction,
};
use engine_world::World;
use std::collections::BTreeSet;

const SOLID: MaterialId = REFERENCE_FRACTURE_MATERIAL;

/// A slab big enough to crack across and still have cells on both sides.
fn slab() -> World {
    let mut world = World::with_materials(reference_fracture_materials());
    world.fill_box(
        CellPos::new(20, 20, 20),
        CellPos::new(23, 25, 21),
        Some(SOLID),
    );
    world.take_dirty();
    world
}

fn fragment_from(world: &World, cells: &BTreeSet<CellPos>, id: FragmentId) -> Fragment {
    Fragment::from_cells(id, world, cells).expect("the cells are occupied")
}

/// Record damage at exact places, with no energy model in the way.
fn record(
    state: &mut FractureState,
    scene: FractureScene<'_>,
    cells: Vec<CellDeposit>,
    bonds: Vec<BondLoad>,
) {
    let load = FractureLoad {
        state: state.revision(),
        space: scene.space(),
        cells,
        bonds,
        measurement: FractureMeasurement::default(),
    };
    state
        .apply(
            &load,
            scene,
            &BaselineFracturePolicy::REFERENCE,
            FractureLimits::UNLIMITED,
        )
        .expect("a hand-built load fits an unlimited budget");
}

fn deposit(cell: CellPos, energy: u32) -> CellDeposit {
    CellDeposit {
        cell,
        material: SOLID,
        energy: DamageAmount(energy),
        distance_milli: 0,
    }
}

fn breaking(bond: BondKey) -> BondLoad {
    BondLoad {
        bond,
        material: SOLID,
        mode: BondMode::Tension,
        load: DamageAmount(10_000),
        loss: BOND_INTEGRITY,
    }
}

#[test]
fn a_detached_plug_takes_its_cracks_into_the_fragment_it_becomes() {
    let world = slab();
    let mut state = FractureState::new();
    let scene = FractureScene::static_world(&world, &world, &AllResident);

    // Damage two neighbouring cells and the bond between them, in world space.
    let a = CellPos::new(21, 22, 20);
    let b = CellPos::new(22, 22, 20);
    let bond = BondKey::between(a, b).unwrap();
    record(
        &mut state,
        scene,
        vec![deposit(a, 400), deposit(b, 900)],
        vec![BondLoad {
            loss: DamageAmount(300),
            ..breaking(bond)
        }],
    );
    assert_eq!(state.cell_entries(), 2);
    assert_eq!(state.bond_entries(), 1);

    // The whole slab comes loose.
    let cells: BTreeSet<CellPos> = world
        .volumes_sorted()
        .into_iter()
        .flat_map(|(pos, volume)| {
            volume
                .iter_occupied()
                .map(move |(local, _)| CellPos::from_parts(pos, local))
        })
        .collect();
    let fragment = fragment_from(&world, &cells, FragmentId::new(1, 0));
    let moved = state.adopt_into_fragment(&fragment);
    assert_eq!(moved, 3, "two cell records and one bond record");

    // Nothing is left describing the static world.
    assert!(
        state
            .cells()
            .chain(std::iter::empty())
            .all(|record| record.site.space != DamageSpace::StaticWorld)
    );
    assert!(
        state
            .bonds()
            .all(|record| record.site.space != DamageSpace::StaticWorld)
    );

    // And the damage is where the fragment keeps its geometry: world - origin.
    let origin = fragment.source_origin;
    let local =
        |cell: CellPos| CellPos::new(cell.x - origin.x, cell.y - origin.y, cell.z - origin.z);
    let space = DamageSpace::FragmentLocal(fragment.id);
    assert_eq!(
        state
            .cell(DamageSite {
                space,
                cell: local(b)
            })
            .unwrap()
            .energy,
        DamageAmount(900)
    );
    assert_eq!(
        state.integrity(BondSite {
            space,
            bond: BondKey::between(local(a), local(b)).unwrap(),
        }),
        DamageAmount(BOND_INTEGRITY.0 - 300)
    );
}

#[test]
fn a_bond_across_the_separation_is_not_carried_into_the_fragment() {
    // The face a plug broke away from is where the separation happened. It is
    // not a crack on either side afterwards; it is a gap.
    let world = slab();
    let mut state = FractureState::new();
    let scene = FractureScene::static_world(&world, &world, &AllResident);

    let inside = CellPos::new(21, 22, 20);
    let outside = CellPos::new(21, 23, 20);
    let crossing = BondKey::between(inside, outside).unwrap();
    record(&mut state, scene, Vec::new(), vec![breaking(crossing)]);
    assert_eq!(state.bond_entries(), 1);

    // A fragment holding only the cells below the cut.
    let cells: BTreeSet<CellPos> = (20..=23)
        .flat_map(|x| (20..=21).map(move |z| (x, z)))
        .flat_map(|(x, z)| (20..=22).map(move |y| CellPos::new(x, y, z)))
        .collect();
    assert!(cells.contains(&inside) && !cells.contains(&outside));
    let fragment = fragment_from(&world, &cells, FragmentId::new(2, 0));

    assert_eq!(state.adopt_into_fragment(&fragment), 0);
    assert!(
        state.is_empty(),
        "the crossing bond belongs to neither side now"
    );
}

/// A detached chunk, and the state that came with it.
fn detached_chunk() -> (Fragment, FractureState) {
    let world = slab();
    let cells: BTreeSet<CellPos> = world
        .volumes_sorted()
        .into_iter()
        .flat_map(|(pos, volume)| {
            volume
                .iter_occupied()
                .map(move |(local, _)| CellPos::from_parts(pos, local))
        })
        .collect();
    let fragment = fragment_from(&world, &cells, FragmentId::new(10, 0));
    (fragment, FractureState::new())
}

/// Hit a fragment in its own coordinate space.
fn hit_fragment(
    fragment: &Fragment,
    state: &mut FractureState,
    sequence: u64,
    energy: u32,
) -> Vec<DamageTarget> {
    let target = CellPos::new(
        (fragment.bounds.min.x + fragment.bounds.max.x) / 2,
        (fragment.bounds.min.y + fragment.bounds.max.y) / 2,
        fragment.bounds.max.z,
    );
    let event = fracture_hit_toward_in(
        DamageEventId::new(sequence, 0),
        DamageSpace::FragmentLocal(fragment.id),
        target,
        [0, 0, -1000],
        DamageAmount(energy),
    );
    // A fragment-local event carries no world-space impulse, so the direction is
    // handed over explicitly — the seam `from_event` exists for.
    let impact =
        FractureImpact::from_event(&event, Some(reference_impact_direction([0, 0, -1000])))
            .expect("the stimulus quantises");
    let policy = BaselineFracturePolicy::REFERENCE;
    let scene = FractureScene::of_fragment(fragment);
    let load = match evaluate_fracture(&impact, scene, &policy, state, FractureLimits::UNLIMITED)
        .expect("the stimulus and the fragment share one space")
    {
        FractureEvaluation::Loaded(load) => load,
        other => panic!("expected a conclusive evaluation, got {other:?}"),
    };
    assert!(!load.is_empty(), "the hit must land on the chunk");
    let scene = FractureScene::of_fragment(fragment);
    state
        .apply(&load, scene, &policy, FractureLimits::UNLIMITED)
        .expect("unlimited budget")
        .failed
}

#[test]
fn a_detached_chunk_takes_damage_in_its_own_coordinate_space() {
    let (fragment, mut state) = detached_chunk();
    hit_fragment(&fragment, &mut state, 1, 900);

    assert!(!state.is_empty());
    let space = DamageSpace::FragmentLocal(fragment.id);
    assert!(state.cells().all(|record| record.site.space == space));
    assert!(state.bonds().all(|record| record.site.space == space));
    // Nothing leaked into the static world, whose cells share no addresses with
    // the fragment's at all.
    assert!(state.static_regions().is_empty());
}

#[test]
fn hitting_a_detached_chunk_continues_its_existing_damage() {
    // The point of the pass: a chunk that was already hit is weaker than a fresh
    // one, in its own space, through the same model.
    let (fragment, mut state) = detached_chunk();
    hit_fragment(&fragment, &mut state, 1, 900);
    let after_one = state.digest();
    assert_eq!(after_one.broken_bonds, 0, "one weak hit cracks nothing yet");

    hit_fragment(&fragment, &mut state, 2, 900);
    let after_two = state.digest();
    assert_ne!(after_two.checksum, after_one.checksum);
    assert!(
        after_two.broken_bonds > after_one.broken_bonds,
        "the second hit must build on the first"
    );

    // And a fresh chunk hit once as hard is not in the same state.
    let (_, mut fresh) = detached_chunk();
    hit_fragment(&fragment, &mut fresh, 1, 900);
    assert_eq!(fresh.digest(), after_one);
    assert_ne!(fresh.digest(), after_two);
}

#[test]
fn a_fragments_damage_is_not_the_static_worlds_damage() {
    // Same numeric coordinates, different spaces. A static hit must not weaken a
    // fragment that happens to overlap it.
    let (fragment, mut state) = detached_chunk();
    hit_fragment(&fragment, &mut state, 1, 900);
    let fragment_only = state.digest();

    let world = slab();
    let scene = FractureScene::static_world(&world, &world, &AllResident);
    record(
        &mut state,
        scene,
        vec![deposit(CellPos::new(21, 22, 20), 500)],
        Vec::new(),
    );
    assert_ne!(state.digest().checksum, fragment_only.checksum);

    // Forgetting the static world leaves the fragment exactly as it was.
    state.forget_space(DamageSpace::StaticWorld);
    assert_eq!(state.digest(), fragment_only);
}

/// A fragment cut in half by cracks: every bond across one plane is broken.
///
/// Also carries damage *away* from the seam — a dented cell and a half-broken
/// bond on each side — so a split has something to hand on as well as something
/// to drop.
fn cracked_in_half() -> (Fragment, FractureState, usize) {
    let (fragment, mut state) = detached_chunk();
    let split_at = (fragment.bounds.min.y + fragment.bounds.max.y) / 2;
    let scene = FractureScene::of_fragment(&fragment);
    let seam: Vec<BondLoad> = fragment
        .occupied_cells()
        .filter(|cell| cell.y == split_at)
        .filter_map(|cell| BondKey::along(cell, Axis::Y))
        .map(breaking)
        .collect();
    assert!(!seam.is_empty());
    let seam_count = seam.len();

    // Damage that is not the seam: one cell and one in-plane bond per side.
    let mut bonds = seam;
    let mut cells = Vec::new();
    for y in [fragment.bounds.min.y, fragment.bounds.max.y] {
        let cell = CellPos::new(fragment.bounds.min.x, y, fragment.bounds.min.z);
        cells.push(deposit(cell, 400));
        if let Some(bond) = BondKey::along(cell, Axis::X) {
            bonds.push(BondLoad {
                loss: DamageAmount(250),
                ..breaking(bond)
            });
        }
    }
    record(&mut state, scene, cells, bonds);
    (fragment, state, seam_count)
}

#[test]
fn a_cracked_chunk_comes_apart_without_a_cell_being_deleted() {
    let (fragment, state, _) = cracked_in_half();
    let before = fragment.cell_count();

    let occupancy = fragment.connected_parts();
    assert_eq!(
        occupancy.len(),
        1,
        "occupancy connectivity still sees one object, and that stays true"
    );

    let through_cracks =
        fragment_parts_through_cracks(&fragment, &state, StructuralLimits::UNLIMITED);
    assert_eq!(through_cracks.len(), 2, "the cracks cut it in two");
    assert_eq!(
        through_cracks.iter().map(BTreeSet::len).sum::<usize>() as u64,
        before,
        "every cell must end up in exactly one piece"
    );
}

#[test]
fn re_fracture_through_cracks_goes_through_the_existing_atomic_transaction() {
    let (fragment, mut state, _) = cracked_in_half();
    let parent = fragment.id;
    let mut store = FragmentStore::default();
    store.insert(fragment.clone());

    // A refusing admission policy changes nothing at all.
    let mut sequence = DestructionSequence::new(0);
    let refused = damage_fragment_store_with_parts(
        &mut store,
        &mut sequence,
        parent,
        &[],
        |f| fragment_parts_through_cracks(f, &state, StructuralLimits::UNLIMITED),
        |_| false,
    );
    assert_eq!(refused, Err(RefractureRefusal::RejectedByPolicy));
    assert_eq!(store.len(), 1);
    assert!(store.contains(parent));
    assert_eq!(sequence.peek(), 0, "a refusal must not consume identity");

    // Accepting replaces the parent with children, no cell deleted.
    let mut sequence = DestructionSequence::new(0);
    let outcome = damage_fragment_store_with_parts(
        &mut store,
        &mut sequence,
        parent,
        &[],
        |f| fragment_parts_through_cracks(f, &state, StructuralLimits::UNLIMITED),
        |_| true,
    )
    .expect("the cracks split it");
    let FragmentDamageResult::Refractured(refracture) = outcome else {
        panic!("expected a re-fracture");
    };
    assert_eq!(refracture.fragments.len(), 2);
    assert!(!store.contains(parent));
    assert_eq!(
        refracture
            .fragments
            .iter()
            .map(Fragment::cell_count)
            .sum::<u64>(),
        fragment.cell_count()
    );

    // The records follow the cells: damage away from the seam survives, and
    // nothing is still addressed to a parent that no longer exists.
    let kept = state.remap_fragment(parent, &refracture.fragments);
    assert!(kept > 0, "damage away from the seam must survive the split");
    assert_eq!(
        state
            .cells()
            .chain(std::iter::empty())
            .filter(|r| r.site.space == DamageSpace::FragmentLocal(parent))
            .count(),
        0
    );
    assert_eq!(
        state
            .bonds()
            .filter(|r| r.site.space == DamageSpace::FragmentLocal(parent))
            .count(),
        0,
        "nothing may still be addressed to the parent"
    );
    for record in state.cells() {
        let DamageSpace::FragmentLocal(owner) = record.site.space else {
            panic!("a fragment's damage must stay a fragment's");
        };
        let child = refracture
            .fragments
            .iter()
            .find(|child| child.id == owner)
            .expect("records belong to a live child");
        assert!(child.material_at(record.site.cell).is_some());
    }
    for record in state.bonds() {
        let DamageSpace::FragmentLocal(owner) = record.site.space else {
            panic!("a fragment's cracks must stay a fragment's");
        };
        let child = refracture
            .fragments
            .iter()
            .find(|child| child.id == owner)
            .expect("records belong to a live child");
        for cell in record.site.bond.cells() {
            assert!(child.material_at(cell).is_some());
        }
    }
}

#[test]
fn a_bond_whose_cells_land_in_different_children_is_dropped() {
    // That bond is the seam the object came apart along.
    let (fragment, mut state, seam_count) = cracked_in_half();
    let parent = fragment.id;
    let before = state.bond_entries();
    assert!(
        before > seam_count,
        "there is non-seam damage to compare against"
    );

    let parts = fragment_parts_through_cracks(&fragment, &state, StructuralLimits::UNLIMITED);
    let children: Vec<Fragment> = parts
        .iter()
        .enumerate()
        .filter_map(|(index, cells)| {
            fragment.subfragment_with_id(FragmentId::new(99, index as u32), cells)
        })
        .collect();
    assert_eq!(children.len(), 2);

    state.remap_fragment(parent, &children);
    assert_eq!(
        state.bond_entries(),
        before - seam_count,
        "exactly the seam bonds are dropped, and nothing else"
    );
    assert_eq!(state.broken_bonds(), 0, "the seam was all that was broken");
}

#[test]
fn occupancy_is_still_the_default_rule_for_splitting_a_fragment() {
    // `damage_fragment_store_if` has not changed meaning. A chunk whose cracks cut
    // it in half is still one object to the occupancy rule until a cell leaves.
    let (fragment, _, _) = cracked_in_half();
    let parent = fragment.id;
    let mut store = FragmentStore::default();
    store.insert(fragment);
    let mut sequence = DestructionSequence::new(0);

    assert_eq!(
        damage_fragment_store_if(&mut store, &mut sequence, parent, &[], |_| true),
        Ok(FragmentDamageResult::Unchanged)
    );
    assert!(store.contains(parent));
}

#[test]
fn a_destroyed_chunk_leaves_no_records_behind() {
    let (fragment, mut state) = detached_chunk();
    let parent = fragment.id;
    hit_fragment(&fragment, &mut state, 1, 900);
    assert!(!state.is_empty());

    let mut store = FragmentStore::default();
    store.insert(fragment.clone());
    let mut sequence = DestructionSequence::new(0);
    let every_cell: Vec<DamageTarget> = fragment
        .occupied_cells()
        .map(|cell| DamageTarget::FragmentCell {
            fragment: parent,
            cell,
            material: SOLID,
        })
        .collect();

    assert_eq!(
        damage_fragment_store_if(&mut store, &mut sequence, parent, &every_cell, |_| true),
        Ok(FragmentDamageResult::Destroyed { parent })
    );
    assert!(store.is_empty());
    assert!(state.forget_space(DamageSpace::FragmentLocal(parent)) > 0);
    assert!(state.is_empty(), "a gone object keeps no cracks");
}

#[test]
fn a_fragment_has_no_anchors_so_nothing_in_it_is_exempt_from_failing() {
    // The static world's anchored-cell rule must not quietly protect fragment
    // cells: a fragment has no foundation, which is what it means to be loose.
    let (fragment, mut state) = detached_chunk();
    let scene = FractureScene::of_fragment(&fragment);
    let cell = fragment.occupied_cells().next().unwrap();
    // Everything around it goes, so the cell is held by nothing.
    let bonds: Vec<BondLoad> = FaceDir::ALL
        .into_iter()
        .filter_map(|dir| BondKey::new(cell, dir))
        .map(breaking)
        .collect();
    let load = FractureLoad {
        state: state.revision(),
        space: DamageSpace::FragmentLocal(fragment.id),
        cells: Vec::new(),
        bonds,
        measurement: FractureMeasurement::default(),
    };
    let outcome = state
        .apply(
            &load,
            scene,
            &BaselineFracturePolicy::REFERENCE,
            FractureLimits::UNLIMITED,
        )
        .unwrap();
    assert!(
        outcome.failed.iter().any(|target| target.cell() == cell),
        "an unbonded fragment cell must fail like any other"
    );
}

#[test]
fn the_same_damage_to_the_same_chunk_gives_the_same_state_every_run() {
    let run = || {
        let (fragment, mut state) = detached_chunk();
        let mut failures = Vec::new();
        for shot in 1..=3u64 {
            failures.push(hit_fragment(&fragment, &mut state, shot, 1_800).len());
        }
        (state, failures)
    };
    let (a, fa) = run();
    let (b, fb) = run();
    assert_eq!(fa, fb);
    assert_eq!(a, b);
    assert_eq!(a.digest(), b.digest());
}
