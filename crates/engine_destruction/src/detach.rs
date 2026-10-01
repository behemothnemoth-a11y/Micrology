//! The detachment transaction: pass 0003.7.
//!
//! A worker decides *what* is detached. It never removes anything. The owning
//! thread validates the result against the world as it is now and performs one
//! atomic transaction — or performs none at all.
//!
//! # All or nothing
//!
//! There is no half-detached structure. If validation fails, nothing is removed,
//! the result is discarded and the question is asked again. The alternative —
//! applying the components that still look fine — would leave a building with
//! some of its mass turned into fragments and the rest still standing on
//! support that no longer exists, which is a state no later analysis can
//! recognise as wrong.
//!
//! # Deterministic order, deterministic identity
//!
//! Components arrive already sorted by their lowest cell, and fragments take
//! their index from that order. Two runs of the same destruction produce the
//! same fragments with the same ids — which is what lets a fragment be saved,
//! reloaded and referred to, rather than being a handle that means whatever
//! this session decided.

use crate::connectivity::Classification;
use crate::fragment::{Fragment, FragmentId};
use crate::jobs::{ResultDisposition, StructureJobResult};
use engine_core::CellPos;
use engine_world::{EditOutcome, World, WorldEditBatch};

/// Why a transaction did nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DetachRefusal {
    /// The world changed while the analysis ran.
    Stale,
    /// Something was indeterminate or deferred. Nothing may be concluded.
    Inconclusive,
    /// The result was current and settled, and detached nothing.
    NothingToDo,
    /// The exact fragments were built successfully, but host policy refused
    /// to admit them. The static world and destruction sequence stay unchanged.
    RejectedByPolicy,
}

/// What a transaction did.
#[derive(Clone, PartialEq, Debug)]
pub struct DetachOutcome {
    /// The fragments created, in canonical order.
    pub fragments: Vec<Fragment>,
    /// What removing their cells did to the static world: which volumes need
    /// re-meshing, which regions need saving.
    pub edit: EditOutcome,
    /// The sequence number this transaction used.
    pub sequence: u64,
}

impl DetachOutcome {
    pub fn cells_detached(&self) -> u64 {
        self.fragments.iter().map(|f| f.cell_count()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.fragments.is_empty()
    }
}

/// Issues fragment ids and keeps destruction reproducible.
///
/// The sequence is part of a fragment's identity, so it belongs to the world
/// rather than to a session: saving it and restoring it is what stops a reloaded
/// world from minting ids that collide with ones already on disk.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct DestructionSequence(u64);

impl DestructionSequence {
    pub const fn new(next: u64) -> Self {
        Self(next)
    }

    pub const fn peek(self) -> u64 {
        self.0
    }

    fn take(&mut self) -> u64 {
        let current = self.0;
        self.0 += 1;
        current
    }
}

/// Validate a structural result and, if it holds, detach what it found.
///
/// Returns `Err` and changes **nothing** if the result is stale, inconclusive,
/// or detaches nothing.
pub fn detach(
    world: &mut World,
    sequence: &mut DestructionSequence,
    result: &StructureJobResult,
) -> Result<DetachOutcome, DetachRefusal> {
    detach_if(world, sequence, result, |_| true)
}

/// Validate and build the exact detached fragments, then ask host policy whether
/// they may be admitted before mutating the static world.
///
/// The policy callback sees the canonical fragments with their would-be IDs.
/// Returning `false` changes nothing: no cells are removed and the destruction
/// sequence is not consumed. This is the safe place for memory/work admission
/// because rejecting after `World::apply` would leave mass missing from both
/// the static world and the dynamic-fragment store.
pub fn detach_if(
    world: &mut World,
    sequence: &mut DestructionSequence,
    result: &StructureJobResult,
    admit: impl FnOnce(&[Fragment]) -> bool,
) -> Result<DetachOutcome, DetachRefusal> {
    match result.judge(world) {
        ResultDisposition::DiscardedStale => return Err(DetachRefusal::Stale),
        ResultDisposition::Inconclusive => return Err(DetachRefusal::Inconclusive),
        ResultDisposition::Accepted => {}
    }

    let detaching: Vec<&crate::Component> = result
        .components
        .components
        .iter()
        .filter(|c| matches!(c.classification, Classification::Detached))
        .collect();
    if detaching.is_empty() {
        return Err(DetachRefusal::NothingToDo);
    }

    // Peek, don't consume. A policy refusal is not a destruction transaction
    // and must not leave a hole in deterministic fragment identity.
    let seq = sequence.peek();

    let mut fragments = Vec::new();
    let mut leaving: Vec<CellPos> = Vec::new();
    for (index, component) in detaching.iter().enumerate() {
        let Some(fragment) =
            Fragment::from_cells(FragmentId::new(seq, index as u32), world, &component.cells)
        else {
            continue;
        };
        leaving.extend(component.cells.iter().copied());
        fragments.push(fragment);
    }

    if fragments.is_empty() {
        return Err(DetachRefusal::NothingToDo);
    }
    if !admit(&fragments) {
        return Err(DetachRefusal::RejectedByPolicy);
    }

    let consumed = sequence.take();
    debug_assert_eq!(consumed, seq);

    let mut batch = WorldEditBatch::new();
    for cell in leaving {
        batch.remove(cell);
    }
    let edit = world.apply(&batch);

    Ok(DetachOutcome {
        fragments,
        edit,
        sequence: seq,
    })
}
