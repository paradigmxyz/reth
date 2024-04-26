//! [`jsonrpsee`] transport adapter implementation for Unix IPC by using Unix Sockets.

use crate::{client::IpcError, stream_codec::StreamCodec};
use futures::StreamExt;
use jsonrpsee::core::client::{ReceivedMessage, TransportReceiverT, TransportSenderT};
use std::path::Path;
use tokio::{
    io::AsyncWriteExt,
    net::{
        unix::{OwnedReadHalf, OwnedWriteHalf},
        UnixStream,
    },
};
use tokio_util::codec::FramedRead;

/// Sending end of IPC transport.
#[derive(Debug)]
pub(crate) struct Sender {
    inner: OwnedWriteHalf,
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
    pub(crate) inner: FramedRead<OwnedReadHalf, StreamCodec>,
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
    pub(crate) async fn build(
        self,
        path: impl AsRef<Path>,
    ) -> Result<(Sender, Receiver), IpcError> {
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
