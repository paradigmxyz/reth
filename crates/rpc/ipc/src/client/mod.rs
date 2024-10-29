//! [`jsonrpsee`] transport adapter implementation for IPC.

use crate::stream_codec::StreamCodec;
use futures::{StreamExt, TryFutureExt};
use interprocess::local_socket::{
    tokio::{prelude::*, RecvHalf, SendHalf},
    GenericFilePath,
};
use jsonrpsee::{
    async_client::{Client, ClientBuilder},
    core::client::{ReceivedMessage, TransportReceiverT, TransportSenderT},
};
use std::io;
use tokio::io::AsyncWriteExt;
use tokio_util::codec::FramedRead;

/// Sending end of IPC transport.
#[derive(Debug)]
pub(crate) struct Sender {
    inner: SendHalf,
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
pub(crate) struct Receiver {
    pub(crate) inner: FramedRead<RecvHalf, StreamCodec>,
}

#[async_trait::async_trait]
impl TransportReceiverT for Receiver {
    type Error = IpcError;

    /// Returns a Future resolving when the server sent us something back.
    async fn receive(&mut self) -> Result<ReceivedMessage, Self::Error> {
        self.inner.next().await.map_or(Err(IpcError::Closed), |val| Ok(ReceivedMessage::Text(val?)))
    }
}

/// Builder for IPC transport [`Sender`] and [`Receiver`] pair.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub(crate) struct IpcTransportClientBuilder;

impl IpcTransportClientBuilder {
    pub(crate) async fn build(self, path: &str) -> Result<(Sender, Receiver), IpcError> {
        let conn = async { path.to_fs_name::<GenericFilePath>() }
            .and_then(LocalSocketStream::connect)
            .await
            .map_err(|err| IpcError::FailedToConnect { path: path.to_string(), err })?;

        let (recv, send) = conn.split();

        Ok((
            Sender { inner: send },
            Receiver { inner: FramedRead::new(recv, StreamCodec::stream_incoming()) },
        ))
    }
}

/// Builder type for [`Client`]
#[derive(Clone, Default, Debug)]
#[non_exhaustive]
pub struct IpcClientBuilder;

impl IpcClientBuilder {
    /// Connects to a IPC socket
    ///
    /// ```
    /// use jsonrpsee::{core::client::ClientT, rpc_params};
    /// use reth_ipc::client::IpcClientBuilder;
    ///
    /// # async fn run_client() -> Result<(), Box<dyn core::error::Error +  Send + Sync>> {
    /// let client = IpcClientBuilder::default().build("/tmp/my-uds").await?;
    /// let response: String = client.request("say_hello", rpc_params![]).await?;
    /// # Ok(()) }
    /// ```
    pub async fn build(self, name: &str) -> Result<Client, IpcError> {
        let (tx, rx) = IpcTransportClientBuilder::default().build(name).await?;
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

/// Error variants that can happen in IPC transport.
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    /// Operation not supported
    #[error("operation not supported")]
    NotSupported,
    /// Stream was closed
    #[error("stream closed")]
    Closed,
    /// Thrown when failed to establish a socket connection.
    #[error("failed to connect to socket {path}: {err}")]
    FailedToConnect {
        /// The path of the socket.
        #[doc(hidden)]
        path: String,
        /// The error occurred while connecting.
        #[doc(hidden)]
        err: io::Error,
    },
    /// Wrapped IO Error
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use interprocess::local_socket::ListenerOptions;

    use super::*;
    use crate::server::dummy_name;

    #[tokio::test]
    async fn test_connect() {
        let name = &dummy_name();

        let binding = ListenerOptions::new()
            .name(name.as_str().to_fs_name::<GenericFilePath>().unwrap())
            .create_tokio()
            .unwrap();
        tokio::spawn(async move {
            let _x = binding.accept().await;
        });

        let (tx, rx) = IpcTransportClientBuilder::default().build(name).await.unwrap();
        let _ = IpcClientBuilder::default().build_with_tokio(tx, rx);
    }
}
