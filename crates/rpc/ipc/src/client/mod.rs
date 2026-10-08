//! IPC client.

use crate::stream_codec::StreamCodec;
use futures::TryFutureExt;
use interprocess::local_socket::{tokio::prelude::*, GenericFilePath};
use reth_jasonrpeesea::client::{Client, ClientBuilder};
use std::{io, time::Duration};
use tokio_util::codec::{FramedRead, FramedWrite};

/// Builder type for [`Client`]
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct IpcClientBuilder {
    request_timeout: Duration,
}

impl Default for IpcClientBuilder {
    fn default() -> Self {
        Self { request_timeout: Duration::from_secs(60) }
    }
}

impl IpcClientBuilder {
    /// Connects to an IPC socket
    ///
    /// ```
    /// use reth_ipc::client::IpcClientBuilder;
    /// use reth_jasonrpeesea::{client::ClientT, rpc_params};
    ///
    /// # async fn run_client() -> Result<(), Box<dyn core::error::Error +  Send + Sync>> {
    /// let client = IpcClientBuilder::default().build("/tmp/my-uds").await?;
    /// let response: String = client.request("say_hello", rpc_params![]).await?;
    /// # Ok(()) }
    /// ```
    pub async fn build(self, name: &str) -> Result<Client, IpcError> {
        let conn = async { name.to_fs_name::<GenericFilePath>() }
            .and_then(LocalSocketStream::connect)
            .await
            .map_err(|err| IpcError::FailedToConnect { path: name.to_string(), err })?;
        let (recv, send) = conn.split();
        Ok(ClientBuilder::default().request_timeout(self.request_timeout).build(
            FramedRead::new(recv, StreamCodec::stream_incoming()),
            FramedWrite::new(send, StreamCodec::stream_incoming()),
        ))
    }

    /// Set request timeout (default is 60 seconds).
    pub const fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }
}

/// Error variants that can happen in IPC transport.
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::dummy_name;
    use interprocess::local_socket::ListenerOptions;

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

        IpcClientBuilder::default().build(name).await.unwrap();
    }
}
