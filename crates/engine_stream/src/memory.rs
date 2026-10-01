//! Residency accounting, in bytes.
//!
//! Distance decides what the camera *wants*; the budget decides what may
//! actually stay. They solve different problems and neither substitutes for the
//! other.
//!
//! # Why bytes, and not region or cell counts
//!
//! The first stress baseline settled this. Mesh cost per occupied cell ranges
//! from **0.1 bytes** on solid terrain to **1,296 bytes** on a 3D checkerboard,
//! because merging collapses one case to almost nothing and cannot help the
//! other at all. A residency limit expressed in regions or cells would therefore
//! be wrong by four orders of magnitude depending on what the terrain happens to
//! look like. The only honest limit is measured bytes.
//!
//! # Categories stay separate
//!
//! Collapsing everything into one number hides which part is growing. Resident
//! cells, palettes, compiled meshes and work in flight all behave differently
//! under pressure: evicting regions does nothing about a queue of completed
//! results waiting to be applied, and throttling jobs does nothing about cells
//! already resident.
//!
//! # In-flight work counts
//!
//! A queue is not free. Job snapshots, completed-but-unapplied results and save
//! snapshots all hold memory, and a budget that ignores them is a budget that
//! can be blown by scheduling alone.

/// Where memory is going, in bytes.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct MemoryAccount {
    /// Cell storage of resident volumes.
    pub volume_bytes: u64,
    /// Material palettes of resident volumes.
    pub palette_bytes: u64,
    /// Compiled geometry held on the CPU.
    pub cpu_mesh_bytes: u64,
    /// Cells copied into mesh jobs currently compiling.
    pub mesh_snapshot_bytes: u64,
    /// Results that have come back and have not been applied yet.
    pub awaiting_apply_bytes: u64,
    /// Region data copied for saves currently writing.
    pub io_snapshot_bytes: u64,
}

impl MemoryAccount {
    /// Everything tracked, added up.
    pub fn total(&self) -> u64 {
        self.volume_bytes
            + self.palette_bytes
            + self.cpu_mesh_bytes
            + self.mesh_snapshot_bytes
            + self.awaiting_apply_bytes
            + self.io_snapshot_bytes
    }

    /// Bytes held by resident world data alone.
    pub fn resident_bytes(&self) -> u64 {
        self.volume_bytes + self.palette_bytes + self.cpu_mesh_bytes
    }

    /// Bytes held by work in flight.
    ///
    /// Tracked separately because evicting regions does nothing about it.
    pub fn in_flight_bytes(&self) -> u64 {
        self.mesh_snapshot_bytes + self.awaiting_apply_bytes + self.io_snapshot_bytes
    }
}

/// How much memory residency may use.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MemoryBudget {
    /// Above this, stop reaching further and start making room.
    pub soft_bytes: u64,
    /// Above this, stop creating optional work entirely and evict hard.
    pub hard_bytes: u64,
}

impl Default for MemoryBudget {
    fn default() -> Self {
        // Deliberately modest: a budget nobody ever reaches is a budget that was
        // never tested. The sandbox and the torture tests both set their own.
        Self {
            soft_bytes: 192 * 1024 * 1024,
            hard_bytes: 256 * 1024 * 1024,
        }
    }
}

impl MemoryBudget {
    pub fn new(soft_bytes: u64, hard_bytes: u64) -> Self {
        debug_assert!(
            hard_bytes >= soft_bytes,
            "hard ceiling must not be below the soft target"
        );
        Self {
            soft_bytes,
            hard_bytes,
        }
    }

    /// A budget with a soft target three quarters of the ceiling.
    pub fn with_ceiling(hard_bytes: u64) -> Self {
        Self::new(hard_bytes / 4 * 3, hard_bytes)
    }

    pub fn pressure(&self, account: &MemoryAccount) -> Pressure {
        let total = account.total();
        if total >= self.hard_bytes {
            Pressure::OverHard
        } else if total >= self.soft_bytes {
            Pressure::OverSoft
        } else {
            Pressure::Comfortable
        }
    }
}

/// How hard memory is pressing.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Debug)]
pub enum Pressure {
    /// Inside the soft target. Stream normally.
    #[default]
    Comfortable,
    /// Past the soft target: stop reaching as far, start making room.
    OverSoft,
    /// Past the ceiling: want only what is essential, and evict as hard as
    /// correctness allows.
    OverHard,
}

impl Pressure {
    /// Whether speculative work should be held back.
    pub fn blocks_prefetch(self) -> bool {
        self != Pressure::Comfortable
    }

    /// Whether only essential work should start.
    pub fn is_critical(self) -> bool {
        self == Pressure::OverHard
    }

    pub fn name(self) -> &'static str {
        match self {
            Pressure::Comfortable => "ok",
            Pressure::OverSoft => "over soft",
            Pressure::OverHard => "over hard",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(total: u64) -> MemoryAccount {
        MemoryAccount {
            volume_bytes: total,
            ..MemoryAccount::default()
        }
    }

    #[test]
    fn categories_add_up_and_stay_distinguishable() {
        let account = MemoryAccount {
            volume_bytes: 10,
            palette_bytes: 2,
            cpu_mesh_bytes: 100,
            mesh_snapshot_bytes: 7,
            awaiting_apply_bytes: 5,
            io_snapshot_bytes: 3,
        };
        assert_eq!(account.total(), 127);
        assert_eq!(account.resident_bytes(), 112);
        assert_eq!(
            account.in_flight_bytes(),
            15,
            "work in flight is tracked apart, because evicting regions does \
             nothing about it"
        );
    }

    #[test]
    fn pressure_has_three_steps() {
        let budget = MemoryBudget::new(100, 200);
        assert_eq!(budget.pressure(&account(0)), Pressure::Comfortable);
        assert_eq!(budget.pressure(&account(99)), Pressure::Comfortable);
        assert_eq!(budget.pressure(&account(100)), Pressure::OverSoft);
        assert_eq!(budget.pressure(&account(199)), Pressure::OverSoft);
        assert_eq!(budget.pressure(&account(200)), Pressure::OverHard);
        assert_eq!(budget.pressure(&account(u64::MAX)), Pressure::OverHard);
    }

    #[test]
    fn in_flight_work_alone_can_exceed_the_budget() {
        // A queue is not free: a budget that ignores work in flight is one that
        // scheduling alone can blow.
        let budget = MemoryBudget::new(100, 200);
        let account = MemoryAccount {
            mesh_snapshot_bytes: 150,
            awaiting_apply_bytes: 60,
            ..MemoryAccount::default()
        };
        assert_eq!(account.resident_bytes(), 0, "nothing is resident at all");
        assert_eq!(budget.pressure(&account), Pressure::OverHard);
    }

    #[test]
    fn pressure_gates_the_right_work() {
        assert!(!Pressure::Comfortable.blocks_prefetch());
        assert!(Pressure::OverSoft.blocks_prefetch());
        assert!(Pressure::OverHard.blocks_prefetch());
        assert!(!Pressure::OverSoft.is_critical());
        assert!(Pressure::OverHard.is_critical());
    }

    #[test]
    fn a_ceiling_implies_a_soft_target_below_it() {
        let budget = MemoryBudget::with_ceiling(400);
        assert_eq!(budget.hard_bytes, 400);
        assert_eq!(budget.soft_bytes, 300);
        assert!(budget.soft_bytes < budget.hard_bytes);
    }
}
