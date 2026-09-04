//! Job timers preserve native Tokio behavior unless a caller supplies an explicit task clock.

use reth_tasks::TaskRuntime;
use std::{
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, SystemTime},
};

#[derive(Debug)]
pub(crate) enum JobDeadline {
    Native(Pin<Box<tokio::time::Sleep>>),
    Controlled(ClockDeadline),
}

impl JobDeadline {
    pub(crate) fn new(runtime: TaskRuntime, duration: Duration, controlled: bool) -> Self {
        if controlled {
            let at = runtime.now() + duration;
            Self::Controlled(ClockDeadline::new(runtime, at))
        } else {
            Self::Native(Box::pin(tokio::time::sleep(duration)))
        }
    }
}

impl Future for JobDeadline {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        match self.get_mut() {
            Self::Native(timer) => timer.as_mut().poll(cx),
            Self::Controlled(timer) => Pin::new(timer).poll(cx),
        }
    }
}

#[derive(Debug)]
pub(crate) enum JobInterval {
    Native(tokio::time::Interval),
    Controlled { runtime: TaskRuntime, period: Duration, next: SystemTime, timer: ClockDeadline },
}

impl JobInterval {
    pub(crate) fn new(runtime: TaskRuntime, period: Duration, controlled: bool) -> Self {
        assert!(!period.is_zero(), "payload job interval must be nonzero");
        if controlled {
            let next = runtime.now();
            let timer = ClockDeadline::new(runtime.clone(), next);
            Self::Controlled { runtime, period, next, timer }
        } else {
            Self::Native(tokio::time::interval(period))
        }
    }

    pub(crate) fn poll_tick(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        match self {
            Self::Native(interval) => interval.poll_tick(cx).map(|_| ()),
            Self::Controlled { runtime, period, next, timer } => {
                std::task::ready!(Pin::new(&mut *timer).poll(cx));
                // Tokio's default missed-tick policy is Burst. Advance from the previous target,
                // not the current time, so ticks missed during a build remain immediately ready.
                *next += *period;
                *timer = ClockDeadline::new(runtime.clone(), *next);
                Poll::Ready(())
            }
        }
    }
}

pub(crate) struct ClockDeadline {
    runtime: TaskRuntime,
    at: SystemTime,
    sleep: Pin<Box<dyn Future<Output = ()> + Send + Sync>>,
}

impl ClockDeadline {
    fn new(runtime: TaskRuntime, at: SystemTime) -> Self {
        let sleeper = runtime.clone();
        let sleep = Box::pin(async move {
            let remaining = at.duration_since(sleeper.now()).unwrap_or_default();
            sleeper.sleep(remaining).await;
        });
        Self { runtime, at, sleep }
    }
}

impl Future for ClockDeadline {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        if this.runtime.now() >= this.at {
            return Poll::Ready(());
        }
        this.sleep.as_mut().poll(cx)
    }
}

impl fmt::Debug for ClockDeadline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClockDeadline").field("at", &self.at).finish_non_exhaustive()
    }
}
