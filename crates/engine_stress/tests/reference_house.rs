//! Static layout, cells, surfaces and native persistence only. No destruction tuning.
use engine_core::{CellPos, CellSource};
use engine_destruction::{Fragment, FragmentId};
use engine_geometry::{ExactCompiler, GreedyCompiler, SurfaceCompiler};
use engine_stress::reference_house as house;
use engine_world::World;
use std::{collections::BTreeSet, sync::OnceLock};
fn full() -> &'static World {
    static W: OnceLock<World> = OnceLock::new();
    W.get_or_init(house::build)
}
fn cells(w: &World) -> BTreeSet<CellPos> {
    w.volumes_sorted()
        .into_iter()
        .flat_map(|(v, volume)| {
            volume
                .iter_occupied()
                .map(move |(p, _)| CellPos::from_parts(v, p))
        })
        .collect()
}
#[test]
fn only_four_materials_are_authored_and_resolution_stays_bounded() {
    let w = full();
    let points = cells(w);
    assert!(points.len() > 10000 && points.len() < 250000);
    let ids: BTreeSet<_> = points.iter().map(|p| w.material_at(*p).unwrap()).collect();
    assert_eq!(ids, house::MATERIALS.iter().map(|p| p.0).collect());
    assert_eq!(house::CELL_MM, 50);
}
#[test]
fn doors_rooms_and_chimney_are_open_not_a_solid_house_shaped_block() {
    let w = full();
    for p in [
        [60, 25, 5],
        [40, 30, 60],
        [98, 30, 60],
        [76, 30, 48],
        [13, 70, 85],
        [13, 96, 85],
    ] {
        assert_eq!(w.material_at(CellPos::new(p[0], p[1], p[2])), None, "{p:?}");
    }
    for (p, m) in [
        ([76, 30, 30], house::WOOD),
        ([5, 20, 60], house::MASONRY),
        ([24, 30, 5], house::GLASS),
        ([8, 70, 85], house::MASONRY),
        ([60, 0, 60], house::CONCRETE),
    ] {
        assert_eq!(
            w.material_at(CellPos::new(p[0], p[1], p[2])),
            Some(m),
            "{p:?}"
        );
    }
}
#[test]
fn glass_is_one_cell_thick_and_attached_to_a_timber_frame() {
    let w = full();
    assert_eq!(w.material_at(CellPos::new(24, 30, 4)), None);
    assert_eq!(w.material_at(CellPos::new(24, 30, 5)), Some(house::GLASS));
    assert_eq!(w.material_at(CellPos::new(24, 30, 6)), None);
    assert_eq!(w.material_at(CellPos::new(18, 30, 5)), Some(house::WOOD));
    assert_eq!(w.material_at(CellPos::new(20, 25, 5)), Some(house::GLASS));
}
#[test]
fn support_is_explicit_only_on_the_occupied_concrete_underside() {
    let w = full();
    let mut count = 0;
    for p in cells(w) {
        if w.is_anchor(p) {
            count += 1;
            assert_eq!(p.y, 0);
            assert_eq!(w.material_at(p), Some(house::CONCRETE));
        }
        if p.y == 0 {
            assert!(w.is_anchor(p));
        }
    }
    assert_eq!(count, 128 * 108 + 32 * 16 + 24 * 8);
    assert!(!w.is_anchor(CellPos::new(8, 70, 85)));
}
#[test]
fn intact_geometry_is_one_face_connected_assembly_without_physics() {
    let w = full();
    let points = cells(w);
    let f = Fragment::from_cells(FragmentId::new(600, 0), w, &points).unwrap();
    assert_eq!(f.cell_count(), points.len() as u64);
    assert_eq!(f.connected_parts().len(), 1);
}
#[test]
fn cutaway_only_subtracts_from_a_clone_and_never_changes_the_intact_fixture() {
    let w = full();
    let original = w.clone();
    let cut = house::cutaway(w);
    assert_eq!(*w, original);
    let before = cells(w);
    let after = cells(&cut);
    assert!(after.is_subset(&before));
    assert!(after.len() < before.len());
    for p in &after {
        assert_eq!(cut.material_at(*p), w.material_at(*p));
    }
    assert_eq!(cut.material_at(CellPos::new(63, 85, 50)), None);
    assert_eq!(cut.material_at(CellPos::new(63, 83, 55)), Some(house::WOOD));
}
#[test]
fn every_exported_surface_matches_the_independent_exact_oracle() {
    for w in [full().clone(), house::cutaway(full())] {
        for v in w.volumes_sorted().into_keys() {
            let exact = ExactCompiler.compile(&w, v);
            let greedy = GreedyCompiler.compile(&w, v);
            assert_eq!(exact.unit_faces(), greedy.unit_faces(), "volume {v:?}");
            assert_eq!(exact.total_area(), greedy.total_area());
        }
    }
}
struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!(
            "micrology-house-step6-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn native_save_reload_preserves_cells_material_ids_and_authored_support() {
    let dir = Temp::new();
    let w = full();
    let meta = house::meta();
    engine_io::v2::save_world_v2(w, &meta, &dir.0).unwrap();
    let loaded = engine_io::v2::load_world_v2(&dir.0).unwrap();
    assert_eq!(loaded.world, *w);
    assert_eq!(loaded.meta, meta);
    for p in cells(w) {
        assert_eq!(loaded.world.is_anchor(p), w.is_anchor(p));
    }
}
#[test]
fn exported_house_cannot_overwrite_an_existing_world() {
    let dir = Temp::new();
    let sentinel = dir.0.join("sentinel.txt");
    std::fs::write(&sentinel, b"untouched").unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_reference_house_fixture"))
        .arg("--export")
        .arg(&dir.0)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(std::fs::read(sentinel).unwrap(), b"untouched");
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
}
#[test]
fn repeated_builds_match_the_first_reviewed_static_layout_baseline() {
    let a = house::report(full());
    let b = house::report(&house::build());
    assert_eq!(a, b);
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/destruction/reference-house-static-v1.json"
    ))
    .unwrap();
    assert_eq!(serde_json::to_value(a).unwrap(), expected);
}
