//! The ECIES Stream implementation which wraps over [`AsyncRead`] and [`AsyncWrite`].

use crate::{
    codec::ECIESCodec, error::ECIESErrorImpl, ECIESError, EgressECIESValue, IngressECIESValue,
};
use alloy_primitives::{
    bytes::{Bytes, BytesMut},
    B512 as PeerId,
};
use futures::{ready, Sink, SinkExt};
use secp256k1::SecretKey;
use std::{
    fmt::Debug,
    io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    time::timeout,
};
use tokio_stream::{Stream, StreamExt};
use tokio_util::codec::{Decoder, Framed};
use tracing::{instrument, trace};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// `ECIES` stream over TCP exchanging raw bytes
#[derive(Debug)]
#[pin_project::pin_project]
pub struct ECIESStream<Io> {
    #[pin]
    stream: Framed<Io, ECIESCodec>,
    remote_id: PeerId,
}

impl<Io> ECIESStream<Io>
where
    Io: AsyncRead + AsyncWrite + Unpin,
{
    /// Connect to an `ECIES` server
    #[instrument(skip(transport, secret_key))]
    pub async fn connect(
        transport: Io,
        secret_key: SecretKey,
        remote_id: PeerId,
    ) -> Result<Self, ECIESError> {
        Self::connect_with_timeout(transport, secret_key, remote_id, HANDSHAKE_TIMEOUT).await
    }

    /// Wrapper around `connect_no_timeout` which enforces a timeout.
    pub async fn connect_with_timeout(
        transport: Io,
        secret_key: SecretKey,
        remote_id: PeerId,
        timeout_limit: Duration,
    ) -> Result<Self, ECIESError> {
        timeout(timeout_limit, Self::connect_without_timeout(transport, secret_key, remote_id))
            .await
            .map_err(|_| ECIESError::from(ECIESErrorImpl::StreamTimeout))?
    }

    /// Connect to an `ECIES` server with no timeout.
    pub async fn connect_without_timeout(
        transport: Io,
        secret_key: SecretKey,
        remote_id: PeerId,
    ) -> Result<Self, ECIESError> {
        let ecies = ECIESCodec::new_client(secret_key, remote_id)
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "invalid handshake"))?;

        let mut transport = ecies.framed(transport);

        trace!("sending ecies auth ...");
        transport.send(EgressECIESValue::Auth).await?;

        trace!("waiting for ecies ack ...");

        let msg = transport.try_next().await?;

        // `Framed` returns `None` if the underlying stream is no longer readable, and the codec is
        // unable to decode another message from the (partially filled) buffer. This usually happens
        // if the remote drops the TcpStream.
        let msg = msg.ok_or(ECIESErrorImpl::UnreadableStream)?;

        trace!("parsing ecies ack ...");
        if matches!(msg, IngressECIESValue::Ack) {
            Ok(Self { stream: transport, remote_id })
        } else {
            Err(ECIESErrorImpl::InvalidHandshake {
                expected: IngressECIESValue::Ack,
                msg: Some(msg),
            }
            .into())
        }
    }

    /// Listen on a just connected ECIES client
    pub async fn incoming(transport: Io, secret_key: SecretKey) -> Result<Self, ECIESError> {
        let ecies = ECIESCodec::new_server(secret_key)?;

        trace!("incoming ecies stream");
        let mut transport = ecies.framed(transport);
        let msg = transport.try_next().await?;

        trace!("receiving ecies auth");
        let remote_id = match &msg {
            Some(IngressECIESValue::AuthReceive(remote_id)) => *remote_id,
            _ => {
                return Err(ECIESErrorImpl::InvalidHandshake {
                    expected: IngressECIESValue::AuthReceive(Default::default()),
                    msg,
                }
                .into())
            }
        };

        trace!("sending ecies ack");
        transport.send(EgressECIESValue::Ack).await?;

        Ok(Self { stream: transport, remote_id })
    }

    /// Get the remote id
    pub const fn remote_id(&self) -> PeerId {
        self.remote_id
    }
}

impl<Io> Stream for ECIESStream<Io>
where
    Io: AsyncRead + Unpin,
{
    type Item = Result<BytesMut, io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match ready!(self.project().stream.poll_next(cx)) {
            Some(Ok(IngressECIESValue::Message(body))) => Poll::Ready(Some(Ok(body))),
            Some(other) => Poll::Ready(Some(Err(io::Error::new(
                io::ErrorKind::Other,
                format!("ECIES stream protocol error: expected message, received {other:?}"),
            )))),
            None => Poll::Ready(None),
        }
    }
}

impl<Io> Sink<Bytes> for ECIESStream<Io>
where
    Io: AsyncWrite + Unpin,
{
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().stream.poll_ready(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        self.project().stream.start_send(EgressECIESValue::Message(item))?;
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().stream.poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().stream.poll_close(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_network_peers::pk2id;
    use secp256k1::SECP256K1;
    use tokio::net::{TcpListener, TcpStream};

    #[tokio::test]
    async fn can_write_and_read() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_key = SecretKey::new(&mut rand::thread_rng());

        let handle = tokio::spawn(async move {
            // roughly based off of the design of tokio::net::TcpListener
            let (incoming, _) = listener.accept().await.unwrap();
            let mut stream = ECIESStream::incoming(incoming, server_key).await.unwrap();

            // use the stream to get the next message
            let message = stream.next().await.unwrap().unwrap();
            assert_eq!(message, Bytes::from("hello"));
        });

        // create the server pubkey
        let server_id = pk2id(&server_key.public_key(SECP256K1));

        let client_key = SecretKey::new(&mut rand::thread_rng());
        let outgoing = TcpStream::connect(addr).await.unwrap();
        let mut client_stream =
            ECIESStream::connect(outgoing, client_key, server_id).await.unwrap();
        client_stream.send(Bytes::from("hello")).await.unwrap();

        // make sure the server receives the message and asserts before ending the test
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn connection_should_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_key = SecretKey::new(&mut rand::thread_rng());

        let _handle = tokio::spawn(async move {
            // Delay accepting the connection for longer than the client's timeout period
            tokio::time::sleep(Duration::from_secs(11)).await;
            let (incoming, _) = listener.accept().await.unwrap();
            let mut stream = ECIESStream::incoming(incoming, server_key).await.unwrap();

            // use the stream to get the next message
            let message = stream.next().await.unwrap().unwrap();
            assert_eq!(message, Bytes::from("hello"));
        });

        // create the server pubkey
        let server_id = pk2id(&server_key.public_key(SECP256K1));

        let client_key = SecretKey::new(&mut rand::thread_rng());
        let outgoing = TcpStream::connect(addr).await.unwrap();

        // Attempt to connect, expecting a timeout due to the server's delayed response
        let connect_result = ECIESStream::connect_with_timeout(
            outgoing,
            client_key,
            server_id,
            Duration::from_secs(1),
        )
        .await;

        // Assert that a timeout error occurred
        assert!(
            matches!(connect_result, Err(e) if e.to_string() == ECIESErrorImpl::StreamTimeout.to_string())
        );
    }
}
