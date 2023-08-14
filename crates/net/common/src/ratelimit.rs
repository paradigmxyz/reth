//! A rate limit implementation to enforce a specific rate.

use std::{
    future::{poll_fn, Future},
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::Sleep;

/// Given a [Rate] this type enforces a rate limit.
#[derive(Debug)]
pub struct RateLimit {
    rate: Rate,
    state: State,
    sleep: Pin<Box<Sleep>>,
}

// === impl RateLimit ===

impl RateLimit {
    /// Create a new rate limiter
    pub fn new(rate: Rate) -> Self {
        let until = tokio::time::Instant::now();
        let state = State::Ready { until, remaining: rate.limit() };

        RateLimit { rate, state, sleep: Box::pin(tokio::time::sleep_until(until)) }
    }

    /// Returns the configured limit of the [RateLimit]
    pub fn limit(&self) -> u64 {
        self.rate.limit()
    }

    /// Checks if the [RateLimit] is ready to handle a new call
    pub fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        match self.state {
            State::Ready { .. } => return Poll::Ready(()),
            State::Limited => {
                if Pin::new(&mut self.sleep).poll(cx).is_pending() {
                    return Poll::Pending
                }
            }
        }

        self.state = State::Ready {
            until: tokio::time::Instant::now() + self.rate.duration(),
            remaining: self.rate.limit(),
        };

        Poll::Ready(())
    }

    /// Wait until the [RateLimit] is ready.
    pub async fn wait(&mut self) {
        poll_fn(|cx| self.poll_ready(cx)).await
    }

    /// Updates the [RateLimit] when a new call was triggered
    ///
    /// # Panics
    ///
    /// Panics if [RateLimit::poll_ready] returned [Poll::Pending]
    pub fn tick(&mut self) {
        match self.state {
            State::Ready { mut until, remaining: mut rem } => {
                let now = tokio::time::Instant::now();

                // If the period has elapsed, reset it.
                if now >= until {
                    until = now + self.rate.duration();
                    rem = self.rate.limit();
                }

                if rem > 1 {
                    rem -= 1;
                    self.state = State::Ready { until, remaining: rem };
                } else {
                    // rate limited until elapsed
                    self.sleep.as_mut().reset(until);
                    self.state = State::Limited;
                }
            }
            State::Limited => panic!("RateLimit limited; poll_ready must be called first"),
        }
    }
}

/// Tracks the state of the [RateLimit]
#[derive(Debug)]
enum State {
    /// Currently limited
    Limited,
    Ready {
        until: tokio::time::Instant,
        remaining: u64,
    },
}

/// A rate of requests per time period.
#[derive(Debug, Copy, Clone)]
pub struct Rate {
    limit: u64,
    duration: Duration,
}

impl Rate {
    /// Create a new [Rate] with the given `limit/duration` ratio.
    pub fn new(limit: u64, duration: Duration) -> Self {
        Rate { limit, duration }
    }

    fn limit(&self) -> u64 {
        self.limit
    }

    fn duration(&self) -> Duration {
        self.duration
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rate_limit() {
        let mut limit = RateLimit::new(Rate::new(2, Duration::from_millis(500)));

        poll_fn(|cx| {
            assert!(limit.poll_ready(cx).is_ready());
            Poll::Ready(())
        })
        .await;

        limit.tick();

        poll_fn(|cx| {
            assert!(limit.poll_ready(cx).is_ready());
            Poll::Ready(())
        })
        .await;

        limit.tick();

        poll_fn(|cx| {
            assert!(limit.poll_ready(cx).is_pending());
            Poll::Ready(())
        })
        .await;

        tokio::time::sleep(limit.rate.duration).await;

        poll_fn(|cx| {
            assert!(limit.poll_ready(cx).is_ready());
            Poll::Ready(())
        })
        .await;
    }
}
