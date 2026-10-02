//! Complete fracture admission: damage, topology, ownership and identities land
//! together. A pending result never deletes cells or consumes a fragment ID.
use crate::*;
use engine_core::{CellPos, CellSource, MaterialId, NoSupport, RegionPos};
use engine_world::{EditOutcome, World};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug)]
pub struct FractureTransactionLimits {
    pub fracture: FractureLimits,
    pub structure: StructuralLimits,
}

impl Default for FractureTransactionLimits {
    fn default() -> Self {
        Self {
            fracture: FractureLimits::new(16_384, 49_152, 1 << 20, 1 << 21),
            structure: StructuralLimits::tight(65_536),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FractureTransactionRefusal {
    Evaluation(FractureDefer),
    Unknown(BTreeSet<RegionPos>),
    State(FractureRefusal),
    SpaceMismatch,
    StructureInconclusive,
    Admission,
    IdentityCollision,
    ParentMissing,
}

fn evaluate(
    impact: &FractureImpact,
    scene: FractureScene<'_>,
    state: &FractureState,
    policy: &dyn FracturePolicy,
    limits: FractureLimits,
) -> Result<FractureLoad, FractureTransactionRefusal> {
    match evaluate_fracture(impact, scene, policy, state, limits)
        .map_err(|_| FractureTransactionRefusal::SpaceMismatch)?
    {
        FractureEvaluation::Loaded(load) => Ok(load),
        FractureEvaluation::Deferred { reason } => {
            Err(FractureTransactionRefusal::Evaluation(reason))
        }
        FractureEvaluation::Indeterminate { required_regions } => {
            Err(FractureTransactionRefusal::Unknown(required_regions))
        }
    }
}

struct AfterFailures<'a> {
    cells: &'a dyn CellSource,
    failed: BTreeSet<CellPos>,
}
impl CellSource for AfterFailures<'_> {
    fn material_at(&self, cell: CellPos) -> Option<MaterialId> {
        if self.failed.contains(&cell) {
            None
        } else {
            self.cells.material_at(cell)
        }
    }
}

#[derive(Clone, Debug)]
pub struct StaticFractureCommit {
    pub fracture: FractureOutcome,
    pub measurement: FractureMeasurement,
    pub separation: FractureSeparation,
    pub edits: Vec<EditOutcome>,
    pub fragments: Vec<FragmentId>,
}

/// Classify a read-only view of the proposed failures and cracks. Build exact
/// candidate fragments for admission before changing any authoritative object.
/// The existing detachment transaction remains the only route into fragments.
#[allow(clippy::too_many_arguments)]
pub fn fracture_static_if(
    world: &mut World,
    store: &mut FragmentStore,
    state: &mut FractureState,
    sequence: &mut DestructionSequence,
    impact: &FractureImpact,
    policy: &dyn FracturePolicy,
    residency: &dyn Residency,
    limits: FractureTransactionLimits,
    admit: impl FnOnce(&[Fragment]) -> bool,
) -> Result<StaticFractureCommit, FractureTransactionRefusal> {
    let scene = FractureScene::static_world(world, world, residency);
    let load = evaluate(impact, scene, state, policy, limits.fracture)?;
    let prepared = state
        .prepare(&load, scene, policy, limits.fracture)
        .map_err(FractureTransactionRefusal::State)?;
    let cells = AfterFailures {
        cells: world,
        failed: prepared
            .outcome
            .failed
            .iter()
            .map(|target| target.cell())
            .collect(),
    };
    let separation = separation_from_cracks(
        &cells,
        world,
        residency,
        &prepared.gate(state, DamageSpace::StaticWorld),
        separation_roots(&prepared.outcome),
        limits.structure,
    );
    if !separation.is_settled() {
        return Err(FractureTransactionRefusal::StructureInconclusive);
    }
    let candidates: Vec<_> = separation
        .separated
        .iter()
        .enumerate()
        .filter_map(|(index, component)| {
            Fragment::from_cells(
                FragmentId::new(sequence.peek(), index as u32),
                world,
                &component.cells,
            )
        })
        .collect();
    if candidates.iter().any(|f| store.contains(f.id))
        || (!candidates.is_empty() && sequence.peek() == u64::MAX)
    {
        return Err(FractureTransactionRefusal::IdentityCollision);
    }
    if !admit(&candidates) {
        return Err(FractureTransactionRefusal::Admission);
    }

    // All fallible work is complete. No callback or concurrent mutation runs
    // between this point and the commit of geometry/ownership.
    let fracture = state
        .commit_prepared(prepared)
        .expect("exclusive state remained current");
    let mut edits = vec![world.apply(&static_failure_batch(&fracture.failed))];
    let mut fragments = Vec::new();
    if !candidates.is_empty() {
        let result = cracked_structure_result(
            world,
            separation.separated.iter().filter_map(|c| c.anchor_cell()),
            separation.components.clone(),
        );
        let detached = detach_if(world, sequence, &result, |_| true)
            .expect("settled current result already admitted");
        edits.push(detached.edit);
        for fragment in detached.fragments {
            state.adopt_into_fragment(&fragment);
            fragments.push(fragment.id);
            store.insert(fragment);
        }
    }
    Ok(StaticFractureCommit {
        fracture,
        measurement: load.measurement,
        separation,
        edits,
        fragments,
    })
}

#[derive(Clone, Debug)]
pub struct FragmentFractureCommit {
    pub fracture: FractureOutcome,
    pub measurement: FractureMeasurement,
    pub result: FragmentDamageResult,
}

/// Only the affected parent is staged, never the complete fragment store or
/// fracture history. A refused split also refuses the new damage records.
#[allow(clippy::too_many_arguments)]
pub fn fracture_fragment_if(
    store: &mut FragmentStore,
    state: &mut FractureState,
    sequence: &mut DestructionSequence,
    impact: &FractureImpact,
    policy: &dyn FracturePolicy,
    limits: FractureTransactionLimits,
    admit: impl FnOnce(&[Fragment]) -> bool,
) -> Result<FragmentFractureCommit, FractureTransactionRefusal> {
    let DamageSpace::FragmentLocal(parent_id) = impact.space() else {
        return Err(FractureTransactionRefusal::SpaceMismatch);
    };
    let parent = store
        .get(parent_id)
        .ok_or(FractureTransactionRefusal::ParentMissing)?;
    let scene = FractureScene::of_fragment(parent);
    let load = evaluate(impact, scene, state, policy, limits.fracture)?;
    let prepared = state
        .prepare(&load, scene, policy, limits.fracture)
        .map_err(FractureTransactionRefusal::State)?;
    let mut staged = FragmentStore::default();
    staged.insert(parent.clone());
    let parts = if let Some(updated) = fragment_after_failures(parent, &prepared.outcome.failed) {
        let components = classify_cracked_from_roots(
            &updated,
            &NoSupport,
            &AllResident,
            &prepared.gate(state, impact.space()),
            updated.occupied_cells(),
            limits.structure,
        );
        if !components.is_settled() {
            return Err(FractureTransactionRefusal::StructureInconclusive);
        }
        components.components.into_iter().map(|c| c.cells).collect()
    } else {
        Vec::new()
    };
    if parts.len() > 1 && sequence.peek() == u64::MAX {
        return Err(FractureTransactionRefusal::IdentityCollision);
    }
    let mut next = *sequence;
    let result = damage_fragment_store_with_parts(
        &mut staged,
        &mut next,
        parent_id,
        &prepared.outcome.failed,
        |_| parts,
        |_| true,
    )
    .map_err(|_| FractureTransactionRefusal::IdentityCollision)?;
    let survivors: Vec<_> = staged.iter().map(|(_, f)| f.clone()).collect();
    if survivors
        .iter()
        .any(|f| f.id != parent_id && store.contains(f.id))
    {
        return Err(FractureTransactionRefusal::IdentityCollision);
    }
    if !admit(&survivors) {
        return Err(FractureTransactionRefusal::Admission);
    }
    let fracture = state
        .commit_prepared(prepared)
        .expect("exclusive state remained current");
    match &result {
        FragmentDamageResult::Destroyed { .. } => {
            state.forget_space(impact.space());
        }
        FragmentDamageResult::Refractured(split) => {
            state.remap_fragment(parent_id, &split.fragments);
        }
        _ => {
            state.forget_removed(
                impact.space(),
                fracture.failed.iter().map(|target| target.cell()),
            );
        }
    }
    store.remove(parent_id);
    for fragment in survivors {
        store.insert(fragment);
    }
    *sequence = next;
    Ok(FragmentFractureCommit {
        fracture,
        measurement: load.measurement,
        result,
    })
}
