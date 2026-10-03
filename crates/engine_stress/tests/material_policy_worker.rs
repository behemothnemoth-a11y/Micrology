//! Owned worker policy transport, not material realism or house tuning.
use engine_core::{CellPos, GlobalPos, MaterialId, RegionPos, Revision};
use engine_destruction::{
    fracture::FractureModel,
    fracture_jobs::{FractureJobInput, FractureJobLimits, JobRefusal, PolicyFractureJobResult},
    fracture_policy::{OwnedFracturePolicy, PolicySnapshotLimits, PolicySnapshotRefusal},
    *,
};
use engine_world::{World, WorldEditBatch};
use std::collections::BTreeSet;

fn known() -> BTreeSet<RegionPos> {
    (-1..=1)
        .flat_map(|x| (-1..=1).flat_map(move |y| (-1..=1).map(move |z| RegionPos::new(x, y, z))))
        .collect()
}
fn profiles() -> Vec<(MaterialId, FractureProfile)> {
    (0..4)
        .map(|i| {
            let mut p = FractureProfile::REFERENCE_SOLID;
            p.toughness_milli = 100 + i64::from(i) * 200;
            p.crush = DamageAmount(2000 + i * 2000);
            p.tensile = DamageAmount(80 + i * 80);
            p.compressive = DamageAmount(350 + i * 300);
            p.shear = DamageAmount(180 + i * 120);
            (MaterialId(101 + i), p)
        })
        .collect()
}
fn owned(model: FractureModel, entries: &[(MaterialId, FractureProfile)]) -> OwnedFracturePolicy {
    OwnedFracturePolicy::capture(model, entries, PolicySnapshotLimits::default()).unwrap()
}
/// Independent linear lookup for the direct-transaction comparison.
struct DirectPolicy<'a>(&'a [(MaterialId, FractureProfile)]);
impl FracturePolicy for DirectPolicy<'_> {
    fn profile(&self, material: MaterialId) -> FractureProfile {
        self.0
            .iter()
            .find(|(m, _)| *m == material)
            .map_or_else(|| FractureModel::Coherent.profile(material), |(_, p)| *p)
    }
    fn surface_gain_milli(&self, space: DamageSpace) -> i64 {
        FractureModel::Coherent.surface_gain_milli(space)
    }
    fn erase_unbonded(&self) -> bool {
        false
    }
}
struct Case {
    w: World,
    s: FragmentStore,
    f: FractureState,
    q: DestructionSequence,
}
impl Case {
    fn mixed() -> Self {
        let mut w = World::new();
        for x in 4..12 {
            w.fill_box(
                CellPos::new(x, 4, 4),
                CellPos::new(x, 7, 5),
                Some(MaterialId(101 + ((x - 4) / 2) as u32)),
            );
        }
        w.set_anchor_box(CellPos::new(4, 4, 4), CellPos::new(11, 4, 5), true);
        w.take_dirty();
        Self {
            w,
            s: FragmentStore::default(),
            f: FractureState::new(),
            q: DestructionSequence::default(),
        }
    }
    fn snapshot(&self, space: DamageSpace, limits: FractureJobLimits) -> FractureJobInput {
        FractureJobInput::capture(&self.w, &self.s, &self.f, self.q, space, known(), limits)
            .unwrap()
    }
    fn signature(&self) -> (World, Vec<Fragment>, FractureState, DestructionSequence) {
        (
            self.w.clone(),
            self.s.iter().map(|(_, f)| f.clone()).collect(),
            self.f.clone(),
            self.q,
        )
    }
    fn revisions(&self) -> (Revision, Revision) {
        (self.w.revision(), self.f.revision())
    }
    fn commit(
        &mut self,
        result: PolicyFractureJobResult,
        current: &OwnedFracturePolicy,
    ) -> Result<engine_destruction::fracture_jobs::JobSummary, JobRefusal> {
        result.commit(
            current,
            &mut self.w,
            &mut self.s,
            &mut self.f,
            &mut self.q,
            |_| true,
            |_| true,
        )
    }
}
fn static_hit() -> FractureImpact {
    FractureImpact::radial_blast(
        DamageSpace::StaticWorld,
        GlobalPos::new(7.5, 6.5, 5.5),
        4,
        DamageAmount(1200),
    )
    .unwrap()
}

#[test]
fn snapshot_is_canonical_immutable_owned_and_send_sync() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<OwnedFracturePolicy>();
    let mut entries = profiles();
    let a = owned(FractureModel::Coherent, &entries);
    entries.reverse();
    let b = owned(FractureModel::Coherent, &entries);
    assert_eq!(a, b);
    entries[0].1.crush = DamageAmount(1);
    assert_ne!(a.profile(entries[0].0), entries[0].1);
    drop(entries);
    assert_eq!(
        std::thread::spawn(move || b.profile_count())
            .join()
            .unwrap(),
        4
    );
    assert_eq!(a.profile_count(), 4);
}

#[test]
fn profile_count_and_payload_caps_are_separate_and_exact_at_boundary() {
    let entries = profiles();
    let p = owned(FractureModel::Coherent, &entries);
    let bytes = p.payload_bytes();
    println!(
        "policy payload: profiles={} bytes={} homogeneous_bytes={} (excludes Arc counters and allocator overhead)",
        p.profile_count(),
        bytes,
        OwnedFracturePolicy::homogeneous(FractureModel::Coherent).payload_bytes()
    );
    let limits = PolicySnapshotLimits {
        max_profiles: 4,
        max_bytes: bytes,
    };
    assert_eq!(
        OwnedFracturePolicy::capture(FractureModel::Coherent, &entries, limits),
        Ok(p)
    );
    assert_eq!(
        OwnedFracturePolicy::capture(
            FractureModel::Coherent,
            &entries,
            PolicySnapshotLimits {
                max_profiles: 3,
                max_bytes: usize::MAX
            }
        ),
        Err(PolicySnapshotRefusal::ProfileBudget)
    );
    assert_eq!(
        OwnedFracturePolicy::capture(
            FractureModel::Coherent,
            &entries,
            PolicySnapshotLimits {
                max_profiles: 4,
                max_bytes: bytes - 1
            }
        ),
        Err(PolicySnapshotRefusal::ByteBudget)
    );
    assert_eq!(
        OwnedFracturePolicy::capture(
            FractureModel::Historical,
            &[],
            PolicySnapshotLimits {
                max_profiles: 0,
                max_bytes: 0
            }
        ),
        Err(PolicySnapshotRefusal::ByteBudget)
    );
}

#[test]
fn duplicate_ids_refuse_and_unprofiled_materials_keep_model_fallback() {
    let entries = profiles();
    let repeated = [entries[0], entries[0]];
    assert_eq!(
        OwnedFracturePolicy::capture(
            FractureModel::Coherent,
            &repeated,
            PolicySnapshotLimits::default()
        ),
        Err(PolicySnapshotRefusal::DuplicateMaterial(entries[0].0))
    );
    for model in [FractureModel::Historical, FractureModel::Coherent] {
        let policy = owned(model, &entries);
        let missing = MaterialId(60000);
        assert_eq!(policy.profile(missing), model.profile(missing));
        assert_eq!(
            policy.surface_gain_milli(DamageSpace::FragmentLocal(FragmentId::new(1, 0))),
            model.surface_gain_milli(DamageSpace::FragmentLocal(FragmentId::new(1, 0)))
        );
        assert_eq!(policy.erase_unbonded(), model.erase_unbonded());
        assert_eq!(
            policy.bond_profile(entries[0].0, entries[1].0),
            policy.bond_profile(entries[1].0, entries[0].0)
        );
    }
}

#[test]
fn threaded_mixed_static_job_matches_direct_policy_transaction() {
    let entries = profiles();
    let policy = owned(FractureModel::Coherent, &entries);
    let mut direct = Case::mixed();
    let mut worker = Case::mixed();
    let before = worker.signature();
    let impact = static_hit();
    let input = worker.snapshot(impact.space(), FractureJobLimits::default());
    let job_policy = policy.clone();
    let result = std::thread::spawn(move || input.run_with_policy(impact, job_policy))
        .join()
        .unwrap()
        .unwrap();
    assert_eq!(
        worker.signature(),
        before,
        "worker analysis must not mutate authority"
    );
    let d = fracture_static_if(
        &mut direct.w,
        &mut direct.s,
        &mut direct.f,
        &mut direct.q,
        &impact,
        &DirectPolicy(&entries),
        &AllResident,
        Default::default(),
        |_| true,
    )
    .unwrap();
    let summary = worker.commit(result, &policy).unwrap();
    assert_eq!(summary.failed, d.fracture.failed.len() as u64);
    assert_eq!(summary.broken, d.fracture.broken.len() as u64);
    assert_eq!(summary.ids, d.fragments);
    assert_eq!(worker.signature(), direct.signature());
    assert!(summary.work > 0);
    let mut baseline = Case::mixed();
    let legacy = baseline
        .snapshot(impact.space(), Default::default())
        .run(impact, FractureModel::Coherent)
        .unwrap();
    legacy
        .commit(
            &mut baseline.w,
            &mut baseline.s,
            &mut baseline.f,
            &mut baseline.q,
            |_| true,
            |_| true,
        )
        .unwrap();
    assert_ne!(
        worker.signature(),
        baseline.signature(),
        "material overrides must not be silently ignored"
    );
}

#[test]
fn empty_snapshots_preserve_both_legacy_worker_models_exactly() {
    for model in [FractureModel::Historical, FractureModel::Coherent] {
        let mut legacy = Case::mixed();
        let mut current = Case::mixed();
        let impact = static_hit();
        let policy = OwnedFracturePolicy::homogeneous(model);
        let a = legacy
            .snapshot(impact.space(), Default::default())
            .run(impact, model)
            .unwrap();
        let b = current
            .snapshot(impact.space(), Default::default())
            .run_with_policy(impact, policy.clone())
            .unwrap();
        let sa = a
            .commit(
                &mut legacy.w,
                &mut legacy.s,
                &mut legacy.f,
                &mut legacy.q,
                |_| true,
                |_| true,
            )
            .unwrap();
        let sb = current.commit(b, &policy).unwrap();
        assert_eq!(
            (sa.failed, sa.broken, sa.created, sa.work, sa.ids),
            (sb.failed, sb.broken, sb.created, sb.work, sb.ids)
        );
        assert_eq!(legacy.signature(), current.signature());
    }
}

#[test]
fn changed_tuning_or_model_rejects_before_admission_or_mutation() {
    for change_model in [false, true] {
        let mut case = Case::mixed();
        let entries = profiles();
        let policy = owned(FractureModel::Coherent, &entries);
        let result = case
            .snapshot(DamageSpace::StaticWorld, Default::default())
            .run_with_policy(static_hit(), policy)
            .unwrap();
        let mut modified = entries;
        modified[0].1.crush = DamageAmount(1);
        let current = if change_model {
            owned(FractureModel::Historical, &profiles())
        } else {
            owned(FractureModel::Coherent, &modified)
        };
        let before = (case.signature(), case.revisions());
        let result = result.commit(
            &current,
            &mut case.w,
            &mut case.s,
            &mut case.f,
            &mut case.q,
            |_| true,
            |_| panic!("obsolete policy must not reach admission"),
        );
        assert!(matches!(result, Err(JobRefusal::Stale)));
        assert_eq!((case.signature(), case.revisions()), before);
    }
}

#[test]
fn equal_policy_contents_from_a_new_allocation_are_not_stale() {
    let mut case = Case::mixed();
    let policy = owned(FractureModel::Coherent, &profiles());
    let result = case
        .snapshot(DamageSpace::StaticWorld, Default::default())
        .run_with_policy(static_hit(), policy)
        .unwrap();
    let mut reordered = profiles();
    reordered.reverse();
    assert!(
        case.commit(result, &owned(FractureModel::Coherent, &reordered))
            .is_ok()
    );
}

#[test]
fn geometry_repaint_identity_residency_and_admission_guards_remain_atomic() {
    for mode in 0..4 {
        let mut case = Case::mixed();
        let policy = owned(FractureModel::Coherent, &profiles());
        let result = case
            .snapshot(DamageSpace::StaticWorld, Default::default())
            .run_with_policy(static_hit(), policy.clone())
            .unwrap();
        if mode == 0 {
            let mut edit = WorldEditBatch::new();
            edit.set(CellPos::new(4, 4, 4), Some(MaterialId(999)));
            case.f.apply_static_edit(&mut case.w, &edit);
        }
        if mode == 1 {
            case.q = DestructionSequence::new(9999);
        }
        let before = (case.signature(), case.revisions());
        assert!(
            result
                .commit(
                    &policy,
                    &mut case.w,
                    &mut case.s,
                    &mut case.f,
                    &mut case.q,
                    |_| mode != 2,
                    |_| mode != 3
                )
                .is_err()
        );
        assert_eq!((case.signature(), case.revisions()), before);
    }
}

#[test]
fn work_bytes_and_unknown_region_refusals_do_not_become_destruction() {
    let case = Case::mixed();
    let before = (case.signature(), case.revisions());
    let policy = owned(FractureModel::Coherent, &profiles());
    assert!(matches!(
        FractureJobInput::capture(
            &case.w,
            &case.s,
            &case.f,
            case.q,
            DamageSpace::StaticWorld,
            known(),
            FractureJobLimits {
                bytes: 0,
                ..Default::default()
            }
        ),
        Err(JobRefusal::SnapshotBudget)
    ));
    let mut limits = FractureJobLimits::default();
    limits.transaction.fracture.max_cells_visited = 0;
    assert!(matches!(
        case.snapshot(DamageSpace::StaticWorld, limits)
            .run_with_policy(static_hit(), policy.clone()),
        Err(JobRefusal::Analysis(_))
    ));
    let input = FractureJobInput::capture(
        &case.w,
        &case.s,
        &case.f,
        case.q,
        DamageSpace::StaticWorld,
        BTreeSet::new(),
        Default::default(),
    )
    .unwrap();
    assert!(matches!(
        input.run_with_policy(static_hit(), policy),
        Err(JobRefusal::Analysis(FractureTransactionRefusal::Unknown(_)))
    ));
    assert_eq!((case.signature(), case.revisions()), before);
}

fn mixed_fragment() -> (Case, FragmentId, FractureImpact) {
    let src = Case::mixed();
    let cells = (4..12)
        .flat_map(|x| (4..8).flat_map(move |y| (4..6).map(move |z| CellPos::new(x, y, z))))
        .collect();
    let id = FragmentId::new(800, 0);
    let fragment = Fragment::from_cells(id, &src.w, &cells).unwrap();
    let centre = fragment.local_centre();
    let impact = ContactFracturePolicy::default()
        .impact(
            DamageSpace::FragmentLocal(id),
            GlobalPos::new(centre[0], centre[1], centre[2]),
            [0., 0., -1.],
            DamageAmount(2500),
        )
        .unwrap();
    let mut case = Case {
        w: World::new(),
        s: FragmentStore::default(),
        f: FractureState::new(),
        q: DestructionSequence::new(801),
    };
    case.s.insert(fragment);
    (case, id, impact)
}

#[test]
fn mixed_native_fragment_worker_matches_direct_refracture_and_keeps_material_ids() {
    let (mut direct, parent_id, impact) = mixed_fragment();
    let parent_motion = FragmentPhysicsState::from_fragment(direct.s.get(parent_id).unwrap());
    let (mut worker, _, _) = mixed_fragment();
    let entries = profiles();
    let policy = owned(FractureModel::Coherent, &entries);
    let input = worker.snapshot(impact.space(), Default::default());
    let copied = policy.clone();
    let result = std::thread::spawn(move || input.run_with_policy(impact, copied))
        .join()
        .unwrap()
        .unwrap();
    fracture_fragment_if(
        &mut direct.s,
        &mut direct.f,
        &mut direct.q,
        &impact,
        &DirectPolicy(&entries),
        Default::default(),
        |_| true,
    )
    .unwrap();
    let summary = worker.commit(result, &policy).unwrap();
    assert_fragment_commit_equivalence(&direct, &worker, parent_motion);
    let remaining: u64 = worker.s.iter().map(|(_, f)| f.cell_count()).sum();
    assert_eq!(remaining + summary.failed, 64);
    assert!(remaining > 0);
    for (_, fragment) in worker.s.iter() {
        for cell in fragment.occupied_cells() {
            use engine_core::CellSource;
            assert!((101..=104).contains(&fragment.material_at(cell).unwrap().0));
        }
    }
}

#[test]
fn material_job_retains_current_motion_and_cannot_touch_another_fragment() {
    let (mut case, id, impact) = mixed_fragment();
    let mut unrelated = case.s.get(id).unwrap().clone();
    unrelated.id = FragmentId::new(900, 0);
    case.s.insert(unrelated.clone());
    let policy = owned(FractureModel::Coherent, &profiles());
    let input = case.snapshot(impact.space(), Default::default());
    let p = policy.clone();
    let result = std::thread::spawn(move || input.run_with_policy(impact, p))
        .join()
        .unwrap()
        .unwrap();
    let parent = case.s.get_mut(id).unwrap();
    parent.pose.translation = GlobalPos::new(100., 200., 300.);
    parent.linear_velocity = [2., 3., 4.];
    parent.angular_velocity = [0.1, 0.2, 0.3];
    let summary = case.commit(result, &policy).unwrap();
    assert_eq!(case.s.get(unrelated.id), Some(&unrelated));
    assert!(!summary.ids.is_empty());
    for child in summary.ids {
        let f = case.s.get(child).unwrap();
        assert_eq!(f.pose.translation, GlobalPos::new(100., 200., 300.));
        assert_eq!(f.linear_velocity, [2., 3., 4.]);
        assert_eq!(f.angular_velocity, [0.1, 0.2, 0.3]);
    }
}

#[test]
fn independent_repeated_worker_runs_produce_identical_material_results() {
    let mut expected = None;
    for _ in 0..8 {
        let mut case = Case::mixed();
        let policy = owned(FractureModel::Coherent, &profiles());
        let input = case.snapshot(DamageSpace::StaticWorld, Default::default());
        let p = policy.clone();
        let result = std::thread::spawn(move || input.run_with_policy(static_hit(), p))
            .join()
            .unwrap()
            .unwrap();
        case.commit(result, &policy).unwrap();
        match &expected {
            Some(e) => assert_eq!(&case.signature(), e),
            None => expected = Some(case.signature()),
        }
    }
}

// A direct geometry transaction and a host-admitted job have different operation
// counts: the latter also samples current physics onto every returned child.
// Apply that documented bridge to an EXPECTED clone, then compare the entire
// Fragment (including both revisions), rather than dropping fields from equality.
fn assert_fragment_commit_equivalence(
    direct: &Case,
    worker: &Case,
    parent_motion: FragmentPhysicsState,
) {
    assert_eq!(worker.w, direct.w);
    assert_eq!(worker.q, direct.q);
    assert_eq!(worker.f, direct.f); // FractureState equality is already content-only.
    assert_eq!(
        worker.f.cells().collect::<Vec<_>>(),
        direct.f.cells().collect::<Vec<_>>()
    );
    assert_eq!(
        worker.f.bonds().collect::<Vec<_>>(),
        direct.f.bonds().collect::<Vec<_>>()
    );
    assert_eq!(
        worker.s.ids().collect::<Vec<_>>(),
        direct.s.ids().collect::<Vec<_>>()
    );
    println!(
        "fracture commit counters: direct={:?} worker={:?}; exact cell/bond records equal",
        direct.f.revision(),
        worker.f.revision()
    );
    for (id, original) in direct.s.iter() {
        let actual = worker.s.get(id).unwrap();
        let mut expected = original.clone();
        parent_motion.apply_to(&mut expected);
        println!(
            "fragment {id:?}: direct_rev={:?} worker_rev={:?} geometry_equal={} full_equal_after_motion_bridge={}",
            original.revision,
            actual.revision,
            original.geometry_revision() == actual.geometry_revision(),
            expected == *actual
        );
        // Avoid dumping thousands of voxel-array entries on a mismatch.
        assert!(
            expected == *actual,
            "full fragment mismatch after expected motion bridge: {id:?}"
        );
    }
}

#[test]
fn both_legacy_fragment_models_have_the_same_motion_bridge_as_owned_policies() {
    for model in [FractureModel::Historical, FractureModel::Coherent] {
        let (mut direct, id, impact) = mixed_fragment();
        let motion = FragmentPhysicsState::from_fragment(direct.s.get(id).unwrap());
        let (mut legacy, _, _) = mixed_fragment();
        let (mut owned_case, _, _) = mixed_fragment();
        fracture_fragment_if(
            &mut direct.s,
            &mut direct.f,
            &mut direct.q,
            &impact,
            &model,
            Default::default(),
            |_| true,
        )
        .unwrap();
        legacy
            .snapshot(impact.space(), Default::default())
            .run(impact, model)
            .unwrap()
            .commit(
                &mut legacy.w,
                &mut legacy.s,
                &mut legacy.f,
                &mut legacy.q,
                |_| true,
                |_| true,
            )
            .unwrap();
        let policy = OwnedFracturePolicy::homogeneous(model);
        let result = owned_case
            .snapshot(impact.space(), Default::default())
            .run_with_policy(impact, policy.clone())
            .unwrap();
        owned_case.commit(result, &policy).unwrap();
        // Both worker paths must be fully identical, not just topologically equal.
        assert!(owned_case.signature() == legacy.signature());
        assert_eq!(owned_case.revisions(), legacy.revisions());
        assert_fragment_commit_equivalence(&direct, &legacy, motion);
    }
}
