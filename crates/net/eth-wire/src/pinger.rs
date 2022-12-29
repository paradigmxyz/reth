use crate::errors::PingerError;
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::{Instant, Interval, Sleep};
use tokio_stream::Stream;

/// The pinger is a state machine that is created with a maximum number of pongs that can be
/// missed.
#[derive(Debug)]
pub(crate) struct Pinger {
    /// The timer used for the next ping.
    ping_interval: Interval,
    /// The timer used for the next ping.
    timeout_timer: Pin<Box<Sleep>>,
    /// The timeout duration for each ping.
    timeout: Duration,
    /// Keeps track of the state
    state: PingState,
}

// === impl Pinger ===

impl Pinger {
    /// Creates a new [`Pinger`] with the given ping interval duration,
    /// and timeout duration.
    pub(crate) fn new(ping_interval: Duration, timeout_duration: Duration) -> Self {
        let now = Instant::now();
        let timeout_timer = tokio::time::sleep(timeout_duration);
        Self {
            state: PingState::Ready,
            ping_interval: tokio::time::interval_at(now + ping_interval, ping_interval),
            timeout_timer: Box::pin(timeout_timer),
            timeout: timeout_duration,
        }
    }

    /// Mark a pong as received, and transition the pinger to the `Ready` state if it was in the
    /// `WaitingForPong` state. Unsets the sleep timer.
    pub(crate) fn on_pong(&mut self) -> Result<(), PingerError> {
        match self.state {
            PingState::Ready => Err(PingerError::UnexpectedPong),
            PingState::WaitingForPong => {
                self.state = PingState::Ready;
                self.ping_interval.reset();
                Ok(())
            }
            PingState::TimedOut => {
                // if we receive a pong after timeout then we also reset the state, since the
                // connection was kept alive after timeout
                self.state = PingState::Ready;
                self.ping_interval.reset();
                Ok(())
            }
        }
    }

    /// Returns the current state of the pinger.
    pub(crate) fn state(&self) -> PingState {
        self.state
    }

    /// Polls the state of the pinger and returns whether a new ping needs to be sent or if a
    /// previous ping timed out.
    pub(crate) fn poll_ping(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<PingerEvent, PingerError>> {
        match self.state() {
            PingState::Ready => {
                if self.ping_interval.poll_tick(cx).is_ready() {
                    self.timeout_timer.as_mut().reset(Instant::now() + self.timeout);
                    self.state = PingState::WaitingForPong;
                    return Poll::Ready(Ok(PingerEvent::Ping))
                }
            }
            PingState::WaitingForPong => {
                if self.timeout_timer.is_elapsed() {
                    self.state = PingState::TimedOut;
                    return Poll::Ready(Ok(PingerEvent::Timeout))
                }
            }
            PingState::TimedOut => {
                // we treat continuous calls while in timeout as pending, since the connection is
                // not yet terminated
                return Poll::Pending
            }
        };
        Poll::Pending
    }
}

impl Stream for Pinger {
    type Item = Result<PingerEvent, PingerError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().poll_ping(cx).map(Some)
    }
}

/// This represents the possible states of the pinger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PingState {
    /// There are no pings in flight, or all pings have been responded to, and we are ready to send
    /// a ping at a later point.
    Ready,
    /// We have sent a ping and are waiting for a pong, but the peer has missed n pongs.
    WaitingForPong,
    /// The peer has failed to respond to a ping.
    TimedOut,
}

/// The element type produced by a [`IntervalPingerStream`], representing either a new [`Ping`]
/// message to send, or an indication that the peer should be timed out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PingerEvent {
    /// A new [`Ping`] message should be sent.
    Ping,

    /// The peer should be timed out.
    Timeout,
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn test_ping_timeout() {
        let interval = Duration::from_millis(300);
        // we should wait for the interval to elapse and receive a pong before the timeout elapses
        let mut pinger = Pinger::new(interval, Duration::from_millis(20));
        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);
        pinger.on_pong().unwrap();
        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);

        tokio::time::sleep(interval).await;
        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Timeout);
        pinger.on_pong().unwrap();

        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);
    }
}
