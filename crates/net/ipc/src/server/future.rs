// Copyright 2019-2021 Parity Technologies (UK) Ltd.
//
// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:
//
// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

//! Utilities for handling async code.

use futures::FutureExt;
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{
    sync::{watch, OwnedSemaphorePermit, Semaphore, TryAcquireError},
    time::{self, Duration, Interval},
};

/// Polling for server stop monitor interval in milliseconds.
const STOP_MONITOR_POLLING_INTERVAL: Duration = Duration::from_millis(1000);

/// This is a flexible collection of futures that need to be driven to completion
/// alongside some other future, such as connection handlers that need to be
/// handled along with a listener for new connections.
///
/// In order to `.await` on these futures and drive them to completion, call
/// `select_with` providing some other future, the result of which you need.
pub(crate) struct FutureDriver<F> {
    futures: Vec<F>,
    stop_monitor_heartbeat: Interval,
}

impl<F> Default for FutureDriver<F> {
    fn default() -> Self {
        let mut heartbeat = time::interval(STOP_MONITOR_POLLING_INTERVAL);

        heartbeat.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

        FutureDriver { futures: Vec::new(), stop_monitor_heartbeat: heartbeat }
    }
}

impl<F> FutureDriver<F> {
    /// Add a new future to this driver
    pub(crate) fn add(&mut self, future: F) {
        self.futures.push(future);
    }
}

impl<F> FutureDriver<F>
where
    F: Future + Unpin,
{
    pub(crate) async fn select_with<S: Future>(&mut self, selector: S) -> S::Output {
        tokio::pin!(selector);

        DriverSelect { selector, driver: self }.await
    }

    fn drive(&mut self, cx: &mut Context<'_>) {
        let mut i = 0;

        while i < self.futures.len() {
            if self.futures[i].poll_unpin(cx).is_ready() {
                // Using `swap_remove` since we don't care about ordering
                // but we do care about removing being `O(1)`.
                //
                // We don't increment `i` in this branch, since we now
                // have a shorter length, and potentially a new value at
                // current index
                self.futures.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }

    fn poll_stop_monitor_heartbeat(&mut self, cx: &mut Context<'_>) {
        // We don't care about the ticks of the heartbeat, it's here only
        // to periodically wake the `Waker` on `cx`.
        let _ = self.stop_monitor_heartbeat.poll_tick(cx);
    }
}

impl<F> Future for FutureDriver<F>
where
    F: Future + Unpin,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = Pin::into_inner(self);

        this.drive(cx);

        if this.futures.is_empty() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

/// This is a glorified select `Future` that will attempt to drive all
/// connection futures `F` to completion on each `poll`, while also
/// handling incoming connections.
struct DriverSelect<'a, S, F> {
    selector: S,
    driver: &'a mut FutureDriver<F>,
}

impl<'a, R, F> Future for DriverSelect<'a, R, F>
where
    R: Future + Unpin,
    F: Future + Unpin,
{
    type Output = R::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = Pin::into_inner(self);

        this.driver.drive(cx);
        this.driver.poll_stop_monitor_heartbeat(cx);

        this.selector.poll_unpin(cx)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StopHandle(watch::Receiver<()>);

impl StopHandle {
    pub(crate) fn new(rx: watch::Receiver<()>) -> Self {
        Self(rx)
    }

    pub(crate) fn shutdown_requested(&self) -> bool {
        // if a message has been seen, it means that `stop` has been called.
        self.0.has_changed().unwrap_or(true)
    }

    pub(crate) async fn shutdown(&mut self) {
        // Err(_) implies that the `sender` has been dropped.
        // Ok(_) implies that `stop` has been called.
        let _ = self.0.changed().await;
    }
}

/// Server handle.
///
/// When all [`StopHandle`]'s have been `dropped` or `stop` has been called
/// the server will be stopped.
#[derive(Debug, Clone)]
pub(crate) struct ServerHandle(Arc<watch::Sender<()>>);

impl ServerHandle {
    /// Wait for the server to stop.
    #[allow(unused)]
    pub(crate) async fn stopped(self) {
        self.0.closed().await
    }
}

/// Limits the number of connections.
pub(crate) struct ConnectionGuard(Arc<Semaphore>);

impl ConnectionGuard {
    pub(crate) fn new(limit: usize) -> Self {
        Self(Arc::new(Semaphore::new(limit)))
    }

    pub(crate) fn try_acquire(&self) -> Option<OwnedSemaphorePermit> {
        match self.0.clone().try_acquire_owned() {
            Ok(guard) => Some(guard),
            Err(TryAcquireError::Closed) => {
                unreachable!("Semaphore::Close is never called and can't be closed; qed")
            }
            Err(TryAcquireError::NoPermits) => None,
        }
    }

    #[allow(unused)]
    pub(crate) fn available_connections(&self) -> usize {
        self.0.available_permits()
    }
}
