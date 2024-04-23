//! [`jsonrpsee`] transport adapter implementation for IPC.

use std::{
    io,
    path::{Path, PathBuf},
};

use jsonrpsee::{
    async_client::{Client, ClientBuilder},
    core::client::{TransportReceiverT, TransportSenderT},
};

#[cfg(unix)]
use crate::client::unix::IpcTransportClientBuilder;
#[cfg(windows)]
use crate::client::win::IpcTransportClientBuilder;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod win;

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
    /// # async fn run_client() -> Result<(), Box<dyn std::error::Error +  Send + Sync>> {
    /// let client = IpcClientBuilder::default().build("/tmp/my-uds").await?;
    /// let response: String = client.request("say_hello", rpc_params![]).await?;
    /// #   Ok(())
    /// # }
    /// ```
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
        path: PathBuf,
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
    use interprocess::local_socket::{
        traits::tokio::Listener, GenericFilePath, ListenerOptions, ToFsName,
    };

    use crate::server::dummy_endpoint;

    use super::*;

    #[tokio::test]
    async fn test_connect() {
        let endpoint = dummy_endpoint();
        let name = endpoint.clone().to_fs_name::<GenericFilePath>().unwrap();
        let binding = ListenerOptions::new().name(name).create_tokio().unwrap();
        tokio::spawn(async move {
            let _x = binding.accept().await;
        });

        let (tx, rx) = IpcTransportClientBuilder::default().build(endpoint).await.unwrap();
        let _ = IpcClientBuilder::default().build_with_tokio(tx, rx);
    }
}
