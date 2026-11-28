//! Monotonic and system time using native clocks or a virtual clock in simulation.

#[cfg(all(feature = "fuzz", dst))]
use crate::sync::deterministic::{self, Timer, Waiter};
#[cfg(all(feature = "fuzz", dst))]
use std::ops::{Add, AddAssign, Sub, SubAssign};

pub use std::time::{Duration, SystemTimeError};

/// Uses the native fast clock outside simulation and the virtual clock under `fuzz` plus
/// `--cfg dst`.
#[cfg(all(feature = "fuzz", dst))]
pub use self::Instant as FastInstant;

#[cfg(not(all(feature = "fuzz", dst)))]
pub use reth_primitives_traits::FastInstant;
#[cfg(not(all(feature = "fuzz", dst)))]
pub use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// A point on the simulated monotonic clock, reset for each runtime execution.
///
/// Signed nanoseconds permit backdating before the execution origin and deadlines beyond one
/// [`Duration::MAX`] of elapsed time. Arithmetic bounds do not depend on the host clock.
#[cfg(all(feature = "fuzz", dst))]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Instant(pub(crate) i128);

#[cfg(all(feature = "fuzz", dst))]
impl Instant {
    /// Returns the current virtual time.
    pub fn now() -> Self {
        Self(deterministic::now())
    }

    /// Returns the time elapsed since this instant, saturating at zero.
    pub fn elapsed(&self) -> Duration {
        Self::now().duration_since(*self)
    }

    /// Returns the elapsed duration, saturating at zero.
    ///
    /// Panics if the difference exceeds [`Duration::MAX`].
    pub const fn duration_since(&self, earlier: Self) -> Duration {
        let nanos = self.0.saturating_sub(earlier.0);
        let nanos = if nanos < 0 { 0 } else { nanos };
        assert!(nanos <= Duration::MAX.as_nanos() as i128, "duration between instants overflowed");
        // The range check above bounds both components to Duration's representation.
        Duration::new((nanos / 1_000_000_000) as u64, (nanos % 1_000_000_000) as u32)
    }

    /// Returns the elapsed duration, saturating at zero.
    pub const fn saturating_duration_since(&self, earlier: Self) -> Duration {
        self.duration_since(earlier)
    }

    /// Adds a duration, returning `None` on overflow.
    pub fn checked_add(&self, duration: Duration) -> Option<Self> {
        self.0.checked_add(duration.as_nanos() as i128).map(Self)
    }

    /// Subtracts a duration, returning `None` if the timestamp representation overflows.
    pub fn checked_sub(&self, duration: Duration) -> Option<Self> {
        self.0.checked_sub(duration.as_nanos() as i128).map(Self)
    }
}

#[cfg(all(feature = "fuzz", dst))]
impl Add<Duration> for Instant {
    type Output = Self;
    fn add(self, rhs: Duration) -> Self {
        self.checked_add(rhs).expect("overflow when adding duration to instant")
    }
}

#[cfg(all(feature = "fuzz", dst))]
impl Sub<Duration> for Instant {
    type Output = Self;
    fn sub(self, rhs: Duration) -> Self {
        self.checked_sub(rhs).expect("overflow when subtracting duration from instant")
    }
}

#[cfg(all(feature = "fuzz", dst))]
impl Sub for Instant {
    type Output = Duration;
    fn sub(self, rhs: Self) -> Duration {
        self.duration_since(rhs)
    }
}

#[cfg(all(feature = "fuzz", dst))]
impl AddAssign<Duration> for Instant {
    fn add_assign(&mut self, rhs: Duration) {
        *self = *self + rhs;
    }
}
#[cfg(all(feature = "fuzz", dst))]
impl SubAssign<Duration> for Instant {
    fn sub_assign(&mut self, rhs: Duration) {
        *self = *self - rhs;
    }
}

/// Suspends the current thread until at least `duration` has elapsed.
///
/// Simulation advances to the earliest timer only when every application task
/// is blocked; it never sleeps an OS thread.
pub fn sleep(duration: Duration) {
    #[cfg(not(all(feature = "fuzz", dst)))]
    std::thread::sleep(duration);
    #[cfg(all(feature = "fuzz", dst))]
    {
        let deadline = Instant::now() + duration;
        let waiter = Waiter::new();
        let timer = Timer::new(deadline, waiter.clone());
        while Instant::now() < deadline {
            waiter.wait();
        }
        drop(timer);
    }
}

/// A system timestamp measured from a fixed Unix epoch in simulation.
///
/// The representation preserves native timestamp arithmetic and [`SystemTimeError`], but
/// [`Self::now`] reads only the virtual clock, never the host wall clock.
#[cfg(all(feature = "fuzz", dst))]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[allow(
    clippy::disallowed_types,
    reason = "Native timestamp representation only; now reads the scheduler virtual clock."
)]
pub struct SystemTime(std::time::SystemTime);

/// The fixed origin of the simulated system clock.
#[cfg(all(feature = "fuzz", dst))]
pub const UNIX_EPOCH: SystemTime = SystemTime::UNIX_EPOCH;

#[cfg(all(feature = "fuzz", dst))]
impl SystemTime {
    /// The fixed origin of the simulated system clock.
    pub const UNIX_EPOCH: Self = Self(std::time::UNIX_EPOCH);

    /// Returns the Unix epoch plus the current execution's virtual elapsed time.
    ///
    /// Panics outside [`crate::sync::Runtime::run`]. Each execution starts at the epoch.
    pub fn now() -> Self {
        Self::UNIX_EPOCH + Instant::now().duration_since(Instant(0))
    }

    /// Returns the duration since `earlier`, or an error containing the reverse duration.
    pub fn duration_since(&self, earlier: Self) -> Result<Duration, SystemTimeError> {
        self.0.duration_since(earlier.0)
    }

    /// Returns the elapsed virtual duration, or an error if this timestamp is in the future.
    pub fn elapsed(&self) -> Result<Duration, SystemTimeError> {
        Self::now().duration_since(*self)
    }

    /// Adds a duration, returning `None` if the native timestamp representation overflows.
    pub fn checked_add(&self, duration: Duration) -> Option<Self> {
        self.0.checked_add(duration).map(Self)
    }

    /// Subtracts a duration, returning `None` if the native timestamp representation overflows.
    pub fn checked_sub(&self, duration: Duration) -> Option<Self> {
        self.0.checked_sub(duration).map(Self)
    }
}

#[cfg(all(feature = "fuzz", dst))]
impl Add<Duration> for SystemTime {
    type Output = Self;

    fn add(self, rhs: Duration) -> Self {
        Self(self.0 + rhs)
    }
}

#[cfg(all(feature = "fuzz", dst))]
impl Sub<Duration> for SystemTime {
    type Output = Self;

    fn sub(self, rhs: Duration) -> Self {
        Self(self.0 - rhs)
    }
}

#[cfg(all(feature = "fuzz", dst))]
impl AddAssign<Duration> for SystemTime {
    fn add_assign(&mut self, rhs: Duration) {
        self.0 += rhs;
    }
}

#[cfg(all(feature = "fuzz", dst))]
impl SubAssign<Duration> for SystemTime {
    fn sub_assign(&mut self, rhs: Duration) {
        self.0 -= rhs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(all(feature = "fuzz", dst))]
    use crate::sync::Runtime;

    #[test]
    fn system_time_arithmetic_preserves_native_errors_and_bounds() {
        let duration = Duration::from_nanos(7);
        let before_epoch = UNIX_EPOCH - duration;
        let error: std::time::SystemTimeError =
            before_epoch.duration_since(UNIX_EPOCH).unwrap_err();
        assert_eq!(error.duration(), duration);
        assert_eq!(UNIX_EPOCH.duration_since(before_epoch).unwrap(), duration);
        assert_eq!(SystemTime::UNIX_EPOCH, UNIX_EPOCH);

        let mut timestamp = before_epoch;
        timestamp += duration;
        assert_eq!(timestamp, UNIX_EPOCH);
        timestamp -= duration;
        assert_eq!(timestamp, before_epoch);
        assert_eq!(UNIX_EPOCH.checked_sub(duration), Some(before_epoch));
        assert_eq!(before_epoch.checked_add(duration), Some(UNIX_EPOCH));
        assert_eq!(
            UNIX_EPOCH.checked_add(Duration::MAX).is_none(),
            std::time::UNIX_EPOCH.checked_add(Duration::MAX).is_none()
        );
        assert_eq!(
            UNIX_EPOCH.checked_sub(Duration::MAX).is_none(),
            std::time::UNIX_EPOCH.checked_sub(Duration::MAX).is_none()
        );
    }

    #[cfg(all(feature = "fuzz", dst))]
    #[test]
    fn system_time_tracks_virtual_time_and_resets_between_executions() {
        for _ in 0..2 {
            Runtime::new().run(|| {
                assert_eq!(SystemTime::now(), UNIX_EPOCH);
                let duration = Duration::from_secs(3);
                let future = SystemTime::now() + duration;
                assert_eq!(future.elapsed().unwrap_err().duration(), duration);
                sleep(duration);
                assert_eq!(SystemTime::now(), future);
                assert_eq!(UNIX_EPOCH.elapsed().unwrap(), duration);
                assert_eq!(future.elapsed().unwrap(), Duration::ZERO);
            });
        }
    }

    #[cfg(not(all(feature = "fuzz", dst)))]
    #[test]
    fn native_system_time_is_the_standard_type() {
        let timestamp: std::time::SystemTime = SystemTime::now();
        let selected: SystemTime = timestamp;
        assert_eq!(selected, timestamp);
    }
}
