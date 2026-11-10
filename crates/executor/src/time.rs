//! Monotonic time using the native clock or a virtual clock in simulation.

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
        Self(crate::deterministic::now())
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
impl std::ops::Add<Duration> for Instant {
    type Output = Self;
    fn add(self, rhs: Duration) -> Self {
        Self(self.0 + rhs)
    }
}

#[cfg(feature = "fuzz")]
impl std::ops::Sub<Duration> for Instant {
    type Output = Self;
    fn sub(self, rhs: Duration) -> Self {
        Self(self.0 - rhs)
    }
}

#[cfg(feature = "fuzz")]
impl std::ops::Sub for Instant {
    type Output = Duration;
    fn sub(self, rhs: Self) -> Duration {
        self.duration_since(rhs)
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
        let timer = crate::deterministic::Timer::new(deadline);
        while Instant::now() < deadline {
            shuttle::thread::park();
        }
        drop(timer);
    }
}

#[cfg(feature = "fuzz")]
impl std::ops::AddAssign<Duration> for Instant {
    fn add_assign(&mut self, rhs: Duration) {
        self.0 += rhs;
    }
}
#[cfg(feature = "fuzz")]
impl std::ops::SubAssign<Duration> for Instant {
    fn sub_assign(&mut self, rhs: Duration) {
        self.0 -= rhs;
    }
}
