use crate::{
    error::{EthStreamError, HandshakeError},
    message::{EthBroadcastMessage, ProtocolBroadcastMessage},
    types::{EthMessage, ProtocolMessage, Status},
};
use bytes::{Bytes, BytesMut};
use futures::{ready, Sink, SinkExt, StreamExt};
use pin_project::pin_project;
use reth_primitives::ForkFilter;
use reth_rlp::{Decodable, Encodable};
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio_stream::Stream;

/// [`MAX_MESSAGE_SIZE`] is the maximum cap on the size of a protocol message.
// https://github.com/ethereum/go-ethereum/blob/30602163d5d8321fbc68afdcbbaf2362b2641bde/eth/protocols/eth/protocol.go#L50
const MAX_MESSAGE_SIZE: usize = 10 * 1024 * 1024;

/// An un-authenticated [`EthStream`]. This is consumed and returns a [`EthStream`] after the
/// `Status` handshake is completed.
#[pin_project]
pub struct UnauthedEthStream<S> {
    #[pin]
    inner: S,
}

impl<S> UnauthedEthStream<S> {
    /// Create a new `UnauthedEthStream` from a type `S` which implements `Stream` and `Sink`.
    pub fn new(inner: S) -> Self {
        Self { inner }
    }

    /// Consumes the type and returns the wrapped stream
    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S, E> UnauthedEthStream<S>
where
    S: Stream<Item = Result<bytes::BytesMut, E>> + Sink<bytes::Bytes, Error = E> + Unpin,
    EthStreamError: From<E>,
{
    /// Consumes the [`UnauthedEthStream`] and returns an [`EthStream`] after the `Status`
    /// handshake is completed successfully. This also returns the `Status` message sent by the
    /// remote peer.
    pub async fn handshake(
        mut self,
        status: Status,
        fork_filter: ForkFilter,
    ) -> Result<(EthStream<S>, Status), EthStreamError> {
        tracing::trace!("sending eth status ...");

        // we need to encode and decode here on our own because we don't have an `EthStream` yet
        let mut our_status_bytes = BytesMut::new();
        ProtocolMessage::from(EthMessage::Status(status)).encode(&mut our_status_bytes);
        let our_status_bytes = our_status_bytes.freeze();
        self.inner.send(our_status_bytes).await?;

        tracing::trace!("waiting for eth status from peer ...");
        let their_msg = self
            .inner
            .next()
            .await
            .ok_or(EthStreamError::HandshakeError(HandshakeError::NoResponse))??;

        if their_msg.len() > MAX_MESSAGE_SIZE {
            return Err(EthStreamError::MessageTooBig(their_msg.len()))
        }

        let msg = match ProtocolMessage::decode(&mut their_msg.as_ref()) {
            Ok(m) => m,
            Err(err) => return Err(err.into()),
        };

        // TODO: Add any missing checks
        // https://github.com/ethereum/go-ethereum/blob/9244d5cd61f3ea5a7645fdf2a1a96d53421e412f/eth/protocols/eth/handshake.go#L87-L89
        match msg.message {
            EthMessage::Status(resp) => {
                if status.genesis != resp.genesis {
                    return Err(HandshakeError::MismatchedGenesis {
                        expected: status.genesis,
                        got: resp.genesis,
                    }
                    .into())
                }

                if status.version != resp.version {
                    return Err(HandshakeError::MismatchedProtocolVersion {
                        expected: status.version,
                        got: resp.version,
                    }
                    .into())
                }

                if status.chain != resp.chain {
                    return Err(HandshakeError::MismatchedChain {
                        expected: status.chain,
                        got: resp.chain,
                    }
                    .into())
                }

                fork_filter.validate(resp.forkid).map_err(HandshakeError::InvalidFork)?;

                // now we can create the `EthStream` because the peer has successfully completed
                // the handshake
                let stream = EthStream::new(self.inner);

                Ok((stream, resp))
            }
            _ => Err(EthStreamError::HandshakeError(HandshakeError::NonStatusMessageInHandshake)),
        }
    }
}

/// An `EthStream` wraps over any `Stream` that yields bytes and makes it
/// compatible with eth-networking protocol messages, which get RLP encoded/decoded.
#[pin_project]
#[derive(Debug)]
pub struct EthStream<S> {
    #[pin]
    inner: S,
}

impl<S> EthStream<S> {
    /// Creates a new unauthed [`EthStream`] from a provided stream. You will need
    /// to manually handshake a peer.
    pub fn new(inner: S) -> Self {
        Self { inner }
    }

    /// Returns the underlying stream.
    pub fn inner(&self) -> &S {
        &self.inner
    }

    /// Returns mutable access to the underlying stream.
    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }
}

impl<S, E> EthStream<S>
where
    S: Sink<Bytes, Error = E> + Unpin,
    EthStreamError: From<E>,
{
    /// Same as [`Sink::start_send`] but accepts a [`EthBroadcastMessage`] instead.
    pub fn start_send_broadcast(
        &mut self,
        item: EthBroadcastMessage,
    ) -> Result<(), EthStreamError> {
        let mut bytes = BytesMut::new();
        ProtocolBroadcastMessage::from(item).encode(&mut bytes);
        let bytes = bytes.freeze();

        self.inner.start_send_unpin(bytes)?;

        Ok(())
    }
}

impl<S, E> Stream for EthStream<S>
where
    S: Stream<Item = Result<BytesMut, E>> + Unpin,
    EthStreamError: From<E>,
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

        if bytes.len() > MAX_MESSAGE_SIZE {
            return Poll::Ready(Some(Err(EthStreamError::MessageTooBig(bytes.len()))))
        }

        let msg = match ProtocolMessage::decode(&mut bytes.as_ref()) {
            Ok(m) => m,
            Err(err) => return Poll::Ready(Some(Err(err.into()))),
        };

        if matches!(msg.message, EthMessage::Status(_)) {
            return Poll::Ready(Some(Err(EthStreamError::HandshakeError(
                HandshakeError::StatusNotInHandshake,
            ))))
        }

        Poll::Ready(Some(Ok(msg.message)))
    }
}

impl<S, E> Sink<EthMessage> for EthStream<S>
where
    S: Sink<Bytes, Error = E> + Unpin,
    EthStreamError: From<E>,
{
    type Error = EthStreamError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_ready(cx).map_err(Into::into)
    }

    fn start_send(self: Pin<&mut Self>, item: EthMessage) -> Result<(), Self::Error> {
        if matches!(item, EthMessage::Status(_)) {
            return Err(EthStreamError::HandshakeError(HandshakeError::StatusNotInHandshake))
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
    use super::UnauthedEthStream;
    use crate::{
        capability::Capability,
        p2pstream::{HelloMessage, ProtocolVersion, UnauthedP2PStream},
        types::{broadcast::BlockHashNumber, EthMessage, EthVersion, Status},
        EthStream, PassthroughCodec,
    };
    use ethers_core::types::Chain;
    use futures::{SinkExt, StreamExt};
    use reth_ecies::{stream::ECIESStream, util::pk2id};
    use reth_primitives::{ForkFilter, H256, U256};
    use secp256k1::{SecretKey, SECP256K1};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_util::codec::Decoder;

    #[tokio::test]
    async fn can_handshake() {
        let genesis = H256::random();
        let fork_filter = ForkFilter::new(0, genesis, vec![]);

        let status = Status {
            version: EthVersion::Eth67 as u8,
            chain: Chain::Mainnet.into(),
            total_difficulty: U256::from(0),
            blockhash: H256::random(),
            genesis,
            // Pass the current fork id.
            forkid: fork_filter.current(),
        };

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let status_clone = status;
        let fork_filter_clone = fork_filter.clone();
        let handle = tokio::spawn(async move {
            // roughly based off of the design of tokio::net::TcpListener
            let (incoming, _) = listener.accept().await.unwrap();
            let stream = crate::PassthroughCodec::default().framed(incoming);
            let (_, their_status) = UnauthedEthStream::new(stream)
                .handshake(status_clone, fork_filter_clone)
                .await
                .unwrap();

            // just make sure it equals our status (our status is a clone of their status)
            assert_eq!(their_status, status_clone);
        });

        let outgoing = TcpStream::connect(local_addr).await.unwrap();
        let sink = crate::PassthroughCodec::default().framed(outgoing);

        // try to connect
        let (_, their_status) =
            UnauthedEthStream::new(sink).handshake(status, fork_filter).await.unwrap();

        // their status is a clone of our status, these should be equal
        assert_eq!(their_status, status);

        // wait for it to finish
        handle.await.unwrap();
    }

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
            let stream = PassthroughCodec::default().framed(incoming);
            let mut stream = EthStream::new(stream);

            // use the stream to get the next message
            let message = stream.next().await.unwrap().unwrap();
            assert_eq!(message, test_msg_clone);
        });

        let outgoing = TcpStream::connect(local_addr).await.unwrap();
        let sink = PassthroughCodec::default().framed(outgoing);
        let mut client_stream = EthStream::new(sink);

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
            let mut stream = EthStream::new(stream);

            // use the stream to get the next message
            let message = stream.next().await.unwrap().unwrap();
            assert_eq!(message, test_msg_clone);
        });

        // create the server pubkey
        let server_id = pk2id(&server_key.public_key(SECP256K1));

        let client_key = SecretKey::new(&mut rand::thread_rng());

        let outgoing = TcpStream::connect(local_addr).await.unwrap();
        let outgoing = ECIESStream::connect(outgoing, client_key, server_id).await.unwrap();
        let mut client_stream = EthStream::new(outgoing);

        client_stream.send(test_msg).await.unwrap();

        // make sure the server receives the message and asserts before ending the test
        handle.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ethstream_over_p2p() {
        // create a p2p stream and server, then confirm that the two are authed
        // create tcpstream
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();
        let server_key = SecretKey::new(&mut rand::thread_rng());
        let test_msg = EthMessage::NewBlockHashes(
            vec![
                BlockHashNumber { hash: H256::random(), number: 5 },
                BlockHashNumber { hash: H256::random(), number: 6 },
            ]
            .into(),
        );

        let genesis = H256::random();
        let fork_filter = ForkFilter::new(0, genesis, vec![]);

        let status = Status {
            version: EthVersion::Eth67 as u8,
            chain: Chain::Mainnet.into(),
            total_difficulty: U256::from(0),
            blockhash: H256::random(),
            genesis,
            // Pass the current fork id.
            forkid: fork_filter.current(),
        };

        let status_copy = status;
        let fork_filter_clone = fork_filter.clone();
        let test_msg_clone = test_msg.clone();
        let handle = tokio::spawn(async move {
            // roughly based off of the design of tokio::net::TcpListener
            let (incoming, _) = listener.accept().await.unwrap();
            let stream = ECIESStream::incoming(incoming, server_key).await.unwrap();

            let server_hello = HelloMessage {
                protocol_version: ProtocolVersion::V5,
                client_version: "bitcoind/1.0.0".to_string(),
                capabilities: vec![Capability::new("eth".into(), EthVersion::Eth67 as usize)],
                port: 30303,
                id: pk2id(&server_key.public_key(SECP256K1)),
            };

            let unauthed_stream = UnauthedP2PStream::new(stream);
            let (p2p_stream, _) = unauthed_stream.handshake(server_hello).await.unwrap();
            let (mut eth_stream, _) = UnauthedEthStream::new(p2p_stream)
                .handshake(status_copy, fork_filter_clone)
                .await
                .unwrap();

            // use the stream to get the next message
            let message = eth_stream.next().await.unwrap().unwrap();
            assert_eq!(message, test_msg_clone);
        });

        // create the server pubkey
        let server_id = pk2id(&server_key.public_key(SECP256K1));

        let client_key = SecretKey::new(&mut rand::thread_rng());

        let outgoing = TcpStream::connect(local_addr).await.unwrap();
        let sink = ECIESStream::connect(outgoing, client_key, server_id).await.unwrap();

        let client_hello = HelloMessage {
            protocol_version: ProtocolVersion::V5,
            client_version: "bitcoind/1.0.0".to_string(),
            capabilities: vec![Capability::new("eth".into(), EthVersion::Eth67 as usize)],
            port: 30303,
            id: pk2id(&client_key.public_key(SECP256K1)),
        };

        let unauthed_stream = UnauthedP2PStream::new(sink);
        let (p2p_stream, _) = unauthed_stream.handshake(client_hello).await.unwrap();

        let (mut client_stream, _) =
            UnauthedEthStream::new(p2p_stream).handshake(status, fork_filter).await.unwrap();

        client_stream.send(test_msg).await.unwrap();

        // make sure the server receives the message and asserts before ending the test
        handle.await.unwrap();
    }
}
