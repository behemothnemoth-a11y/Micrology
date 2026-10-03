//! Only disposable files: prove owner-side sidecar cleanup at a region seam.
use engine_core::{Axis, CellPos, MaterialId, REGION_EDGE_CELLS};
use engine_destruction::{
    BondFracture, BondKey, BondSite, DamageAmount, DamageSpace, FractureLimits, FractureState,
    RegionFracture,
};
use engine_io::{fracture_path, load_all_fracture, save_region_fracture};
use engine_world::{World, WorldEditBatch};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "micrology-material-history-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        // Never reuse a directory that might belong to another process/run.
        std::fs::create_dir(&path).expect("fresh test-only directory");
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn crack(key: BondKey) -> RegionFracture {
    RegionFracture {
        region: key.lower().region(),
        cells: vec![],
        bonds: vec![BondFracture {
            site: BondSite {
                space: DamageSpace::StaticWorld,
                bond: key,
            },
            material: MaterialId(101),
            integrity: DamageAmount(0),
        }],
    }
}

#[test]
fn upper_repaint_reports_unchanged_neighbor_owner_and_prevents_sidecar_resurrection() {
    let dir = Scratch::new();
    let key = BondKey::along(CellPos::new(REGION_EDGE_CELLS - 1, 4, 4), Axis::X).unwrap();
    let distant = BondKey::along(CellPos::new(REGION_EDGE_CELLS * 8, 4, 4), Axis::X).unwrap();
    let mut state = FractureState::new();
    for record in [crack(key), crack(distant)] {
        save_region_fracture(&dir.0, &record).unwrap();
        state
            .restore_region(record, FractureLimits::UNLIMITED)
            .unwrap();
    }
    let distant_path = fracture_path(&dir.0, distant.lower().region());
    let distant_before = std::fs::read(&distant_path).unwrap();
    let mut world = World::new();
    world.set(key.lower(), Some(MaterialId(101)));
    world.set(key.upper(), Some(MaterialId(102)));
    world.take_dirty();
    let mut batch = WorldEditBatch::new();
    batch.set(key.upper(), Some(MaterialId(103)));
    let outcome = state.apply_static_edit(&mut world, &batch);
    assert_eq!(outcome.cleanup.records_removed, 1);
    assert_eq!(
        outcome.cleanup.static_regions,
        [key.lower().region()].into_iter().collect()
    );
    assert!(outcome.edit.dirtied_regions.contains(&key.upper().region()));
    assert!(!outcome.edit.dirtied_regions.contains(&key.lower().region()));
    // Save the reported ownership regions even when they no longer hold records.
    for owner in outcome.cleanup.static_regions {
        save_region_fracture(&dir.0, &state.region_fracture(owner)).unwrap();
    }
    assert!(!fracture_path(&dir.0, key.lower().region()).exists());
    assert_eq!(std::fs::read(distant_path).unwrap(), distant_before);
    let loaded = load_all_fracture(&dir.0, FractureLimits::UNLIMITED).unwrap();
    assert_eq!(loaded, state);
    assert!(!loaded.is_broken(BondSite {
        space: DamageSpace::StaticWorld,
        bond: key
    }));
    assert!(loaded.is_broken(BondSite {
        space: DamageSpace::StaticWorld,
        bond: distant
    }));
}

#[test]
fn no_op_repaint_neither_reports_nor_rewrites_a_damage_sidecar() {
    let dir = Scratch::new();
    let key = BondKey::along(CellPos::new(4, 4, 4), Axis::X).unwrap();
    let record = crack(key);
    save_region_fracture(&dir.0, &record).unwrap();
    let path = fracture_path(&dir.0, record.region);
    let before = std::fs::read(&path).unwrap();
    let mut state = FractureState::new();
    state
        .restore_region(record, FractureLimits::UNLIMITED)
        .unwrap();
    let mut world = World::new();
    world.set(key.lower(), Some(MaterialId(101)));
    world.set(key.upper(), Some(MaterialId(102)));
    let mut batch = WorldEditBatch::new();
    batch.set(key.upper(), Some(MaterialId(102)));
    let result = state.apply_static_edit(&mut world, &batch);
    assert!(result.edit.is_empty());
    assert_eq!(result.cleanup.records_removed, 0);
    assert!(result.cleanup.static_regions.is_empty());
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert_eq!(
        load_all_fracture(&dir.0, FractureLimits::UNLIMITED).unwrap(),
        state
    );
}
