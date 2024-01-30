use serde_derive::{Deserialize, Serialize};
use std::thread::sleep;
use std::{
    fmt,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::Interval;

/// Encapsulates calling a function every so often
pub struct Ticker {
    interval: Interval,
}

impl Ticker {
    pub fn new(duration: Duration) -> Self {
        let start = tokio::time::Instant::now() + duration;
        Self { interval: tokio::time::interval_at(start, duration) }
    }

    // Do some work if the timeout has expired
    pub fn poll(&mut self, cx: &mut Context<'_>) -> Poll<u64> {
        if self.interval.poll_tick(cx).is_ready() {
            let timestamp = chrono::prelude::Local::now().timestamp() as u64;
            return Poll::Ready(timestamp);
        }
        Poll::Pending
    }

    pub fn reset(&mut self, duration: Duration) {
        let start = tokio::time::Instant::now() + duration;
        self.interval = tokio::time::interval_at(start, duration);
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum TimeoutState {
    Active,
    Inactive,
    Expired,
}

/// A timer that expires after a given duration
/// Check back on this timer every so often to see if it's expired
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Timeout {
    state: TimeoutState,
    duration: Duration,
    #[serde(with = "serde_millis")]
    start: std::time::Instant,
}

impl Timeout {
    pub fn new(duration: Duration) -> Self {
        Timeout { state: TimeoutState::Inactive, duration, start: std::time::Instant::now() }
    }

    /// Update the timer state, and check if the timer is expired
    pub fn check_expired(&mut self) -> bool {
        if self.state == TimeoutState::Active
            && std::time::Instant::now() - self.start > self.duration
        {
            self.state = TimeoutState::Expired;
        }
        match self.state {
            TimeoutState::Active | TimeoutState::Inactive => false,
            TimeoutState::Expired => true,
        }
    }

    pub fn start(&mut self) {
        self.state = TimeoutState::Active;
        self.start = std::time::Instant::now();
    }

    pub fn stop(&mut self) {
        self.state = TimeoutState::Inactive;
        self.start = std::time::Instant::now();
    }

    #[cfg(test)]
    pub fn duration(&self) -> Duration {
        self.duration
    }

    pub fn is_active(&self) -> bool {
        self.state == TimeoutState::Active
    }
}

/// With exponential backoff, repeatedly try the callback until the result is `Ok`
pub fn retry_until_ok<T, E, F: FnMut() -> Result<T, E>>(
    base: Duration,
    max: Duration,
    mut callback: F,
) -> T {
    let mut delay = base;
    loop {
        match callback() {
            Ok(res) => return res,
            Err(_) => {
                sleep(delay);
                // Only increase delay if it's less than the max
                if delay < max {
                    delay = delay
                        .checked_mul(2)
                        .unwrap_or_else(|| Duration::from_millis(std::u64::MAX));
                    // Make sure the max isn't exceeded
                    if delay > max {
                        delay = max;
                    }
                }
            }
        }
    }
}
