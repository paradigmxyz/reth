use crate::types::{EthMessage, ProtocolMessage};
use bytes::BytesMut;
use reth_rlp::{Decodable, Encodable};

use futures::{ready, Sink};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio_stream::Stream;

/// An EthStream wraps over any raw Bytes stream and ser/deserializes the
/// messages from/to RLP.
struct EthStream<S> {
    stream: S,
}

#[derive(thiserror::Error, Debug)]
enum EthStreamError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Rlp(#[from] reth_rlp::DecodeError),
    #[error("status message can only be recv/sent in handshake")]
    StatusInHandshake,
}

impl<S> Stream for EthStream<S>
where
    S: Stream<Item = Result<bytes::Bytes, io::Error>> + Unpin,
{
    type Item = Result<EthMessage, EthStreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let stream = Pin::new(&mut self.get_mut().stream);
        let res = ready!(stream.poll_next(cx));
        let bytes = match res {
            Some(Ok(bytes)) => bytes,
            Some(Err(err)) => return Poll::Ready(Some(Err(err.into()))),
            None => return Poll::Ready(None),
        };

        let msg = match ProtocolMessage::decode(&mut bytes.as_ref()) {
            Ok(m) => m,
            Err(err) => return Poll::Ready(Some(Err(err.into()))),
        };

        if matches!(msg.message, EthMessage::Status(_)) {
            return Poll::Ready(Some(Err(EthStreamError::StatusInHandshake)))
        }

        Poll::Ready(Some(Ok(msg.message)))
    }
}

impl<S> Sink<EthMessage> for EthStream<S>
where
    S: Sink<bytes::Bytes, Error = io::Error> + Unpin,
{
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().stream).poll_ready(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: EthMessage) -> Result<(), Self::Error> {
        let mut bytes = BytesMut::new();
        item.encode(&mut bytes);
        let bytes = bytes.freeze();

        Pin::new(&mut self.get_mut().stream).start_send(bytes)?;

        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().stream).poll_close(cx)
    }
}
