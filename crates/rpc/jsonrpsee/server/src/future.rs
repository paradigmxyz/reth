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

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use tokio::sync::watch;

/// Create channel to determine whether
/// the server shall continue to run or not.
pub fn stop_channel() -> (StopHandle, ServerHandle) {
    let (tx, rx) = tokio::sync::watch::channel(());
    (StopHandle::new(rx), ServerHandle::new(tx))
}

/// Represent a stop handle which is a wrapper over a `multi-consumer receiver`
/// and cloning [`StopHandle`] will get a separate instance of the underlying receiver.
#[derive(Debug, Clone)]
pub struct StopHandle(watch::Receiver<()>);

impl StopHandle {
    /// Create a new stop handle.
    pub(crate) fn new(rx: watch::Receiver<()>) -> Self {
        Self(rx)
    }

    /// A future that resolves when server has been stopped
    /// it consumes the stop handle.
    pub async fn shutdown(mut self) {
        let _ = self.0.changed().await;
    }
}

/// Error when the server has already been stopped.
#[derive(Debug, Copy, Clone, thiserror::Error)]
#[error("The server is already stopped")]
pub struct AlreadyStoppedError;

/// Server handle.
///
/// When all [`StopHandle`]'s have been `dropped` or `stop` has been called
/// the server will be stopped.
#[derive(Debug, Clone)]
pub struct ServerHandle(Arc<watch::Sender<()>>);

impl ServerHandle {
    /// Create a new server handle.
    pub(crate) fn new(tx: watch::Sender<()>) -> Self {
        Self(Arc::new(tx))
    }

    /// Tell the server to stop without waiting for the server to stop.
    pub fn stop(&self) -> Result<(), AlreadyStoppedError> {
        self.0.send(()).map_err(|_| AlreadyStoppedError)
    }

    /// Wait for the server to stop.
    pub async fn stopped(self) {
        self.0.closed().await
    }
}

/// Limits the number of connections.
///
/// Permits are only ever tried, never waited for, so they are counted with an atomic: a semaphore
/// would lock its waiter list on every release.
#[derive(Clone, Debug)]
pub struct ConnectionGuard {
    in_use: Arc<AtomicUsize>,
    max: usize,
}

impl ConnectionGuard {
    /// Create a new connection guard.
    pub fn new(limit: usize) -> Self {
        Self { in_use: Arc::new(AtomicUsize::new(0)), max: limit }
    }

    /// Acquire a connection permit.
    pub fn try_acquire(&self) -> Option<ConnectionPermit> {
        self.in_use
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |in_use| {
                (in_use < self.max).then_some(in_use + 1)
            })
            .ok()
            .map(|_| ConnectionPermit(self.in_use.clone()))
    }

    /// Get the number of available connection slots.
    pub fn available_connections(&self) -> usize {
        self.max - self.in_use.load(Ordering::Relaxed)
    }

    /// Get the maximum number of connections.
    pub fn max_connections(&self) -> usize {
        self.max
    }
}

/// Connection permit, which frees its connection slot when dropped.
#[derive(Debug)]
pub struct ConnectionPermit(Arc<AtomicUsize>);

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}
