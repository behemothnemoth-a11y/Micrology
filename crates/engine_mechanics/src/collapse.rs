//! Mechanical overload becoming ordinary cell failure.
//!
//! This is the first module in DROP 0005 that changes what the world does, and
//! it deliberately adds no new machinery for doing it. An overloaded connection
//! produces a failed cell; that cell goes through `static_failure_batch`, the
//! existing `WorldEditBatch` path, connectivity and detachment exactly as a
//! cell failed by applied damage already does. There is no mechanical fragment
//! system and no second detachment mechanism, because a second way to destroy
//! something is a second thing to keep correct.
//!
//! # Order, and why it is fixed
//!
//! Decision F in the scope sets one step as: damage accumulates, failed cells
//! are removed, connectivity runs and detachment is admitted, **then** mechanics
//! evaluates the resulting configuration. Failures it produces re-enter on the
//! *next* generation rather than recursing inside this one. Mechanics therefore
//! always sees a settled world, and never races the system whose answer it
//! depends on.
//!
//! # What mechanics will not do
//!
//! A structure that reaches no anchor at all is *unsupported*, not
//! *under-strength*, and this module returns nothing for it. Detaching it is
//! connectivity's job and has been since DROP 0003. Mechanics may only ever add
//! a failure reason to geometry that is genuinely attached — it may not
//! pre-empt, duplicate or second-guess the topological answer.
//!
//! # Bounded three ways
//!
//! Generation depth, failures admitted per step, and the capacity budgets
//! underneath all bound independently, reusing the discipline 0004.8
//! established for secondary damage. A collapse must not be able to run away
//! inside a single frame, and exhausting any budget must hold rather than
//! collapse.

use crate::capacity::CapacityLimits;
use crate::verdict::{CapacityVerdict, Inconclusive, evaluate_structure};
use crate::witness::{CapacityWitness, witness_capacity};
use crate::{MechanicalRegistry, ParticipationPolicy};
use engine_core::{CellPos, CellSource, MaterialId, SupportSource};
use engine_destruction::{DamageTarget, Residency};

/// Independent ceilings on a collapse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CollapseLimits {
    /// How many generations of mechanical failure may follow one trigger.
    pub max_generation: u8,
    /// How many cells may fail in a single step.
    pub max_failures_per_step: usize,
    /// Budgets for the capacity analysis underneath.
    pub capacity: CapacityLimits,
}

impl CollapseLimits {
    pub const fn new(
        max_generation: u8,
        max_failures_per_step: usize,
        capacity: CapacityLimits,
    ) -> Self {
        Self {
            max_generation,
            max_failures_per_step,
            capacity,
        }
    }
}

/// Why no failure was produced, when the structure was not simply sound.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Held {
    /// Structural capacity does not participate here.
    NotEvaluated,
    /// The collapse reached its generation ceiling. Further failure needs a new
    /// trigger, not a deeper recursion.
    GenerationCeiling,
    /// Capacity could not be decided. Nothing may be concluded, and in
    /// particular nothing may be destroyed.
    Inconclusive(Inconclusive),
    /// Nothing reaches an anchor at all. This is connectivity's question;
    /// mechanics declines it on purpose.
    Unsupported,
}

/// What mechanics wants done to the world this step.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MechanicalFailures {
    /// The structure carries its load. Nothing to do.
    Sound,
    /// These cells gave way. Feed them through `static_failure_batch` and the
    /// ordinary destruction pipeline.
    Failed {
        targets: Vec<DamageTarget>,
        generation: u8,
    },
    /// Nothing may be done, for a stated reason.
    Held(Held),
}

impl MechanicalFailures {
    /// The cells to fail, or nothing.
    pub fn targets(&self) -> &[DamageTarget] {
        match self {
            Self::Failed { targets, .. } => targets,
            _ => &[],
        }
    }

    /// Whether this step changes the world at all.
    pub fn is_actionable(&self) -> bool {
        !self.targets().is_empty()
    }
}

/// Decide which cells give way, for one generation.
///
/// The cheap witness runs first: a structure it can prove sound costs one
/// linear sweep and never builds a flow network. Only a structure the witness
/// declines reaches the exact solver.
#[allow(clippy::too_many_arguments)]
pub fn mechanical_failures(
    cells: &dyn CellSource,
    support: &dyn SupportSource,
    residency: &dyn Residency,
    registry: &MechanicalRegistry,
    policy: &ParticipationPolicy,
    roots: impl IntoIterator<Item = CellPos> + Clone,
    generation: u8,
    limits: CollapseLimits,
) -> MechanicalFailures {
    if generation > limits.max_generation {
        return MechanicalFailures::Held(Held::GenerationCeiling);
    }

    // Cheap path first. A proof of sufficiency ends the step with no network
    // built and no solver run.
    match witness_capacity(
        cells,
        support,
        residency,
        registry,
        policy,
        roots.clone(),
        limits.capacity,
    ) {
        CapacityWitness::NotEvaluated => return MechanicalFailures::Held(Held::NotEvaluated),
        CapacityWitness::DefinitelySufficient { .. } => return MechanicalFailures::Sound,
        CapacityWitness::NeedExact { .. } => {}
    }

    let verdict = evaluate_structure(
        cells,
        support,
        residency,
        registry,
        policy,
        roots,
        limits.capacity,
    );

    failures_from_verdict(cells, support, registry, verdict, generation, limits)
}

/// Apply the existing conservative failure selection to a conclusive capacity result.
pub fn failures_from_verdict(
    cells: &dyn CellSource,
    support: &dyn SupportSource,
    registry: &MechanicalRegistry,
    verdict: CapacityVerdict,
    generation: u8,
    limits: CollapseLimits,
) -> MechanicalFailures {
    if generation > limits.max_generation {
        return MechanicalFailures::Held(Held::GenerationCeiling);
    }
    let cut = match &verdict {
        CapacityVerdict::NotEvaluated => return MechanicalFailures::Held(Held::NotEvaluated),
        CapacityVerdict::Sufficient { .. } => return MechanicalFailures::Sound,
        CapacityVerdict::Inconclusive(reason) => {
            return MechanicalFailures::Held(Held::Inconclusive(reason.clone()));
        }
        CapacityVerdict::Insufficient { cut, .. } => cut,
    };

    if cut.is_empty() {
        // Unmet demand with no joint to blame: nothing reaches an anchor. Not
        // this module's call.
        return MechanicalFailures::Held(Held::Unsupported);
    }

    // A connection gives way at its weaker side, never at an anchor.
    let mut targets: Vec<DamageTarget> = Vec::new();
    for connection in cut {
        let Some(cell) = failing_side(
            cells,
            support,
            registry,
            connection.from,
            connection.to,
            connection.mode,
        ) else {
            continue;
        };
        let Some(material) = cells.material_at(cell) else {
            continue;
        };
        targets.push(DamageTarget::StaticCell { cell, material });
    }

    targets.sort_by_key(|target| target.cell());
    targets.dedup_by_key(|target| target.cell());
    targets.truncate(limits.max_failures_per_step);

    if targets.is_empty() {
        return MechanicalFailures::Held(Held::Unsupported);
    }

    MechanicalFailures::Failed {
        targets,
        generation,
    }
}

/// Which end of an overloaded connection gives way.
///
/// Two rules, both load-bearing.
///
/// **An anchored cell never fails.** An anchor is the author's statement that
/// something is held up, and mechanics may break what rests on a foundation
/// without being allowed to erase the foundation. Without this rule a tie
/// between two cells of the same material sheds the footing, which turns a
/// local joint failure into a wholly unsupported structure — the opposite of
/// progressive, and a far more drastic answer than the evidence supports.
///
/// **A tie sheds the load, not the support.** `from` is the side pushing load
/// into the joint, so when capacities are equal it is the one that gives way.
/// Collapse then propagates from the top down, the way a real one does.
fn failing_side(
    cells: &dyn CellSource,
    support: &dyn SupportSource,
    registry: &MechanicalRegistry,
    from: CellPos,
    to: CellPos,
    mode: crate::LoadMode,
) -> Option<CellPos> {
    let capacity_of = |cell: CellPos| -> Option<(MaterialId, crate::Milli)> {
        let material = cells.material_at(cell)?;
        let profile = registry.get(material)?;
        Some((material, profile.capacity(mode)))
    };
    let (_, from_capacity) = capacity_of(from)?;
    let (_, to_capacity) = capacity_of(to)?;

    // Weaker side first; a tie prefers the load-shedding side.
    let preferred = match from_capacity.cmp(&to_capacity) {
        std::cmp::Ordering::Greater => [to, from],
        _ => [from, to],
    };
    preferred.into_iter().find(|cell| !support.is_anchor(*cell))
}
