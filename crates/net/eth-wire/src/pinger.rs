use crate::errors::PingerError;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, Waker},
    time::Duration,
};
use tokio::time::{Instant, Sleep};
use tokio_stream::Stream;

/// The pinger is a simple state machine that sends a ping, waits for a pong,
/// and transitions to timeout if the pong is not received within the timeout.
#[derive(Debug)]
pub(crate) struct Pinger {
    /// The timer used for the next ping.
    ping_timer: Pin<Box<Sleep>>,
    /// The last task waker registered with the ping timer.
    ping_waker: Option<Waker>,
    /// The duration between pings.
    ping_interval: Duration,
    /// The timer used to detect a ping timeout.
    timeout_timer: Pin<Box<Sleep>>,
    /// The last task waker registered with the timeout timer.
    timeout_waker: Option<Waker>,
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
        let ping_timer = tokio::time::sleep_until(now + ping_interval);
        let timeout_timer = tokio::time::sleep(timeout_duration);
        Self {
            state: PingState::Ready,
            ping_timer: Box::pin(ping_timer),
            ping_waker: None,
            ping_interval,
            timeout_timer: Box::pin(timeout_timer),
            timeout_waker: None,
            timeout: timeout_duration,
        }
    }

    /// Mark a pong as received, and transition the pinger to the `Ready` state if it was in the
    /// `WaitingForPong` state. Resets readiness by resetting the ping interval.
    pub(crate) fn on_pong(&mut self) -> Result<(), PingerError> {
        match self.state {
            PingState::Ready => Err(PingerError::UnexpectedPong),
            PingState::WaitingForPong => {
                self.state = PingState::Ready;
                self.ping_timer.as_mut().reset(Instant::now() + self.ping_interval);
                self.ping_waker = None;
                self.timeout_waker = None;
                Ok(())
            }
            PingState::TimedOut => {
                // if we receive a pong after timeout then we also reset the state, since the
                // connection was kept alive after timeout
                self.state = PingState::Ready;
                self.ping_timer.as_mut().reset(Instant::now() + self.ping_interval);
                self.ping_waker = None;
                self.timeout_waker = None;
                Ok(())
            }
        }
    }

    /// Returns the current state of the pinger.
    pub(crate) const fn state(&self) -> PingState {
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
                if self.ping_waker.as_ref().is_some_and(|waker| waker.will_wake(cx.waker())) &&
                    !self.ping_timer.is_elapsed()
                {
                    return Poll::Pending
                }

                if self.ping_timer.as_mut().poll(cx).is_ready() {
                    self.timeout_timer.as_mut().reset(Instant::now() + self.timeout);
                    self.ping_waker = None;
                    self.timeout_waker = None;
                    self.state = PingState::WaitingForPong;
                    return Poll::Ready(Ok(PingerEvent::Ping))
                }
                self.ping_waker = Some(cx.waker().clone());
            }
            PingState::WaitingForPong => {
                if self.timeout_waker.as_ref().is_some_and(|waker| waker.will_wake(cx.waker())) &&
                    !self.timeout_timer.is_elapsed()
                {
                    return Poll::Pending
                }

                if self.timeout_timer.as_mut().poll(cx).is_ready() {
                    self.timeout_waker = None;
                    self.state = PingState::TimedOut;
                    return Poll::Ready(Ok(PingerEvent::Timeout))
                }
                self.timeout_waker = Some(cx.waker().clone());
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

/// The element type produced by a [`Pinger`], representing either a new
/// [`Ping`](super::P2PMessage::Ping)
/// message to send, or an indication that the peer should be timed out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PingerEvent {
    /// A new [`Ping`](super::P2PMessage::Ping) message should be sent.
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
