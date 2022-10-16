use crate::{
    error::EthStreamError,
    types::{EthMessage, ProtocolMessage},
};
use bytes::BytesMut;
use futures::{ready, Sink};
use pin_project::pin_project;
use reth_rlp::{Decodable, Encodable};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio_stream::Stream;

/// An `EthStream` wraps over any `Stream` that yields rlp encoded messages.
#[pin_project]
pub struct EthStream<S> {
    #[pin]
    inner: S,
}

impl<S> Stream for EthStream<S>
where
    S: Stream<Item = Result<bytes::Bytes, io::Error>> + Unpin,
{
    type Item = Result<EthMessage, EthStreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        let res = ready!(this.inner.poll_next(cx));
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
    type Error = EthStreamError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_ready(cx).map_err(Into::into)
    }

    fn start_send(self: Pin<&mut Self>, item: EthMessage) -> Result<(), Self::Error> {
        if matches!(item, EthMessage::Status(_)) {
            return Err(EthStreamError::StatusInHandshake)
        }

        let mut bytes = BytesMut::new();
        ProtocolMessage::from(item).encode(&mut bytes);
        let bytes = bytes.freeze();

        self.project().inner.start_send(bytes)?;

        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_flush(cx).map_err(Into::into)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_close(cx).map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        codec::RlpCodec,
        types::{broadcast::BlockHashNumber, EthMessage},
        EthStream,
    };
    use futures::{SinkExt, StreamExt};
    use reth_ecies::{stream::ECIESStream, util::pk2id};
    use secp256k1::{SecretKey, SECP256K1};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_util::codec::Decoder;

    #[tokio::test]
    async fn can_write_and_read_cleartext() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();
        let test_msg = EthMessage::NewBlockHashes(
            vec![
                BlockHashNumber { hash: reth_primitives::H256::random(), number: 5 },
                BlockHashNumber { hash: reth_primitives::H256::random(), number: 6 },
            ]
            .into(),
        );

        let test_msg_clone = test_msg.clone();
        let handle = tokio::spawn(async move {
            // roughly based off of the design of tokio::net::TcpListener
            let (incoming, _) = listener.accept().await.unwrap();
            let stream = RlpCodec::prepend_header().framed(incoming);
            let mut stream = EthStream { inner: stream };

            // use the stream to get the next message
            let message = stream.next().await.unwrap().unwrap();
            assert_eq!(message, test_msg_clone);
        });

        let outgoing = TcpStream::connect(local_addr).await.unwrap();
        let sink = RlpCodec::prepend_header().framed(outgoing);
        let mut client_stream = EthStream { inner: sink };

        client_stream.send(test_msg).await.unwrap();

        // make sure the server receives the message and asserts before ending the test
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn can_write_and_read_ecies() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();
        let server_key = SecretKey::new(&mut rand::thread_rng());
        let test_msg = EthMessage::NewBlockHashes(
            vec![
                BlockHashNumber { hash: reth_primitives::H256::random(), number: 5 },
                BlockHashNumber { hash: reth_primitives::H256::random(), number: 6 },
            ]
            .into(),
        );

        let test_msg_clone = test_msg.clone();
        let handle = tokio::spawn(async move {
            // roughly based off of the design of tokio::net::TcpListener
            let (incoming, _) = listener.accept().await.unwrap();
            let stream = ECIESStream::incoming(incoming, server_key).await.unwrap();
            let mut stream = EthStream { inner: stream };

            // use the stream to get the next message
            let message = stream.next().await.unwrap().unwrap();
            assert_eq!(message, test_msg_clone);
        });

        // create the server pubkey
        let server_id = pk2id(&server_key.public_key(SECP256K1));

        let client_key = SecretKey::new(&mut rand::thread_rng());
        let outgoing = TcpStream::connect(local_addr).await.unwrap();
        let inner = ECIESStream::connect(outgoing, client_key, server_id).await.unwrap();

        let mut client_stream = EthStream { inner };
        client_stream.send(test_msg).await.unwrap();

        // make sure the server receives the message and asserts before ending the test
        handle.await.unwrap();
    }
}
