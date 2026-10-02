//! Frozen dc30b64 full-copy commit, used only as an independent test oracle.
//! Do not share the sparse transaction implementation with this reference.
use super::*;

#[test]
fn sparse_commit_matches_full_copy_across_repeated_shared_impacts_and_refusals() {
    use crate::{
        AllResident, DamageEventId, reference_fracture_hit_at, reference_fracture_world_at,
        static_failure_batch,
    };
    let policy = BaselineFracturePolicy::REFERENCE;
    let mut world = reference_fracture_world_at(CellPos::ZERO);
    let mut sparse = FractureState::new();
    let mut reference = sparse.clone();
    for hit in 0..9 {
        let event = reference_fracture_hit_at(
            DamageEventId::new(hit, 0),
            CellPos::ZERO,
            DamageAmount(1800),
        );
        let impact = FractureImpact::from_static_event(&event).unwrap();
        let scene = FractureScene::static_world(&world, &world, &AllResident);
        let load = evaluate_fracture(&impact, scene, &policy, &sparse, FractureLimits::UNLIMITED)
            .unwrap()
            .load()
            .unwrap()
            .clone();
        for limits in [
            FractureLimits::new(u64::MAX, u64::MAX, 0, usize::MAX),
            FractureLimits::new(u64::MAX, u64::MAX, usize::MAX, 0),
        ] {
            let mut a = sparse.clone();
            let mut b = reference.clone();
            let before = a.clone();
            let result = a.apply(&load, scene, &policy, limits);
            assert_eq!(
                result,
                b.apply_full_copy_reference(&load, scene, &policy, limits)
            );
            assert_eq!(a, b);
            assert_eq!(a.revision(), b.revision());
            if result.is_err() {
                assert_eq!(a, before);
                assert_eq!(a.revision(), before.revision());
            }
        }
        let outcome = sparse
            .apply(&load, scene, &policy, FractureLimits::UNLIMITED)
            .unwrap();
        assert_eq!(
            Ok(outcome.clone()),
            reference.apply_full_copy_reference(&load, scene, &policy, FractureLimits::UNLIMITED)
        );
        assert_eq!(sparse, reference);
        assert_eq!(sparse.revision(), reference.revision());
        world.apply(&static_failure_batch(&outcome.failed));
    }
}

#[test]
fn sparse_commit_matches_full_copy_for_duplicate_loads_replacements_and_tombstones() {
    use crate::{AllResident, REFERENCE_FRACTURE_MATERIAL, reference_fracture_world};
    let world = reference_fracture_world();
    let scene = FractureScene::static_world(&world, &world, &AllResident);
    let policy = BaselineFracturePolicy::REFERENCE;
    let mut sparse = FractureState::new();
    let mut reference = sparse.clone();
    let mut seed = 17u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for step in 0..256 {
        let mut load = FractureLoad {
            state: sparse.revision(),
            space: DamageSpace::StaticWorld,
            cells: Vec::new(),
            bonds: Vec::new(),
            measurement: FractureMeasurement::default(),
        };
        for _ in 0..16 {
            let r = next();
            let cell = CellPos::new(62 + (r % 3) as i32, 9 + ((r >> 8) % 3) as i32, 64);
            let material = if r & 32 == 0 {
                MaterialId(9001)
            } else {
                REFERENCE_FRACTURE_MATERIAL
            };
            load.cells.push(CellDeposit {
                cell,
                material,
                energy: DamageAmount((r % 6000) as u32),
                distance_milli: 0,
            });
            load.bonds.push(BondLoad {
                bond: BondKey::new(cell, FaceDir::ALL[((r >> 12) % 6) as usize]).unwrap(),
                material,
                mode: BondMode::Tension,
                load: DamageAmount(100),
                loss: DamageAmount((r % 10001) as u32),
            });
        }
        if step % 19 == 0 {
            load.state = Revision(u64::MAX);
        }
        if step % 23 == 0 {
            load.space = DamageSpace::FragmentLocal(FragmentId::new(2, 1));
        }
        let limit = if step % 5 == 0 { 5 } else { usize::MAX };
        let limits = FractureLimits::new(u64::MAX, u64::MAX, limit, limit);
        assert_eq!(
            sparse.apply(&load, scene, &policy, limits),
            reference.apply_full_copy_reference(&load, scene, &policy, limits),
            "step {step}"
        );
        assert_eq!(sparse, reference, "step {step}");
        assert_eq!(sparse.revision(), reference.revision());
    }
}

#[test]
fn staged_storage_depends_on_touched_keys_not_accumulated_history() {
    let base: BTreeMap<u32, u32> = (0..100_000).map(|key| (key, key)).collect();
    let mut delta = MapDelta::new(&base);
    assert!(delta.edits.is_empty());
    delta.remove(&3);
    assert!(delta.get(&3).is_none());
    delta.remove(&3);
    delta.insert(4, 999);
    delta.insert(100_001, 1);
    delta.insert(100_001, 2);
    assert_eq!(delta.len(), 100_000);
    assert_eq!(delta.edits.len() + delta.removed.len(), 3);
    assert_eq!(delta.get(&4), Some(&999));
    assert_eq!(delta.get(&99_999), Some(&99_999));
    assert_eq!(base.get(&3), Some(&3), "staging never writes the base");
}

impl FractureState {
    pub(super) fn apply_full_copy_reference(
        &mut self,
        load: &FractureLoad,
        scene: FractureScene<'_>,
        policy: &dyn FracturePolicy,
        limits: FractureLimits,
    ) -> Result<FractureOutcome, FractureRefusal> {
        if load.space != scene.space() {
            return Err(FractureRefusal::SpaceMismatch(FractureSpaceMismatch {
                impact: load.space,
                scene: scene.space(),
            }));
        }
        if load.state != self.revision {
            return Err(FractureRefusal::StaleEvaluation {
                expected: load.state,
                found: self.revision,
            });
        }

        let space = load.space;
        let mut staged_cells = self.cells.clone();
        let mut staged_bonds = self.bonds.clone();
        let mut crushed: BTreeSet<CellPos> = BTreeSet::new();
        let mut broken: Vec<BondSite> = Vec::new();

        for deposit in &load.cells {
            if deposit.energy == DamageAmount::ZERO {
                continue;
            }
            let site = DamageSite {
                space,
                cell: deposit.cell,
            };
            // A material replacement resets partial damage rather than
            // inheriting it, exactly as the progressive accumulator does.
            let previous = staged_cells
                .get(&site)
                .filter(|record| record.material == deposit.material)
                .map_or(DamageAmount::ZERO, |record| record.energy);
            let total = previous.saturating_add(deposit.energy);
            let threshold = policy.profile(deposit.material).crush;
            if threshold != DamageAmount::ZERO && total >= threshold {
                staged_cells.remove(&site);
                crushed.insert(deposit.cell);
            } else {
                staged_cells.insert(
                    site,
                    CellFracture {
                        site,
                        material: deposit.material,
                        energy: total,
                    },
                );
            }
        }

        for bond_load in &load.bonds {
            if bond_load.loss == DamageAmount::ZERO {
                continue;
            }
            let site = BondSite {
                space,
                bond: bond_load.bond,
            };
            let existing = staged_bonds
                .get(&site)
                .filter(|record| record.material == bond_load.material)
                .copied();
            let before = existing.map_or(BOND_INTEGRITY, |record| record.integrity);
            let after = DamageAmount(before.0.saturating_sub(bond_load.loss.0));
            staged_bonds.insert(
                site,
                BondFracture {
                    site,
                    material: bond_load.material,
                    integrity: after,
                },
            );
            if after == DamageAmount::ZERO && before != DamageAmount::ZERO {
                broken.push(site);
            }
        }

        // A cell held by nothing is rubble. Only cells touching a bond that
        // broke in *this* transaction can newly become that, so the check is
        // bounded by the breaks rather than by the world.
        let mut unbonded: BTreeSet<CellPos> = BTreeSet::new();
        let mut candidates: BTreeSet<CellPos> = BTreeSet::new();
        for site in &broken {
            candidates.extend(site.bond.cells());
        }
        for cell in candidates {
            if crushed.contains(&cell) {
                continue;
            }
            if scene.support.is_anchor(cell) {
                // An anchor is the author's statement that this is held up.
                // Mechanics may break what rests on a foundation; it may not
                // erase the foundation.
                continue;
            }
            if scene.cells.material_at(cell).is_none() {
                continue;
            }
            let mut occupied = 0usize;
            let mut intact = 0usize;
            let mut unknown = false;
            for dir in FaceDir::ALL {
                let Some(neighbour) = cell.checked_step(dir) else {
                    continue;
                };
                if !scene.residency.is_resident(neighbour) {
                    unknown = true;
                    break;
                }
                if scene.cells.material_at(neighbour).is_none() {
                    continue;
                }
                occupied += 1;
                let Some(bond) = BondKey::new(cell, dir) else {
                    continue;
                };
                let broken_here = staged_bonds
                    .get(&BondSite { space, bond })
                    .is_some_and(BondFracture::is_broken);
                if !broken_here {
                    intact += 1;
                }
            }
            // Unknown space is not empty: a neighbour the engine cannot see may
            // still be holding this cell.
            if unknown || occupied == 0 || intact > 0 {
                continue;
            }
            unbonded.insert(cell);
        }

        // Records for a cell that is about to leave the world are meaningless,
        // and leaving them would let stale integrity decide a later impact.
        let mut failed_cells: BTreeSet<CellPos> = crushed.clone();
        failed_cells.extend(unbonded.iter().copied());
        for cell in &failed_cells {
            staged_cells.remove(&DamageSite { space, cell: *cell });
            for dir in FaceDir::ALL {
                if let Some(bond) = BondKey::new(*cell, dir) {
                    staged_bonds.remove(&BondSite { space, bond });
                }
            }
        }
        broken.retain(|site| {
            !site
                .bond
                .cells()
                .into_iter()
                .any(|cell| failed_cells.contains(&cell))
        });

        if staged_cells.len() > limits.max_cell_entries {
            return Err(FractureRefusal::CellEntryLimit {
                required: staged_cells.len(),
                limit: limits.max_cell_entries,
            });
        }
        if staged_bonds.len() > limits.max_bond_entries {
            return Err(FractureRefusal::BondEntryLimit {
                required: staged_bonds.len(),
                limit: limits.max_bond_entries,
            });
        }

        let failed: Vec<DamageTarget> = failed_cells
            .iter()
            .filter_map(|cell| {
                scene
                    .cells
                    .material_at(*cell)
                    .map(|material| scene.target(*cell, material))
            })
            .collect();
        // Counted from `failed` rather than from the crush set, so the two
        // numbers can never disagree about a cell the world has already lost by
        // another path.
        let crushed = failed
            .iter()
            .filter(|target| crushed.contains(&target.cell()))
            .count();

        self.cells = staged_cells;
        self.bonds = staged_bonds;
        self.revision.bump();

        Ok(FractureOutcome {
            broken,
            failed,
            crushed,
            cell_entries: self.cells.len(),
            bond_entries: self.bonds.len(),
        })
    }
}
