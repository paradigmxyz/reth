use futures::{Future, StreamExt, ready, FutureExt};
use pin_project::pin_project;
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::{Sleep, interval, sleep};
use tokio_stream::{Stream, wrappers::IntervalStream};

use crate::error::PingerError;

/// This represents the possible states of the pinger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PingState {
    /// There are no pings in flight, or all pings have been responded to and we are ready to send
    /// a ping at a later point.
    Ready,

    /// We have sent a ping and are waiting for a pong, but the peer has missed n pongs.
    WaitingForPong(u8),

    /// The peer has missed n pongs and is considered timed out.
    TimedOut(u8),
}

/// The pinger is a state machine that is created with a maximum number of pongs that can be
/// missed.
#[derive(Debug, Clone)]
pub struct Pinger {
    /// The maximum number of pongs that can be missed.
    max_missed: u8,

    /// The current state of the pinger.
    state: PingState,
}

impl Pinger {
    /// Create a new pinger with the given maximum number of pongs that can be missed.
    pub fn new(max_missed: u8) -> Self {
        Self { max_missed, state: PingState::Ready }
    }

    /// Return the current state of the pinger.
    pub fn state(&self) -> &PingState {
        &self.state
    }

    /// Check if the pinger is in the `Ready` state.
    pub fn is_ready(&self) -> bool {
        matches!(self.state, PingState::Ready)
    }

    /// Check if the pinger is in the `WaitingForPong` state.
    pub fn is_waiting_for_pong(&self) -> bool {
        matches!(self.state, PingState::WaitingForPong(_))
    }

    /// Check if the pinger is in the `TimedOut` state.
    pub fn is_timed_out(&self) -> bool {
        matches!(self.state, PingState::TimedOut(_))
    }

    /// Transition the pinger to the `WaitingForPong` state if it was in the `Ready` state.
    ///
    /// If the pinger is in the `WaitingForPong` state, the number of missed pongs will be
    /// incremented. If the number of missed pongs exceeds the maximum missed pongs allowed, the
    /// pinger will be transitioned to the `TimedOut` state.
    ///
    /// If the pinger is in the `TimedOut` state, this method will return an error.
    pub fn next_state(&mut self) -> Result<(), PingerError> {
        match self.state {
            PingState::Ready => {
                self.state = PingState::WaitingForPong(0);
                Ok(())
            }
            PingState::WaitingForPong(missed) => {
                if missed + 1 >= self.max_missed {
                    self.state = PingState::TimedOut(missed + 1);
                    Ok(())
                } else {
                    self.state = PingState::WaitingForPong(missed + 1);
                    Ok(())
                }
            }
            PingState::TimedOut(_) => Err(PingerError::PingWhileTimedOut),
        }
    }

    /// Mark a pong as received, and transition the pinger to the `Ready` state if it was in the
    /// `WaitingForPong` state.
    ///
    /// If the pinger is in the `Ready` or `TimedOut` state, this method will return an error.
    pub fn pong_received(&mut self) -> Result<(), PingerError> {
        match self.state {
            PingState::Ready => Err(PingerError::PongWhileReady),
            PingState::WaitingForPong(_) => {
                self.state = PingState::Ready;
                Ok(())
            }
            PingState::TimedOut(_) => Err(PingerError::PongWhileTimedOut),
        }
    }
}

/// A Pinger that can be used as a `Stream`, which will emit
#[derive(Debug, Clone)]
pub struct PingerStream {
    /// The pinger.
    pinger: Pinger,

    /// Whether a `Timeout` event has already been sent.
    timeout_sent: bool,
}

impl PingerStream {
    /// Poll the [`Pinger`] for a [`Option<PingEvent>`], which can be either a [`PingEvent::Ping`] or a
    /// final [`PingEvent::Timeout`] event, after which the stream will end and return None.
    pub fn poll(&mut self) -> Option<Result<PingerEvent, PingerError>> {
        // the stream has already sent a timeout event, so we return None
        if self.timeout_sent {
            return None;
        }

        match self.pinger.state {
            PingState::Ready  => {
                // the pinger is ready, send a ping
                match self.pinger.next_state() {
                    Ok(()) => Some(Ok(PingerEvent::Ping)),
                    Err(e) => Some(Err(e)),
                }
            }
            PingState::WaitingForPong(_) => {
                // the peer has not timed out (yet), send another ping if the pinger does
                // not exceed the maximum number of missed pongs
                match self.pinger.next_state() {
                    Ok(()) => {
                        match self.pinger.state() {
                            PingState::TimedOut(_) => {
                                // the pinger has timed out, send a timeout event and end the
                                // stream
                                self.timeout_sent = true;
                                Some(Ok(PingerEvent::Timeout))
                            }
                            _ => {
                                // the pinger is still waiting for a pong, send another ping
                                Some(Ok(PingerEvent::Ping))
                            }
                        }
                    }
                    Err(e) => {Some(Err(e))}
                }
            }
            PingState::TimedOut(_) => {
                self.timeout_sent = true;
                Some(Ok(PingerEvent::Timeout))
            }
        }
    }
}

impl Stream for PingerStream {
    type Item = Result<PingerEvent, PingerError>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.timeout_sent {
            return Poll::Ready(None);
        }

        match self.pinger.state {
            PingState::Ready  => {
                // the pinger is ready, send a ping
                self.pinger.next_state()?;
                Poll::Ready(Some(Ok(PingerEvent::Ping)))
            }
            PingState::WaitingForPong(_) => {
                // the peer has not timed out (yet), send another ping if the pinger does
                // not exceed the maximum number of missed pongs
                self.pinger.next_state()?;
                match self.pinger.state() {
                    PingState::TimedOut(_) => {
                        // the pinger has timed out, send a timeout event
                        Poll::Ready(Some(Ok(PingerEvent::Timeout)))
                    }
                    _ => {
                        // the pinger is still waiting for a pong, send another ping
                        Poll::Ready(Some(Ok(PingerEvent::Ping)))
                    }
                }
            }
            PingState::TimedOut(_) => {
                self.timeout_sent = true;
                Poll::Ready(Some(Ok(PingerEvent::Timeout)))
            }
        }
    }
}

/// A version of [`Pinger`] that contains an [`Interval`], which will stream [`PingerEvent`]s when
/// polled.
/// These [`PingerEvent`]s can be used to determine when to send a ping.
#[derive(Debug)]
pub struct IntervalPingerStream {
    /// The pinger stream.
    pinger_stream: PingerStream,

    /// The interval that determines ping retries.
    interval: IntervalStream,
}

impl IntervalPingerStream {
    /// Creates a new [`IntervalPingerStream`] with the given max missed pongs and interval duration.
    pub fn new(max_missed: u8, interval_duration: Duration) -> Self {
        Self {
            pinger_stream: PingerStream { pinger: Pinger::new(max_missed), timeout_sent: false },
            interval: IntervalStream::new(interval(interval_duration)),
        }
    }

    /// Mark a pong as received, and transition the pinger to the `Ready` state if it was in the
    /// `WaitingForPong` state.
    ///
    /// If the pinger is in the `Ready` or `TimedOut` state, this method will return an error.
    pub fn pong_received(&mut self) -> Result<(), PingerError> {
        self.interval.as_mut().reset();
        self.pinger_stream.pinger.pong_received()
    }
}

/// The element type produced by a [`IntervalPingerStream`], representing either a new [`Ping`]
/// message to send, or an indication that the peer should be timed out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PingerEvent {
    /// A new [`Ping`] message should be sent.
    Ping,

    /// The peer should be timed out.
    Timeout,
}

impl Stream for IntervalPingerStream {
    type Item = Result<PingerEvent, PingerError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        let res = ready!(this.interval.poll_next_unpin(cx));
        if res.is_none() {
            return Poll::Ready(None);
        }

        // poll the interval, only advance the state if the interval is ready
        Poll::Ready(this.pinger_stream.poll())
    }
}

/// A type of [`Pinger`] that uses an interval and a timeout to determine when to send a ping and
/// when to consider the peer timed out.
#[derive(Debug)]
pub struct IntervalTimeoutPinger {
    /// The interval pinger stream.
    interval_stream: IntervalStream,

    /// The pinger stream we are using.
    pinger_stream: PingerStream,

    /// The timeout duration for each ping.
    timeout: Duration,

    /// The Interval that determines when to timeout the peer and send another ping.
    sleep: Option<IntervalStream>,
}

impl IntervalTimeoutPinger {
    /// Creates a new [`IntervalTimeoutPinger`] with the given max missed pongs, interval duration, and
    /// timeout duration.
    pub fn new(max_missed: u8, interval_duration: Duration, timeout_duration: Duration) -> Self {
        Self {
            interval_stream: IntervalStream::new(interval(interval_duration)),
            pinger_stream: PingerStream { pinger: Pinger::new(max_missed), timeout_sent: false },
            timeout: timeout_duration,
            sleep: None,
        }
    }

    /// Mark a pong as received, and transition the pinger to the `Ready` state if it was in the
    /// `WaitingForPong` state. Unsets the sleep timer.
    pub fn pong_received(&mut self) -> Result<(), PingerError> {
        println!("resetting state from {:?}", self.pinger_stream.pinger.state());
        self.interval_stream.as_mut().reset();
        self.pinger_stream.pinger.pong_received()?;
        println!("current state after reset: {:?}", self.pinger_stream.pinger.state());
        self.sleep = None;
        Ok(())
    }

    /// Waits until the pinger sends a timeout event by exhausting the stream.
    pub async fn wait_for_timeout(&mut self) {
        while self.next().await.is_some() {}
    }

    /// Returns the current state of the pinger.
    pub fn state(&self) -> &PingState {
        self.pinger_stream.pinger.state()
    }
}

impl Stream for IntervalTimeoutPinger {
    type Item = Result<PingerEvent, PingerError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // if we have a sleep timer, prefer that over the interval stream
        if let Some(inner_sleep) = this.sleep.as_mut() {
            // if the sleep is pending, we should return pending (we are waiting for a timeout)
            let pinned_sleep = Pin::new(inner_sleep);
            ready!(pinned_sleep.poll_next(cx));

            // the sleep has elapsed, create a new sleep for the next timeout interval, then send a
            // new ping
            this.sleep = Some(IntervalStream::new(interval(this.timeout)));
            println!("pinging sleep");
            println!("state: {:?}", this.pinger_stream.pinger.state());
            let rest = Pin::new(&mut this.pinger_stream).poll_next(cx);
            println!("state: {:?}", this.pinger_stream.pinger.state());
            rest
        } else {
            // first poll the interval stream, if it is ready, send a ping
            let res = ready!(this.interval_stream.poll_next_unpin(cx));
            if res.is_none() {
                // this should never happen (the Stream impl of IntervalStream never is always Some)
                return Poll::Ready(None);
            }

            let pinned_stream = Pin::new(&mut this.pinger_stream);
            let stream_res = ready!(pinned_stream.poll_next(cx));
            match stream_res {
                Some(Ok(PingerEvent::Ping)) => {
                    // the pinger is ready, set the sleep to Some
                    this.sleep = Some(IntervalStream::new(interval(this.timeout)));
                    println!("pinging interval");
                    println!("state: {:?}", this.pinger_stream.pinger.state());
                    Poll::Ready(Some(Ok(PingerEvent::Ping)))
                }
                Some(Ok(PingerEvent::Timeout)) => {
                    // the pinger has timed out, set the sleep to Some
                    Poll::Ready(Some(Ok(PingerEvent::Timeout)))
                }
                Some(Err(e)) => Poll::Ready(Some(Err(e))),
                None => Poll::Ready(None),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures::pin_mut;
    use tokio::select;

    use super::*;

    #[test]
    fn send_many_pings() {
        // tests the simple pinger by sending many pings without pongs
        let mut pinger = Pinger::new(3);

        pinger.next_state().unwrap();
        assert_eq!(*pinger.state(), PingState::WaitingForPong(0));

        pinger.next_state().unwrap();
        assert_eq!(*pinger.state(), PingState::WaitingForPong(1));

        pinger.next_state().unwrap();
        assert_eq!(*pinger.state(), PingState::WaitingForPong(2));

        pinger.next_state().unwrap();
        assert_eq!(*pinger.state(), PingState::TimedOut(3));
    }

    #[test]
    fn send_many_pings_with_pongs() {
        // tests the simple pinger by sending many pings with pongs
        let mut pinger = Pinger::new(3);

        pinger.next_state().unwrap();
        assert_eq!(*pinger.state(), PingState::WaitingForPong(0));

        pinger.pong_received().unwrap();
        assert_eq!(*pinger.state(), PingState::Ready);

        pinger.next_state().unwrap();
        assert_eq!(*pinger.state(), PingState::WaitingForPong(0));

        pinger.pong_received().unwrap();
        assert_eq!(*pinger.state(), PingState::Ready);
    }

    #[test]
    fn send_many_pings_stream() {
        let mut pinger_stream = PingerStream { pinger: Pinger::new(3), timeout_sent: false };

        assert_eq!(pinger_stream.poll().unwrap().unwrap(), PingerEvent::Ping);
        assert_eq!(pinger_stream.poll().unwrap().unwrap(), PingerEvent::Ping);
        assert_eq!(pinger_stream.poll().unwrap().unwrap(), PingerEvent::Ping);
        assert_eq!(pinger_stream.poll().unwrap().unwrap(), PingerEvent::Timeout);
    }

    #[tokio::test]
    async fn send_many_pings_interval() {
        // here we just need to wait for the interval to elapse and we should be able to assert
        // that the pinger times out after not receiving a pong
        let mut pinger = IntervalPingerStream::new(3, Duration::from_millis(10));

        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);
        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);
        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);
        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Timeout);
    }

    #[tokio::test]
    async fn send_many_pings_interval_with_pongs() {
        // here we need to wait for the interval to elapse and then receive a pong before the
        // interval elapses again
        let mut pinger = IntervalPingerStream::new(3, Duration::from_millis(10));

        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);
        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);

        pinger.pong_received().unwrap();

        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);
        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);
        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);
        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Timeout);
    }

    #[tokio::test]
    async fn send_many_pings_interval_timeout() {
        // we should wait for the interval to elapse, just like the interval-only version
        // TODO: should the timeout ever be less than the interval?
        let pinger = IntervalTimeoutPinger::new(3, Duration::from_millis(20), Duration::from_millis(10));
        pin_mut!(pinger);

        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);
        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);
        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);
        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Timeout);
    }

    #[tokio::test]
    async fn send_many_pings_interval_timeout_with_pongs() {
        // we should wait for the interval to elapse and receive a pong before the timeout elapses

        let mut pinger = IntervalTimeoutPinger::new(3, Duration::from_millis(20), Duration::from_millis(10));

        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);
        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);

        pinger.pong_received().unwrap();

        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);
        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);
        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);
        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Timeout);
    }

    #[tokio::test]
    async fn check_timing_over_interval() {
        // send pongs after a ping event, timing the interval between the two
        let mut pinger = IntervalTimeoutPinger::new(3, Duration::from_millis(20), Duration::from_millis(10));

        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);
        pinger.pong_received().unwrap();

        // wait for the interval to elapse, and compare it to the interval ping
        let sleep = tokio::time::sleep(Duration::from_millis(21));
        let wait_for_timeout = pinger.next();

        select! {
            _ = sleep => panic!("interval should have elapsed"),
            _ = wait_for_timeout => {}
        }
    }

    #[tokio::test]
    async fn check_timing_under_interval() {
        // send pongs after a ping event, timing the interval between the two
        let mut pinger = IntervalTimeoutPinger::new(3, Duration::from_millis(20), Duration::from_millis(10));

        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);
        pinger.pong_received().unwrap();

        // wait for the interval to elapse, and compare it to the interval ping
        // to avoid flakiness let's do 15?
        let sleep = tokio::time::sleep(Duration::from_millis(19));
        let wait_for_timeout = pinger.next();

        select! {
            _ = sleep => {}
            _ = wait_for_timeout => panic!("sleep should have elapsed first")
        }
    }

    #[tokio::test]
    async fn timeout_with_pongs() {
        // we should wait for the interval to elapse and receive a pong before the timeout elapses
        let mut pinger = IntervalTimeoutPinger::new(3, Duration::from_millis(20), Duration::from_millis(10));

        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);
        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);

        pinger.pong_received().unwrap();

        // let's wait for the timeout to elapse (3 ping timeouts + interval)
        // TODO: fix this test - this should fail
        let sleep = tokio::time::sleep(Duration::from_millis(31));
        let wait_for_timeout = pinger.wait_for_timeout();

        select! {
            _ = sleep => panic!("timeout should have elapsed by now"),
            _ = wait_for_timeout => (),
        }

        assert!(false)
    }
}
