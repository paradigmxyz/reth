//! [`jsonrpsee`] transport adapter implementation for IPC.

use jsonrpsee::core::client::{ReceivedMessage, TransportReceiverT, TransportSenderT};

/// Sending end of WebSocket transport.
#[derive(Debug)]
pub struct Sender {
    // inner: connection::Sender<BufReader<BufWriter<EitherStream>>>,
}

#[async_trait::async_trait]
impl TransportSenderT for Sender {
    type Error = IpcError;

    /// Sends out a request. Returns a Future that finishes when the request has been successfully
    /// sent.
    async fn send(&mut self, msg: String) -> Result<(), Self::Error> {
        todo!()
    }

    async fn send_ping(&mut self) -> Result<(), Self::Error> {
        tracing::trace!("send ping - not implemented");
        Err(IpcError::NotSupported)
    }

    /// Close the connection.
    async fn close(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Receiving end of WebSocket transport.
#[derive(Debug)]
pub struct Receiver {
    // inner: connection::Receiver<BufReader<BufWriter<EitherStream>>>,
}

#[async_trait::async_trait]
impl TransportReceiverT for Receiver {
    type Error = IpcError;

    /// Returns a Future resolving when the server sent us something back.
    async fn receive(&mut self) -> Result<ReceivedMessage, Self::Error> {
        todo!()
    }
}

/// Builder for a IPC transport [`Sender`] and ['Receiver`] pair.
#[derive(Debug)]
pub struct IpcTransportClientBuilder {
    /// Max payload size.
    pub max_request_body_size: u32,
}

impl Default for IpcTransportClientBuilder {
    fn default() -> Self {
        todo!()
    }
}

/// Error variants that can happen in IPC transport.
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    /// Operation not supported
    #[error("Operation not supported")]
    NotSupported,
}
