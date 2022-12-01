//! Tracks a size value.

use std::ops::{AddAssign, SubAssign};

/// Keeps track of accumulated size in bytes.
///
/// Note: We do not assume that size tracking is always exact. Depending on the bookkeeping of the
/// additions and subtractions the total size might be slightly off. Therefore, the underlying value
/// is an `isize`, so that the value does not wrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SizeTracker(isize);

impl AddAssign<usize> for SizeTracker {
    fn add_assign(&mut self, rhs: usize) {
        self.0 += rhs as isize
    }
}

impl SubAssign<usize> for SizeTracker {
    fn sub_assign(&mut self, rhs: usize) {
        self.0 -= rhs as isize
    }
}

impl From<SizeTracker> for usize {
    fn from(value: SizeTracker) -> Self {
        value.0 as usize
    }
}
