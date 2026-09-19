//! xorshift64* — the world's only source of randomness.
//!
//! Chosen over `rand` because the world has to be reproducible byte for byte
//! across platforms and across versions of the app: an RNG we own can never
//! change its stream under us. The state is a single `u64`, so it costs eight
//! bytes in the snapshot.

/// Deterministic pseudo random generator. Same seed, same sequence, forever.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// Seed 0 is remapped: xorshift is stuck at zero.
    pub const fn new(seed: u64) -> Rng {
        Rng {
            state: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
        }
    }

    /// Raw state, for the snapshot.
    pub const fn state(self) -> u64 {
        self.state
    }

    pub const fn from_state(state: u64) -> Rng {
        Rng::new(state)
    }

    /// A private stream derived from this one, so that adding an entity does
    /// not shift the numbers every other entity sees. SplitMix64 finaliser.
    pub const fn stream(self, id: u64) -> Rng {
        let mut z = self.state ^ id.wrapping_mul(0xA24B_AED4_963E_E407);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        Rng::new(z ^ (z >> 31))
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Uniform in `0..n` with the high bits, unbiased enough for a game and
    /// free of the modulo rejection loop that would make the cost vary.
    #[inline]
    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        ((self.next_u32() as u64 * n as u64) >> 32) as u32
    }

    /// Inclusive range; `lo > hi` yields `lo`.
    #[inline]
    pub fn range(&mut self, lo: i32, hi: i32) -> i32 {
        if lo >= hi {
            return lo;
        }
        lo + self.below((hi - lo + 1) as u32) as i32
    }

    /// True with probability `num/den`.
    #[inline]
    pub fn chance(&mut self, num: u32, den: u32) -> bool {
        den != 0 && self.below(den) < num
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn zero_seed_is_not_stuck() {
        let mut r = Rng::new(0);
        let first = r.next_u64();
        assert_ne!(first, 0);
        assert_ne!(r.next_u64(), first);
    }

    #[test]
    fn frozen_vector_guards_the_stream() {
        // If this ever changes, every recorded world diverges. It is a wire
        // format, not an implementation detail.
        let mut r = Rng::new(1);
        let got: Vec<u64> = (0..4).map(|_| r.next_u64()).collect();
        assert_eq!(
            got,
            vec![
                5_180_492_295_206_395_165,
                12_380_297_144_915_551_517,
                13_389_498_078_930_870_103,
                5_599_127_315_341_312_413
            ]
        );
    }

    #[test]
    fn below_stays_in_range() {
        let mut r = Rng::new(7);
        for _ in 0..10_000 {
            assert!(r.below(6) < 6);
        }
        assert_eq!(r.below(0), 0);
    }

    #[test]
    fn range_is_inclusive_and_safe() {
        let mut r = Rng::new(9);
        let mut seen_lo = false;
        let mut seen_hi = false;
        for _ in 0..10_000 {
            let v = r.range(-2, 2);
            assert!((-2..=2).contains(&v));
            seen_lo |= v == -2;
            seen_hi |= v == 2;
        }
        assert!(seen_lo && seen_hi);
        assert_eq!(r.range(5, 5), 5);
        assert_eq!(r.range(9, 1), 9);
    }

    #[test]
    fn streams_are_independent_and_stable() {
        let base = Rng::new(123);
        let mut a = base.stream(1);
        let mut b = base.stream(2);
        assert_ne!(a.next_u64(), b.next_u64());
        assert_eq!(base.stream(1).state(), Rng::new(123).stream(1).state());
    }

    #[test]
    fn chance_is_roughly_fair() {
        let mut r = Rng::new(5);
        let hits = (0..10_000).filter(|_| r.chance(1, 4)).count();
        assert!((2200..2800).contains(&hits), "{hits}");
        assert!(!r.chance(1, 0));
    }
}
