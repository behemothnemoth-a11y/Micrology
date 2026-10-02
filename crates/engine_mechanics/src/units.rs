//! The deterministic numeric type for every mechanical quantity.
//!
//! DROP 0005 decided that no floating point enters the capacity path at any
//! stage. Stress solving is normally iterative and float-sensitive, and
//! Micrology requires byte-identical results across runs and machines; the
//! cheapest way to guarantee that is to never introduce the problem.
//!
//! [`Milli`] is a signed fixed-point scalar in thousandths. It is deliberately
//! not generic, not a newtype over `f64`, and has no `From<f64>`: a conversion
//! that existed would eventually be used on a hot path.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::iter::Sum;

/// Units per whole. One `Milli` of 1000 is 1.0.
pub const SCALE: i64 = 1_000;

/// A fixed-point scalar in thousandths.
///
/// Every operation saturates. Overflow in a mechanical quantity means the model
/// was handed something absurd, and clamping to the representable extreme keeps
/// the answer deterministic and conservative rather than wrapping into a
/// plausible-looking lie.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Milli(pub i64);

impl Milli {
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(SCALE);
    pub const MAX: Self = Self(i64::MAX);
    pub const MIN: Self = Self(i64::MIN);

    /// Exactly `whole` units.
    #[inline]
    pub const fn whole(whole: i64) -> Self {
        Self(whole.saturating_mul(SCALE))
    }

    /// Raw thousandths.
    #[inline]
    pub const fn raw(self) -> i64 {
        self.0
    }

    #[inline]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    #[inline]
    pub const fn is_positive(self) -> bool {
        self.0 > 0
    }

    #[inline]
    pub fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }

    #[inline]
    pub fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }

    /// Multiply by a whole count, as when aggregating one cell's weight over many
    /// identical cells.
    #[inline]
    pub fn saturating_scale(self, count: i64) -> Self {
        Self(self.0.saturating_mul(count))
    }

    /// Fixed-point multiply, truncating toward zero.
    ///
    /// The intermediate is `i128` so that two large quantities multiplied before
    /// rescaling cannot wrap; the result still saturates into `i64`.
    #[inline]
    pub fn saturating_mul(self, other: Self) -> Self {
        let wide = (self.0 as i128) * (other.0 as i128) / (SCALE as i128);
        Self(wide.clamp(i64::MIN as i128, i64::MAX as i128) as i64)
    }

    /// Fixed-point divide, truncating toward zero. Division by zero saturates to
    /// `MAX`/`MIN` by sign, and `0 / 0` is `ZERO`.
    #[inline]
    pub fn saturating_div(self, other: Self) -> Self {
        if other.0 == 0 {
            return match self.0.signum() {
                1 => Self::MAX,
                -1 => Self::MIN,
                _ => Self::ZERO,
            };
        }
        let wide = (self.0 as i128) * (SCALE as i128) / (other.0 as i128);
        Self(wide.clamp(i64::MIN as i128, i64::MAX as i128) as i64)
    }

    #[inline]
    pub fn min(self, other: Self) -> Self {
        if self.0 <= other.0 { self } else { other }
    }

    #[inline]
    pub fn max(self, other: Self) -> Self {
        if self.0 >= other.0 { self } else { other }
    }

    /// Display/diagnostic only. Never use this in a capacity decision.
    #[inline]
    pub fn to_f64_lossy(self) -> f64 {
        self.0 as f64 / SCALE as f64
    }
}

impl Sum for Milli {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, |acc, value| acc.saturating_add(value))
    }
}

impl fmt::Debug for Milli {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let whole = self.0 / SCALE;
        let frac = (self.0 % SCALE).abs();
        write!(f, "{whole}.{frac:03}")
    }
}

impl fmt::Display for Milli {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_and_raw_round_trip() {
        assert_eq!(Milli::whole(3).raw(), 3_000);
        assert_eq!(Milli::ONE, Milli::whole(1));
        assert_eq!(Milli(1_500).to_f64_lossy(), 1.5);
    }

    #[test]
    fn arithmetic_saturates_rather_than_wrapping() {
        assert_eq!(Milli::MAX.saturating_add(Milli::ONE), Milli::MAX);
        assert_eq!(Milli::MIN.saturating_sub(Milli::ONE), Milli::MIN);
        assert_eq!(Milli::MAX.saturating_scale(2), Milli::MAX);
        assert_eq!(Milli::whole(i64::MAX), Milli::MAX);
    }

    #[test]
    fn multiply_does_not_wrap_through_the_intermediate() {
        // Both operands are large enough that an i64 intermediate would wrap.
        let big = Milli::whole(4_000_000_000);
        assert_eq!(big.saturating_mul(big), Milli::MAX);
        // Ordinary values stay exact.
        assert_eq!(
            Milli::whole(3).saturating_mul(Milli::whole(4)),
            Milli::whole(12)
        );
        assert_eq!(Milli(500).saturating_mul(Milli(500)), Milli(250));
    }

    #[test]
    fn division_by_zero_saturates_by_sign() {
        assert_eq!(Milli::whole(5).saturating_div(Milli::ZERO), Milli::MAX);
        assert_eq!(Milli::whole(-5).saturating_div(Milli::ZERO), Milli::MIN);
        assert_eq!(Milli::ZERO.saturating_div(Milli::ZERO), Milli::ZERO);
        assert_eq!(
            Milli::whole(12).saturating_div(Milli::whole(4)),
            Milli::whole(3)
        );
    }

    #[test]
    fn ordering_is_total_so_traversal_can_be_canonical() {
        let mut values = vec![Milli::whole(3), Milli::MIN, Milli::ZERO, Milli::MAX];
        values.sort();
        assert_eq!(
            values,
            vec![Milli::MIN, Milli::ZERO, Milli::whole(3), Milli::MAX]
        );
    }

    #[test]
    fn debug_shows_thousandths() {
        assert_eq!(format!("{:?}", Milli(1_500)), "1.500");
        assert_eq!(format!("{:?}", Milli(-1_500)), "-1.500");
    }
}
