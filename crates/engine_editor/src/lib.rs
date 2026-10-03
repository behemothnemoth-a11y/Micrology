//! Bevy-free editor command/history kernel.
//!
//! DROP 0007 keeps authoring on the same mutation path as every other Micrology
//! host. A command captures exact before/after cell state, validates that the
//! world still matches the state it was authored against, then applies an
//! ordinary `WorldEditBatch`. Undo and redo are the same command in the opposite
//! direction.
//!
//! No editor command knows about render entities, physics bodies or Bevy.

use engine_core::{CellPos, MaterialId};
use engine_world::{EditOutcome, World, WorldEditBatch};
use std::collections::{BTreeMap, VecDeque};
use std::mem::size_of;

/// One canonical cell change captured by an editor command.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CellChange {
    pub cell: CellPos,
    pub before: Option<MaterialId>,
    pub after: Option<MaterialId>,
}

/// A stale-safe, exactly reversible cell edit.
///
/// Changes are stored by cell address in a `BTreeMap`, so command identity and
/// replay order are independent of the order a UI happened to gather writes.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct EditorCommand {
    changes: BTreeMap<CellPos, CellChange>,
}

impl EditorCommand {
    /// Capture a command against the current authoritative world without
    /// mutating it.
    ///
    /// Writes that would not change the world are omitted. This means an empty
    /// command is a real no-op rather than a history entry that merely happens
    /// to do nothing.
    pub fn capture(world: &World, batch: &WorldEditBatch) -> Self {
        let mut changes = BTreeMap::new();
        for (cell, after) in batch.iter() {
            let before = world.get(cell);
            if before == after {
                continue;
            }
            changes.insert(
                cell,
                CellChange {
                    cell,
                    before,
                    after,
                },
            );
        }
        Self { changes }
    }

    pub fn len(&self) -> usize {
        self.changes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    pub fn changes(&self) -> impl Iterator<Item = CellChange> + '_ {
        self.changes.values().copied()
    }

    /// Deterministic tracked payload used by the editor history budget.
    ///
    /// This deliberately accounts the command payload rather than allocator
    /// implementation details. The same command therefore costs the same number
    /// of tracked bytes on every platform.
    pub fn tracked_bytes(&self) -> usize {
        self.changes.len().saturating_mul(
            size_of::<CellPos>().saturating_add(size_of::<Option<MaterialId>>() * 2),
        )
    }

    fn validate_forward(&self, world: &World) -> bool {
        self.changes
            .values()
            .all(|change| world.get(change.cell) == change.before)
    }

    fn validate_reverse(&self, world: &World) -> bool {
        self.changes
            .values()
            .all(|change| world.get(change.cell) == change.after)
    }

    fn forward_batch(&self) -> WorldEditBatch {
        self.changes
            .values()
            .map(|change| (change.cell, change.after))
            .collect()
    }

    fn reverse_batch(&self) -> WorldEditBatch {
        self.changes
            .values()
            .map(|change| (change.cell, change.before))
            .collect()
    }

    pub fn apply(&self, world: &mut World) -> Result<EditOutcome, CommandRefusal> {
        if !self.validate_forward(world) {
            return Err(CommandRefusal::Stale);
        }
        Ok(world.apply(&self.forward_batch()))
    }

    pub fn undo(&self, world: &mut World) -> Result<EditOutcome, CommandRefusal> {
        if !self.validate_reverse(world) {
            return Err(CommandRefusal::Stale);
        }
        Ok(world.apply(&self.reverse_batch()))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CommandRefusal {
    /// At least one addressed cell no longer matches the state the command was
    /// captured against. Nothing is mutated.
    Stale,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HistoryLimits {
    pub max_entries: usize,
    pub max_bytes: usize,
}

impl HistoryLimits {
    pub const fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            max_entries,
            max_bytes,
        }
    }
}

impl Default for HistoryLimits {
    fn default() -> Self {
        Self {
            max_entries: 256,
            max_bytes: 32 << 20,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HistoryRefusal {
    Stale,
    /// One command cannot fit even in otherwise-empty history. Refused before
    /// world mutation so "undoable edit" remains an actual guarantee.
    CommandTooLarge {
        required_bytes: usize,
        limit_bytes: usize,
    },
    HistoryDisabled,
}

impl From<CommandRefusal> for HistoryRefusal {
    fn from(value: CommandRefusal) -> Self {
        match value {
            CommandRefusal::Stale => Self::Stale,
        }
    }
}

/// Bounded editor undo/redo history.
///
/// New edits clear redo. To admit a new command, the oldest undo commands are
/// evicted deterministically until both entry and tracked-byte limits fit.
/// Commands are never partially retained.
#[derive(Clone, Debug)]
pub struct EditHistory {
    limits: HistoryLimits,
    undo: VecDeque<EditorCommand>,
    redo: Vec<EditorCommand>,
    tracked_bytes: usize,
}

impl EditHistory {
    pub fn new(limits: HistoryLimits) -> Self {
        Self {
            limits,
            undo: VecDeque::new(),
            redo: Vec::new(),
            tracked_bytes: 0,
        }
    }

    pub fn limits(&self) -> HistoryLimits {
        self.limits
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    pub fn tracked_bytes(&self) -> usize {
        self.tracked_bytes
    }

    /// Execute a captured command and record it for undo.
    ///
    /// Staleness and single-command budget refusal are checked before either the
    /// world or history changes.
    pub fn execute(
        &mut self,
        world: &mut World,
        command: EditorCommand,
    ) -> Result<EditOutcome, HistoryRefusal> {
        if command.is_empty() {
            return Ok(EditOutcome::default());
        }
        if !command.validate_forward(world) {
            return Err(HistoryRefusal::Stale);
        }
        if self.limits.max_entries == 0 || self.limits.max_bytes == 0 {
            return Err(HistoryRefusal::HistoryDisabled);
        }

        let bytes = command.tracked_bytes();
        if bytes > self.limits.max_bytes {
            return Err(HistoryRefusal::CommandTooLarge {
                required_bytes: bytes,
                limit_bytes: self.limits.max_bytes,
            });
        }

        self.clear_redo();

        while self.undo.len() >= self.limits.max_entries
            || self.tracked_bytes.saturating_add(bytes) > self.limits.max_bytes
        {
            let Some(evicted) = self.undo.pop_front() else {
                break;
            };
            self.tracked_bytes = self.tracked_bytes.saturating_sub(evicted.tracked_bytes());
        }

        let outcome = world.apply(&command.forward_batch());
        self.tracked_bytes = self.tracked_bytes.saturating_add(bytes);
        self.undo.push_back(command);
        Ok(outcome)
    }

    pub fn undo(&mut self, world: &mut World) -> Result<Option<EditOutcome>, HistoryRefusal> {
        let Some(command) = self.undo.back() else {
            return Ok(None);
        };
        if !command.validate_reverse(world) {
            return Err(HistoryRefusal::Stale);
        }

        let command = self.undo.pop_back().expect("validated undo entry exists");
        let outcome = world.apply(&command.reverse_batch());
        self.redo.push(command);
        Ok(Some(outcome))
    }

    pub fn redo(&mut self, world: &mut World) -> Result<Option<EditOutcome>, HistoryRefusal> {
        let Some(command) = self.redo.last() else {
            return Ok(None);
        };
        if !command.validate_forward(world) {
            return Err(HistoryRefusal::Stale);
        }

        let command = self.redo.pop().expect("validated redo entry exists");
        let outcome = world.apply(&command.forward_batch());
        self.undo.push_back(command);
        Ok(Some(outcome))
    }

    fn clear_redo(&mut self) {
        for command in self.redo.drain(..) {
            self.tracked_bytes = self.tracked_bytes.saturating_sub(command.tracked_bytes());
        }
    }
}

impl Default for EditHistory {
    fn default() -> Self {
        Self::new(HistoryLimits::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::{MaterialId, VOLUME_EDGE};

    const A: MaterialId = MaterialId(1);
    const B: MaterialId = MaterialId(2);

    fn world_with_line() -> World {
        let mut world = World::new();
        world.fill_box(CellPos::ZERO, CellPos::new(3, 0, 0), Some(A));
        world.take_dirty();
        world
    }

    #[test]
    fn command_identity_is_independent_of_input_write_order() {
        let world = world_with_line();

        let mut first = WorldEditBatch::new();
        first.set(CellPos::new(3, 0, 0), Some(B));
        first.remove(CellPos::new(1, 0, 0));

        let mut second = WorldEditBatch::new();
        second.remove(CellPos::new(1, 0, 0));
        second.set(CellPos::new(3, 0, 0), Some(B));

        assert_eq!(
            EditorCommand::capture(&world, &first),
            EditorCommand::capture(&world, &second)
        );
    }

    #[test]
    fn no_op_writes_do_not_enter_the_command() {
        let world = world_with_line();
        let mut batch = WorldEditBatch::new();
        batch.set(CellPos::ZERO, Some(A));
        assert!(EditorCommand::capture(&world, &batch).is_empty());
    }

    #[test]
    fn execute_undo_and_redo_are_exact_world_round_trips() {
        let mut world = world_with_line();
        let original = world.clone();

        let mut batch = WorldEditBatch::new();
        batch.remove(CellPos::new(1, 0, 0));
        batch.set(CellPos::new(2, 0, 0), Some(B));
        batch.set(CellPos::new(8, 0, 0), Some(A));

        let command = EditorCommand::capture(&world, &batch);
        let mut history = EditHistory::default();
        history.execute(&mut world, command).unwrap();
        let edited = world.clone();

        history.undo(&mut world).unwrap().expect("undo exists");
        assert_eq!(world, original);

        history.redo(&mut world).unwrap().expect("redo exists");
        assert_eq!(world, edited);
    }

    #[test]
    fn stale_execute_refuses_without_world_or_history_changes() {
        let mut world = world_with_line();
        let mut batch = WorldEditBatch::new();
        batch.remove(CellPos::new(1, 0, 0));
        let command = EditorCommand::capture(&world, &batch);

        world.set(CellPos::new(1, 0, 0), Some(B));
        let external = world.clone();

        let mut history = EditHistory::default();
        assert_eq!(
            history.execute(&mut world, command),
            Err(HistoryRefusal::Stale)
        );
        assert_eq!(world, external);
        assert_eq!(history.undo_len(), 0);
        assert_eq!(history.redo_len(), 0);
    }

    #[test]
    fn stale_undo_refuses_atomically() {
        let mut world = world_with_line();
        let mut batch = WorldEditBatch::new();
        batch.remove(CellPos::new(1, 0, 0));
        let command = EditorCommand::capture(&world, &batch);

        let mut history = EditHistory::default();
        history.execute(&mut world, command).unwrap();
        world.set(CellPos::new(1, 0, 0), Some(B));
        let external = world.clone();

        assert_eq!(history.undo(&mut world), Err(HistoryRefusal::Stale));
        assert_eq!(world, external);
        assert_eq!(history.undo_len(), 1);
        assert_eq!(history.redo_len(), 0);
    }

    #[test]
    fn stale_redo_refuses_atomically() {
        let mut world = world_with_line();
        let mut batch = WorldEditBatch::new();
        batch.remove(CellPos::new(1, 0, 0));
        let command = EditorCommand::capture(&world, &batch);

        let mut history = EditHistory::default();
        history.execute(&mut world, command).unwrap();
        history.undo(&mut world).unwrap();
        world.set(CellPos::new(1, 0, 0), Some(B));
        let external = world.clone();

        assert_eq!(history.redo(&mut world), Err(HistoryRefusal::Stale));
        assert_eq!(world, external);
        assert_eq!(history.undo_len(), 0);
        assert_eq!(history.redo_len(), 1);
    }

    #[test]
    fn a_new_edit_clears_redo() {
        let mut world = world_with_line();
        let mut history = EditHistory::default();

        let mut first = WorldEditBatch::new();
        first.remove(CellPos::new(1, 0, 0));
        let first = EditorCommand::capture(&world, &first);
        history.execute(&mut world, first).unwrap();
        history.undo(&mut world).unwrap();
        assert_eq!(history.redo_len(), 1);

        let mut second = WorldEditBatch::new();
        second.set(CellPos::new(2, 0, 0), Some(B));
        let second = EditorCommand::capture(&world, &second);
        history.execute(&mut world, second).unwrap();
        assert_eq!(history.redo_len(), 0);
    }

    #[test]
    fn entry_limit_evicts_the_oldest_command_deterministically() {
        let mut world = world_with_line();
        let mut history = EditHistory::new(HistoryLimits::new(2, usize::MAX));

        for x in 0..3 {
            let mut batch = WorldEditBatch::new();
            batch.set(CellPos::new(x, 0, 0), Some(B));
            let command = EditorCommand::capture(&world, &batch);
            history.execute(&mut world, command).unwrap();
        }

        assert_eq!(history.undo_len(), 2);
        history.undo(&mut world).unwrap().expect("third command");
        history.undo(&mut world).unwrap().expect("second command");
        assert!(history.undo(&mut world).unwrap().is_none());

        // The oldest command was evicted, so x=0 stays edited.
        assert_eq!(world.get(CellPos::new(0, 0, 0)), Some(B));
        assert_eq!(world.get(CellPos::new(1, 0, 0)), Some(A));
        assert_eq!(world.get(CellPos::new(2, 0, 0)), Some(A));
    }

    #[test]
    fn byte_limit_refuses_one_oversize_command_before_mutation() {
        let mut world = world_with_line();
        let original = world.clone();

        let mut batch = WorldEditBatch::new();
        batch.remove(CellPos::new(0, 0, 0));
        batch.remove(CellPos::new(1, 0, 0));
        let command = EditorCommand::capture(&world, &batch);
        let required = command.tracked_bytes();

        let mut history = EditHistory::new(HistoryLimits::new(8, required - 1));
        assert_eq!(
            history.execute(&mut world, command),
            Err(HistoryRefusal::CommandTooLarge {
                required_bytes: required,
                limit_bytes: required - 1,
            })
        );
        assert_eq!(world, original);
        assert_eq!(history.tracked_bytes(), 0);
    }

    #[test]
    fn byte_limit_evicts_oldest_history_until_the_new_edit_fits() {
        let mut world = world_with_line();

        let mut sample = WorldEditBatch::new();
        sample.set(CellPos::ZERO, Some(B));
        let bytes = EditorCommand::capture(&world, &sample).tracked_bytes();
        let mut history = EditHistory::new(HistoryLimits::new(8, bytes * 2));

        for x in 0..3 {
            let mut batch = WorldEditBatch::new();
            batch.set(CellPos::new(x, 0, 0), Some(B));
            let command = EditorCommand::capture(&world, &batch);
            history.execute(&mut world, command).unwrap();
        }

        assert_eq!(history.undo_len(), 2);
        assert_eq!(history.tracked_bytes(), bytes * 2);
    }

    #[test]
    fn boundary_edit_still_uses_world_dirty_tracking() {
        let mut world = World::new();
        let left = CellPos::new(VOLUME_EDGE - 1, 0, 0);
        let right = CellPos::new(VOLUME_EDGE, 0, 0);
        world.set(left, Some(A));
        world.set(right, Some(A));
        world.take_dirty();

        let mut batch = WorldEditBatch::new();
        batch.remove(left);
        let command = EditorCommand::capture(&world, &batch);
        let mut history = EditHistory::default();
        let outcome = history.execute(&mut world, command).unwrap();

        assert!(outcome.dirtied_volumes.contains(&left.volume()));
        assert!(outcome.dirtied_volumes.contains(&right.volume()));
    }
}
