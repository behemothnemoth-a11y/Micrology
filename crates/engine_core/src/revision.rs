//! Monotonic revision counters.
//!
//! Derived data (meshes, collision shapes, occupancy caches) is expensive to
//! rebuild. The engine's rule is to cache it against the revision of the data
//! it came from and rebuild only when that revision moves. Carrying a revision
//! rather than a dirty flag means a cache can tell "unchanged" from "changed
//! and changed back again" without comparing contents.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A monotonically increasing version stamp.
#[derive(
    Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Debug, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Revision(pub u64);

impl Revision {
    pub const ZERO: Revision = Revision(0);

    /// Advance to the next revision.
    #[inline]
    pub fn bump(&mut self) {
        self.0 = self.0.wrapping_add(1);
    }

    /// Advance and return the new value.
    #[inline]
    pub fn bumped(mut self) -> Self {
        self.bump();
        self
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "r{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bump_advances_monotonically() {
        let mut r = Revision::ZERO;
        let first = r;
        r.bump();
        assert!(r > first);
        assert_eq!(r, Revision(1));
        assert_eq!(Revision(4).bumped(), Revision(5));
    }
}
