use std::sync::Arc;
use tokio::sync::watch;

/// Creates a pair of handles to stop a server and to observe the stop signal.
pub fn stop_channel() -> (StopHandle, ServerHandle) {
    let (tx, rx) = watch::channel(());
    (StopHandle(rx), ServerHandle(Arc::new(tx)))
}

/// Observes the stop signal of a server.
///
/// The server is stopped once every [`StopHandle`] is dropped.
#[derive(Clone, Debug)]
pub struct StopHandle(watch::Receiver<()>);

impl StopHandle {
    /// Resolves once the server is told to stop.
    pub async fn shutdown(mut self) {
        let _ = self.0.changed().await;
    }
}

/// Handle to stop a running server.
#[derive(Clone, Debug)]
pub struct ServerHandle(Arc<watch::Sender<()>>);

impl ServerHandle {
    /// Tells the server to stop.
    pub fn stop(&self) -> Result<(), AlreadyStoppedError> {
        self.0.send(()).map_err(|_| AlreadyStoppedError)
    }

    /// Resolves once the server has stopped.
    pub async fn stopped(self) {
        self.0.closed().await
    }

    /// Returns `true` if the server has stopped.
    pub fn is_stopped(&self) -> bool {
        self.0.is_closed()
    }
}

/// Error returned when stopping a server that already stopped.
#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("the server is already stopped")]
pub struct AlreadyStoppedError;
