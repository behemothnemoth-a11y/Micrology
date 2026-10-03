//! Sparse persistent fracture state and the reference impact model: DROP 0006.0.
//!
//! DROP 0004 gave the engine a damage pipeline that works:
//!
//! ```text
//! DamageEvent -> DamageWork -> ProgressiveDamageStore -> failed cells -> WorldEdit -> connectivity
//! ```
//!
//! That pipeline is correct and stays exactly as it is. What it cannot express is
//! the interesting part of destruction: material that is **still there and
//! weaker**. A cell either has not reached its threshold, in which case nothing
//! about it has changed, or it has, in which case it is gone. There is no state
//! between intact and absent, so a hit can only ever delete a shape, and the
//! shape it deletes is the shape of the event.
//!
//! This module adds that missing state. It does not replace anything.
//!
//! # Where this sits, and why
//!
//! Fracture **consumes [`DamageEvent`] directly, beside the scalar path**, rather
//! than sitting after `DamageWork` or inside `ProgressiveDamageStore`:
//!
//! * **Not after `DamageWork`.** [`DamageWork`](crate::DamageWork) is
//!   `{event, target, amount}`. By the time an event has become work, the
//!   geometry is gone — there is no impact position left in it, no direction and
//!   no distance. A fracture model whose whole subject is *where the energy went*
//!   cannot be built on a representation that has thrown the geometry away.
//! * **Not inside `ProgressiveDamageStore`.** That store is one scalar per cell
//!   and deletes its record at threshold, which is the right contract for
//!   accumulated applied damage and the wrong one for a crack. Merging the two
//!   would also put one accumulator under two different threshold policies: a
//!   weapon hit and an impact are not the same question about a cell.
//! * **Beside, from the same event.** [`DamageEvent`] already carries
//!   `source_world`, `impulse_world` and the affected volume — exactly the three
//!   things a fracture model needs. So a host may run both paths from one event,
//!   or either alone, and neither knows about the other.
//!
//! ```text
//!              DamageEvent
//!                   |
//!     +-------------+--------------+
//!     |                            |
//!  DamageWork                 FractureImpact
//!     |                            |
//!  ProgressiveDamageStore      FractureState  (sparse: cell energy + bond integrity)
//!     |                            |
//!     +-------------+--------------+
//!                   |
//!             failed cells
//!                   |
//!        WorldEditBatch -> connectivity -> Fragment
//! ```
//!
//! # Bonds, and the invariant this does not touch
//!
//! State is held two ways, both sparse:
//!
//! * per **cell**, accumulated absorbed energy, which destroys the cell outright
//!   when it reaches the profile's crush threshold;
//! * per **bond** — one face between two occupied cells — a remaining integrity
//!   that falls as the bond is loaded and reaches zero when it breaks.
//!
//! A bond representation is the right one here because Micrology's structural
//! topology is *already* face-connected, so a crack has somewhere natural to
//! live: two cells stay present while the connection between them stops
//! existing. That is the state everything later wants — an irregular break
//! surface, a weak region that the next hit concentrates into, a plug that is
//! loose before it is gone.
//!
//! **Occupancy connectivity remains the topological oracle.** Nothing in this
//! module changes [`classify_from_roots`](crate::classify_from_roots) or what it
//! means, and no broken bond detaches anything by itself. Fracture-aware
//! connectivity — treating a broken bond as a cut when deciding what is held up
//! — is a separate layer for a later pass, and when it arrives it must sit
//! *beside* the exact occupancy classifier rather than replacing it. Weakening
//! one of the strongest invariants in the engine to make cracking easier would
//! be a bad trade at any price.
//!
//! In this pass a bond breaking reaches geometry by exactly one route: a cell
//! all of whose bonds have broken is no longer held by anything, and fails. That
//! failure is an ordinary [`DamageTarget`], so it flows through
//! [`static_failure_batch`](crate::static_failure_batch), `WorldEditBatch`,
//! connectivity and the fragment pipeline without any of them learning a new
//! concept.
//!
//! # Determinism
//!
//! The model is integer. `DamageEvent` carries `f64` world coordinates, so there
//! is exactly one quantisation step — [`FractureImpact::from_event`] — and
//! everything downstream of it is `i64`/`i128` arithmetic with saturating
//! conversions. Cell centres never go through floating point at all: a cell
//! address is an `i32`, so its centre in milli-cells is `x * 1000 + 500`
//! exactly. Output is canonicalised through `BTreeMap`, so traversal order is
//! not observable.
//!
//! # Conservative under everything
//!
//! * Unknown space is not empty. A non-resident neighbour *within the impact's
//!   reach* yields [`FractureEvaluation::Indeterminate`] and the regions needed.
//!   Beyond the reach it cannot change the answer, so it is not reported.
//! * A spent budget is not destruction. Work ceilings yield
//!   [`FractureEvaluation::Deferred`], which carries no deltas and can mutate
//!   nothing.
//! * Memory ceilings refuse the whole transaction. There is no half-applied
//!   fracture state.
//! * An anchored cell is never failed by bond loss. Losing its bonds is a
//!   structural inference, and an anchor is the author's statement that
//!   something is held up regardless. It can still be crushed, because that is
//!   applied damage and applied damage has never respected anchors.
//!
//! # One material, on purpose
//!
//! [`BaselineFracturePolicy`] applies **one** profile to every material, by
//! construction: there is no table to key, so material diversity is not merely
//! unused here, it is unexpressible. Making fracture material-aware later is the
//! same move DROP 0005 made for damage — a new [`FracturePolicy`] implementation
//! in `engine_mechanics`, with nothing in this module changing.

use crate::{
    DamageAmount, DamageEvent, DamageSite, DamageSpace, DamageTarget, DamageVolume, Fragment,
    FragmentId, Residency,
};
use engine_core::{
    Axis, CellPos, CellSource, FaceDir, GlobalPos, MaterialId, RegionPos, Revision, SupportSource,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

#[cfg(test)]
mod reference_commit;

/// A transaction reads unchanged records in place and stages only touched keys.
/// Removed keys are tombstones, never a fallback to the old record. Keeping
/// the final length lets admission remain atomic without copying world history.
struct MapDelta<'a, K, V> {
    base: &'a BTreeMap<K, V>,
    edits: BTreeMap<K, V>,
    removed: BTreeSet<K>,
    len: usize,
}

impl<'a, K: Ord + Copy, V> MapDelta<'a, K, V> {
    fn new(base: &'a BTreeMap<K, V>) -> Self {
        Self {
            base,
            edits: BTreeMap::new(),
            removed: BTreeSet::new(),
            len: base.len(),
        }
    }

    fn get(&self, key: &K) -> Option<&V> {
        self.edits.get(key).or_else(|| {
            if self.removed.contains(key) {
                None
            } else {
                self.base.get(key)
            }
        })
    }

    fn insert(&mut self, key: K, value: V) {
        if self.get(&key).is_none() {
            self.len += 1;
        }
        self.removed.remove(&key);
        self.edits.insert(key, value);
    }

    fn remove(&mut self, key: &K) {
        if self.get(key).is_some() {
            self.len -= 1;
            self.edits.remove(key);
            self.removed.insert(*key);
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn into_edits(self) -> (BTreeMap<K, V>, BTreeSet<K>) {
        (self.edits, self.removed)
    }
}

fn commit_edits<K: Ord, V>(
    base: &mut BTreeMap<K, V>,
    (edits, removed): (BTreeMap<K, V>, BTreeSet<K>),
) {
    if base.is_empty() {
        // The first impact has no history to preserve: transfer the already
        // allocated tree instead of allocating and inserting every record twice.
        *base = edits;
        return;
    }
    for key in removed {
        base.remove(&key);
    }
    for (key, value) in edits {
        base.insert(key, value);
    }
}

/// Owned local changes, retained until geometry and fragment admission succeed.
/// Kept private to the crate so callers cannot commit against changed geometry.
pub(crate) struct PreparedFracture {
    revision: Revision,
    cells: (BTreeMap<DamageSite, CellFracture>, BTreeSet<DamageSite>),
    bonds: (BTreeMap<BondSite, BondFracture>, BTreeSet<BondSite>),
    pub outcome: FractureOutcome,
}

impl PreparedFracture {
    pub fn gate<'a>(
        &'a self,
        state: &'a FractureState,
        space: DamageSpace,
    ) -> impl crate::BondGate + 'a {
        struct Gate<'a> {
            prepared: &'a PreparedFracture,
            state: &'a FractureState,
            space: DamageSpace,
        }
        impl crate::BondGate for Gate<'_> {
            fn carries(&self, bond: BondKey) -> bool {
                let site = BondSite {
                    space: self.space,
                    bond,
                };
                if let Some(record) = self.prepared.bonds.0.get(&site) {
                    !record.is_broken()
                } else {
                    self.prepared.bonds.1.contains(&site) || !self.state.is_broken(site)
                }
            }
        }
        Gate {
            prepared: self,
            state,
            space,
        }
    }
}

/// Fixed-point scale: one whole unit is 1000.
///
/// Deliberately not `engine_mechanics::Milli`. That type lives downstream of
/// this crate, and fracture state has to be storable beside fragments whether a
/// world opted into mechanics or not.
const MILLI: i64 = 1_000;

/// Integrity an intact bond starts with.
///
/// A scale constant, not a material property: a bond's strength is its
/// [`FracturePolicy`] capacity, and integrity is the fraction of that capacity
/// still unspent. Keeping the scale uniform is what lets one bond accumulate
/// damage from several impacts that loaded it in different modes.
pub const BOND_INTEGRITY: DamageAmount = DamageAmount(1_000);

/// How much a cell's deposit is raised facing the impulse and lowered facing
/// away, in thousandths. The factor spans `[1 - gain, 1 + gain]`.
const DIRECTIONAL_GAIN_MILLI: i64 = 600;

/// Extra deposit at a free surface, in thousandths.
///
/// A cell with an exposed face has nowhere to pass the energy on to, so it keeps
/// what would have travelled onward. This is why the *back* of a struck wall
/// breaks out rather than only the face that was hit, and it is the main source
/// of an irregular break surface rather than a clean shell.
const SPALL_GAIN_MILLI: i64 = 500;

/// Extra load per already-broken neighbouring bond, in thousandths.
///
/// The crack-tip rule, and the reason a repeated hit concentrates instead of
/// re-damaging a fresh ring: a bond beside existing damage carries more of the
/// next hit than a bond in sound material.
const CONCENTRATION_GAIN_MILLI: i64 = 350;

/// Ceiling on the concentration multiplier, in thousandths. Without it a bond
/// surrounded by damage would break to an arbitrarily small load.
const MAX_CONCENTRATION_MILLI: i64 = 3_000;

/// The fatigue floor for bonds, in thousandths of [`BOND_INTEGRITY`].
///
/// A load this small is not recorded at all. Two things come out of that, and
/// both are wanted. The state stays genuinely sparse — one hit otherwise writes
/// an entry for every bond it merely brushed, which is most of the bonds inside
/// its reach and almost none of the damage. And an arbitrarily weak hit,
/// repeated forever, never breaks anything, which is what real materials do:
/// below a threshold, cycling does not accumulate.
const MIN_BOND_LOSS_MILLI: i64 = 20;

/// The same floor for cells, in thousandths of the crush threshold.
const MIN_CELL_ENERGY_MILLI: i64 = 20;

/// How far, in cells, the seed scan looks for material around the impact origin.
///
/// An impact usually stops *at* a surface, so its origin is often just outside
/// the solid it hit. Two cells is enough to find the struck face without
/// reaching through a thin wall into whatever is behind it.
const SEED_RADIUS_CELLS: i32 = 2;

/// How much further than the reach, in multiples, energy may travel *through
/// material* to arrive somewhere.
///
/// Attenuation is by straight-line distance, but a cell is only reached by
/// walking faces, and a face walk in a solid is a Manhattan path — up to
/// `sqrt(3)` times the straight line. Two covers that with margin while still
/// refusing a long detour around a void, which is what keeps local geometry in
/// the answer.
const DETOUR_ALLOWANCE: u32 = 2;

/// How an impact loads one bond.
///
/// The static-load counterpart is `engine_mechanics::LoadMode`, and the three
/// names line up one to one deliberately — but these are different questions
/// about the same face. `LoadMode` asks what a bond is carrying while standing
/// still; `BondMode` asks what an impact just did to it. Keeping them separate
/// keeps mechanics optional and downstream.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum BondMode {
    /// Pulled apart: the far cell is moving away faster than the near one.
    Tension,
    /// Driven together: the near cell is pushed into the far one.
    Compression,
    /// Loaded across the face rather than along it.
    Shear,
}

impl BondMode {
    /// Declaration order, which is also canonical iteration order.
    pub const ALL: [Self; 3] = [Self::Tension, Self::Compression, Self::Shear];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Tension => "tension",
            Self::Compression => "compression",
            Self::Shear => "shear",
        }
    }
}

/// The canonical identity of one face bond.
///
/// A face is shared by two cells, so a naive `(cell, FaceDir)` key names every
/// bond twice and two aliases of one crack would accumulate damage separately.
/// This type stores the **lower** cell plus the axis, which gives exactly one key
/// per bond, and keeps its fields private so that invariant cannot be bypassed.
/// Derived ordering is `(lower, axis)`, which is what makes `BTreeMap` output
/// canonical.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct BondKey {
    lower: CellPos,
    axis: Axis,
}

impl BondKey {
    /// The bond across `cell`'s `dir` face, or `None` when either cell would
    /// leave the representable cell-address space.
    pub fn new(cell: CellPos, dir: FaceDir) -> Option<Self> {
        let lower = if dir.is_positive() {
            cell
        } else {
            cell.checked_step(dir)?
        };
        // Proves the upper cell is representable, so `upper()` cannot overflow.
        lower.checked_step(positive_dir(dir.axis()))?;
        Some(Self {
            lower,
            axis: dir.axis(),
        })
    }

    /// The bond along `axis` from `lower`, or `None` when the upper cell would
    /// leave the representable cell-address space.
    ///
    /// The form a persisted bond is rebuilt from: the fields stay private, so a
    /// file cannot smuggle in a key that breaks the one-name-per-face invariant.
    pub fn along(lower: CellPos, axis: Axis) -> Option<Self> {
        Self::new(lower, positive_dir(axis))
    }

    /// The bond between two face-adjacent cells, or `None` when they are not
    /// face neighbours.
    pub fn between(a: CellPos, b: CellPos) -> Option<Self> {
        FaceDir::ALL
            .into_iter()
            .find(|dir| a.checked_step(*dir) == Some(b))
            .and_then(|dir| Self::new(a, dir))
    }

    /// The lower of the two cells, in canonical axis order.
    pub const fn lower(self) -> CellPos {
        self.lower
    }

    /// The upper of the two cells. Never overflows: see [`BondKey::new`].
    pub const fn upper(self) -> CellPos {
        self.lower.step_axis(self.axis, 1)
    }

    pub const fn axis(self) -> Axis {
        self.axis
    }

    /// Both cells, lower first.
    pub const fn cells(self) -> [CellPos; 2] {
        [self.lower(), self.upper()]
    }
}

impl fmt::Display for BondKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let c = self.lower;
        write!(f, "({},{},{}){:?}", c.x, c.y, c.z, self.axis)
    }
}

/// The positive face direction along an axis.
const fn positive_dir(axis: Axis) -> FaceDir {
    match axis {
        Axis::X => FaceDir::PosX,
        Axis::Y => FaceDir::PosY,
        Axis::Z => FaceDir::PosZ,
    }
}

/// Where a bond lives, including which coordinate space.
///
/// The cell-level counterpart is [`DamageSite`], reused rather than duplicated:
/// a place in a space is one idea.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct BondSite {
    pub space: DamageSpace,
    pub bond: BondKey,
}

/// Remaining integrity of one bond. Absence from the store means intact.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BondFracture {
    pub site: BondSite,
    /// Snapshot of the lower cell's material, so a material replacement can
    /// deliberately reset incompatible fracture state instead of inheriting it.
    pub material: MaterialId,
    /// Out of [`BOND_INTEGRITY`]. Zero is a broken bond — a crack that persists.
    pub integrity: DamageAmount,
}

impl BondFracture {
    pub const fn is_broken(&self) -> bool {
        self.integrity.0 == 0
    }
}

/// Accumulated absorbed energy at one cell. Absence from the store means
/// untouched, which is how an unhit world allocates nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CellFracture {
    pub site: DamageSite,
    pub material: MaterialId,
    pub energy: DamageAmount,
}

/// One homogeneous material's fracture numbers.
///
/// Every value is a tuning constant, not a physical claim. The relationships
/// between them are what matter: a material weaker in tension than in
/// compression sheds its back face before its struck face, and one whose crush
/// threshold is far above its bond capacities cracks apart before it powders.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FractureProfile {
    /// Mass of one occupied cell, in thousandths.
    ///
    /// Carried because it is authored data and cheaper to have from the start
    /// than to retrofit. Fracture does not read it: fragment mass already has
    /// its own reference policy, and giving density a second job here would be
    /// two models of the same number.
    pub density_milli: i64,
    /// Thousandths of the local energy this material soaks up rather than
    /// turning into cracks.
    ///
    /// Used exactly once, as the complement of brittleness in bond loading.
    /// Raising it anywhere else would make the material quietly twice as durable
    /// as its numbers say — the mistake `engine_mechanics` records for toughness
    /// in applied damage, in the same shape.
    pub toughness_milli: i64,
    /// Accumulated absorbed energy that destroys a cell outright.
    pub crush: DamageAmount,
    /// Bond capacity when pulled apart. Lowest, for most real solids.
    pub tensile: DamageAmount,
    /// Bond capacity when driven together. Highest.
    pub compressive: DamageAmount,
    /// Bond capacity across the face.
    pub shear: DamageAmount,
}

impl FractureProfile {
    /// The one baseline profile DROP 0006 develops destruction against.
    ///
    /// It is called a reference solid rather than concrete, masonry or steel
    /// because nothing here is tuned for realism yet, and naming it after a real
    /// material would invite exactly the comparison this pass is not ready for.
    /// The ordering is the only deliberate part: weak in tension, strong in
    /// compression, shear between them, and a crush threshold well above all
    /// three so that material cracks before it vanishes.
    pub const REFERENCE_SOLID: Self = Self {
        density_milli: 2_400,
        toughness_milli: 350,
        crush: DamageAmount(6_000),
        tensile: DamageAmount(180),
        compressive: DamageAmount(1_100),
        shear: DamageAmount(420),
    };

    /// Capacity against one mode.
    pub const fn capacity(&self, mode: BondMode) -> DamageAmount {
        match mode {
            BondMode::Tension => self.tensile,
            BondMode::Compression => self.compressive,
            BondMode::Shear => self.shear,
        }
    }

    /// Thousandths of local energy that becomes bond load. The complement of
    /// toughness, clamped so an absurd profile cannot produce a negative load.
    const fn brittleness_milli(&self) -> i64 {
        if self.toughness_milli <= 0 {
            MILLI
        } else if self.toughness_milli >= MILLI {
            0
        } else {
            MILLI - self.toughness_milli
        }
    }
}

/// What a material does under impact.
///
/// The seam DROP 0005 proved for damage, in the same shape: the trait is defined
/// here with a single uniform reference implementation beside it, so a
/// material-aware version is a new implementation downstream rather than a
/// change to this contract.
pub trait FracturePolicy: Send + Sync {
    /// The profile governing one material.
    fn profile(&self, material: MaterialId) -> FractureProfile;
    fn surface_gain_milli(&self, _space: DamageSpace) -> i64 {
        SPALL_GAIN_MILLI
    }
    fn erase_unbonded(&self) -> bool {
        true
    }
}

/// One profile for every material, by construction.
///
/// There is no table to key by material, so this policy *cannot* express
/// material diversity. That is the point for DROP 0006.0: destruction is being
/// developed against one homogeneous solid, and a policy that could accidentally
/// become material-aware would make it impossible to tell a fracture-model
/// problem from a tuning problem.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BaselineFracturePolicy {
    profile: FractureProfile,
}

/// The historical fixture stays reproducible. The coherent model treats broken
/// connections as separation, not vanished mass, and removes the all-surface
/// amplification on native fragments. It uses one tougher reference solid: a
/// wider gap between cracking and crushing lets separated chunks survive hits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FractureModel {
    #[default]
    Historical,
    Coherent,
}
impl FracturePolicy for FractureModel {
    fn profile(&self, _: MaterialId) -> FractureProfile {
        let mut profile = FractureProfile::REFERENCE_SOLID;
        if *self == Self::Coherent {
            profile.crush = DamageAmount(12000);
            profile.toughness_milli = 650;
        }
        profile
    }
    fn surface_gain_milli(&self, space: DamageSpace) -> i64 {
        if *self == Self::Coherent && matches!(space, DamageSpace::FragmentLocal(_)) {
            0
        } else {
            SPALL_GAIN_MILLI
        }
    }
    fn erase_unbonded(&self) -> bool {
        *self == Self::Historical
    }
}

/// Worker-owned sparse delta. Only changed sites are copied back to authority.
#[derive(Clone, Debug)]
pub(crate) struct FracturePatch {
    revision: Revision,
    cells: Vec<(DamageSite, Option<CellFracture>)>,
    bonds: Vec<(BondSite, Option<BondFracture>)>,
}
impl FractureState {
    pub(crate) fn snapshot_space(&self, space: DamageSpace, max: usize) -> Option<Self> {
        let mut result = Self {
            revision: self.revision,
            ..Self::new()
        };
        for r in self.cells_in(space) {
            if result.cells.len() >= max {
                return None;
            }
            result.cells.insert(r.site, r);
        }
        for r in self.bonds_in(space) {
            if result.cells.len() + result.bonds.len() >= max {
                return None;
            }
            result.bonds.insert(r.site, r);
        }
        Some(result)
    }
    pub(crate) fn patch_from(&self, before: &Self) -> FracturePatch {
        let cells = before
            .cells
            .keys()
            .chain(self.cells.keys())
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|k| before.cells.get(k) != self.cells.get(k))
            .map(|k| (k, self.cells.get(&k).copied()))
            .collect();
        let bonds = before
            .bonds
            .keys()
            .chain(self.bonds.keys())
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|k| before.bonds.get(k) != self.bonds.get(k))
            .map(|k| (k, self.bonds.get(&k).copied()))
            .collect();
        FracturePatch {
            revision: before.revision,
            cells,
            bonds,
        }
    }
    pub(crate) fn patch_fits(&self, patch: &FracturePatch, limits: FractureLimits) -> bool {
        if patch.revision != self.revision {
            return false;
        }
        let cells = patch
            .cells
            .iter()
            .try_fold(self.cells.len(), |count, (key, value)| {
                count
                    .checked_sub(usize::from(self.cells.contains_key(key)))?
                    .checked_add(usize::from(value.is_some()))
            });
        let bonds = patch
            .bonds
            .iter()
            .try_fold(self.bonds.len(), |count, (key, value)| {
                count
                    .checked_sub(usize::from(self.bonds.contains_key(key)))?
                    .checked_add(usize::from(value.is_some()))
            });
        cells.is_some_and(|count| count <= limits.max_cell_entries)
            && bonds.is_some_and(|count| count <= limits.max_bond_entries)
    }
    pub(crate) fn apply_patch(&mut self, patch: FracturePatch) {
        debug_assert_eq!(patch.revision, self.revision);
        for (k, v) in patch.cells {
            match v {
                Some(v) => {
                    self.cells.insert(k, v);
                }
                None => {
                    self.cells.remove(&k);
                }
            }
        }
        for (k, v) in patch.bonds {
            match v {
                Some(v) => {
                    self.bonds.insert(k, v);
                }
                None => {
                    self.bonds.remove(&k);
                }
            }
        }
        self.revision.bump();
    }
}

impl BaselineFracturePolicy {
    pub const fn new(profile: FractureProfile) -> Self {
        Self { profile }
    }

    /// The baseline reference solid.
    pub const REFERENCE: Self = Self::new(FractureProfile::REFERENCE_SOLID);

    pub const fn baseline(&self) -> FractureProfile {
        self.profile
    }
}

impl Default for BaselineFracturePolicy {
    fn default() -> Self {
        Self::REFERENCE
    }
}

impl FracturePolicy for BaselineFracturePolicy {
    fn profile(&self, _material: MaterialId) -> FractureProfile {
        self.profile
    }
}

/// Why an impact could not be built from an event.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FractureImpactError {
    /// A coordinate or impulse component was not finite, or was too large to
    /// quantise into the fixed-point domain.
    Unquantisable,
}

impl fmt::Display for FractureImpactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unquantisable => write!(
                f,
                "impact geometry is not finite or too large for fixed-point quantisation"
            ),
        }
    }
}

impl std::error::Error for FractureImpactError {}

/// One localised impact, in integer form.
///
/// Built once from a [`DamageEvent`] and then never touched by floating point
/// again. Everything that varies between impacts is here; everything that varies
/// between materials is in [`FracturePolicy`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FractureImpact {
    space: DamageSpace,
    /// Where the energy went in, in milli-cells of the target space.
    origin_milli: [i64; 3],
    /// Unit direction of travel in milli, or zero when the event said nothing
    /// about direction. Zero means isotropic, and the model then uses the radial
    /// direction at each bond instead of a special case.
    direction_milli: [i64; 3],
    /// Energy deposited at the origin, in the same abstract currency as
    /// [`DamageAmount`].
    energy: DamageAmount,
    /// Straight-line reach in cells. At least one.
    reach_cells: u32,
    seed_radius_cells: i32,
}

impl FractureImpact {
    /// An opt-in radial pressure deposit. Seeds all occupied cells within a
    /// bounded sphere, including disconnected surfaces across air. This is a
    /// gameplay blast field, without occlusion or fluid/shock-wave simulation.
    /// Unknown cells in the sphere make the normal transaction inconclusive.
    pub fn radial_blast(
        space: DamageSpace,
        center: GlobalPos,
        radius: u32,
        energy: DamageAmount,
    ) -> Option<Self> {
        if !(1..=8).contains(&radius) {
            return None;
        }
        let event = DamageEvent::new(
            crate::DamageEventId::new(0, 0),
            space,
            None,
            DamageVolume::Sphere {
                center,
                radius: f64::from(radius),
                falloff: crate::DamageFalloff::Linear,
            },
            energy,
            None,
        )
        .ok()?;
        let mut impact = Self::from_event(&event, Some([0.0; 3])).ok()?;
        impact.seed_radius_cells = radius as i32;
        Some(impact)
    }

    /// Build an impact from an event whose impulse is already in the event's own
    /// coordinate space.
    ///
    /// `impulse_target` exists because `DamageEvent::impulse_world` is **world**
    /// space while a fragment-local event's volume is not, and the engine stores
    /// fragment rotation opaquely on purpose. A host that knows its quaternion
    /// convention rotates the impulse and passes it here; for a static-world
    /// event, [`FractureImpact::from_static_event`] does it directly.
    ///
    /// When no impulse is given, the direction falls back to `source_world`
    /// pointing at the impact centre, and failing that to isotropic.
    pub fn from_event(
        event: &DamageEvent,
        impulse_target: Option<[f64; 3]>,
    ) -> Result<Self, FractureImpactError> {
        let centre = volume_centre_milli(event.volume).ok_or(FractureImpactError::Unquantisable)?;
        let direction = match impulse_target {
            Some(vector) => quantise_direction(vector).ok_or(FractureImpactError::Unquantisable)?,
            None => match event.source_world {
                // Pointing from where the hit came from towards where it landed
                // is the best statement of direction an event without an impulse
                // can make.
                Some(source) => {
                    let source_milli =
                        quantise_point(source).ok_or(FractureImpactError::Unquantisable)?;
                    unit_milli(sub3(centre, source_milli))
                }
                None => [0; 3],
            },
        };
        Ok(Self {
            space: event.space,
            origin_milli: centre,
            direction_milli: direction,
            energy: event.strength,
            reach_cells: volume_reach_cells(event.volume),
            seed_radius_cells: SEED_RADIUS_CELLS,
        })
    }

    /// Build from a static-world event, whose volume and impulse share one space.
    pub fn from_static_event(event: &DamageEvent) -> Result<Self, FractureImpactError> {
        Self::from_event(event, event.impulse_world.map(|i| i.vector()))
    }

    /// Replace the reach derived from the event's volume.
    ///
    /// The volume says how far the event *delivers damage*; reach says how far
    /// the fracture field extends, and a host may legitimately want cracks to
    /// run further than cells are deleted.
    #[must_use]
    pub const fn with_reach_cells(mut self, reach_cells: u32) -> Self {
        self.reach_cells = if reach_cells == 0 { 1 } else { reach_cells };
        self
    }

    /// Replace the deposited energy.
    #[must_use]
    pub const fn with_energy(mut self, energy: DamageAmount) -> Self {
        self.energy = energy;
        self
    }

    pub const fn space(self) -> DamageSpace {
        self.space
    }

    pub const fn energy(self) -> DamageAmount {
        self.energy
    }

    pub const fn reach_cells(self) -> u32 {
        self.reach_cells
    }

    /// The impact origin in milli-cells of the target space.
    pub const fn origin_milli(self) -> [i64; 3] {
        self.origin_milli
    }

    /// The unit direction of travel in milli, or zero for isotropic.
    pub const fn direction_milli(self) -> [i64; 3] {
        self.direction_milli
    }

    /// The cell the origin falls in, for seeding and diagnostics.
    pub fn origin_cell(self) -> CellPos {
        CellPos::new(
            milli_to_cell(self.origin_milli[0]),
            milli_to_cell(self.origin_milli[1]),
            milli_to_cell(self.origin_milli[2]),
        )
    }
}

/// Everything the model reads about one coordinate space.
///
/// Bundled, and carrying the space, for the reason `DamageSource` carries it: a
/// fragment-local impact evaluated against the static world would otherwise
/// produce confident results labelled with the wrong space.
#[derive(Clone, Copy)]
pub struct FractureScene<'a> {
    space: DamageSpace,
    cells: &'a dyn CellSource,
    support: &'a dyn SupportSource,
    residency: &'a dyn Residency,
}

impl<'a> FractureScene<'a> {
    pub const fn static_world(
        cells: &'a dyn CellSource,
        support: &'a dyn SupportSource,
        residency: &'a dyn Residency,
    ) -> Self {
        Self {
            space: DamageSpace::StaticWorld,
            cells,
            support,
            residency,
        }
    }

    pub const fn fragment(
        fragment: FragmentId,
        cells: &'a dyn CellSource,
        support: &'a dyn SupportSource,
        residency: &'a dyn Residency,
    ) -> Self {
        Self {
            space: DamageSpace::FragmentLocal(fragment),
            cells,
            support,
            residency,
        }
    }

    /// A whole fragment, in its own local coordinates.
    ///
    /// Nothing in a fragment is anchored — a fragment has no foundation, which is
    /// what it means to have come loose — and a fragment held in memory is
    /// entirely resident, so there is no unknown space inside one.
    pub fn of_fragment(fragment: &'a Fragment) -> Self {
        static NO_SUPPORT: engine_core::NoSupport = engine_core::NoSupport;
        static ALL_RESIDENT: crate::AllResident = crate::AllResident;
        Self::fragment(fragment.id, fragment, &NO_SUPPORT, &ALL_RESIDENT)
    }

    pub const fn space(&self) -> DamageSpace {
        self.space
    }

    pub const fn cells(&self) -> &'a dyn CellSource {
        self.cells
    }

    pub const fn support(&self) -> &'a dyn SupportSource {
        self.support
    }

    pub const fn residency(&self) -> &'a dyn Residency {
        self.residency
    }

    fn target(&self, cell: CellPos, material: MaterialId) -> DamageTarget {
        match self.space {
            DamageSpace::StaticWorld => DamageTarget::StaticCell { cell, material },
            DamageSpace::FragmentLocal(fragment) => DamageTarget::FragmentCell {
                fragment,
                cell,
                material,
            },
        }
    }
}

/// Work and memory ceilings, bounded separately.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FractureLimits {
    /// Work: occupied cells the propagation may visit.
    pub max_cells_visited: u64,
    /// Work: distinct bonds the propagation may load.
    pub max_bonds_considered: u64,
    /// Memory: sparse cell entries the state may hold.
    pub max_cell_entries: usize,
    /// Memory: sparse bond entries the state may hold.
    pub max_bond_entries: usize,
}

impl FractureLimits {
    pub const UNLIMITED: Self = Self {
        max_cells_visited: u64::MAX,
        max_bonds_considered: u64::MAX,
        max_cell_entries: usize::MAX,
        max_bond_entries: usize::MAX,
    };

    pub const fn new(
        max_cells_visited: u64,
        max_bonds_considered: u64,
        max_cell_entries: usize,
        max_bond_entries: usize,
    ) -> Self {
        Self {
            max_cells_visited,
            max_bonds_considered,
            max_cell_entries,
            max_bond_entries,
        }
    }
}

/// Why nothing could be concluded about an impact.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FractureDefer {
    /// The propagation outgrew `max_cells_visited`.
    CellBudgetSpent,
    /// The propagation outgrew `max_bonds_considered`.
    BondBudgetSpent,
}

/// What the propagation cost, so the model can be judged on evidence.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct FractureMeasurement {
    pub cells_visited: u64,
    pub bonds_considered: u64,
    /// Furthest graph distance, in face steps, that energy actually travelled.
    pub max_steps: u32,
    /// Lookups spent asking the existing state where the cracks already are.
    pub concentration_lookups: u64,
}

/// One bond's share of an impact.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BondLoad {
    pub bond: BondKey,
    pub material: MaterialId,
    /// Dominant mode, for diagnostics. Capacity itself is blended, so this is
    /// a label rather than the number the model used.
    pub mode: BondMode,
    /// Energy the bond actually took.
    pub load: DamageAmount,
    /// Integrity it will lose, out of [`BOND_INTEGRITY`].
    pub loss: DamageAmount,
}

/// One cell's share of an impact.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CellDeposit {
    pub cell: CellPos,
    pub material: MaterialId,
    /// Energy absorbed here.
    pub energy: DamageAmount,
    /// Straight-line distance from the impact origin, in milli-cells. Kept for
    /// diagnostics: it is the quantity the whole falloff is a function of.
    pub distance_milli: i64,
}

/// A complete, conclusive evaluation. Canonical, and applicable exactly once.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FractureLoad {
    /// The state revision this was computed against.
    ///
    /// Checked on apply, so a result computed on a worker thread cannot be
    /// committed onto state that has moved under it.
    pub state: Revision,
    pub space: DamageSpace,
    /// Ascending by cell.
    pub cells: Vec<CellDeposit>,
    /// Ascending by bond key.
    pub bonds: Vec<BondLoad>,
    pub measurement: FractureMeasurement,
}

impl FractureLoad {
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty() && self.bonds.is_empty()
    }
}

/// The answer. Only [`Loaded`](FractureEvaluation::Loaded) is a conclusion.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FractureEvaluation {
    Loaded(FractureLoad),
    /// The propagation reached data the engine does not have, close enough to
    /// the impact to matter.
    Indeterminate {
        required_regions: BTreeSet<RegionPos>,
    },
    /// A work budget was spent before an answer existed.
    Deferred {
        reason: FractureDefer,
    },
}

impl FractureEvaluation {
    /// The load, if this is a conclusion at all.
    ///
    /// Exists so no caller writes the "may I act on this" condition itself.
    pub fn load(&self) -> Option<&FractureLoad> {
        match self {
            Self::Loaded(load) => Some(load),
            _ => None,
        }
    }

    pub const fn is_conclusive(&self) -> bool {
        matches!(self, Self::Loaded(_))
    }
}

/// Evaluating an impact against the wrong coordinate space.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FractureSpaceMismatch {
    pub impact: DamageSpace,
    pub scene: DamageSpace,
}

impl fmt::Display for FractureSpaceMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "fracture impact space {:?} does not match scene space {:?}",
            self.impact, self.scene
        )
    }
}

impl std::error::Error for FractureSpaceMismatch {}

/// Why a transaction was refused outright.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FractureRefusal {
    /// The state moved between evaluation and application.
    StaleEvaluation {
        expected: Revision,
        found: Revision,
    },
    SpaceMismatch(FractureSpaceMismatch),
    CellEntryLimit {
        required: usize,
        limit: usize,
    },
    BondEntryLimit {
        required: usize,
        limit: usize,
    },
}

impl fmt::Display for FractureRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleEvaluation { expected, found } => write!(
                f,
                "fracture evaluation was computed against {expected} but state is {found}"
            ),
            Self::SpaceMismatch(mismatch) => mismatch.fmt(f),
            Self::CellEntryLimit { required, limit } => write!(
                f,
                "fracture needs {required} sparse cell entries but limit is {limit}"
            ),
            Self::BondEntryLimit { required, limit } => write!(
                f,
                "fracture needs {required} sparse bond entries but limit is {limit}"
            ),
        }
    }
}

impl std::error::Error for FractureRefusal {}

/// What one accepted transaction changed.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct FractureOutcome {
    /// Bonds that went from intact to broken, canonical order. These are the
    /// cracks, and they persist.
    pub broken: Vec<BondSite>,
    /// Cells that failed outright, canonical order.
    ///
    /// Ordinary [`DamageTarget`]s: feed them to
    /// [`static_failure_batch`](crate::static_failure_batch) or
    /// [`fragment_after_failures`](crate::fragment_after_failures) exactly as a
    /// progressive-damage failure is fed.
    pub failed: Vec<DamageTarget>,
    /// How many of `failed` were crushed rather than fully unbonded.
    pub crushed: usize,
    pub cell_entries: usize,
    pub bond_entries: usize,
}

/// Sparse counters for a HUD or a report.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct FractureStats {
    pub cell_entries: usize,
    pub bond_entries: usize,
    pub broken_bonds: usize,
    pub revision: Revision,
}

/// A content checksum of fracture state, for benchmarks and persistence.
///
/// Covers every record and nothing else: the revision is excluded for the same
/// reason [`FractureState`]'s equality excludes it — two states holding the same
/// cracks are the same state however many transactions got there. That is what
/// lets a saved-and-reloaded crack field be checked against the one that was
/// written.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct FractureDigest {
    pub cell_entries: usize,
    pub bond_entries: usize,
    pub broken_bonds: usize,
    pub checksum: u64,
}

/// Every fracture record one region owns.
///
/// Ownership follows identity rather than inventing a second rule: a cell record
/// belongs to its cell's region, and a bond record belongs to its **lower**
/// cell's region, which is already the bond's canonical name.
///
/// A bond whose upper cell lives in a different region therefore leaves memory
/// with the lower one. That is the conservative direction: while the record is
/// away the face reads as intact, and an intact bond carries support, so a
/// structure stays standing rather than falling because part of it was evicted.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct RegionFracture {
    pub region: RegionPos,
    /// Canonical order, so the same damage always serialises the same way.
    pub cells: Vec<CellFracture>,
    pub bonds: Vec<BondFracture>,
}

impl RegionFracture {
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty() && self.bonds.is_empty()
    }

    pub fn len(&self) -> usize {
        self.cells.len() + self.bonds.len()
    }
}

/// What a hygiene sweep found and what it cost.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct FracturePrune {
    pub cells_dropped: usize,
    pub bonds_dropped: usize,
    /// Records looked at. The size of the state, never the size of the world.
    pub records_examined: u64,
}

impl FracturePrune {
    pub const fn dropped_anything(&self) -> bool {
        self.cells_dropped > 0 || self.bonds_dropped > 0
    }
}

/// A fragment-local cell back in world space, or `None` if that would leave the
/// representable address space.
fn add_cell(local: CellPos, origin: CellPos) -> Option<CellPos> {
    Some(CellPos::new(
        local.x.checked_add(origin.x)?,
        local.y.checked_add(origin.y)?,
        local.z.checked_add(origin.z)?,
    ))
}

/// Whether `region` owns this cell record.
fn owns_cell(region: RegionPos, site: DamageSite) -> bool {
    site.space == DamageSpace::StaticWorld && site.cell.region() == region
}

/// Whether `region` owns this bond record.
fn owns_bond(region: RegionPos, site: BondSite) -> bool {
    site.space == DamageSpace::StaticWorld && site.bond.lower().region() == region
}

/// Sparse, deterministic, persistent fracture state.
///
/// An untouched world allocates nothing: a cell with no absorbed energy has no
/// entry, and an intact bond has no entry. What is stored is exactly the damage.
#[derive(Clone, Default, Debug)]
pub struct FractureState {
    cells: BTreeMap<DamageSite, CellFracture>,
    bonds: BTreeMap<BondSite, BondFracture>,
    revision: Revision,
}

/// Fracture state compares by content, not by how it got there.
///
/// The revision is deliberately excluded, as `World`'s is: two states holding
/// the same cracks are the same state, and a determinism test that compared
/// transaction counts would be testing the test harness.
impl PartialEq for FractureState {
    fn eq(&self, other: &Self) -> bool {
        self.cells == other.cells && self.bonds == other.bonds
    }
}

impl Eq for FractureState {}

impl FractureState {
    pub fn new() -> Self {
        Self::default()
    }

    /// The revision an evaluation must have been computed against.
    pub const fn revision(&self) -> Revision {
        self.revision
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty() && self.bonds.is_empty()
    }

    pub fn cell_entries(&self) -> usize {
        self.cells.len()
    }

    pub fn bond_entries(&self) -> usize {
        self.bonds.len()
    }

    pub fn broken_bonds(&self) -> usize {
        self.bonds.values().filter(|b| b.is_broken()).count()
    }

    pub fn stats(&self) -> FractureStats {
        FractureStats {
            cell_entries: self.cell_entries(),
            bond_entries: self.bond_entries(),
            broken_bonds: self.broken_bonds(),
            revision: self.revision,
        }
    }

    /// A content checksum over every record, in canonical order.
    ///
    /// `BTreeMap` iteration is what makes it reproducible; nothing about the
    /// order a hit happened to visit cells in can reach it.
    pub fn digest(&self) -> FractureDigest {
        let mut hash = 0xcbf29ce484222325u64;
        let mut feed = |value: u64| {
            for byte in value.to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        };
        for record in self.cells.values() {
            feed(space_tag(record.site.space));
            feed(record.site.cell.x as i64 as u64);
            feed(record.site.cell.y as i64 as u64);
            feed(record.site.cell.z as i64 as u64);
            feed(u64::from(record.material.0));
            feed(u64::from(record.energy.0));
        }
        for record in self.bonds.values() {
            feed(space_tag(record.site.space));
            let lower = record.site.bond.lower();
            feed(lower.x as i64 as u64);
            feed(lower.y as i64 as u64);
            feed(lower.z as i64 as u64);
            feed(record.site.bond.axis().index() as u64);
            feed(u64::from(record.material.0));
            feed(u64::from(record.integrity.0));
        }
        FractureDigest {
            cell_entries: self.cell_entries(),
            bond_entries: self.bond_entries(),
            broken_bonds: self.broken_bonds(),
            checksum: hash,
        }
    }

    pub fn cell(&self, site: DamageSite) -> Option<CellFracture> {
        self.cells.get(&site).copied()
    }

    pub fn bond(&self, site: BondSite) -> Option<BondFracture> {
        self.bonds.get(&site).copied()
    }

    /// Whether this bond has been broken. An absent bond is intact.
    pub fn is_broken(&self, site: BondSite) -> bool {
        self.bonds.get(&site).is_some_and(BondFracture::is_broken)
    }

    /// Remaining integrity, which for an absent bond is [`BOND_INTEGRITY`].
    pub fn integrity(&self, site: BondSite) -> DamageAmount {
        self.bonds
            .get(&site)
            .map_or(BOND_INTEGRITY, |record| record.integrity)
    }

    /// Cell records in canonical order.
    pub fn cells(&self) -> impl Iterator<Item = CellFracture> + '_ {
        self.cells.values().copied()
    }

    /// Bond records in canonical order.
    pub fn bonds(&self) -> impl Iterator<Item = BondFracture> + '_ {
        self.bonds.values().copied()
    }

    /// Space is the leading ordered key. A fragment query never walks damage
    /// belonging to the static world or another fragment.
    pub fn cells_in(&self, space: DamageSpace) -> impl Iterator<Item = CellFracture> + '_ {
        let start = DamageSite {
            space,
            cell: CellPos::new(i32::MIN, i32::MIN, i32::MIN),
        };
        self.cells
            .range(start..)
            .take_while(move |(site, _)| site.space == space)
            .map(|(_, record)| *record)
    }
    pub fn bonds_in(&self, space: DamageSpace) -> impl Iterator<Item = BondFracture> + '_ {
        let start = BondSite {
            space,
            bond: BondKey::along(CellPos::new(i32::MIN, i32::MIN, i32::MIN), Axis::X)
                .expect("minimum cell has a positive neighbour"),
        };
        self.bonds
            .range(start..)
            .take_while(move |(site, _)| site.space == space)
            .map(|(_, record)| *record)
    }

    /// Forget everything about one cell, including every bond touching it.
    ///
    /// Call this when geometry disappears by a path fracture did not drive — a
    /// carve, a detachment, a repaint. A bond to a cell that no longer exists is
    /// not a crack, it is a hole, and leaving the record would let stale
    /// integrity decide a later impact.
    pub fn forget_cell(&mut self, space: DamageSpace, cell: CellPos) -> usize {
        let mut removed = 0usize;
        if self.cells.remove(&DamageSite { space, cell }).is_some() {
            removed += 1;
        }
        for dir in FaceDir::ALL {
            if let Some(bond) = BondKey::new(cell, dir)
                && self.bonds.remove(&BondSite { space, bond }).is_some()
            {
                removed += 1;
            }
        }
        if removed > 0 {
            self.revision.bump();
        }
        removed
    }

    /// Which regions hold any static fracture state, in canonical order.
    ///
    /// What a streaming host iterates when deciding what to save.
    pub fn static_regions(&self) -> BTreeSet<RegionPos> {
        let mut regions = BTreeSet::new();
        for site in self.cells.keys() {
            if site.space == DamageSpace::StaticWorld {
                regions.insert(site.cell.region());
            }
        }
        for site in self.bonds.keys() {
            if site.space == DamageSpace::StaticWorld {
                regions.insert(site.bond.lower().region());
            }
        }
        regions
    }

    /// Read one region's static fracture state without removing it.
    ///
    /// For saving a region that stays resident. Canonical order, so the same
    /// damage always serialises to the same bytes.
    pub fn region_fracture(&self, region: RegionPos) -> RegionFracture {
        RegionFracture {
            region,
            cells: self
                .cells
                .values()
                .filter(|record| owns_cell(region, record.site))
                .copied()
                .collect(),
            bonds: self
                .bonds
                .values()
                .filter(|record| owns_bond(region, record.site))
                .copied()
                .collect(),
        }
    }

    /// Remove and return one region's static fracture state.
    ///
    /// For eviction: the damage leaves memory with the cells it describes, and
    /// [`restore_region`](Self::restore_region) puts it back byte for byte.
    pub fn take_static_region(&mut self, region: RegionPos) -> RegionFracture {
        let extracted = self.region_fracture(region);
        if extracted.is_empty() {
            return extracted;
        }
        for record in &extracted.cells {
            self.cells.remove(&record.site);
        }
        for record in &extracted.bonds {
            self.bonds.remove(&record.site);
        }
        self.revision.bump();
        extracted
    }

    /// Put a region's static fracture state back, atomically.
    ///
    /// Refused as a whole if it would exceed an entry ceiling, so a load can
    /// never leave half a region's damage in memory. A zero-amount cell record
    /// or a full-integrity bond record is dropped rather than stored: those mean
    /// "undamaged", and storing them would make the state's size depend on how
    /// it was written rather than on how much damage there is.
    pub fn restore_region(
        &mut self,
        records: RegionFracture,
        limits: FractureLimits,
    ) -> Result<(), FractureRefusal> {
        let mut staged_cells = MapDelta::new(&self.cells);
        let mut staged_bonds = MapDelta::new(&self.bonds);
        for record in records.cells {
            if record.energy == DamageAmount::ZERO {
                staged_cells.remove(&record.site);
            } else {
                staged_cells.insert(record.site, record);
            }
        }
        for record in records.bonds {
            if record.integrity >= BOND_INTEGRITY {
                staged_bonds.remove(&record.site);
            } else {
                staged_bonds.insert(record.site, record);
            }
        }
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
        let cells = staged_cells.into_edits();
        let bonds = staged_bonds.into_edits();
        commit_edits(&mut self.cells, cells);
        commit_edits(&mut self.bonds, bonds);
        self.revision.bump();
        Ok(())
    }

    /// Forget every record belonging to cells that have left.
    ///
    /// The bounded hygiene path, driven by an edit's own
    /// `EditOutcome::removed_cells` rather than by a sweep of the whole state.
    /// Call it whenever geometry disappears by a route fracture did not drive —
    /// a carve, a detachment, a collapse.
    pub fn forget_removed(
        &mut self,
        space: DamageSpace,
        cells: impl IntoIterator<Item = CellPos>,
    ) -> usize {
        cells
            .into_iter()
            .map(|cell| self.forget_cell(space, cell))
            .sum()
    }

    /// Drop every record that no longer describes the material it was recorded
    /// against.
    ///
    /// The belt-and-braces sweep, for after a bulk change whose extent a host
    /// did not track — a repaint, a region swap, a world reload. Cost is the
    /// size of the *state*, not the size of the world, and it is reported so the
    /// sweep can be judged on evidence rather than on comfort.
    ///
    /// A cell that is gone, or now made of something else, cannot carry the
    /// damage recorded for what used to be there.
    pub fn prune_against(&mut self, space: DamageSpace, cells: &dyn CellSource) -> FracturePrune {
        let mut prune = FracturePrune::default();
        let mut stale_cells = Vec::new();
        for (site, record) in &self.cells {
            prune.records_examined += 1;
            if site.space != space {
                continue;
            }
            if cells.material_at(site.cell) != Some(record.material) {
                stale_cells.push(*site);
            }
        }
        let mut stale_bonds = Vec::new();
        for (site, record) in &self.bonds {
            prune.records_examined += 1;
            if site.space != space {
                continue;
            }
            // A bond is named by its lower cell, so that is the material it was
            // recorded against; the upper cell only has to still be there.
            let lower_matches = cells.material_at(site.bond.lower()) == Some(record.material);
            let upper_present = cells.material_at(site.bond.upper()).is_some();
            if !lower_matches || !upper_present {
                stale_bonds.push(*site);
            }
        }
        for site in &stale_cells {
            self.cells.remove(site);
        }
        for site in &stale_bonds {
            self.bonds.remove(site);
        }
        prune.cells_dropped = stale_cells.len();
        prune.bonds_dropped = stale_bonds.len();
        if prune.dropped_anything() {
            self.revision.bump();
        }
        prune
    }

    /// Move a detached component's records out of the static world and into the
    /// fragment it became: DROP 0006.3.
    ///
    /// A plug's cracks are its cracks. Before this they were forgotten on
    /// detachment, which threw away real damage and made every fresh fragment
    /// pristine however hard it had been hit.
    ///
    /// Local coordinates are `world - source_origin`, so the records move without
    /// their geometry changing shape. A bond with **one** cell in the fragment is
    /// not moved and not kept: that face is where the separation happened, so the
    /// bond no longer exists on either side.
    ///
    /// Returns how many records moved. Call it after the detach transaction has
    /// been accepted; running it on a refused one would take damage off a wall
    /// that is still standing.
    pub fn adopt_into_fragment(&mut self, fragment: &Fragment) -> usize {
        let origin = fragment.source_origin;
        let local_cells: BTreeSet<CellPos> = fragment.occupied_cells().collect();
        let space = DamageSpace::FragmentLocal(fragment.id);
        let mut moved = 0usize;
        let mut world_cells = Vec::with_capacity(local_cells.len());

        for local in &local_cells {
            let Some(world) = add_cell(*local, origin) else {
                continue;
            };
            world_cells.push(world);

            if let Some(mut record) = self.cells.remove(&DamageSite {
                space: DamageSpace::StaticWorld,
                cell: world,
            }) {
                record.site = DamageSite {
                    space,
                    cell: *local,
                };
                self.cells.insert(record.site, record);
                moved += 1;
            }

            // Only the three positive axes: a bond is named once, by its lower
            // cell, so walking all six would visit each face twice.
            for axis in Axis::ALL {
                if !local_cells.contains(&local.step_axis(axis, 1)) {
                    continue;
                }
                let Some(world_bond) = BondKey::along(world, axis) else {
                    continue;
                };
                let Some(local_bond) = BondKey::along(*local, axis) else {
                    continue;
                };
                if let Some(mut record) = self.bonds.remove(&BondSite {
                    space: DamageSpace::StaticWorld,
                    bond: world_bond,
                }) {
                    record.site = BondSite {
                        space,
                        bond: local_bond,
                    };
                    self.bonds.insert(record.site, record);
                    moved += 1;
                }
            }
        }

        // Whatever is left at those world cells described faces the fragment
        // broke away from, so it describes nothing now.
        for world in world_cells {
            self.forget_cell(DamageSpace::StaticWorld, world);
        }
        if moved > 0 {
            self.revision.bump();
        }
        moved
    }

    /// Reassign a parent fragment's records to the children that replaced it.
    ///
    /// Children keep the parent's local frame — that is what makes re-fracture
    /// valid after arbitrary backend rotation — so coordinates do not move and
    /// only the space tag changes.
    ///
    /// A cell no child holds is dropped. A bond whose two cells ended up in
    /// **different** children is dropped too: that is the seam the object came
    /// apart along, and a bond across it is not a crack any more, it is a gap.
    ///
    /// Returns how many records survived the split.
    pub fn remap_fragment(&mut self, parent: FragmentId, children: &[Fragment]) -> usize {
        let parent_space = DamageSpace::FragmentLocal(parent);
        let owners: BTreeMap<CellPos, FragmentId> = children
            .iter()
            .flat_map(|child| child.occupied_cells().map(move |cell| (cell, child.id)))
            .collect();
        let owner_of = |cell: CellPos| owners.get(&cell).copied();

        let cell_sites: Vec<DamageSite> = self
            .cells_in(parent_space)
            .map(|record| record.site)
            .collect();
        let bond_sites: Vec<BondSite> = self
            .bonds_in(parent_space)
            .map(|record| record.site)
            .collect();
        let cells: Vec<CellFracture> = cell_sites
            .into_iter()
            .filter_map(|site| self.cells.remove(&site))
            .collect();
        let bonds: Vec<BondFracture> = bond_sites
            .into_iter()
            .filter_map(|site| self.bonds.remove(&site))
            .collect();
        let had = cells.len() + bonds.len();

        let mut kept = 0usize;
        for mut record in cells {
            let Some(child) = owner_of(record.site.cell) else {
                continue;
            };
            record.site.space = DamageSpace::FragmentLocal(child);
            self.cells.insert(record.site, record);
            kept += 1;
        }
        for mut record in bonds {
            let [lower, upper] = record.site.bond.cells();
            let Some(child) = owner_of(lower) else {
                continue;
            };
            if owner_of(upper) != Some(child) {
                continue;
            }
            record.site.space = DamageSpace::FragmentLocal(child);
            self.bonds.insert(record.site, record);
            kept += 1;
        }
        if had > 0 {
            self.revision.bump();
        }
        kept
    }

    /// Forget everything in one coordinate space, as when a fragment dies.
    pub fn forget_space(&mut self, space: DamageSpace) -> usize {
        let cells: Vec<_> = self.cells_in(space).map(|record| record.site).collect();
        let bonds: Vec<_> = self.bonds_in(space).map(|record| record.site).collect();
        let removed = cells.len() + bonds.len();
        for site in cells {
            self.cells.remove(&site);
        }
        for site in bonds {
            self.bonds.remove(&site);
        }
        if removed > 0 {
            self.revision.bump();
        }
        removed
    }

    /// Commit one evaluated load atomically.
    ///
    /// Either every delta lands and the revision moves, or nothing does. The
    /// scene is read again here — for the material of a failed cell, for the
    /// neighbours of a newly broken bond, and for anchors — so it must be the
    /// same geometry the load was evaluated against; the revision check is what
    /// catches the state half of that.
    pub fn apply(
        &mut self,
        load: &FractureLoad,
        scene: FractureScene<'_>,
        policy: &dyn FracturePolicy,
        limits: FractureLimits,
    ) -> Result<FractureOutcome, FractureRefusal> {
        let prepared = self.prepare(load, scene, policy, limits)?;
        self.commit_prepared(prepared)
    }

    pub(crate) fn prepare(
        &self,
        load: &FractureLoad,
        scene: FractureScene<'_>,
        policy: &dyn FracturePolicy,
        limits: FractureLimits,
    ) -> Result<PreparedFracture, FractureRefusal> {
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
        let mut staged_cells = MapDelta::new(&self.cells);
        let mut staged_bonds = MapDelta::new(&self.bonds);
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
        if policy.erase_unbonded() {
            failed_cells.extend(unbonded.iter().copied());
        }
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

        let outcome = FractureOutcome {
            broken,
            failed,
            crushed,
            cell_entries: staged_cells.len(),
            bond_entries: staged_bonds.len(),
        };
        Ok(PreparedFracture {
            revision: self.revision,
            cells: staged_cells.into_edits(),
            bonds: staged_bonds.into_edits(),
            outcome,
        })
    }

    pub(crate) fn commit_prepared(
        &mut self,
        prepared: PreparedFracture,
    ) -> Result<FractureOutcome, FractureRefusal> {
        if prepared.revision != self.revision {
            return Err(FractureRefusal::StaleEvaluation {
                expected: prepared.revision,
                found: self.revision,
            });
        }
        commit_edits(&mut self.cells, prepared.cells);
        commit_edits(&mut self.bonds, prepared.bonds);
        self.revision.bump();
        Ok(prepared.outcome)
    }
}

/// Evaluate one impact. Pure: nothing is mutated, including `state`.
///
/// `state` is read for two things only — which bonds are already broken, so the
/// next hit concentrates where the last one left damage, and the revision the
/// result must be committed against.
pub fn evaluate_fracture(
    impact: &FractureImpact,
    scene: FractureScene<'_>,
    policy: &dyn FracturePolicy,
    state: &FractureState,
    limits: FractureLimits,
) -> Result<FractureEvaluation, FractureSpaceMismatch> {
    if impact.space != scene.space() {
        return Err(FractureSpaceMismatch {
            impact: impact.space,
            scene: scene.space(),
        });
    }

    let mut measurement = FractureMeasurement::default();
    let mut required: BTreeSet<RegionPos> = BTreeSet::new();
    let reach_milli = i64::from(impact.reach_cells).saturating_mul(MILLI);
    let max_steps = impact.reach_cells.saturating_mul(DETOUR_ALLOWANCE);

    // Pass 1: walk the material, depositing energy. Walking rather than
    // sampling a sphere is what puts local geometry in the answer — energy
    // cannot cross a void, and a cell only reachable the long way round is out
    // of range even if it is close.
    let mut deposits: BTreeMap<CellPos, (MaterialId, i64, i64)> = BTreeMap::new();
    let mut queue: VecDeque<(CellPos, u32)> = VecDeque::new();
    let mut visited: BTreeSet<CellPos> = BTreeSet::new();

    for seed in seed_cells(impact, scene, &mut required) {
        if visited.insert(seed) {
            queue.push_back((seed, 0));
        }
    }

    while let Some((cell, steps)) = queue.pop_front() {
        if measurement.cells_visited >= limits.max_cells_visited {
            return Ok(FractureEvaluation::Deferred {
                reason: FractureDefer::CellBudgetSpent,
            });
        }
        measurement.cells_visited += 1;
        measurement.max_steps = measurement.max_steps.max(steps);

        let Some(material) = scene.cells.material_at(cell) else {
            continue;
        };
        let offset = sub3(cell_centre_milli(cell), impact.origin_milli);
        let distance_milli = length_milli(offset);
        if distance_milli >= reach_milli {
            // Beyond the reach there is nothing to deposit and nothing past it
            // can matter, so the walk stops rather than paying for the shell.
            continue;
        }

        let attenuation = (reach_milli - distance_milli)
            .saturating_mul(MILLI)
            .checked_div(reach_milli)
            .unwrap_or(MILLI);
        let directional =
            MILLI + DIRECTIONAL_GAIN_MILLI * cosine_milli(offset, impact.direction_milli) / MILLI;
        let mut exposed = false;
        let mut neighbours: Vec<(CellPos, u32)> = Vec::new();
        for dir in FaceDir::ALL {
            let Some(neighbour) = cell.checked_step(dir) else {
                continue;
            };
            if !scene.residency.is_resident(neighbour) {
                // Unknown is not empty. Only unknown space close enough to
                // carry a deposit can change the answer; beyond the reach it
                // cannot, so reporting it would defer every impact near a
                // region boundary for nothing.
                let n_distance =
                    length_milli(sub3(cell_centre_milli(neighbour), impact.origin_milli));
                if n_distance < reach_milli {
                    required.insert(neighbour.region());
                }
                continue;
            }
            if scene.cells.material_at(neighbour).is_none() {
                exposed = true;
                continue;
            }
            if steps < max_steps {
                neighbours.push((neighbour, steps + 1));
            }
        }
        let spall = if exposed {
            MILLI
                + policy
                    .surface_gain_milli(impact.space)
                    .clamp(0, SPALL_GAIN_MILLI)
        } else {
            MILLI
        };
        let energy = scale_milli(
            i64::from(impact.energy.0),
            &[attenuation, directional, spall],
        );
        // The deposit is still recorded for the bond pass even when it is below
        // the reporting floor: a bond's load is the *gradient* across it, so the
        // energy at the cold side of a crack is part of the answer even when it
        // is too small to be worth storing.
        deposits.insert(cell, (material, energy, distance_milli));

        for (neighbour, next_steps) in neighbours {
            if visited.insert(neighbour) {
                queue.push_back((neighbour, next_steps));
            }
        }
    }

    if !required.is_empty() {
        return Ok(FractureEvaluation::Indeterminate {
            required_regions: required,
        });
    }

    // Pass 2: load the bonds. A bond is loaded by the *difference* in deposit
    // across it, because that is what drives the two cells apart; uniform energy
    // on both sides moves them together and breaks nothing. This is why the
    // biggest loads land at the edge of the damaged region rather than at its
    // hottest point, and why a plug comes loose instead of a sphere vanishing.
    let mut bonds: BTreeMap<BondKey, BondLoad> = BTreeMap::new();
    for cell in deposits.keys() {
        for dir in FaceDir::ALL {
            let Some(neighbour) = cell.checked_step(dir) else {
                continue;
            };
            if scene.cells.material_at(neighbour).is_none() {
                continue;
            }
            let Some(key) = BondKey::new(*cell, dir) else {
                continue;
            };
            if bonds.contains_key(&key) {
                continue;
            }
            if measurement.bonds_considered >= limits.max_bonds_considered {
                return Ok(FractureEvaluation::Deferred {
                    reason: FractureDefer::BondBudgetSpent,
                });
            }
            measurement.bonds_considered += 1;

            let lower = key.lower();
            let upper = key.upper();
            let lower_energy = deposits.get(&lower).map_or(0, |entry| entry.1);
            let upper_energy = deposits.get(&upper).map_or(0, |entry| entry.1);
            // The lower cell's material names the bond, the way a component's
            // lowest cell names the component: one of the two has to, and
            // position is the only tie-break that does not depend on traversal.
            let material = match scene.cells.material_at(lower) {
                Some(material) => material,
                None => continue,
            };
            let profile = policy.profile(material);

            // Both deposits are clamped into the u32 range, so the difference
            // cannot overflow i64.
            let gradient = (lower_energy - upper_energy).abs();
            if gradient == 0 {
                continue;
            }

            let concentration = concentration_milli(state, scene.space(), key, &mut measurement);
            let load = scale_milli(gradient, &[profile.brittleness_milli(), concentration]);
            if load == 0 {
                continue;
            }

            let direction = bond_direction(impact, key);
            let align = direction[key.axis().index()].abs().clamp(0, MILLI);
            let axial = axial_mode(direction, key, lower_energy, upper_energy);
            let capacity = (i64::from(profile.capacity(axial).0) * align
                + i64::from(profile.shear.0) * (MILLI - align))
                / MILLI;
            let loss = if capacity <= 0 {
                i64::from(BOND_INTEGRITY.0)
            } else {
                scale_milli_div(load, i64::from(BOND_INTEGRITY.0), capacity)
            };
            if loss.saturating_mul(MILLI) / i64::from(BOND_INTEGRITY.0) < MIN_BOND_LOSS_MILLI {
                continue;
            }
            let mode = if align >= MILLI / 2 {
                axial
            } else {
                BondMode::Shear
            };

            bonds.insert(
                key,
                BondLoad {
                    bond: key,
                    material,
                    mode,
                    load: clamp_amount(load),
                    loss: clamp_amount(loss),
                },
            );
        }
    }

    let cells: Vec<CellDeposit> = deposits
        .into_iter()
        .filter(|(_, (material, energy, _))| significant_energy(policy, *material, *energy))
        .map(|(cell, (material, energy, distance_milli))| CellDeposit {
            cell,
            material,
            energy: clamp_amount(energy),
            distance_milli,
        })
        .collect();

    Ok(FractureEvaluation::Loaded(FractureLoad {
        state: state.revision(),
        space: scene.space(),
        cells,
        bonds: bonds.into_values().collect(),
        measurement,
    }))
}

/// Whether a cell deposit is large enough to be worth a sparse entry.
fn significant_energy(policy: &dyn FracturePolicy, material: MaterialId, energy: i64) -> bool {
    if energy <= 0 {
        return false;
    }
    let crush = i64::from(policy.profile(material).crush.0);
    if crush <= 0 {
        return true;
    }
    energy.saturating_mul(MILLI) / crush >= MIN_CELL_ENERGY_MILLI
}

/// Occupied cells near the impact origin, where the walk starts.
///
/// An impact usually stops at a surface rather than inside a solid, so the
/// origin is often in air. Scanning a small fixed box around it finds the struck
/// face without letting the seed reach through a thin wall.
fn seed_cells(
    impact: &FractureImpact,
    scene: FractureScene<'_>,
    required: &mut BTreeSet<RegionPos>,
) -> Vec<CellPos> {
    let origin = impact.origin_cell();
    let reach_milli = i64::from(impact.reach_cells).saturating_mul(MILLI);
    let mut seeds = Vec::new();
    for dy in -impact.seed_radius_cells..=impact.seed_radius_cells {
        for dz in -impact.seed_radius_cells..=impact.seed_radius_cells {
            for dx in -impact.seed_radius_cells..=impact.seed_radius_cells {
                let cell = CellPos::new(
                    origin.x.saturating_add(dx),
                    origin.y.saturating_add(dy),
                    origin.z.saturating_add(dz),
                );
                if impact.seed_radius_cells != SEED_RADIUS_CELLS
                    && length_milli(sub3(cell_centre_milli(cell), impact.origin_milli))
                        >= reach_milli
                {
                    continue;
                }
                if !scene.residency.is_resident(cell) {
                    // Only unknown space that could carry a deposit matters.
                    let distance = length_milli(sub3(cell_centre_milli(cell), impact.origin_milli));
                    if distance < reach_milli {
                        required.insert(cell.region());
                    }
                    continue;
                }
                if scene.cells.material_at(cell).is_some() {
                    seeds.push(cell);
                }
            }
        }
    }
    seeds
}

/// How much more of an impact a bond takes because of damage already around it.
///
/// Counting broken bonds that share a cell is the cheapest honest statement of a
/// crack tip: a face beside an existing crack has less material around it to
/// share the load with.
fn concentration_milli(
    state: &FractureState,
    space: DamageSpace,
    key: BondKey,
    measurement: &mut FractureMeasurement,
) -> i64 {
    let mut broken = 0i64;
    for cell in key.cells() {
        for dir in FaceDir::ALL {
            let Some(neighbour) = BondKey::new(cell, dir) else {
                continue;
            };
            if neighbour == key {
                continue;
            }
            measurement.concentration_lookups += 1;
            if state.is_broken(BondSite {
                space,
                bond: neighbour,
            }) {
                broken += 1;
            }
        }
    }
    (MILLI + CONCENTRATION_GAIN_MILLI.saturating_mul(broken)).min(MAX_CONCENTRATION_MILLI)
}

/// The direction the impact pushes at one bond.
///
/// An event that carried an impulse has one direction everywhere. An event that
/// did not is isotropic, and the honest direction for an isotropic deposit is
/// radially outward from the origin — which removes the special case rather than
/// branching on it.
fn bond_direction(impact: &FractureImpact, key: BondKey) -> [i64; 3] {
    if impact.direction_milli != [0; 3] {
        return impact.direction_milli;
    }
    let lower = cell_centre_milli(key.lower());
    let mut midpoint = lower;
    midpoint[key.axis().index()] += MILLI / 2;
    unit_milli(sub3(midpoint, impact.origin_milli))
}

/// Whether an axial bond is being pulled apart or driven together.
///
/// The cell the wave reaches first is upstream. If upstream absorbed more, it is
/// being driven into the one ahead: compression. If downstream absorbed more, it
/// is running away from upstream: tension. That single rule is what makes the
/// struck face crush while the back face — raised by the free-surface bonus —
/// tears off in tension, where the material is weakest.
fn axial_mode(direction: [i64; 3], key: BondKey, lower: i64, upper: i64) -> BondMode {
    let along = direction[key.axis().index()];
    let (upstream, downstream) = if along >= 0 {
        (lower, upper)
    } else {
        (upper, lower)
    };
    if upstream >= downstream {
        BondMode::Compression
    } else {
        BondMode::Tension
    }
}

/// A cell's centre in milli-cells. Exact: no floating point is involved.
const fn cell_centre_milli(cell: CellPos) -> [i64; 3] {
    [
        cell.x as i64 * MILLI + MILLI / 2,
        cell.y as i64 * MILLI + MILLI / 2,
        cell.z as i64 * MILLI + MILLI / 2,
    ]
}

/// Which cell a milli-cell coordinate falls in.
fn milli_to_cell(value: i64) -> i32 {
    value
        .div_euclid(MILLI)
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

fn sub3(a: [i64; 3], b: [i64; 3]) -> [i64; 3] {
    [
        a[0].saturating_sub(b[0]),
        a[1].saturating_sub(b[1]),
        a[2].saturating_sub(b[2]),
    ]
}

/// Euclidean length in the same milli units as the input.
fn length_milli(vector: [i64; 3]) -> i64 {
    let squared: u128 = vector
        .iter()
        .map(|component| {
            let magnitude = u128::from(component.unsigned_abs());
            magnitude * magnitude
        })
        .sum();
    squared.isqrt().min(i64::MAX as u128) as i64
}

/// `cos` between two vectors, in milli, with the second already a unit vector in
/// milli. Zero when either side has no direction.
fn cosine_milli(offset: [i64; 3], direction: [i64; 3]) -> i64 {
    let length = length_milli(offset);
    if length == 0 || direction == [0; 3] {
        return 0;
    }
    let dot: i128 = offset
        .iter()
        .zip(direction.iter())
        .map(|(a, b)| i128::from(*a) * i128::from(*b))
        .sum();
    (dot / i128::from(length)).clamp(i128::from(-MILLI), i128::from(MILLI)) as i64
}

/// Rescale an integer vector to unit length in milli. Zero stays zero.
fn unit_milli(vector: [i64; 3]) -> [i64; 3] {
    let length = length_milli(vector);
    if length == 0 {
        return [0; 3];
    }
    let scale = |component: i64| -> i64 {
        (i128::from(component) * i128::from(MILLI) / i128::from(length))
            .clamp(i128::from(-MILLI), i128::from(MILLI)) as i64
    };
    [scale(vector[0]), scale(vector[1]), scale(vector[2])]
}

/// The one quantisation step for a direction given as floats.
fn quantise_direction(vector: [f64; 3]) -> Option<[i64; 3]> {
    if !vector.iter().all(|component| component.is_finite()) {
        return None;
    }
    let mut raw = [0i64; 3];
    for (slot, component) in raw.iter_mut().zip(vector.iter()) {
        *slot = quantise_scalar(*component)?;
    }
    Some(unit_milli(raw))
}

/// The one quantisation step for a world position given as floats.
fn quantise_point(point: GlobalPos) -> Option<[i64; 3]> {
    Some([
        quantise_scalar(point.x)?,
        quantise_scalar(point.y)?,
        quantise_scalar(point.z)?,
    ])
}

/// Which coordinate space a record belongs to, as a checksum input.
///
/// Fragment identity is part of the state's content: the same crack on two
/// different fragments is not the same crack.
fn space_tag(space: DamageSpace) -> u64 {
    match space {
        DamageSpace::StaticWorld => 0,
        DamageSpace::FragmentLocal(fragment) => {
            fragment.sequence.wrapping_mul(0x9e37_79b9_7f4a_7c15)
                ^ (u64::from(fragment.index) | 1 << 63)
        }
    }
}

/// Clamped to the milli image of the cell-address space, so that differences of
/// two quantised points can never overflow `i64`.
fn quantise_scalar(value: f64) -> Option<i64> {
    if !value.is_finite() {
        return None;
    }
    let limit = (f64::from(i32::MAX) + 1.0) * MILLI as f64;
    Some((value * MILLI as f64).round().clamp(-limit, limit) as i64)
}

/// The centre of a damage volume in milli-cells, which is where the energy went in.
fn volume_centre_milli(volume: DamageVolume) -> Option<[i64; 3]> {
    match volume {
        DamageVolume::Cell(cell) => Some(cell_centre_milli(cell)),
        DamageVolume::Sphere { center, .. } => quantise_point(center),
        DamageVolume::Box(bounds) => {
            let axis = |min: i32, max: i32| -> i64 {
                // Midpoint of the inclusive cell box, in milli-cells.
                (i64::from(min) + i64::from(max) + 1) * MILLI / 2
            };
            Some([
                axis(bounds.min.x, bounds.max.x),
                axis(bounds.min.y, bounds.max.y),
                axis(bounds.min.z, bounds.max.z),
            ])
        }
    }
}

/// How far the fracture field reaches, from the event's own geometry.
fn volume_reach_cells(volume: DamageVolume) -> u32 {
    let reach = match volume {
        DamageVolume::Cell(_) => 1,
        DamageVolume::Sphere { radius, .. } => {
            if radius.is_finite() && radius > 0.0 {
                radius.floor().min(f64::from(u32::MAX)) as u32
            } else {
                1
            }
        }
        DamageVolume::Box(bounds) => {
            // Half the longest edge: the reach of a box event is its own extent.
            let span = |min: i32, max: i32| -> u32 {
                let edge = (i64::from(max) - i64::from(min)).unsigned_abs();
                (edge / 2 + 1).min(u64::from(u32::MAX)) as u32
            };
            span(bounds.min.x, bounds.max.x)
                .max(span(bounds.min.y, bounds.max.y))
                .max(span(bounds.min.z, bounds.max.z))
        }
    };
    reach.max(1)
}

/// Multiply by a chain of milli factors. `i128` throughout so a chain cannot
/// wrap before it is rescaled.
fn scale_milli(value: i64, factors: &[i64]) -> i64 {
    let mut wide = i128::from(value);
    for factor in factors {
        wide = wide * i128::from(*factor) / i128::from(MILLI);
    }
    wide.clamp(0, i128::from(u32::MAX)) as i64
}

/// `value * numerator / denominator`, widened so the product cannot wrap.
fn scale_milli_div(value: i64, numerator: i64, denominator: i64) -> i64 {
    if denominator == 0 {
        return i64::from(u32::MAX);
    }
    (i128::from(value) * i128::from(numerator) / i128::from(denominator))
        .clamp(0, i128::from(u32::MAX)) as i64
}

fn clamp_amount(value: i64) -> DamageAmount {
    DamageAmount(value.clamp(0, i64::from(u32::MAX)) as u32)
}
