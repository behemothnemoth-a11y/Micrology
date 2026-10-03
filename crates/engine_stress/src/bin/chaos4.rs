//! Adversarial chaos pass for DROP 0004.9.
//!
//! This is the DROP 0004 counterpart to `chaos.rs` (DROP 0003.15). It is not a
//! committed performance baseline: it tries to make destruction-at-scale and
//! applied damage do the wrong thing, and records what survived.
//!
//! Chaos Pass rule: every destructive case runs against isolated disposable
//! state. The live target is digested before the run and re-verified afterwards.
//! The run is not complete until that integrity check passes.

use engine_core::{CellPos, CellSource, GlobalPos, MaterialId, MaterialRegistry};
use engine_destruction::{
    DamageAmount, DamageEvaluationLimits, DamageEvent, DamageEventId, DamageFalloff, DamageImpulse,
    DamageSequence, DamageSource, DamageSpace, DamageTarget, DamageVolume, DamageWork,
    DestructionSequence, DetachRefusal, Fragment, FragmentCollisionJobInput, FragmentDamageResult,
    FragmentId, FragmentMeshJobInput, FragmentResidency, FragmentResidencyAction,
    FragmentResidencyConfig, FragmentSaveOutcome, FragmentSpatialIndex, FragmentStore,
    ProgressiveDamageLimits, ProgressiveDamageStore, ResultDisposition, SecondaryDamage,
    SecondaryDamageLimits, SecondaryDamageQueue, SecondaryDamageRefusal, SnapshotLimits,
    StructuralLimits, StructureJobInput, UniformDamagePolicy, UniformFailurePolicy,
    damage_fragment_store_if, detach_if, evaluate_damage_event,
};
use engine_world::{World, WorldEditBatch};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const STONE: MaterialId = MaterialId(1);

#[derive(Serialize)]
struct Case {
    name: &'static str,
    ok: bool,
    detail: String,
}

#[derive(Serialize)]
struct ChaosReport {
    generated_by: &'static str,
    drop_section: &'static str,
    cases: Vec<Case>,
    integrity_verified: bool,
    all_ok: bool,
}

fn push(cases: &mut Vec<Case>, name: &'static str, ok: bool, detail: impl Into<String>) {
    cases.push(Case {
        name,
        ok,
        detail: detail.into(),
    });
}

// ---------------------------------------------------------------------------
// Disposable isolation helpers
// ---------------------------------------------------------------------------

fn sandbox_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("micrology-chaos4-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create chaos sandbox");
    dir
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn digest_into(root: &Path, dir: &Path, out: &mut Vec<(String, u64)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            digest_into(root, &path, out);
        } else if let Ok(bytes) = std::fs::read(&path) {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, fnv1a(&bytes)));
        }
    }
}

/// Content digest of every file under `dir`, in stable path order.
fn digest_tree(dir: &Path) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    digest_into(dir, dir, &mut out);
    out.sort();
    out
}

fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("create copy target");
    let Ok(entries) = std::fs::read_dir(src) else {
        return;
    };
    for entry in entries.flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("copy chaos fixture file");
        }
    }
}

// ---------------------------------------------------------------------------
// Geometry helpers
// ---------------------------------------------------------------------------

fn box_cells(min: CellPos, max: CellPos) -> BTreeSet<CellPos> {
    let mut cells = BTreeSet::new();
    for y in min.y..=max.y {
        for z in min.z..=max.z {
            for x in min.x..=max.x {
                cells.insert(CellPos::new(x, y, z));
            }
        }
    }
    cells
}

fn box_fragment(id: FragmentId, min: CellPos, max: CellPos) -> Fragment {
    let mut world = World::new();
    world.fill_box(min, max, Some(STONE));
    world.take_dirty();
    Fragment::from_cells(id, &world, &box_cells(min, max)).expect("non-empty fragment")
}

/// A straight run of `len` cells along +X starting at `start`.
fn line_fragment(id: FragmentId, start: CellPos, len: i32) -> Fragment {
    box_fragment(id, start, CellPos::new(start.x + len - 1, start.y, start.z))
}

/// A column standing on an anchored floor, well away from any volume face, so
/// the exact snapshot has complete knowledge.
fn interior_tower() -> World {
    let mut world = World::new();
    world.fill_box(CellPos::new(4, 4, 4), CellPos::new(11, 4, 11), Some(STONE));
    world.set_anchor_box(CellPos::new(4, 4, 4), CellPos::new(11, 4, 11), true);
    world.fill_box(CellPos::new(7, 5, 7), CellPos::new(8, 9, 8), Some(STONE));
    world.fill_box(
        CellPos::new(5, 10, 5),
        CellPos::new(10, 12, 10),
        Some(STONE),
    );
    world.take_dirty();
    world
}

fn cut_tower_neck(world: &mut World) -> engine_destruction::StructureJobResult {
    let mut cut = WorldEditBatch::new();
    cut.fill_box(CellPos::new(7, 7, 7), CellPos::new(8, 7, 8), None);
    let outcome = world.apply(&cut);
    StructureJobInput::snapshot(
        world,
        outcome.structural_candidates.iter().copied(),
        SnapshotLimits::default(),
        StructuralLimits::UNLIMITED,
    )
    .run()
}

// ---------------------------------------------------------------------------
// 1. Fragment population much larger than active residency
// ---------------------------------------------------------------------------

fn population_beyond_residency(cases: &mut Vec<Case>) {
    const POPULATION: u64 = 512;
    let mut persisted = FragmentStore::default();
    for n in 0..POPULATION {
        // Packed tightly on purpose: a large population concentrated into very
        // few regions is the case that must not drag everything into residency.
        let x = (n as i32 % 16) * 6;
        let z = (n as i32 / 16) * 6;
        persisted.insert(box_fragment(
            FragmentId::new(n, 0),
            CellPos::new(x, 8, z),
            CellPos::new(x + 1, 9, z + 1),
        ));
    }
    let spatial = FragmentSpatialIndex::from_store(&persisted).expect("spatial index");

    // Nothing is resident: this is the "persisted but not loaded" state.
    let resident = FragmentStore::default();
    let mut residency = FragmentResidency::new(FragmentResidencyConfig::new(4, 4));

    // Pick the most crowded region, lowest position breaking ties, so the
    // active-load cap is genuinely exercised rather than trivially satisfied.
    let target_region = spatial
        .regions()
        .max_by(|a, b| a.1.len().cmp(&b.1.len()).then(b.0.cmp(&a.0)))
        .map(|(region, _)| region)
        .expect("at least one region");
    let in_region: BTreeSet<FragmentId> = spatial.fragments_in(target_region).collect();

    let actions = residency.update(&resident, &spatial, [target_region]);
    let loads: Vec<FragmentId> = actions
        .iter()
        .filter_map(|action| match action {
            FragmentResidencyAction::Load(ticket) => Some(ticket.id),
            _ => None,
        })
        .collect();

    let only_loads = actions.len() == loads.len();
    // The region holds far more than the cap, so the cap must bind exactly.
    let crowded = in_region.len() > 4;
    let bounded = loads.len() == 4;
    let all_wanted = loads.iter().all(|id| in_region.contains(id));
    let discovery_complete = spatial.fragment_count() == POPULATION as usize;

    push(
        cases,
        "population_beyond_residency",
        only_loads && crowded && bounded && all_wanted && discovery_complete,
        format!(
            "persisted={} discoverable, busiest region holds {} (>cap), emitted exactly {} load(s) at cap 4, \
             all wanted={all_wanted}, no spurious actions={only_loads}",
            spatial.fragment_count(),
            in_region.len(),
            loads.len()
        ),
    );
}

// ---------------------------------------------------------------------------
// 2. Travel away and back with moving and sleeping fragments
// ---------------------------------------------------------------------------

fn travel_away_and_back(cases: &mut Vec<Case>) {
    let moving_id = FragmentId::new(1, 0);
    let sleeping_id = FragmentId::new(2, 0);

    let mut store = FragmentStore::default();
    store.insert(box_fragment(
        moving_id,
        CellPos::new(4, 8, 4),
        CellPos::new(5, 9, 5),
    ));
    store.insert(box_fragment(
        sleeping_id,
        CellPos::new(400, 8, 400),
        CellPos::new(401, 9, 401),
    ));
    let spatial = FragmentSpatialIndex::from_store(&store).expect("spatial index");

    let mut residency = FragmentResidency::new(FragmentResidencyConfig::new(4, 4));
    for (_, fragment) in store.iter() {
        residency.note_loaded(fragment);
    }

    // The moving fragment is dirty; the sleeping one is untouched and clean.
    let mut moving = store.get(moving_id).cloned().expect("moving fragment");
    let mut pose = moving.pose;
    pose.translation = GlobalPos::new(64.0, 40.0, 64.0);
    moving.update_from_physics(pose, [3.0, -1.0, 0.0], [0.0; 3], false);
    store.insert(moving);

    let home: Vec<_> = spatial.regions_for(moving_id).collect();

    // Travel away: nothing is wanted at all.
    let away = residency.update(&store, &spatial, []);
    let saved_dirty = away.iter().any(
        |action| matches!(action, FragmentResidencyAction::Save(ticket) if ticket.id == moving_id),
    );
    let dropped_clean = away
        .iter()
        .any(|action| matches!(action, FragmentResidencyAction::Drop(id) if *id == sleeping_id));
    let clean_not_saved = !away.iter().any(|action| {
        matches!(action, FragmentResidencyAction::Save(ticket) if ticket.id == sleeping_id)
    });

    // Host completes the eviction of the clean fragment only.
    store.remove(sleeping_id);
    residency.note_dropped(sleeping_id);

    // Travel back: the evicted payload is still discoverable and reloads.
    let back = residency.update(&store, &spatial, home.iter().copied());
    let reload_possible = spatial.fragments_in(home[0]).any(|id| id == moving_id);
    let no_resurrection_of_unwanted = !back.iter().any(|action| {
        matches!(action, FragmentResidencyAction::Load(ticket) if ticket.id == sleeping_id)
    });

    let ok = saved_dirty
        && dropped_clean
        && clean_not_saved
        && reload_possible
        && no_resurrection_of_unwanted;
    push(
        cases,
        "travel_away_and_back",
        ok,
        format!(
            "dirty saved before eviction={saved_dirty}, clean dropped={dropped_clean}, \
             clean never saved={clean_not_saved}, still discoverable after eviction={reload_possible}, \
             unwanted not reloaded={no_resurrection_of_unwanted}"
        ),
    );
}

// ---------------------------------------------------------------------------
// 3. Dirty-fragment eviction racing continued motion
// ---------------------------------------------------------------------------

fn dirty_eviction_during_motion(cases: &mut Vec<Case>) {
    let id = FragmentId::new(7, 0);
    let mut store = FragmentStore::default();
    store.insert(box_fragment(
        id,
        CellPos::new(4, 8, 4),
        CellPos::new(5, 9, 5),
    ));
    let spatial = FragmentSpatialIndex::from_store(&store).expect("spatial index");

    let mut residency = FragmentResidency::new(FragmentResidencyConfig::new(2, 2));
    residency.note_loaded(store.get(id).expect("resident"));

    // Move it, then evict it: the save must be issued before any drop.
    let mut fragment = store.get(id).cloned().expect("resident");
    let mut pose = fragment.pose;
    pose.translation = GlobalPos::new(10.0, 20.0, 30.0);
    fragment.update_from_physics(pose, [1.0, 0.0, 0.0], [0.0; 3], false);
    store.insert(fragment.clone());

    let actions = residency.update(&store, &spatial, []);
    let ticket = actions.iter().find_map(|action| match action {
        FragmentResidencyAction::Save(ticket) if ticket.id == id => Some(*ticket),
        _ => None,
    });
    let no_drop_while_dirty = !actions
        .iter()
        .any(|action| matches!(action, FragmentResidencyAction::Drop(dropped) if *dropped == id));

    // The fragment keeps moving while that write is in flight.
    let mut moved_again = store.get(id).cloned().expect("resident");
    let mut pose = moved_again.pose;
    pose.translation = GlobalPos::new(11.0, 20.0, 30.0);
    moved_again.update_from_physics(pose, [2.0, 0.0, 0.0], [0.0; 3], false);
    store.insert(moved_again.clone());

    let outcome = ticket.map(|ticket| residency.on_save_finished(&store, ticket, true));
    let still_dirty = outcome == Some(FragmentSaveOutcome::StillDirty)
        && residency.is_dirty(store.get(id).expect("resident"));

    let ok = ticket.is_some() && no_drop_while_dirty && still_dirty;
    push(
        cases,
        "dirty_eviction_during_motion",
        ok,
        format!(
            "save issued={}, no drop while dirty={no_drop_while_dirty}, \
             stale write left it dirty={still_dirty}, outcome={outcome:?}",
            ticket.is_some()
        ),
    );
}

// ---------------------------------------------------------------------------
// 4. Crash windows during incremental fragment save / index update
// ---------------------------------------------------------------------------

fn incremental_persistence_crash_windows(cases: &mut Vec<Case>) {
    let dir = sandbox_dir("persistence");
    let keep = FragmentId::new(30, 0);
    let doomed = FragmentId::new(30, 1);

    let mut store = FragmentStore::default();
    store.insert(box_fragment(
        keep,
        CellPos::new(4, 4, 4),
        CellPos::new(5, 5, 5),
    ));
    store.insert(box_fragment(
        doomed,
        CellPos::new(9, 4, 9),
        CellPos::new(10, 5, 10),
    ));
    engine_io::save_fragment_store(&dir, &store, DestructionSequence::new(31))
        .expect("save fragment store");

    let mut index = engine_io::load_fragment_index(&dir).expect("load index");

    // Real destruction: index-first, payload-second.
    let deleted = engine_io::delete_persisted_fragment(&dir, &mut index, doomed)
        .expect("delete persisted fragment");
    let index_dropped = !index.contains(doomed) && index.contains(keep);
    let payload_gone = !engine_io::fragment_path(&dir, doomed).exists();

    // Crash window: the index commit landed, the payload removal did not.
    engine_io::save_fragment(
        &dir,
        &box_fragment(doomed, CellPos::new(9, 4, 9), CellPos::new(10, 5, 10)),
    )
    .expect("re-create orphan payload");
    let reloaded = engine_io::load_fragment_store(&dir);
    let orphan_not_resurrected = reloaded
        .as_ref()
        .is_ok_and(|(loaded, _, _)| loaded.contains(keep) && !loaded.contains(doomed));

    // Orphans are prunable once the index is durable.
    let pruned = engine_io::prune_fragment_orphans(&dir, &index).expect("prune orphans");
    let orphan_removed = pruned == 1 && !engine_io::fragment_path(&dir, doomed).exists();
    let survivor_intact = engine_io::load_fragment(&dir, keep).is_ok();

    let _ = std::fs::remove_dir_all(&dir);
    let ok = deleted
        && index_dropped
        && payload_gone
        && orphan_not_resurrected
        && orphan_removed
        && survivor_intact;
    push(
        cases,
        "incremental_persistence_crash_windows",
        ok,
        format!(
            "index-first delete={deleted}, index dropped={index_dropped}, payload removed={payload_gone}, \
             orphan not resurrected={orphan_not_resurrected}, pruned={pruned}, survivor intact={survivor_intact}"
        ),
    );
}

// ---------------------------------------------------------------------------
// 5. Structural demand at a region boundary
// ---------------------------------------------------------------------------

fn structural_demand_at_boundary(cases: &mut Vec<Case>) {
    // The floor sits on the volume's own -Y face, exactly the 0004.8 acceptance
    // finding. Exact growth must leave the region boundary rather than assume
    // the absent space below is empty.
    let mut world = World::new();
    world.fill_box(CellPos::new(4, 0, 4), CellPos::new(11, 0, 11), Some(STONE));
    world.set_anchor_box(CellPos::new(4, 0, 4), CellPos::new(11, 0, 11), true);
    world.fill_box(CellPos::new(7, 1, 7), CellPos::new(8, 6, 8), Some(STONE));
    world.take_dirty();

    let mut cut = WorldEditBatch::new();
    cut.fill_box(CellPos::new(7, 3, 7), CellPos::new(8, 3, 8), None);
    let outcome = world.apply(&cut);
    let result = StructureJobInput::snapshot(
        &world,
        outcome.structural_candidates.iter().copied(),
        SnapshotLimits::default(),
        StructuralLimits::UNLIMITED,
    )
    .run();

    let disposition = result.judge(&world);
    let required = result.required_regions();
    let before = world.occupied_count();
    let mut sequence = DestructionSequence::new(500);
    let refusal = detach_if(&mut world, &mut sequence, &result, |_| true);

    let inconclusive = disposition == ResultDisposition::Inconclusive;
    let refused = refusal == Err(DetachRefusal::Inconclusive);
    let unchanged = world.occupied_count() == before && sequence.peek() == 500;

    push(
        cases,
        "structural_demand_at_boundary",
        inconclusive && refused && unchanged,
        format!(
            "disposition={disposition:?}, required_regions={}, detach={refusal:?}, \
             world unchanged={unchanged} (unknown is not empty)",
            required.len()
        ),
    );
}

// ---------------------------------------------------------------------------
// 6. Many simultaneous damage events in one transaction
// ---------------------------------------------------------------------------

fn many_simultaneous_damage_events(cases: &mut Vec<Case>) {
    let mut world = World::new();
    world.fill_box(CellPos::new(4, 4, 4), CellPos::new(19, 4, 19), Some(STONE));
    world.take_dirty();

    let mut sequence = DamageSequence::new(1);
    let mut work: Vec<DamageWork> = Vec::new();
    let mut truncated_any = false;
    for x in 4..20 {
        for z in 4..20 {
            let event = DamageEvent::new(
                sequence.next_root(),
                DamageSpace::StaticWorld,
                None,
                DamageVolume::Cell(CellPos::new(x, 4, z)),
                DamageAmount(20),
                None,
            )
            .expect("valid event");
            let evaluation = evaluate_damage_event(
                &event,
                DamageSource::Static(&world),
                &UniformDamagePolicy,
                DamageEvaluationLimits::UNLIMITED,
            )
            .expect("evaluate");
            truncated_any |= evaluation.truncated;
            work.extend(evaluation.work);
        }
    }

    let mut damage = ProgressiveDamageStore::default();
    let policy = UniformFailurePolicy::new(DamageAmount(18));
    let outcome = damage
        .apply(work.clone(), &policy, ProgressiveDamageLimits::new(16_384))
        .expect("accepted transaction");

    // Canonical site order, and every failure cleared its partial entry.
    let mut sorted = outcome.failed.clone();
    sorted.sort_by_key(|target| target.cell());
    let deterministic_order = sorted == outcome.failed;
    let all_failed = outcome.failed.len() == 256;
    let nothing_partial = outcome.remaining_entries == 0;

    // Over-budget transactions refuse atomically. Failing cells never consume a
    // sparse entry, so the budget is only reachable with sub-threshold work.
    let partial: Vec<DamageWork> = work
        .iter()
        .map(|item| DamageWork {
            amount: DamageAmount(1),
            ..*item
        })
        .collect();
    let mut tight = ProgressiveDamageStore::default();
    let refusal = tight.apply(partial.clone(), &policy, ProgressiveDamageLimits::new(4));
    let atomic_refusal = refusal.is_err() && tight.is_empty();

    // The same sub-threshold work inside budget leaves one entry per cell and
    // fails nothing.
    let mut roomy = ProgressiveDamageStore::default();
    let accumulated = roomy
        .apply(partial, &policy, ProgressiveDamageLimits::new(16_384))
        .expect("accepted");
    let accumulates = accumulated.failed.is_empty() && accumulated.remaining_entries == 256;

    let ok = !truncated_any
        && all_failed
        && deterministic_order
        && nothing_partial
        && atomic_refusal
        && accumulates;
    push(
        cases,
        "many_simultaneous_damage_events",
        ok,
        format!(
            "{} events in one transaction, failed={}, canonical order={deterministic_order}, \
             partial entries left={}, sub-threshold over-budget refused atomically={atomic_refusal}, \
             in-budget sub-threshold accumulated 256 entries without failing={accumulates}",
            256,
            outcome.failed.len(),
            outcome.remaining_entries
        ),
    );
}

// ---------------------------------------------------------------------------
// 7. Repeated sub-threshold damage
// ---------------------------------------------------------------------------

fn repeated_subthreshold_damage(cases: &mut Vec<Case>) {
    let target = DamageTarget::StaticCell {
        cell: CellPos::new(6, 6, 6),
        material: STONE,
    };
    let policy = UniformFailurePolicy::new(DamageAmount(10));
    let limits = ProgressiveDamageLimits::new(64);
    let mut damage = ProgressiveDamageStore::default();

    let mut failures = 0usize;
    let mut crossed_at = 0usize;
    for pulse in 1..=4u64 {
        let outcome = damage
            .apply(
                [DamageWork {
                    event: DamageEventId::new(pulse, 0),
                    target,
                    amount: DamageAmount(3),
                }],
                &policy,
                limits,
            )
            .expect("accepted");
        if !outcome.failed.is_empty() {
            failures += outcome.failed.len();
            if crossed_at == 0 {
                crossed_at = pulse as usize;
            }
        }
    }

    // A fifth weak pulse must start a fresh accumulation, not re-fail.
    let after = damage
        .apply(
            [DamageWork {
                event: DamageEventId::new(5, 0),
                target,
                amount: DamageAmount(3),
            }],
            &policy,
            limits,
        )
        .expect("accepted");

    let ok = failures == 1 && crossed_at == 4 && after.failed.is_empty();
    push(
        cases,
        "repeated_subthreshold_damage",
        ok,
        format!(
            "3+3+3 stayed intact, crossed on pulse {crossed_at} with {failures} failure(s), \
             post-failure pulse re-accumulated without re-failing={}",
            after.failed.is_empty()
        ),
    );
}

// ---------------------------------------------------------------------------
// 8. Very large radial event clipped at integer/world limits
// ---------------------------------------------------------------------------

fn radial_event_at_integer_limits(cases: &mut Vec<Case>) {
    let mut world = World::new();
    world.fill_box(CellPos::new(4, 4, 4), CellPos::new(9, 9, 9), Some(STONE));
    world.take_dirty();

    let huge = DamageEvent::new(
        DamageEventId::new(900, 0),
        DamageSpace::StaticWorld,
        None,
        DamageVolume::Sphere {
            center: GlobalPos::new(6.0, 6.0, 6.0),
            radius: 1.0e18,
            falloff: DamageFalloff::Uniform,
        },
        DamageAmount(5),
        None,
    )
    .expect("valid event");

    let bounds = huge.affected_cell_bounds();
    let no_wrap =
        bounds.is_none_or(|b| b.min.x <= b.max.x && b.min.y <= b.max.y && b.min.z <= b.max.z);

    let bounded = evaluate_damage_event(
        &huge,
        DamageSource::Static(&world),
        &UniformDamagePolicy,
        DamageEvaluationLimits {
            max_cells_considered: 64,
        },
    )
    .expect("evaluate");
    // A truncated evaluation is never partially actionable.
    let not_actionable = bounded.truncated && !bounded.may_apply();

    // The same clipping discipline at the top of the integer range.
    let edge = DamageEvent::new(
        DamageEventId::new(901, 0),
        DamageSpace::StaticWorld,
        None,
        DamageVolume::Sphere {
            center: GlobalPos::new(f64::from(i32::MAX), 0.0, 0.0),
            radius: 4.0,
            falloff: DamageFalloff::Linear,
        },
        DamageAmount(5),
        None,
    )
    .expect("valid event");
    let edge_bounds = edge.affected_cell_bounds();
    // The real hazard is wrap-around: a naive center+radius would overflow to a
    // negative minimum and silently select the wrong half of the world.
    let edge_sane = edge_bounds.is_none_or(|b| b.min.x <= b.max.x && b.min.x > 0);

    let ok = no_wrap && not_actionable && edge_sane;
    push(
        cases,
        "radial_event_at_integer_limits",
        ok,
        format!(
            "1e18-radius bounds did not wrap={no_wrap}, truncated evaluation not actionable={not_actionable} \
             (considered {} cells), i32::MAX-centred sphere clipped sanely={edge_sane}",
            bounded.cells_considered
        ),
    );
}

// ---------------------------------------------------------------------------
// 9. Fragment re-fracture storm
// ---------------------------------------------------------------------------

fn refracture_storm(cases: &mut Vec<Case>) {
    let root = FragmentId::new(1_000, 0);
    let mut store = FragmentStore::default();
    store.insert(line_fragment(root, CellPos::new(0, 0, 0), 129));
    let mut sequence = DestructionSequence::new(1_001);

    let mut seen_ids: BTreeSet<FragmentId> = BTreeSet::new();
    seen_ids.insert(root);
    let mut refractures = 0usize;
    let mut destroyed = 0usize;
    let mut cells_removed = 0u64;
    let mut id_collision = false;
    let mut empty_fragment = false;

    for _ in 0..64 {
        // Deterministically pick the largest fragment, lowest id wins ties.
        let Some((target_id, cells)) = store
            .iter()
            .map(|(id, fragment)| {
                (
                    fragment.cell_count(),
                    id,
                    fragment.occupied_cells().collect::<Vec<_>>(),
                )
            })
            .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
            .map(|(_, id, cells)| (id, cells))
        else {
            break;
        };
        if cells.len() < 3 {
            break;
        }
        let middle = cells[cells.len() / 2];
        let failed = [DamageTarget::FragmentCell {
            fragment: target_id,
            cell: middle,
            material: STONE,
        }];
        let before = store.len();
        let Ok(result) =
            damage_fragment_store_if(&mut store, &mut sequence, target_id, &failed, |_| true)
        else {
            break;
        };
        cells_removed += 1;
        match result {
            FragmentDamageResult::Refractured(outcome) => {
                refractures += 1;
                for child in &outcome.fragments {
                    if !seen_ids.insert(child.id) {
                        id_collision = true;
                    }
                    if child.cell_count() == 0 {
                        empty_fragment = true;
                    }
                }
                if store.len() != before + outcome.fragments.len() - 1 {
                    id_collision = true;
                }
            }
            FragmentDamageResult::Destroyed { .. } => destroyed += 1,
            FragmentDamageResult::Updated(fragment) => {
                if fragment.cell_count() == 0 {
                    empty_fragment = true;
                }
            }
            FragmentDamageResult::Unchanged => break,
        }
    }

    let live_cells: u64 = store.iter().map(|(_, f)| f.cell_count()).sum();
    let conserved = live_cells + cells_removed == 129;
    let monotonic = sequence.peek() > 1_001;

    let ok = refractures > 0 && !id_collision && !empty_fragment && conserved && monotonic;
    push(
        cases,
        "refracture_storm",
        ok,
        format!(
            "{refractures} re-fractures, {destroyed} destroyed, {} live fragments, \
             unique ids={}, no empty fragments={}, cells {live_cells}+{cells_removed}=129 conserved={conserved}, \
             sequence advanced to {}",
            store.len(),
            !id_collision,
            !empty_fragment,
            sequence.peek()
        ),
    );
}

// ---------------------------------------------------------------------------
// 10. Secondary-impact recursion and budget exhaustion
// ---------------------------------------------------------------------------

fn secondary_recursion_exhaustion(cases: &mut Vec<Case>) {
    let limits = SecondaryDamageLimits::new(8, 2, 3);
    let mut queue = SecondaryDamageQueue::default();

    let event = |n: u64| {
        DamageEvent::new(
            DamageEventId::new(n, 0),
            DamageSpace::StaticWorld,
            None,
            DamageVolume::Cell(CellPos::new(5, 5, 5)),
            DamageAmount(4),
            None,
        )
        .expect("valid event")
    };

    let mut pending_refusals = 0usize;
    for n in 0..64u64 {
        if queue.enqueue(
            SecondaryDamage {
                event: event(n),
                generation: 1,
            },
            limits,
        ) == Err(SecondaryDamageRefusal::PendingLimit)
        {
            pending_refusals += 1;
        }
    }
    let queue_capped = queue.len() == 8 && pending_refusals == 56;

    // Generation ceiling is independent of queue space.
    let mut fresh = SecondaryDamageQueue::default();
    let too_deep = fresh.enqueue(
        SecondaryDamage {
            event: event(900),
            generation: 4,
        },
        limits,
    );
    let generation_capped =
        too_deep == Err(SecondaryDamageRefusal::GenerationLimit) && fresh.is_empty();

    // Per-step drain is bounded and order-preserving.
    let first = queue.drain_step(limits);
    let second = queue.drain_step(limits);
    let step_capped = first.len() == 2 && second.len() == 2 && queue.len() == 4;
    let ordered = first[0].event.id == DamageEventId::new(0, 0)
        && first[1].event.id == DamageEventId::new(1, 0)
        && second[0].event.id == DamageEventId::new(2, 0);

    // A runaway chain cannot outrun the generation ceiling.
    let mut chain = SecondaryDamageQueue::default();
    let mut generation = 1u8;
    let mut accepted_generations = 0usize;
    while chain
        .enqueue(
            SecondaryDamage {
                event: event(u64::from(generation) + 1_000),
                generation,
            },
            limits,
        )
        .is_ok()
    {
        accepted_generations += 1;
        generation = generation.saturating_add(1);
        if generation > 32 {
            break;
        }
    }
    let chain_terminates = accepted_generations == 3;

    let counters_ok = queue.rejected_pending() == 56 && fresh.rejected_generation() == 1;

    let ok = queue_capped
        && generation_capped
        && step_capped
        && ordered
        && chain_terminates
        && counters_ok;
    push(
        cases,
        "secondary_recursion_exhaustion",
        ok,
        format!(
            "pending capped at {} with {pending_refusals} refusals, generation ceiling held={generation_capped}, \
             per-step drain 2+2 leaving {} ={step_capped}, order preserved={ordered}, \
             recursion chain stopped after {accepted_generations} generations",
            queue.len() + 4,
            queue.len()
        ),
    );
}

// ---------------------------------------------------------------------------
// 11. Stale async fragment mesh/collider results
// ---------------------------------------------------------------------------

fn stale_derived_results(cases: &mut Vec<Case>) {
    let id = FragmentId::new(2_000, 0);
    let fragment = line_fragment(id, CellPos::new(0, 0, 0), 5);
    let materials = MaterialRegistry::default();

    let mesh = FragmentMeshJobInput::new(&fragment, &materials).run();
    let collision = FragmentCollisionJobInput::new(&fragment).run();

    // Motion alone must not stale derived geometry.
    let mut moved = fragment.clone();
    let mut pose = moved.pose;
    pose.translation = GlobalPos::new(100.0, 50.0, 25.0);
    moved.update_from_physics(pose, [9.0, 0.0, 0.0], [1.0, 0.0, 0.0], false);
    let mesh_survives_motion = mesh.is_current(&moved);
    let collision_survives_motion = collision.is_current(&moved);

    // Geometry mutation must stale both.
    let damaged = moved
        .without_local_cells(&BTreeSet::from([CellPos::new(2, 0, 0)]))
        .expect("survivor");
    let mesh_staled = !mesh.is_current(&damaged);
    let collision_staled = !collision.is_current(&damaged);

    // A re-fractured child is a different object entirely.
    let child_staled = !mesh.is_current(
        &fragment
            .subfragment_with_id(
                FragmentId::new(2_001, 0),
                &BTreeSet::from([CellPos::new(0, 0, 0), CellPos::new(1, 0, 0)]),
            )
            .expect("child"),
    );

    let ok = mesh_survives_motion
        && collision_survives_motion
        && mesh_staled
        && collision_staled
        && child_staled;
    push(
        cases,
        "stale_derived_results",
        ok,
        format!(
            "motion kept mesh current={mesh_survives_motion} and collider current={collision_survives_motion}, \
             geometry damage staled mesh={mesh_staled} and collider={collision_staled}, \
             re-fracture child staled parent mesh={child_staled}"
        ),
    );
}

// ---------------------------------------------------------------------------
// 12. Deterministic replay of the whole DROP 4 loop
// ---------------------------------------------------------------------------

type ReplaySignature = (u64, Vec<(FragmentId, u64)>, u64, Vec<CellPos>, usize);

fn replay_once() -> Option<ReplaySignature> {
    let mut world = interior_tower();
    let result = cut_tower_neck(&mut world);
    let mut sequence = DestructionSequence::new(5);
    let mut outcome = detach_if(&mut world, &mut sequence, &result, |_| true).ok()?;

    // Couple an impulse into the detachment, then damage the survivor.
    let event = DamageEvent::new(
        DamageEventId::new(1, 0),
        DamageSpace::StaticWorld,
        Some(GlobalPos::new(0.0, 0.0, 0.0)),
        DamageVolume::Cell(CellPos::new(7, 8, 7)),
        DamageAmount(30),
        Some(DamageImpulse::new([12.0, 4.0, 0.0]).expect("valid impulse")),
    )
    .expect("valid event");
    let coupled = engine_destruction::couple_damage_to_detachment(&mut outcome, &event);

    let mut store = FragmentStore::default();
    for fragment in &outcome.fragments {
        store.insert(fragment.clone());
    }

    // Progressive damage on the static remainder.
    let mut damage = ProgressiveDamageStore::default();
    let policy = UniformFailurePolicy::new(DamageAmount(12));
    let static_event = DamageEvent::new(
        DamageEventId::new(2, 0),
        DamageSpace::StaticWorld,
        None,
        DamageVolume::Sphere {
            center: GlobalPos::new(7.5, 4.0, 7.5),
            radius: 2.5,
            falloff: DamageFalloff::Uniform,
        },
        DamageAmount(15),
        None,
    )
    .expect("valid event");
    let evaluation = evaluate_damage_event(
        &static_event,
        DamageSource::Static(&world),
        &UniformDamagePolicy,
        DamageEvaluationLimits::UNLIMITED,
    )
    .ok()?;
    let failures = damage
        .apply(
            evaluation.work,
            &policy,
            ProgressiveDamageLimits::new(4_096),
        )
        .ok()?;

    let mut fragments: Vec<(FragmentId, u64)> = store
        .iter()
        .map(|(id, fragment)| (id, fragment.cell_count()))
        .collect();
    fragments.sort();
    let failed_cells: Vec<CellPos> = failures.failed.iter().map(|target| target.cell()).collect();

    Some((
        world.occupied_count(),
        fragments,
        sequence.peek(),
        failed_cells,
        coupled,
    ))
}

fn deterministic_replay(cases: &mut Vec<Case>) {
    let mut signature: Option<ReplaySignature> = None;
    let mut ok = true;
    let mut runs = 0usize;
    for _ in 0..24 {
        let Some(current) = replay_once() else {
            ok = false;
            break;
        };
        runs += 1;
        match &signature {
            None => signature = Some(current),
            Some(expected) if expected == &current => {}
            Some(_) => {
                ok = false;
                break;
            }
        }
    }

    let detail = match &signature {
        Some((occupied, fragments, seq, failed, coupled)) => format!(
            "{runs} identical runs: occupied={occupied}, fragments={fragments:?}, \
             sequence={seq}, failed_cells={}, impulses coupled={coupled}",
            failed.len()
        ),
        None => "no run completed".to_string(),
    };
    push(cases, "deterministic_replay", ok && runs == 24, detail);
}

// ---------------------------------------------------------------------------
// 13. Fantasy mode: structural participation disabled
// ---------------------------------------------------------------------------

fn fantasy_mode_structural_disabled(cases: &mut Vec<Case>) {
    let dir = sandbox_dir("fantasy");

    // A floating island with no anchors anywhere and nothing holding it up.
    let mut world = World::new();
    world.fill_box(
        CellPos::new(20, 40, 20),
        CellPos::new(27, 43, 27),
        Some(STONE),
    );
    world.fill_box(
        CellPos::new(22, 44, 22),
        CellPos::new(25, 47, 25),
        Some(STONE),
    );
    world.take_dirty();
    let authored = world.occupied_count();

    // The world is authored, saved and reloaded without any structural system
    // ever running. No physical system is mandatory for world validity.
    let path = dir.join("fantasy.world.json");
    engine_io::save_world(&world, &path).expect("save fantasy world");
    let reloaded = engine_io::load_world(&path).expect("load fantasy world");
    let round_trips = reloaded.occupied_count() == authored;

    // Damage still works with structural analysis disabled.
    let mut damage = ProgressiveDamageStore::default();
    let policy = UniformFailurePolicy::new(DamageAmount(10));
    let event = DamageEvent::new(
        DamageEventId::new(77, 0),
        DamageSpace::StaticWorld,
        None,
        DamageVolume::Sphere {
            center: GlobalPos::new(23.5, 45.5, 23.5),
            radius: 2.0,
            falloff: DamageFalloff::Uniform,
        },
        DamageAmount(12),
        None,
    )
    .expect("valid event");
    let evaluation = evaluate_damage_event(
        &event,
        DamageSource::Static(&world),
        &UniformDamagePolicy,
        DamageEvaluationLimits::UNLIMITED,
    )
    .expect("evaluate");
    let outcome = damage
        .apply(
            evaluation.work,
            &policy,
            ProgressiveDamageLimits::new(1_024),
        )
        .expect("accepted");
    let batch = engine_destruction::static_failure_batch(&outcome.failed);
    let edit = world.apply(&batch);
    let damage_worked = !outcome.failed.is_empty() && world.occupied_count() < authored;

    // The island is genuinely unsupported: if the host *did* opt in, structural
    // analysis would detach it. The point is that the host is free not to.
    let before_probe = world.occupied_count();
    let probe = StructureJobInput::snapshot(
        &world,
        edit.structural_candidates.iter().copied(),
        SnapshotLimits::default(),
        StructuralLimits::UNLIMITED,
    )
    .run();
    let would_detach = probe.components.detached().count() > 0;

    // Running the analysis is not the same as acting on it: detach_if was never
    // called, so the authored island is still exactly where it was placed. This
    // is the fantasy-mode contract.
    let still_authored = world.occupied_count() == before_probe
        && reloaded.occupied_count() == authored
        && world.is_occupied(CellPos::new(20, 40, 20));

    let _ = std::fs::remove_dir_all(&dir);
    let ok = round_trips && damage_worked && still_authored;
    push(
        cases,
        "fantasy_mode_structural_disabled",
        ok,
        format!(
            "unanchored floating island of {authored} cells round-tripped={round_trips}, \
             damage worked without structural participation={damage_worked} ({} cells failed), \
             opt-in analysis would have detached {} component(s)={would_detach}, \
             island still exactly where authored with structural system never applied={still_authored}",
            outcome.failed.len(),
            probe.components.detached().count()
        ),
    );
}

// ---------------------------------------------------------------------------
// Chaos Pass rule: isolation and post-run integrity verification
// ---------------------------------------------------------------------------

fn isolation_and_integrity(cases: &mut Vec<Case>) -> bool {
    let live = sandbox_dir("live-target");

    // Build the pristine "live" target: a world plus a persisted fragment store.
    let mut world = interior_tower();
    let result = cut_tower_neck(&mut world);
    let mut sequence = DestructionSequence::new(10);
    let mut store = FragmentStore::default();
    if let Ok(outcome) = detach_if(&mut world, &mut sequence, &result, |_| true) {
        for fragment in outcome.fragments {
            store.insert(fragment);
        }
    }
    engine_io::save_world(&world, live.join("world.json")).expect("save live world");
    engine_io::save_fragment_store(&live, &store, sequence).expect("save live fragments");

    let baseline = digest_tree(&live);
    let baseline_files = baseline.len();

    // Abuse happens only on a disposable copy.
    let copy = sandbox_dir("disposable-copy");
    copy_tree(&live, &copy);
    let copy_matched_baseline = digest_tree(&copy) == baseline;

    let mut index = engine_io::load_fragment_index(&copy).expect("load copy index");
    let victims: Vec<_> = index.fragment_ids().collect();
    let mut deletions = 0usize;
    for id in victims {
        if engine_io::delete_persisted_fragment(&copy, &mut index, id).unwrap_or(false) {
            deletions += 1;
        }
    }
    let mut wrecked = engine_io::load_world(copy.join("world.json")).expect("load copy world");
    let mut batch = WorldEditBatch::new();
    batch.fill_box(CellPos::new(0, 0, 0), CellPos::new(31, 31, 31), None);
    wrecked.apply(&batch);
    engine_io::save_world(&wrecked, copy.join("world.json")).expect("save wrecked copy");

    let copy_changed = digest_tree(&copy) != baseline;

    // Integrity verification: the live target must be byte-identical.
    let after = digest_tree(&live);
    let intact = after == baseline;

    // Teardown.
    let _ = std::fs::remove_dir_all(&copy);
    let torn_down = !copy.exists();
    let _ = std::fs::remove_dir_all(&live);

    let ok = copy_matched_baseline && deletions > 0 && copy_changed && intact && torn_down;
    push(
        cases,
        "isolation_and_integrity",
        ok,
        format!(
            "baseline={baseline_files} files, copy matched baseline={copy_matched_baseline}, \
             abused copy ({deletions} fragment deletions + full world wipe) diverged={copy_changed}, \
             live target byte-identical afterwards={intact}, sandbox torn down={torn_down}"
        ),
    );
    intact
}

// ---------------------------------------------------------------------------

fn write_report(path: &Path, report: &ChaosReport) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create report directory");
    }
    let mut json = serde_json::to_string_pretty(report).expect("serialize chaos report");
    json.push('\n');
    std::fs::write(path, json).expect("write chaos report");
}

fn main() {
    let output = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("docs/diagnostics/drop0004_9_chaos/report.json"));

    let mut cases = Vec::new();
    population_beyond_residency(&mut cases);
    travel_away_and_back(&mut cases);
    dirty_eviction_during_motion(&mut cases);
    incremental_persistence_crash_windows(&mut cases);
    structural_demand_at_boundary(&mut cases);
    many_simultaneous_damage_events(&mut cases);
    repeated_subthreshold_damage(&mut cases);
    radial_event_at_integer_limits(&mut cases);
    refracture_storm(&mut cases);
    secondary_recursion_exhaustion(&mut cases);
    stale_derived_results(&mut cases);
    deterministic_replay(&mut cases);
    fantasy_mode_structural_disabled(&mut cases);
    let integrity_verified = isolation_and_integrity(&mut cases);

    let all_ok = cases.iter().all(|case| case.ok) && integrity_verified;
    let report = ChaosReport {
        generated_by: "engine_stress::chaos4",
        drop_section: "0004.9",
        cases,
        integrity_verified,
        all_ok,
    };
    write_report(&output, &report);
    println!("{}", serde_json::to_string_pretty(&report).unwrap());

    if !all_ok {
        std::process::exit(2);
    }
}
