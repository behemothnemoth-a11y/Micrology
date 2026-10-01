//! The region layer: ownership, lookup, load/unload, and the property that
//! matters most for streaming — world contents must not depend on the order
//! regions arrive in.

use engine_core::{
    CellPos, LocalPos, MaterialId, MaterialRegistry, REGION_EDGE_CELLS, REGION_EDGE_VOLUMES,
    RegionPos, Rgb, VOLUME_EDGE, VolumePos,
};
use engine_volume::Volume;
use engine_world::{Region, World};

const STONE: MaterialId = MaterialId(1);
const DIRT: MaterialId = MaterialId(2);

fn world() -> World {
    let mut materials = MaterialRegistry::new();
    materials.define(1, Rgb::new(128, 128, 128), "stone");
    materials.define(2, Rgb::new(120, 72, 40), "dirt");
    World::with_materials(materials)
}

fn volume_of(material: MaterialId, at: LocalPos) -> Volume {
    let mut volume = Volume::new();
    volume.set(at, Some(material));
    volume
}

#[test]
fn a_cell_lands_in_the_region_that_owns_its_volume() {
    let mut w = world();
    w.set(CellPos::new(5, 6, 7), Some(STONE));

    assert_eq!(w.region_count(), 1);
    assert_eq!(
        w.region_positions().collect::<Vec<_>>(),
        vec![RegionPos::ZERO]
    );

    let region = w.region(RegionPos::ZERO).expect("region exists");
    assert_eq!(region.volume_count(), 1);
    assert_eq!(region.occupied_count(), 1);
    assert!(region.volume(VolumePos::new(0, 0, 0)).is_some());
}

#[test]
fn regions_tile_the_world_without_gaps_or_overlap() {
    // Walk a line of cells across several region boundaries and confirm each
    // lands in exactly one region, with no cell unaccounted for.
    let mut w = world();
    let span = REGION_EDGE_CELLS * 2;
    for x in -span..span {
        w.set(CellPos::new(x, 0, 0), Some(STONE));
    }

    let total: u64 = w.regions().map(|(_, r)| r.occupied_count()).sum();
    assert_eq!(total, (span * 2) as u64, "every cell is owned exactly once");

    for x in -span..span {
        let cell = CellPos::new(x, 0, 0);
        let owners: Vec<RegionPos> = w
            .regions()
            .filter(|(_, region)| region.volume(cell.volume()).is_some())
            .map(|(pos, _)| pos)
            .collect();
        assert_eq!(owners, vec![cell.region()], "{cell:?} has one owner");
    }
}

#[test]
fn negative_coordinates_get_their_own_regions() {
    let mut w = world();
    w.set(CellPos::new(-1, -1, -1), Some(STONE));
    w.set(CellPos::new(0, 0, 0), Some(DIRT));

    assert_eq!(
        w.region_positions().collect::<Vec<_>>(),
        vec![RegionPos::new(-1, -1, -1), RegionPos::ZERO],
        "the cell below the origin belongs to the region below it"
    );
    assert_eq!(w.get(CellPos::new(-1, -1, -1)), Some(STONE));
    assert_eq!(w.get(CellPos::new(0, 0, 0)), Some(DIRT));
}

#[test]
fn cells_on_a_region_boundary_do_not_leak_across_it() {
    let mut w = world();
    let last = REGION_EDGE_CELLS - 1;
    w.set(CellPos::new(last, last, last), Some(STONE));
    w.set(CellPos::new(REGION_EDGE_CELLS, 0, 0), Some(DIRT));

    assert_eq!(w.region_count(), 2);
    assert_eq!(
        w.region(RegionPos::ZERO).unwrap().occupied_count(),
        1,
        "the last cell of a region stays in it"
    );
    assert_eq!(
        w.region(RegionPos::new(1, 0, 0)).unwrap().occupied_count(),
        1
    );
    assert_eq!(w.get(CellPos::new(last, last, last)), Some(STONE));
    assert_eq!(w.get(CellPos::new(REGION_EDGE_CELLS, 0, 0)), Some(DIRT));
}

#[test]
fn a_region_can_be_removed_and_reinstalled_exactly() {
    let mut w = world();
    w.set(CellPos::new(3, 3, 3), Some(STONE));
    w.set(CellPos::new(20, 4, 4), Some(DIRT));
    let before = w.clone();

    let evicted = w
        .remove_region(RegionPos::ZERO)
        .expect("region was resident");
    assert_eq!(w.region_count(), 0);
    assert_eq!(
        w.get(CellPos::new(3, 3, 3)),
        None,
        "unloaded reads as absent"
    );

    w.insert_region(RegionPos::ZERO, evicted);
    assert_eq!(w, before, "a round trip through eviction must be exact");
    assert_eq!(w.get(CellPos::new(3, 3, 3)), Some(STONE));
    assert_eq!(w.get(CellPos::new(20, 4, 4)), Some(DIRT));
}

#[test]
fn removing_an_absent_region_is_harmless() {
    let mut w = world();
    assert!(w.remove_region(RegionPos::new(9, 9, 9)).is_none());
    assert_eq!(w.region_count(), 0);
}

#[test]
fn world_contents_do_not_depend_on_region_load_order() {
    // The property streaming rests on: regions arrive in whatever order the
    // disk and the scheduler produce them, and the result must be identical.
    let mut source = world();
    for region in [
        RegionPos::new(0, 0, 0),
        RegionPos::new(1, 0, 0),
        RegionPos::new(0, 1, 0),
        RegionPos::new(-1, 0, -1),
    ] {
        let origin = region.origin_cell();
        source.set(origin, Some(STONE));
        source.set(
            CellPos::new(origin.x + 5, origin.y + 6, origin.z + 7),
            Some(DIRT),
        );
    }

    let loaded: Vec<(RegionPos, Region)> = source
        .regions()
        .map(|(pos, region)| (pos, region.clone()))
        .collect();

    // Forwards, backwards, and an interleaved order must all agree.
    let mut orders = vec![loaded.clone(), {
        let mut r = loaded.clone();
        r.reverse();
        r
    }];
    let mut interleaved = Vec::new();
    let (front, back) = loaded.split_at(loaded.len() / 2);
    for i in 0..front.len().max(back.len()) {
        if let Some(entry) = back.get(i) {
            interleaved.push(entry.clone());
        }
        if let Some(entry) = front.get(i) {
            interleaved.push(entry.clone());
        }
    }
    orders.push(interleaved);

    for order in orders {
        let mut rebuilt = world();
        for (pos, region) in order {
            rebuilt.insert_region(pos, region);
        }
        assert_eq!(rebuilt, source, "load order changed the world");
        assert_eq!(rebuilt.occupied_count(), source.occupied_count());
    }
}

#[test]
fn loading_a_region_marks_its_volumes_and_its_new_neighbours_dirty() {
    let mut w = world();
    // A volume at the very edge of region (0,0,0), touching region (1,0,0).
    let edge_volume = VolumePos::new(REGION_EDGE_VOLUMES - 1, 0, 0);
    w.insert_volume(edge_volume, Volume::filled(STONE));
    w.take_dirty();

    // A neighbouring region arrives with a volume against that seam.
    let arriving = VolumePos::new(REGION_EDGE_VOLUMES, 0, 0);
    let region = Region::from_volumes([(arriving, Volume::filled(DIRT))]);
    w.insert_region(RegionPos::new(1, 0, 0), region);

    let dirty: Vec<VolumePos> = w.take_dirty().into_iter().collect();
    assert!(
        dirty.contains(&arriving),
        "the arriving volume needs a mesh"
    );
    assert!(
        dirty.contains(&edge_volume),
        "the existing neighbour now hides faces against the newcomer and must \
         be rebuilt, or the seam keeps a wall that should be gone"
    );
}

#[test]
fn evicting_a_region_marks_the_neighbours_it_was_hiding_against() {
    let mut w = world();
    let keep = VolumePos::new(REGION_EDGE_VOLUMES - 1, 0, 0);
    let evict = VolumePos::new(REGION_EDGE_VOLUMES, 0, 0);
    w.insert_volume(keep, Volume::filled(STONE));
    w.insert_volume(evict, Volume::filled(DIRT));
    w.take_dirty();

    w.remove_region(RegionPos::new(1, 0, 0)).expect("resident");

    let dirty: Vec<VolumePos> = w.take_dirty().into_iter().collect();
    assert!(
        dirty.contains(&keep),
        "the surviving neighbour's hidden faces are now exposed"
    );
    assert!(dirty.contains(&evict), "the departed volume's mesh must go");
}

#[test]
fn regions_track_whether_they_need_saving() {
    let mut w = world();
    w.set(CellPos::new(1, 1, 1), Some(STONE));
    assert_eq!(
        w.dirty_regions().collect::<Vec<_>>(),
        vec![RegionPos::ZERO],
        "an edited region has unsaved work"
    );

    w.region_mut(RegionPos::ZERO).unwrap().mark_clean();
    assert_eq!(w.dirty_regions().count(), 0);

    // Editing it again marks it unsaved once more.
    w.set(CellPos::new(2, 2, 2), Some(DIRT));
    assert_eq!(w.dirty_regions().collect::<Vec<_>>(), vec![RegionPos::ZERO]);

    // A region that arrived from disk is clean.
    let loaded = Region::from_volumes([(
        VolumePos::new(REGION_EDGE_VOLUMES, 0, 0),
        volume_of(STONE, LocalPos::at(0, 0, 0)),
    )]);
    w.insert_region(RegionPos::new(1, 0, 0), loaded);
    assert!(
        !w.region(RegionPos::new(1, 0, 0)).unwrap().is_dirty(),
        "a freshly loaded region already matches storage"
    );
}

#[test]
fn iteration_is_deterministic_and_ascending() {
    let mut w = world();
    // Insert in a deliberately jumbled order.
    for region in [
        RegionPos::new(2, 0, 0),
        RegionPos::new(-1, 0, 0),
        RegionPos::new(0, 3, 0),
        RegionPos::new(0, 0, -2),
    ] {
        w.set(region.origin_cell(), Some(STONE));
    }

    let order: Vec<RegionPos> = w.region_positions().collect();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted, "regions must iterate in ascending order");
    assert_eq!(order, w.region_positions().collect::<Vec<_>>());

    // Volume iteration is region-major and reproducible; the sorted view is
    // what anything canonical should use.
    let volumes: Vec<VolumePos> = w.volume_positions().collect();
    assert_eq!(volumes, w.volume_positions().collect::<Vec<_>>());
    let sorted_volumes: Vec<VolumePos> = w.volumes_sorted().keys().copied().collect();
    let mut expected = volumes.clone();
    expected.sort();
    assert_eq!(sorted_volumes, expected);
}

#[test]
fn residency_accounting_adds_up() {
    let mut w = world();
    // One uniform volume: no cell storage, just a palette.
    w.insert_volume(VolumePos::new(0, 0, 0), Volume::filled(STONE));
    // One mixed volume: a full dense allocation.
    let mut mixed = Volume::filled(STONE);
    mixed.set(LocalPos::at(0, 0, 0), Some(DIRT));
    w.insert_volume(VolumePos::new(1, 0, 0), mixed);

    let summaries = w.region_summaries();
    assert_eq!(summaries.len(), 1);
    let summary = &summaries[0];
    assert_eq!(summary.pos, RegionPos::ZERO);
    assert_eq!(summary.volumes, 2);
    assert!(summary.dirty);

    let world_total = w.footprint();
    assert_eq!(world_total.cell_bytes, summary.footprint.cell_bytes);
    assert_eq!(
        world_total.cell_bytes,
        (VOLUME_EDGE * VOLUME_EDGE * VOLUME_EDGE * 2) as u64,
        "only the mixed volume owns cell storage"
    );
    assert!(world_total.palette_bytes > 0);
}

#[test]
fn an_edit_still_only_dirties_what_it_must_across_a_region_seam() {
    // The 0002.2 neighbour rule has to keep working now that the volumes
    // either side of a seam live in different regions.
    let mut w = world();
    let left = VolumePos::new(REGION_EDGE_VOLUMES - 1, 0, 0);
    let right = VolumePos::new(REGION_EDGE_VOLUMES, 0, 0);
    w.insert_volume(left, Volume::filled(STONE));
    w.insert_volume(right, Volume::filled(STONE));
    w.take_dirty();

    // Carve the cell on the right-hand side of the seam.
    let seam = right.origin();
    assert!(w.set(seam, None));

    let dirty: Vec<VolumePos> = w.take_dirty().into_iter().collect();
    assert_eq!(
        dirty,
        vec![left, right],
        "both sides of a region seam must be rebuilt"
    );
    assert_ne!(
        left.region(),
        right.region(),
        "and they really are different regions"
    );
}
