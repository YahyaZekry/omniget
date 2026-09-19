//! Fixed-point scalar used for every position in the world.
//!
//! The simulation must produce byte-identical snapshots on macOS, Linux and
//! Windows, so no `f32`/`f64` is allowed anywhere in the state. One tile is
//! divided into 256 sub-units and stored in an `i32`, which gives a range of
//! +/- 8 388 608 tiles: far beyond any house or room map.

use std::fmt;
use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

/// Sub-units per tile.
pub const ONE: i32 = 256;
/// Shift matching [`ONE`].
pub const SHIFT: u32 = 8;

/// A position or distance in 1/256 of a tile.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct Fixed(pub i32);

impl Fixed {
    pub const ZERO: Fixed = Fixed(0);
    pub const ONE: Fixed = Fixed(ONE);
    pub const HALF: Fixed = Fixed(ONE / 2);

    /// Whole tiles -> fixed. Saturates instead of wrapping.
    #[inline]
    pub const fn from_tiles(t: i32) -> Fixed {
        Fixed(t.saturating_mul(ONE))
    }

    /// Exact ratio `num / den` in fixed point, truncated toward zero.
    /// `den == 0` yields zero rather than panicking: the simulation never
    /// aborts on data.
    #[inline]
    pub fn ratio(num: i32, den: i32) -> Fixed {
        if den == 0 {
            return Fixed::ZERO;
        }
        Fixed(((num as i64 * ONE as i64) / den as i64) as i32)
    }

    /// Truncating conversion toward negative infinity, so tile(-1) of a
    /// position slightly left of the origin is -1 and not 0.
    #[inline]
    pub const fn floor_tile(self) -> i32 {
        self.0 >> SHIFT
    }

    /// Nearest whole tile, half away from zero, deterministic.
    #[inline]
    pub const fn round_tile(self) -> i32 {
        if self.0 >= 0 {
            (self.0 + ONE / 2) >> SHIFT
        } else {
            -((-self.0 + ONE / 2) >> SHIFT)
        }
    }

    #[inline]
    pub const fn abs(self) -> Fixed {
        Fixed(self.0.saturating_abs())
    }

    #[inline]
    pub const fn signum(self) -> i32 {
        if self.0 > 0 {
            1
        } else if self.0 < 0 {
            -1
        } else {
            0
        }
    }

    /// Fixed * fixed with the extra shift removed, in `i64` so the product of
    /// two large values does not wrap. Named `scale` and not `mul` because
    /// `Mul` is already implemented for `Fixed * i32`, which is a different
    /// operation.
    #[inline]
    pub fn scale(self, other: Fixed) -> Fixed {
        Fixed(((self.0 as i64 * other.0 as i64) >> SHIFT) as i32)
    }

    #[inline]
    pub fn clamp_to(self, lo: Fixed, hi: Fixed) -> Fixed {
        if self.0 < lo.0 {
            lo
        } else if self.0 > hi.0 {
            hi
        } else {
            self
        }
    }

    /// Integer square root of `x`, used by the movement step. Pure integer
    /// Newton iteration seeded from the bit length: no `sqrt()` from libm,
    /// whose last bit is not portable, and no float anywhere.
    #[inline]
    pub fn isqrt64(x: i64) -> i64 {
        if x <= 0 {
            return 0;
        }
        // 2^ceil(bits/2) is always >= sqrt(x), which is what Newton needs.
        let mut r: i64 = 1i64 << ((64 - (x as u64).leading_zeros()).div_ceil(2));
        loop {
            let next = (r + x / r) / 2;
            if next >= r {
                break;
            }
            r = next;
        }
        r
    }

    /// Euclidean length of the vector `(dx, dy)` in sub-units.
    #[inline]
    pub fn len(dx: Fixed, dy: Fixed) -> Fixed {
        let sq = dx.0 as i64 * dx.0 as i64 + dy.0 as i64 * dy.0 as i64;
        Fixed(Fixed::isqrt64(sq) as i32)
    }
}

impl Add for Fixed {
    type Output = Fixed;
    #[inline]
    fn add(self, rhs: Fixed) -> Fixed {
        Fixed(self.0.saturating_add(rhs.0))
    }
}

impl Sub for Fixed {
    type Output = Fixed;
    #[inline]
    fn sub(self, rhs: Fixed) -> Fixed {
        Fixed(self.0.saturating_sub(rhs.0))
    }
}

impl Neg for Fixed {
    type Output = Fixed;
    #[inline]
    fn neg(self) -> Fixed {
        Fixed(self.0.saturating_neg())
    }
}

impl Mul<i32> for Fixed {
    type Output = Fixed;
    #[inline]
    fn mul(self, rhs: i32) -> Fixed {
        Fixed((self.0 as i64 * rhs as i64) as i32)
    }
}

impl Div<i32> for Fixed {
    type Output = Fixed;
    #[inline]
    fn div(self, rhs: i32) -> Fixed {
        if rhs == 0 {
            Fixed::ZERO
        } else {
            Fixed(self.0 / rhs)
        }
    }
}

impl AddAssign for Fixed {
    #[inline]
    fn add_assign(&mut self, rhs: Fixed) {
        *self = *self + rhs;
    }
}

impl SubAssign for Fixed {
    #[inline]
    fn sub_assign(&mut self, rhs: Fixed) {
        *self = *self - rhs;
    }
}

impl fmt::Debug for Fixed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Printed as tiles with 3 decimals, for test failures only.
        let whole = self.0 / ONE;
        let frac = (self.0 % ONE).abs() * 1000 / ONE;
        write!(f, "{whole}.{frac:03}t")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_tiles_and_back() {
        assert_eq!(Fixed::from_tiles(3).0, 768);
        assert_eq!(Fixed::from_tiles(3).floor_tile(), 3);
        assert_eq!(Fixed(-1).floor_tile(), -1);
        assert_eq!(Fixed(255).floor_tile(), 0);
    }

    #[test]
    fn round_tile_is_symmetric() {
        assert_eq!(Fixed(128).round_tile(), 1);
        assert_eq!(Fixed(127).round_tile(), 0);
        assert_eq!(Fixed(-128).round_tile(), -1);
        assert_eq!(Fixed(-127).round_tile(), 0);
    }

    #[test]
    fn ratio_never_divides_by_zero() {
        assert_eq!(Fixed::ratio(1, 0), Fixed::ZERO);
        assert_eq!(Fixed::ratio(1, 2), Fixed::HALF);
    }

    #[test]
    fn isqrt_is_exact() {
        for n in [0i64, 1, 2, 3, 4, 8, 15, 16, 17, 255, 256, 65_535, 1 << 40] {
            let r = Fixed::isqrt64(n);
            assert!(r * r <= n, "{n}");
            assert!((r + 1) * (r + 1) > n, "{n}");
        }
    }

    #[test]
    fn len_of_3_4_is_5() {
        let d = Fixed::len(Fixed::from_tiles(3), Fixed::from_tiles(4));
        assert_eq!(d, Fixed::from_tiles(5));
    }

    #[test]
    fn saturating_arithmetic() {
        assert_eq!(Fixed(i32::MAX) + Fixed(10), Fixed(i32::MAX));
        assert_eq!(Fixed(i32::MIN) - Fixed(10), Fixed(i32::MIN));
    }

    #[test]
    fn scale_uses_a_64_bit_intermediate() {
        // 1000 tiles * 0.5 = 500 tiles, would overflow in i32 before the shift.
        assert_eq!(
            Fixed::from_tiles(1000).scale(Fixed::HALF),
            Fixed::from_tiles(500)
        );
    }
}
