//! [`jsonrpsee`] transport adapter implementation for Windows IPC by using NamedPipes.

use crate::{client::IpcError, stream_codec::StreamCodec};
use jsonrpsee::core::client::{ReceivedMessage, TransportReceiverT, TransportSenderT};
use std::{path::Path, sync::Arc};
use tokio::{
    io::AsyncWriteExt,
    net::windows::named_pipe::{ClientOptions, NamedPipeClient},
    time,
    time::Duration,
};
use tokio_stream::StreamExt;
use tokio_util::codec::FramedRead;
use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;

/// Sending end of IPC transport.
#[derive(Debug)]
pub struct Sender {
    inner: Arc<NamedPipeClient>,
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
    inner: FramedRead<Arc<NamedPipeClient>, StreamCodec>,
}

#[async_trait::async_trait]
impl TransportReceiverT for Receiver {
    type Error = IpcError;

    /// Returns a Future resolving when the server sent us something back.
    async fn receive(&mut self) -> Result<ReceivedMessage, Self::Error> {
        self.inner.next().await.map_or(Err(IpcError::Closed), |val| Ok(ReceivedMessage::Text(val?)))
    }
}

/// Builder for IPC transport [`crate::client::win::Sender`] and [`crate::client::win::Receiver`]
/// pair.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct IpcTransportClientBuilder;

impl IpcTransportClientBuilder {
    pub async fn build(self, path: impl AsRef<Path>) -> Result<(Sender, Receiver), IpcError> {
        let addr = path.as_ref().as_os_str();
        let client = loop {
            match ClientOptions::new().open(addr) {
                Ok(client) => break client,
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => (),
                Err(e) => return IpcError::FailedToConnect { path: path.to_path_buf(), err: e },
            }
            time::sleep(Duration::from_mills(50)).await;
        };
        let client = Arc::new(client);
        Ok((
            Sender { inner: client.clone() },
            Receiver { inner: FramedRead::new(client, StreamCodec::stream_incoming()) },
        ))
    }
}
