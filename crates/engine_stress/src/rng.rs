//! A deterministic pseudo-random source.
//!
//! Seeded and reproducible: a stress scenario that cannot be regenerated exactly
//! is not a regression test, and a flaky benchmark is worse than none. There is
//! no `rand` dependency and there should not be one.

/// SplitMix64.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..bound`.
    pub fn below(&mut self, bound: u32) -> u32 {
        (self.next_u64() % u64::from(bound)) as u32
    }

    /// Uniform in `min..=max`.
    pub fn range(&mut self, min: i32, max: i32) -> i32 {
        debug_assert!(max >= min);
        min + self.below((max - min + 1) as u32) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_replays_exactly() {
        let a: Vec<u64> = (0..64).map(|_| Rng::new(7).next_u64()).collect();
        let mut b = Rng::new(7);
        assert_eq!(a[0], b.next_u64());

        let first: Vec<u64> = {
            let mut r = Rng::new(99);
            (0..64).map(|_| r.next_u64()).collect()
        };
        let second: Vec<u64> = {
            let mut r = Rng::new(99);
            (0..64).map(|_| r.next_u64()).collect()
        };
        assert_eq!(first, second);
    }

    #[test]
    fn bounds_are_respected() {
        let mut rng = Rng::new(3);
        for _ in 0..2000 {
            let v = rng.below(10);
            assert!(v < 10);
            let r = rng.range(-5, 5);
            assert!((-5..=5).contains(&r));
        }
    }
}
