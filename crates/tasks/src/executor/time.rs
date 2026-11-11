//! Monotonic time using the native clock or a virtual clock in simulation.

#[cfg(feature = "fuzz")]
use crate::executor::deterministic::{self, Timer};
#[cfg(feature = "fuzz")]
use shuttle::thread;
#[cfg(feature = "fuzz")]
use std::ops::{Add, AddAssign, Sub, SubAssign};

pub use std::time::Duration;
#[cfg(not(feature = "fuzz"))]
pub use std::time::Instant;

/// A point on the simulated monotonic clock, reset for each runtime execution.
#[cfg(feature = "fuzz")]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Instant(pub(crate) Duration);

#[cfg(feature = "fuzz")]
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
    pub const fn duration_since(&self, earlier: Self) -> Duration {
        self.0.saturating_sub(earlier.0)
    }

    /// Returns the elapsed duration, saturating at zero.
    pub const fn saturating_duration_since(&self, earlier: Self) -> Duration {
        self.duration_since(earlier)
    }

    /// Adds a duration, returning `None` on overflow.
    pub fn checked_add(&self, duration: Duration) -> Option<Self> {
        self.0.checked_add(duration).map(Self)
    }

    /// Subtracts a duration, returning `None` on underflow.
    pub fn checked_sub(&self, duration: Duration) -> Option<Self> {
        self.0.checked_sub(duration).map(Self)
    }
}

#[cfg(feature = "fuzz")]
impl Add<Duration> for Instant {
    type Output = Self;
    fn add(self, rhs: Duration) -> Self {
        Self(self.0 + rhs)
    }
}

#[cfg(feature = "fuzz")]
impl Sub<Duration> for Instant {
    type Output = Self;
    fn sub(self, rhs: Duration) -> Self {
        Self(self.0 - rhs)
    }
}

#[cfg(feature = "fuzz")]
impl Sub for Instant {
    type Output = Duration;
    fn sub(self, rhs: Self) -> Duration {
        self.duration_since(rhs)
    }
}

#[cfg(feature = "fuzz")]
impl AddAssign<Duration> for Instant {
    fn add_assign(&mut self, rhs: Duration) {
        self.0 += rhs;
    }
}
#[cfg(feature = "fuzz")]
impl SubAssign<Duration> for Instant {
    fn sub_assign(&mut self, rhs: Duration) {
        self.0 -= rhs;
    }
}

/// Suspends the current thread until at least `duration` has elapsed.
///
/// Simulation advances to the earliest timer only when every application task
/// is blocked; it never sleeps an OS thread.
pub fn sleep(duration: Duration) {
    #[cfg(not(feature = "fuzz"))]
    std::thread::sleep(duration);
    #[cfg(feature = "fuzz")]
    {
        let deadline = Instant::now() + duration;
        let timer = Timer::new(deadline);
        while Instant::now() < deadline {
            thread::park();
        }
        drop(timer);
    }
}
