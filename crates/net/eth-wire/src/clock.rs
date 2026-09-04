//! Clocks used by protocol deadlines and rate limiting.

use std::{
    future::Future,
    ops::{Add, AddAssign},
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};

/// The native clock remains the default for every production stream constructor.
#[derive(Clone, Debug)]
pub(crate) enum ProtocolClock {
    Native,
    #[cfg(any(test, feature = "test-utils"))]
    Runtime(reth_tasks::TaskRuntime),
}

impl ProtocolClock {
    pub(crate) fn now(&self) -> Timestamp {
        match self {
            Self::Native => Timestamp::Native(Instant::now()),
            #[cfg(any(test, feature = "test-utils"))]
            Self::Runtime(runtime) => Timestamp::Runtime(runtime.now()),
        }
    }

    pub(crate) fn sleep(&self, duration: Duration) -> ProtocolTimer {
        match self {
            Self::Native => ProtocolTimer::Native(Box::pin(tokio::time::sleep(duration))),
            #[cfg(any(test, feature = "test-utils"))]
            Self::Runtime(runtime) => {
                let deadline = runtime.now() + duration;
                ProtocolTimer::Runtime {
                    runtime: runtime.clone(),
                    deadline,
                    future: sleep_until(runtime.clone(), deadline),
                    elapsed: false,
                }
            }
        }
    }
}

pub(crate) enum ProtocolTimer {
    Native(Pin<Box<tokio::time::Sleep>>),
    #[cfg(any(test, feature = "test-utils"))]
    Runtime {
        runtime: reth_tasks::TaskRuntime,
        deadline: std::time::SystemTime,
        future: Pin<Box<dyn Future<Output = ()> + Send + Sync + 'static>>,
        elapsed: bool,
    },
}

impl ProtocolTimer {
    pub(crate) fn is_elapsed(&self) -> bool {
        match self {
            Self::Native(timer) => timer.is_elapsed(),
            #[cfg(any(test, feature = "test-utils"))]
            Self::Runtime { runtime, deadline, elapsed, .. } => {
                *elapsed || runtime.now() >= *deadline
            }
        }
    }

    pub(crate) fn reset(&mut self, duration: Duration) {
        match self {
            Self::Native(timer) => timer.as_mut().reset(tokio::time::Instant::now() + duration),
            #[cfg(any(test, feature = "test-utils"))]
            Self::Runtime { runtime, deadline, future, elapsed } => {
                *deadline = runtime.now() + duration;
                *future = sleep_until(runtime.clone(), *deadline);
                *elapsed = false;
            }
        }
    }

    pub(crate) fn now(&self) -> Timestamp {
        match self {
            Self::Native(_) => Timestamp::Native(Instant::now()),
            #[cfg(any(test, feature = "test-utils"))]
            Self::Runtime { runtime, .. } => Timestamp::Runtime(runtime.now()),
        }
    }
}

impl Future for ProtocolTimer {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        match self.get_mut() {
            Self::Native(timer) => timer.as_mut().poll(cx),
            #[cfg(any(test, feature = "test-utils"))]
            Self::Runtime { future, elapsed, .. } => {
                if *elapsed || future.as_mut().poll(cx).is_ready() {
                    *elapsed = true;
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            }
        }
    }
}

impl std::fmt::Debug for ProtocolTimer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtocolTimer").field("elapsed", &self.is_elapsed()).finish()
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Timestamp {
    Native(Instant),
    #[cfg(any(test, feature = "test-utils"))]
    Runtime(std::time::SystemTime),
}

impl Timestamp {
    pub(crate) fn saturating_duration_since(self, earlier: Self) -> Duration {
        match (self, earlier) {
            (Self::Native(now), Self::Native(earlier)) => now.saturating_duration_since(earlier),
            #[cfg(any(test, feature = "test-utils"))]
            (Self::Runtime(now), Self::Runtime(earlier)) => {
                now.duration_since(earlier).unwrap_or_default()
            }
            #[cfg(any(test, feature = "test-utils"))]
            _ => panic!("protocol timestamps must use the same clock"),
        }
    }
}

impl AddAssign<Duration> for Timestamp {
    fn add_assign(&mut self, duration: Duration) {
        match self {
            Self::Native(instant) => *instant += duration,
            #[cfg(any(test, feature = "test-utils"))]
            Self::Runtime(instant) => *instant += duration,
        }
    }
}

impl Add<Duration> for Timestamp {
    type Output = Self;

    fn add(mut self, duration: Duration) -> Self {
        self += duration;
        self
    }
}

#[cfg(any(test, feature = "test-utils"))]
fn sleep_until(
    runtime: reth_tasks::TaskRuntime,
    deadline: std::time::SystemTime,
) -> Pin<Box<dyn Future<Output = ()> + Send + Sync + 'static>> {
    Box::pin(async move {
        // Timers may be created before the future is first polled. Preserve their creation-time
        // deadline rather than starting the full duration at that first poll.
        let remaining = deadline.duration_since(runtime.now()).unwrap_or_default();
        runtime.sleep(remaining).await;
    })
}
