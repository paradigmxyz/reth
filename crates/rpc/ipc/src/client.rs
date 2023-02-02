//! [`jsonrpsee`] transport adapter implementation for IPC.

use crate::stream_codec::StreamCodec;
use futures::StreamExt;
use jsonrpsee::{
    async_client::{Client, ClientBuilder},
    core::client::{ReceivedMessage, TransportReceiverT, TransportSenderT},
};
use std::{
    io,
    path::{Path, PathBuf},
};
use tokio::{io::AsyncWriteExt, net::UnixStream};
use tokio_util::codec::FramedRead;

/// Builder type for [`Client`]
#[derive(Clone, Default, Debug)]
#[non_exhaustive]
pub struct IpcClientBuilder;

impl IpcClientBuilder {
    /// Connects to a IPC socket
    pub async fn build(self, path: impl AsRef<Path>) -> Result<Client, IpcError> {
        let (tx, rx) = IpcTransportClientBuilder::default().build(path).await?;
        Ok(self.build_with_tokio(tx, rx))
    }

    /// Uses the sender and receiver channels to connect to the socket.
    pub fn build_with_tokio<S, R>(self, sender: S, receiver: R) -> Client
    where
        S: TransportSenderT + Send,
        R: TransportReceiverT + Send,
    {
        ClientBuilder::default().build_with_tokio(sender, receiver)
    }
}

/// Sending end of IPC transport.
#[derive(Debug)]
pub struct Sender {
    inner: tokio::net::unix::OwnedWriteHalf,
}

#[async_trait::async_trait]
impl TransportSenderT for Sender {
    type Error = IpcError;

    /// Sends out a request. Returns a Future that finishes when the request has been successfully
    /// sent.
    async fn send(&mut self, msg: String) -> Result<(), Self::Error> {
        Ok(self.inner.write_all(msg.as_bytes()).await?)
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

/// Receiving end of IPC transport.
#[derive(Debug)]
pub struct Receiver {
    inner: FramedRead<tokio::net::unix::OwnedReadHalf, StreamCodec>,
}

#[async_trait::async_trait]
impl TransportReceiverT for Receiver {
    type Error = IpcError;

    /// Returns a Future resolving when the server sent us something back.
    async fn receive(&mut self) -> Result<ReceivedMessage, Self::Error> {
        match self.inner.next().await {
            None => Err(IpcError::Closed),
            Some(val) => Ok(ReceivedMessage::Text(val?)),
        }
    }
}

/// Builder for IPC transport [`Sender`] and ['Receiver`] pair.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct IpcTransportClientBuilder;

impl IpcTransportClientBuilder {
    /// Try to establish the connection.
    ///
    /// ```
    /// use jsonrpsee::rpc_params;
    /// use reth_ipc::client::IpcClientBuilder;
    /// use jsonrpsee::core::client::ClientT;
    /// # async fn run_client() -> Result<(), Box<dyn std::error::Error +  Send + Sync>> {
    ///     let client = IpcClientBuilder::default().build("/tmp/my-uds").await?;
    ///     let response: String = client.request("say_hello", rpc_params![]).await?;
    /// #   Ok(())
    /// # }
    /// ```
    pub async fn build(self, path: impl AsRef<Path>) -> Result<(Sender, Receiver), IpcError> {
        let path = path.as_ref();
        let stream = UnixStream::connect(path)
            .await
            .map_err(|err| IpcError::FailedToConnect { path: path.to_path_buf(), err })?;

        let (rhlf, whlf) = stream.into_split();

        Ok((
            Sender { inner: whlf },
            Receiver { inner: FramedRead::new(rhlf, StreamCodec::stream_incoming()) },
        ))
    }
}

/// Error variants that can happen in IPC transport.
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum IpcError {
    /// Operation not supported
    #[error("Operation not supported")]
    NotSupported,
    /// Stream was closed
    #[error("Stream closed")]
    Closed,
    /// Thrown when failed to establish a socket connection.
    #[error("Failed to connect to socket {path}: {err}")]
    FailedToConnect {
        /// The path of the socket.
        path: PathBuf,
        err: io::Error,
    },
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use parity_tokio_ipc::{dummy_endpoint, Endpoint};

    #[tokio::test]
    async fn test_connect() {
        let endpoint = dummy_endpoint();
        let _incoming = Endpoint::new(endpoint.clone()).incoming().unwrap();

        let (tx, rx) = IpcTransportClientBuilder::default().build(endpoint).await.unwrap();
        let _ = IpcClientBuilder::default().build_with_tokio(tx, rx);
    }
}
