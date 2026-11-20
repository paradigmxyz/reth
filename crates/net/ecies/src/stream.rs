//! The ECIES Stream implementation which wraps over [`AsyncRead`] and [`AsyncWrite`].

use crate::{
    codec::{ECIESCodec, INITIAL_BUFFER_CAPACITY, MAX_RETAINED_BUFFER_CAPACITY},
    error::ECIESErrorImpl,
    ECIESError, EgressECIESValue, IngressECIESValue,
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

/// Write buffer size at which the underlying `Framed` transport starts flushing frames to the
/// socket from `poll_ready`, replacing the tokio-util default of 8KiB.
///
/// `RLPx` callers queue batches of messages (e.g. transaction broadcasts) and drive one flush per
/// batch; the 8KiB default splits such a batch into one write syscall per few messages. A larger
/// boundary batches them into fewer, larger writes. A flush keeps the grown write buffer up to
/// 256KiB, so this boundary plus one gossip frame also bounds the resident write buffer per
/// connection.
pub const DEFAULT_BACKPRESSURE_BOUNDARY: usize = 64 * 1024;

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
    #[instrument(level = "trace", target = "net::ecies", skip(transport, secret_key))]
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
        let ecies = ECIESCodec::new_client(secret_key, remote_id)?;

        let mut transport = ecies.framed(transport);
        transport.set_backpressure_boundary(DEFAULT_BACKPRESSURE_BOUNDARY);

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
        transport.set_backpressure_boundary(DEFAULT_BACKPRESSURE_BOUNDARY);
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

impl<Io> ECIESStream<Io> {
    /// Sets the write buffer size at which the underlying transport starts flushing to the socket
    /// from `poll_ready`, overriding [`DEFAULT_BACKPRESSURE_BOUNDARY`].
    ///
    /// A larger boundary batches more frames into a single write syscall when many messages are
    /// sent back to back, at the cost of buffering more encrypted data in memory per connection.
    /// A flush releases a write buffer that grew past 256KiB, so with a boundary near or above
    /// that size every large batch reallocates the buffer.
    pub fn set_backpressure_boundary(&mut self, boundary: usize) {
        self.stream.set_backpressure_boundary(boundary);
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
            Some(other) => Poll::Ready(Some(Err(io::Error::other(format!(
                "ECIES stream protocol error: expected message, received {other:?}"
            ))))),
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
        let mut stream = self.project().stream;
        ready!(stream.as_mut().poll_flush(cx))?;

        // A drained buffer keeps its whole allocation, and `try_reclaim` checks without
        // allocating whether that allocation is larger than the retain limit.
        let buf = stream.write_buffer_mut();
        if buf.is_empty() && buf.try_reclaim(MAX_RETAINED_BUFFER_CAPACITY + 1) {
            *buf = BytesMut::with_capacity(INITIAL_BUFFER_CAPACITY);
        }
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().stream.poll_close(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::MAX_FRAME_SIZE;
    use reth_network_peers::pk2id;
    use tokio::net::{TcpListener, TcpStream};

    #[tokio::test]
    async fn can_write_and_read() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_key = SecretKey::new(&mut rand::rng());

        let handle = tokio::spawn(async move {
            // roughly based off of the design of tokio::net::TcpListener
            let (incoming, _) = listener.accept().await.unwrap();
            let mut stream = ECIESStream::incoming(incoming, server_key).await.unwrap();

            // use the stream to get the next message
            let message = stream.next().await.unwrap().unwrap();
            assert_eq!(message, Bytes::from("hello"));
        });

        // create the server pubkey
        let server_id = pk2id(&server_key.public_key());

        let client_key = SecretKey::new(&mut rand::rng());
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
        let server_key = SecretKey::new(&mut rand::rng());

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
        let server_id = pk2id(&server_key.public_key());

        let client_key = SecretKey::new(&mut rand::rng());
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

    #[tokio::test]
    async fn rejects_frame_larger_than_header_can_describe() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_key = SecretKey::new(&mut rand::rng());

        let handle = tokio::spawn(async move {
            let (incoming, _) = listener.accept().await.unwrap();
            ECIESStream::incoming(incoming, server_key).await.unwrap()
        });

        let server_id = pk2id(&server_key.public_key());
        let client_key = SecretKey::new(&mut rand::rng());
        let outgoing = TcpStream::connect(addr).await.unwrap();
        let mut client_stream =
            ECIESStream::connect(outgoing, client_key, server_id).await.unwrap();
        let _server_stream = handle.await.unwrap();

        let err = client_stream.send(Bytes::from(vec![0; MAX_FRAME_SIZE + 1])).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            ECIESErrorImpl::FrameTooLarge { size: MAX_FRAME_SIZE + 1, max_size: MAX_FRAME_SIZE }
                .to_string()
        );
    }

    /// Returns a connected client and server stream over TCP.
    async fn connected_pair() -> (ECIESStream<TcpStream>, ECIESStream<TcpStream>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_key = SecretKey::new(&mut rand::rng());

        let handle = tokio::spawn(async move {
            let (incoming, _) = listener.accept().await.unwrap();
            ECIESStream::incoming(incoming, server_key).await.unwrap()
        });

        let server_id = pk2id(&server_key.public_key());
        let outgoing = TcpStream::connect(addr).await.unwrap();
        let client = ECIESStream::connect(outgoing, SecretKey::new(&mut rand::rng()), server_id)
            .await
            .unwrap();
        (client, handle.await.unwrap())
    }

    #[tokio::test]
    async fn releases_buffers_after_large_frame() {
        let (mut client, mut server) = connected_pair().await;
        let large = Bytes::from(vec![1; 2 << 20]);
        let small = Bytes::from_static(&[2; 200]);
        let messages = [small.clone(), large, small.clone(), small.clone(), small];

        let expected = messages.clone();
        let reader = tokio::spawn(async move {
            for message in expected {
                assert_eq!(server.next().await.unwrap().unwrap(), message);
            }
            server
        });
        for message in messages {
            client.feed(message).await.unwrap();
        }
        client.flush().await.unwrap();
        let mut server = reader.await.unwrap();

        // Neither buffer can reclaim more than the retain limit without allocating.
        let write_buffer = client.stream.write_buffer_mut();
        assert!(write_buffer.is_empty());
        assert!(!write_buffer.try_reclaim(MAX_RETAINED_BUFFER_CAPACITY + 1));
        let read_buffer = server.stream.read_buffer_mut();
        assert!(read_buffer.is_empty());
        assert!(!read_buffer.try_reclaim(MAX_RETAINED_BUFFER_CAPACITY + 1));
    }
}
