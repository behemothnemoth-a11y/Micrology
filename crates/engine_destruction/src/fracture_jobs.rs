//! Bounded owned fracture jobs. Workers analyze and build fragments; the host
//! validates and admits a sparse result. Pending or stale work changes nothing.
use crate::fracture::{FractureModel, FracturePatch};
use crate::fracture_policy::OwnedFracturePolicy;
use crate::*;
use engine_core::{CellPos, FaceDir, RegionPos, Revision};
use engine_world::{World, WorldEditBatch};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug)]
pub struct FractureJobLimits {
    pub bytes: u64,
    pub volumes: usize,
    pub damage_records: usize,
    pub transaction: FractureTransactionLimits,
}
impl Default for FractureJobLimits {
    fn default() -> Self {
        Self {
            bytes: 4 << 20,
            volumes: 128,
            damage_records: 32768,
            transaction: Default::default(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobRefusal {
    SnapshotBudget,
    InvalidTargets,
    MissingParent,
    Stale,
    Admission,
    Identity,
    Analysis(FractureTransactionRefusal),
}

#[derive(Clone, Debug)]
pub struct KnownRegions(pub BTreeSet<RegionPos>);
impl Residency for KnownRegions {
    fn is_resident(&self, cell: CellPos) -> bool {
        self.0.contains(&cell.region())
    }
}

pub struct FractureJobInput {
    pub world: World,
    pub state: FractureState,
    pub residency: KnownRegions,
    store: FragmentStore,
    space: DamageSpace,
    world_revision: Revision,
    regions: Vec<(RegionPos, Option<Revision>, Option<Revision>)>,
    volumes: Vec<(engine_core::VolumePos, Option<Revision>)>,
    damage_revision: Revision,
    parent_revision: Option<Revision>,
    sequence: DestructionSequence,
    limits: FractureJobLimits,
}

#[derive(Clone, Debug, Default)]
pub struct JobSummary {
    pub failed: u64,
    pub broken: u64,
    pub created: u64,
    pub work: u64,
    /// Topology work, separate from impact propagation.
    pub structure_cells: u64,
    pub occupancy_cross_check_cells: u64,
    pub witness_work: Option<crate::fracture_witness::WitnessWork>,
    pub ids: Vec<FragmentId>,
    /// What the DROP 0006.6 grouping pass cost and produced, so a host can see
    /// partition cost separately from damage analysis.
    pub partition_work: crate::fragment_partition::PartitionWork,
}
pub struct FractureJobResult {
    space: DamageSpace,
    world_revision: Revision,
    regions: Vec<(RegionPos, Option<Revision>, Option<Revision>)>,
    volumes: Vec<(engine_core::VolumePos, Option<Revision>)>,
    damage_revision: Revision,
    parent_revision: Option<Revision>,
    sequence: DestructionSequence,
    next: DestructionSequence,
    patch: FracturePatch,
    leaving: BTreeSet<CellPos>,
    fragments: Vec<Fragment>,
    limits: FractureTransactionLimits,
    pub summary: JobSummary,
}

impl FractureJobInput {
    /// `known_regions` must be known resident, including explicitly known empty
    /// regions. Anything outside this finite snapshot remains unknown. Hosts may
    /// pass a smaller region set; truncation never grants permission to detach.
    pub fn capture(
        world: &World,
        store: &FragmentStore,
        state: &FractureState,
        sequence: DestructionSequence,
        space: DamageSpace,
        known_regions: BTreeSet<RegionPos>,
        limits: FractureJobLimits,
    ) -> Result<Self, JobRefusal> {
        if known_regions.len() > 27 {
            return Err(JobRefusal::SnapshotBudget);
        }
        // DROP 0006.5: bounded by the snapshot's own regions. This used to copy
        // every static record in the world, which made a local hit cost work
        // proportional to unrelated damage history and let `damage_records`
        // refuse a capture because of cracks nowhere near the impact. The worker
        // can only read the regions it was given, so those are the only records
        // that can change its answer.
        let state_copy = state
            .snapshot_regions(space, &known_regions, limits.damage_records)
            .ok_or(JobRefusal::SnapshotBudget)?;
        let mut copy = World::new();
        let mut owned = FragmentStore::default();
        let mut parent_revision = None;
        match space {
            DamageSpace::StaticWorld => {
                let mut bytes = 0u64;
                let mut volumes = 0usize;
                for pos in &known_regions {
                    if let Some(region) = world.region(*pos) {
                        bytes = bytes.saturating_add(region.footprint().total());
                        volumes += region.allocated_volume_count();
                        if bytes > limits.bytes || volumes > limits.volumes {
                            return Err(JobRefusal::SnapshotBudget);
                        }
                    }
                }
                for pos in &known_regions {
                    if let Some(region) = world.region(*pos) {
                        copy.insert_region(*pos, region.clone());
                    }
                }
            }
            DamageSpace::FragmentLocal(id) => {
                let f = store.get(id).ok_or(JobRefusal::MissingParent)?;
                if f.footprint_bytes() > limits.bytes || f.volumes().count() > limits.volumes {
                    return Err(JobRefusal::SnapshotBudget);
                }
                parent_revision = Some(f.geometry_revision());
                owned.insert(f.clone());
            }
        }
        let regions = if space == DamageSpace::StaticWorld {
            known_regions
                .iter()
                .map(|&p| {
                    (
                        p,
                        world.region(p).map(|r| r.revision()),
                        world.support_revision(p),
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        let volumes = copy
            .volumes()
            .map(|(p, _)| (p, world.volume_revision(p)))
            .collect();
        Ok(Self {
            regions,
            volumes,
            world: copy,
            state: state_copy,
            residency: KnownRegions(known_regions),
            store: owned,
            space,
            world_revision: world.revision(),
            damage_revision: state.revision(),
            parent_revision,
            sequence,
            limits,
        })
    }
    /// Static records this snapshot actually copied.
    ///
    /// DROP 0006.5 measurement surface: the number the locality benchmark holds
    /// constant while unrelated damage history grows around it.
    pub fn state_records(&self) -> usize {
        self.state.cell_entries() + self.state.bond_entries()
    }

    /// Bytes of fracture record this snapshot owns.
    ///
    /// Record counts times their in-memory size — the staged payload, not the
    /// tree overhead around it, so it is a floor rather than a total.
    pub fn state_bytes(&self) -> usize {
        self.state.cell_entries() * size_of::<crate::CellFracture>()
            + self.state.bond_entries() * size_of::<crate::BondFracture>()
    }

    /// Compatibility entry point: the accepted homogeneous models are unchanged.
    pub fn run(
        self,
        impact: FractureImpact,
        model: FractureModel,
    ) -> Result<FractureJobResult, JobRefusal> {
        self.run_using(impact, &model)
    }

    /// Moves an immutable, bounded policy into the worker. No live registry can
    /// be observed during analysis. The result requires current policy equality
    /// at commit, in addition to the existing geometry/history/admission guards.
    pub fn run_with_policy(
        self,
        impact: FractureImpact,
        policy: OwnedFracturePolicy,
    ) -> Result<PolicyFractureJobResult, JobRefusal> {
        let result = self.run_using(impact, &policy)?;
        Ok(PolicyFractureJobResult { result, policy })
    }

    fn run_using(
        mut self,
        impact: FractureImpact,
        policy: &dyn FracturePolicy,
    ) -> Result<FractureJobResult, JobRefusal> {
        let before = self.state.clone();
        let mut next = self.sequence;
        let mut leaving = BTreeSet::new();
        let summary = match self.space {
            DamageSpace::StaticWorld => {
                let c = fracture_static_if(
                    &mut self.world,
                    &mut self.store,
                    &mut self.state,
                    &mut next,
                    &impact,
                    policy,
                    &self.residency,
                    self.limits.transaction,
                    |_| true,
                )
                .map_err(JobRefusal::Analysis)?;
                for edit in c.edits {
                    leaving.extend(edit.removed_cells);
                }
                JobSummary {
                    failed: c.fracture.failed.len() as u64,
                    broken: c.fracture.broken.len() as u64,
                    created: c.fragments.len() as u64,
                    work: c.measurement.cells_visited + c.measurement.bonds_considered,
                    structure_cells: c.separation.components.cells_visited,
                    occupancy_cross_check_cells: c.separation.cross_check_cells_visited,
                    witness_work: c.witness_work,
                    ids: c.fragments,
                    // The static path groups through `detach_if`, which is one
                    // fragment per occupancy component already; no grouping
                    // pass runs, so there is nothing to report.
                    partition_work: Default::default(),
                }
            }
            DamageSpace::FragmentLocal(id) => {
                let c = fracture_fragment_if(
                    &mut self.store,
                    &mut self.state,
                    &mut next,
                    &impact,
                    policy,
                    self.limits.transaction,
                    |_| true,
                )
                .map_err(JobRefusal::Analysis)?;
                let ids = self.store.ids().collect::<Vec<_>>();
                JobSummary {
                    failed: c.fracture.failed.len() as u64,
                    broken: c.fracture.broken.len() as u64,
                    created: if ids.first() == Some(&id) {
                        0
                    } else {
                        ids.len() as u64
                    },
                    work: c.measurement.cells_visited + c.measurement.bonds_considered,
                    ids,
                    partition_work: c.partition_work,
                    ..Default::default()
                }
            }
        };
        Ok(self.finish(before, next, leaving, summary))
    }
    /// Mechanical failures and explicit cuts use the same worker transaction
    /// boundary and independent occupancy cross-check as impact-driven changes.
    pub fn run_removals(self, targets: Vec<DamageTarget>) -> Result<FractureJobResult, JobRefusal> {
        self.run_removals_with_bond_breaks(targets, Vec::new())
    }

    /// Apply authored/mechanical bond failures in the same owned transaction as
    /// explicit cell removals. The live state is untouched until ordinary commit
    /// stale/admission checks pass.
    pub fn run_removals_with_bond_breaks(
        mut self,
        targets: Vec<DamageTarget>,
        bonds: Vec<BondKey>,
    ) -> Result<FractureJobResult, JobRefusal> {
        if self.space != DamageSpace::StaticWorld {
            return Err(JobRefusal::MissingParent);
        }
        if targets.len() as u64 > self.limits.transaction.fracture.max_cells_visited
            || bonds.len() as u64 > self.limits.transaction.fracture.max_bonds_considered
            || targets.iter().any(|target| match target {
                DamageTarget::StaticCell { cell, material } => {
                    !self.residency.is_resident(*cell) || self.world.get(*cell) != Some(*material)
                }
                _ => true,
            })
        {
            return Err(JobRefusal::InvalidTargets);
        }

        let before = self.state.clone();
        let mut next = self.sequence;
        let edit = self.world.apply(&static_failure_batch(&targets));
        let mut leaving: BTreeSet<_> = edit.removed_cells.iter().copied().collect();
        self.state
            .forget_removed(DamageSpace::StaticWorld, leaving.iter().copied());

        let bonds: BTreeSet<_> = bonds.into_iter().collect();
        let mut records = BTreeMap::<RegionPos, Vec<BondFracture>>::new();
        let mut newly_broken = 0u64;
        for bond in &bonds {
            let lower = bond.lower();
            let upper = bond.upper();
            if !self.residency.is_resident(lower)
                || !self.residency.is_resident(upper)
                || self.world.get(lower).is_none()
                || self.world.get(upper).is_none()
            {
                return Err(JobRefusal::InvalidTargets);
            }
            let site = BondSite {
                space: DamageSpace::StaticWorld,
                bond: *bond,
            };
            newly_broken += u64::from(self.state.integrity(site).0 != 0);
            records
                .entry(lower.region())
                .or_default()
                .push(BondFracture {
                    site,
                    material: self.world.get(lower).expect("validated occupied lower"),
                    integrity: DamageAmount::ZERO,
                });
        }
        for (region, bonds) in records {
            self.state
                .restore_region(
                    RegionFracture {
                        region,
                        cells: Vec::new(),
                        bonds,
                    },
                    self.limits.transaction.fracture,
                )
                .map_err(|error| JobRefusal::Analysis(FractureTransactionRefusal::State(error)))?;
        }

        let roots: BTreeSet<_> = leaving
            .iter()
            .flat_map(|p| FaceDir::ALL.into_iter().filter_map(|d| p.checked_step(d)))
            .chain(bonds.iter().flat_map(|bond| bond.cells()))
            .filter(|cell| self.world.get(*cell).is_some())
            .collect();
        let (separation, witness_work) = if self.limits.transaction.support_witnesses {
            let result = crate::fracture_witness::separation_with_support_witnesses(
                &self.world,
                &self.world,
                &self.residency,
                &CrackedBonds::new(&self.state, DamageSpace::StaticWorld),
                roots.iter().copied(),
                self.limits.transaction.structure,
            )
            .map_err(|_| JobRefusal::Analysis(FractureTransactionRefusal::StructureInconclusive))?;
            // Success explicitly accounts for every root by support proof or
            // fully closed detached component. Supported paths are NOT partial
            // Component sets and must not be passed to detachment as such.
            (result.separation, Some(result.work))
        } else {
            let separation = separation_from_cracks(
                &self.world,
                &self.world,
                &self.residency,
                &CrackedBonds::new(&self.state, DamageSpace::StaticWorld),
                roots.iter().copied(),
                self.limits.transaction.structure,
            );
            let covered: BTreeSet<_> = separation
                .components
                .components
                .iter()
                .flat_map(|c| c.cells.iter().copied())
                .collect();
            if !separation.is_settled()
                || roots.iter().any(|p| {
                    !self.residency.is_resident(*p)
                        || (self.world.get(*p).is_some() && !covered.contains(p))
                })
            {
                return Err(JobRefusal::Analysis(
                    FractureTransactionRefusal::StructureInconclusive,
                ));
            }
            (separation, None)
        };
        let structure_cells = separation.components.cells_visited;
        let occupancy_cross_check_cells = separation.cross_check_cells_visited;
        let mut ids = Vec::new();
        if !separation.separated.is_empty() {
            if next.peek() == u64::MAX {
                return Err(JobRefusal::Identity);
            }
            let result = cracked_structure_result(&self.world, roots, separation.components);
            let detached = detach_if(&mut self.world, &mut next, &result, |_| true)
                .map_err(|_| JobRefusal::Stale)?;
            leaving.extend(detached.edit.removed_cells);
            for f in detached.fragments {
                self.state.adopt_into_fragment(&f);
                ids.push(f.id);
                self.store.insert(f);
            }
        }
        let summary = JobSummary {
            failed: edit.removed_cells.len() as u64,
            broken: newly_broken,
            structure_cells,
            occupancy_cross_check_cells,
            witness_work,
            created: ids.len() as u64,
            ids,
            ..Default::default()
        };
        Ok(self.finish(before, next, leaving, summary))
    }
    fn finish(
        self,
        before: FractureState,
        next: DestructionSequence,
        leaving: BTreeSet<CellPos>,
        summary: JobSummary,
    ) -> FractureJobResult {
        FractureJobResult {
            space: self.space,
            world_revision: self.world_revision,
            regions: self.regions,
            volumes: self.volumes,
            damage_revision: self.damage_revision,
            parent_revision: self.parent_revision,
            sequence: self.sequence,
            next,
            patch: self.state.patch_from(&before),
            leaving,
            fragments: self.store.iter().map(|(_, f)| f.clone()).collect(),
            limits: self.limits.transaction,
            summary,
        }
    }
}
impl FractureJobResult {
    /// Revalidate current region residency as well as geometry/support stamps.
    /// The residency callback must describe current authority, not the snapshot.
    pub fn commit(
        mut self,
        world: &mut World,
        store: &mut FragmentStore,
        state: &mut FractureState,
        sequence: &mut DestructionSequence,
        resident: impl Fn(RegionPos) -> bool,
        admit: impl FnOnce(&[Fragment]) -> bool,
    ) -> Result<JobSummary, JobRefusal> {
        if state.revision() != self.damage_revision || *sequence != self.sequence {
            return Err(JobRefusal::Stale);
        }
        let parent = match self.space {
            DamageSpace::StaticWorld => {
                if world.revision() != self.world_revision
                    || self.regions.iter().any(|(p, geometry, support)| {
                        !resident(*p)
                            || world.region(*p).map(|r| r.revision()) != *geometry
                            || world.support_revision(*p) != *support
                    })
                    || self
                        .volumes
                        .iter()
                        .any(|(p, revision)| world.volume_revision(*p) != *revision)
                {
                    return Err(JobRefusal::Stale);
                }
                None
            }
            DamageSpace::FragmentLocal(id) => {
                let f = store.get(id).ok_or(JobRefusal::Stale)?;
                if Some(f.geometry_revision()) != self.parent_revision {
                    return Err(JobRefusal::Stale);
                }
                let motion = FragmentPhysicsState::from_fragment(f);
                // Geometry is local; bodies may continue moving while workers run.
                for child in &mut self.fragments {
                    motion.apply_to(child);
                }
                Some(id)
            }
        };
        if self
            .fragments
            .iter()
            .any(|f| Some(f.id) != parent && store.contains(f.id))
        {
            return Err(JobRefusal::Identity);
        }
        if !state.patch_fits(&self.patch, self.limits.fracture) || !admit(&self.fragments) {
            return Err(JobRefusal::Admission);
        }
        let mut batch = WorldEditBatch::new();
        for cell in self.leaving {
            batch.remove(cell);
        }
        world.apply(&batch);
        state.apply_patch(self.patch);
        if let Some(id) = parent {
            store.remove(id);
        }
        for f in self.fragments {
            store.insert(f);
        }
        *sequence = self.next;
        Ok(self.summary)
    }
}

/// Policy-stamped result. The underlying result stays private so material jobs
/// cannot accidentally call the legacy commit without checking current tuning.
pub struct PolicyFractureJobResult {
    result: FractureJobResult,
    policy: OwnedFracturePolicy,
}
impl PolicyFractureJobResult {
    pub fn summary(&self) -> &JobSummary {
        &self.result.summary
    }

    /// Validate immutable policy contents and authoritative state in one call.
    /// Same contents in a newly allocated snapshot are valid; a different model
    /// or table refuses before admission or mutation. No hash collisions/epochs.
    #[allow(clippy::too_many_arguments)] // Existing explicit commit boundary + policy guard.
    pub fn commit(
        self,
        current_policy: &OwnedFracturePolicy,
        world: &mut World,
        store: &mut FragmentStore,
        state: &mut FractureState,
        sequence: &mut DestructionSequence,
        resident: impl Fn(RegionPos) -> bool,
        admit: impl FnOnce(&[Fragment]) -> bool,
    ) -> Result<JobSummary, JobRefusal> {
        if &self.policy != current_policy {
            return Err(JobRefusal::Stale);
        }
        self.result
            .commit(world, store, state, sequence, resident, admit)
    }
}
