use crate::{
    errors::{EthHandshakeError, EthStreamError},
    message::{EthBroadcastMessage, ProtocolBroadcastMessage},
    p2pstream::HANDSHAKE_TIMEOUT,
    CanDisconnect, DisconnectReason, EthMessage, EthNetworkPrimitives, EthVersion, ProtocolMessage,
    Status,
};
use alloy_primitives::bytes::{Bytes, BytesMut};
use futures::{ready, Sink, SinkExt, StreamExt};
use pin_project::pin_project;
use reth_eth_wire_types::NetworkPrimitives;
use reth_ethereum_forks::ForkFilter;
use reth_primitives_traits::GotExpected;
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::timeout;
use tokio_stream::Stream;
use tracing::{debug, trace};

/// [`MAX_MESSAGE_SIZE`] is the maximum cap on the size of a protocol message.
// https://github.com/ethereum/go-ethereum/blob/30602163d5d8321fbc68afdcbbaf2362b2641bde/eth/protocols/eth/protocol.go#L50
pub const MAX_MESSAGE_SIZE: usize = 10 * 1024 * 1024;

/// [`MAX_STATUS_SIZE`] is the maximum cap on the size of the initial status message
pub(crate) const MAX_STATUS_SIZE: usize = 500 * 1024;

/// An un-authenticated [`EthStream`]. This is consumed and returns a [`EthStream`] after the
/// `Status` handshake is completed.
#[pin_project]
#[derive(Debug)]
pub struct UnauthedEthStream<S> {
    #[pin]
    inner: S,
}

impl<S> UnauthedEthStream<S> {
    /// Create a new `UnauthedEthStream` from a type `S` which implements `Stream` and `Sink`.
    pub const fn new(inner: S) -> Self {
        Self { inner }
    }

    /// Consumes the type and returns the wrapped stream
    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S, E> UnauthedEthStream<S>
where
    S: Stream<Item = Result<BytesMut, E>> + CanDisconnect<Bytes> + Unpin,
    EthStreamError: From<E> + From<<S as Sink<Bytes>>::Error>,
{
    /// Consumes the [`UnauthedEthStream`] and returns an [`EthStream`] after the `Status`
    /// handshake is completed successfully. This also returns the `Status` message sent by the
    /// remote peer.
    pub async fn handshake<N: NetworkPrimitives>(
        self,
        status: Status,
        fork_filter: ForkFilter,
    ) -> Result<(EthStream<S, N>, Status), EthStreamError> {
        self.handshake_with_timeout(status, fork_filter, HANDSHAKE_TIMEOUT).await
    }

    /// Wrapper around handshake which enforces a timeout.
    pub async fn handshake_with_timeout<N: NetworkPrimitives>(
        self,
        status: Status,
        fork_filter: ForkFilter,
        timeout_limit: Duration,
    ) -> Result<(EthStream<S, N>, Status), EthStreamError> {
        timeout(timeout_limit, Self::handshake_without_timeout(self, status, fork_filter))
            .await
            .map_err(|_| EthStreamError::StreamTimeout)?
    }

    /// Handshake with no timeout
    pub async fn handshake_without_timeout<N: NetworkPrimitives>(
        mut self,
        status: Status,
        fork_filter: ForkFilter,
    ) -> Result<(EthStream<S, N>, Status), EthStreamError> {
        trace!(
            %status,
            "sending eth status to peer"
        );

        // we need to encode and decode here on our own because we don't have an `EthStream` yet
        // The max length for a status with TTD is: <msg id = 1 byte> + <rlp(status) = 88 byte>
        self.inner
            .send(
                alloy_rlp::encode(ProtocolMessage::<N>::from(EthMessage::<N>::Status(status)))
                    .into(),
            )
            .await?;

        let their_msg_res = self.inner.next().await;

        let their_msg = match their_msg_res {
            Some(msg) => msg,
            None => {
                self.inner.disconnect(DisconnectReason::DisconnectRequested).await?;
                return Err(EthStreamError::EthHandshakeError(EthHandshakeError::NoResponse))
            }
        }?;

        if their_msg.len() > MAX_STATUS_SIZE {
            self.inner.disconnect(DisconnectReason::ProtocolBreach).await?;
            return Err(EthStreamError::MessageTooBig(their_msg.len()))
        }

        let version = status.version;
        let msg = match ProtocolMessage::<N>::decode_message(version, &mut their_msg.as_ref()) {
            Ok(m) => m,
            Err(err) => {
                debug!("decode error in eth handshake: msg={their_msg:x}");
                self.inner.disconnect(DisconnectReason::DisconnectRequested).await?;
                return Err(EthStreamError::InvalidMessage(err))
            }
        };

        // The following checks should match the checks in go-ethereum:
        // https://github.com/ethereum/go-ethereum/blob/9244d5cd61f3ea5a7645fdf2a1a96d53421e412f/eth/protocols/eth/handshake.go#L87-L89
        match msg.message {
            EthMessage::Status(resp) => {
                trace!(
                    status=%resp,
                    "validating incoming eth status from peer"
                );
                if status.genesis != resp.genesis {
                    self.inner.disconnect(DisconnectReason::ProtocolBreach).await?;
                    return Err(EthHandshakeError::MismatchedGenesis(
                        GotExpected { expected: status.genesis, got: resp.genesis }.into(),
                    )
                    .into())
                }

                if status.version != resp.version {
                    self.inner.disconnect(DisconnectReason::ProtocolBreach).await?;
                    return Err(EthHandshakeError::MismatchedProtocolVersion(GotExpected {
                        got: resp.version,
                        expected: status.version,
                    })
                    .into())
                }

                if status.chain != resp.chain {
                    self.inner.disconnect(DisconnectReason::ProtocolBreach).await?;
                    return Err(EthHandshakeError::MismatchedChain(GotExpected {
                        got: resp.chain,
                        expected: status.chain,
                    })
                    .into())
                }

                // TD at mainnet block #7753254 is 76 bits. If it becomes 100 million times
                // larger, it will still fit within 100 bits
                if status.total_difficulty.bit_len() > 100 {
                    self.inner.disconnect(DisconnectReason::ProtocolBreach).await?;
                    return Err(EthHandshakeError::TotalDifficultyBitLenTooLarge {
                        got: status.total_difficulty.bit_len(),
                        maximum: 100,
                    }
                    .into())
                }

                if let Err(err) =
                    fork_filter.validate(resp.forkid).map_err(EthHandshakeError::InvalidFork)
                {
                    self.inner.disconnect(DisconnectReason::ProtocolBreach).await?;
                    return Err(err.into())
                }

                // now we can create the `EthStream` because the peer has successfully completed
                // the handshake
                let stream = EthStream::new(version, self.inner);

                Ok((stream, resp))
            }
            _ => {
                self.inner.disconnect(DisconnectReason::ProtocolBreach).await?;
                Err(EthStreamError::EthHandshakeError(
                    EthHandshakeError::NonStatusMessageInHandshake,
                ))
            }
        }
    }
}

/// An `EthStream` wraps over any `Stream` that yields bytes and makes it
/// compatible with eth-networking protocol messages, which get RLP encoded/decoded.
#[pin_project]
#[derive(Debug)]
pub struct EthStream<S, N = EthNetworkPrimitives> {
    /// Negotiated eth version.
    version: EthVersion,
    #[pin]
    inner: S,

    _pd: std::marker::PhantomData<N>,
}

impl<S, N> EthStream<S, N> {
    /// Creates a new unauthed [`EthStream`] from a provided stream. You will need
    /// to manually handshake a peer.
    #[inline]
    pub const fn new(version: EthVersion, inner: S) -> Self {
        Self { version, inner, _pd: std::marker::PhantomData }
    }

    /// Returns the eth version.
    #[inline]
    pub const fn version(&self) -> EthVersion {
        self.version
    }

    /// Returns the underlying stream.
    #[inline]
    pub const fn inner(&self) -> &S {
        &self.inner
    }

    /// Returns mutable access to the underlying stream.
    #[inline]
    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    /// Consumes this type and returns the wrapped stream.
    #[inline]
    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S, E, N> EthStream<S, N>
where
    S: Sink<Bytes, Error = E> + Unpin,
    EthStreamError: From<E>,
    N: NetworkPrimitives,
{
    /// Same as [`Sink::start_send`] but accepts a [`EthBroadcastMessage`] instead.
    pub fn start_send_broadcast(
        &mut self,
        item: EthBroadcastMessage<N>,
    ) -> Result<(), EthStreamError> {
        self.inner.start_send_unpin(Bytes::from(alloy_rlp::encode(
            ProtocolBroadcastMessage::from(item),
        )))?;

        Ok(())
    }
}

impl<S, E, N> Stream for EthStream<S, N>
where
    S: Stream<Item = Result<BytesMut, E>> + Unpin,
    EthStreamError: From<E>,
    N: NetworkPrimitives,
{
    type Item = Result<EthMessage<N>, EthStreamError>;

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

        let msg = match ProtocolMessage::decode_message(*this.version, &mut bytes.as_ref()) {
            Ok(m) => m,
            Err(err) => {
                let msg = if bytes.len() > 50 {
                    format!("{:02x?}...{:x?}", &bytes[..10], &bytes[bytes.len() - 10..])
                } else {
                    format!("{bytes:02x?}")
                };
                debug!(
                    version=?this.version,
                    %msg,
                    "failed to decode protocol message"
                );
                return Poll::Ready(Some(Err(EthStreamError::InvalidMessage(err))))
            }
        };

        if matches!(msg.message, EthMessage::Status(_)) {
            return Poll::Ready(Some(Err(EthStreamError::EthHandshakeError(
                EthHandshakeError::StatusNotInHandshake,
            ))))
        }

        Poll::Ready(Some(Ok(msg.message)))
    }
}

impl<S, N> Sink<EthMessage<N>> for EthStream<S, N>
where
    S: CanDisconnect<Bytes> + Unpin,
    EthStreamError: From<<S as Sink<Bytes>>::Error>,
    N: NetworkPrimitives,
{
    type Error = EthStreamError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_ready(cx).map_err(Into::into)
    }

    fn start_send(self: Pin<&mut Self>, item: EthMessage<N>) -> Result<(), Self::Error> {
        if matches!(item, EthMessage::Status(_)) {
            // TODO: to disconnect here we would need to do something similar to P2PStream's
            // start_disconnect, which would ideally be a part of the CanDisconnect trait, or at
            // least similar.
            //
            // Other parts of reth do not yet need traits like CanDisconnect because atm they work
            // exclusively with EthStream<P2PStream<S>>, where the inner P2PStream is accessible,
            // allowing for its start_disconnect method to be called.
            //
            // self.project().inner.start_disconnect(DisconnectReason::ProtocolBreach);
            return Err(EthStreamError::EthHandshakeError(EthHandshakeError::StatusNotInHandshake))
        }

        self.project()
            .inner
            .start_send(Bytes::from(alloy_rlp::encode(ProtocolMessage::from(item))))?;

        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_flush(cx).map_err(Into::into)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_close(cx).map_err(Into::into)
    }
}

impl<S, N> CanDisconnect<EthMessage<N>> for EthStream<S, N>
where
    S: CanDisconnect<Bytes> + Send,
    EthStreamError: From<<S as Sink<Bytes>>::Error>,
    N: NetworkPrimitives,
{
    async fn disconnect(&mut self, reason: DisconnectReason) -> Result<(), EthStreamError> {
        self.inner.disconnect(reason).await.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::UnauthedEthStream;
    use crate::{
        broadcast::BlockHashNumber,
        errors::{EthHandshakeError, EthStreamError},
        hello::DEFAULT_TCP_PORT,
        p2pstream::UnauthedP2PStream,
        EthMessage, EthStream, EthVersion, HelloMessageWithProtocols, PassthroughCodec,
        ProtocolVersion, Status,
    };
    use alloy_chains::NamedChain;
    use alloy_primitives::{B256, U256};
    use futures::{SinkExt, StreamExt};
    use reth_ecies::stream::ECIESStream;
    use reth_eth_wire_types::EthNetworkPrimitives;
    use reth_ethereum_forks::{ForkFilter, Head};
    use reth_network_peers::pk2id;
    use secp256k1::{SecretKey, SECP256K1};
    use std::time::Duration;
    use tokio::net::{TcpListener, TcpStream};
    use tokio_util::codec::Decoder;

    #[tokio::test]
    async fn can_handshake() {
        let genesis = B256::random();
        let fork_filter = ForkFilter::new(Head::default(), genesis, 0, Vec::new());

        let status = Status {
            version: EthVersion::Eth67,
            chain: NamedChain::Mainnet.into(),
            total_difficulty: U256::ZERO,
            blockhash: B256::random(),
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
            let stream = PassthroughCodec::default().framed(incoming);
            let (_, their_status) = UnauthedEthStream::new(stream)
                .handshake::<EthNetworkPrimitives>(status_clone, fork_filter_clone)
                .await
                .unwrap();

            // just make sure it equals our status (our status is a clone of their status)
            assert_eq!(their_status, status_clone);
        });

        let outgoing = TcpStream::connect(local_addr).await.unwrap();
        let sink = PassthroughCodec::default().framed(outgoing);

        // try to connect
        let (_, their_status) = UnauthedEthStream::new(sink)
            .handshake::<EthNetworkPrimitives>(status, fork_filter)
            .await
            .unwrap();

        // their status is a clone of our status, these should be equal
        assert_eq!(their_status, status);

        // wait for it to finish
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn pass_handshake_on_low_td_bitlen() {
        let genesis = B256::random();
        let fork_filter = ForkFilter::new(Head::default(), genesis, 0, Vec::new());

        let status = Status {
            version: EthVersion::Eth67,
            chain: NamedChain::Mainnet.into(),
            total_difficulty: U256::from(2).pow(U256::from(100)) - U256::from(1),
            blockhash: B256::random(),
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
            let stream = PassthroughCodec::default().framed(incoming);
            let (_, their_status) = UnauthedEthStream::new(stream)
                .handshake::<EthNetworkPrimitives>(status_clone, fork_filter_clone)
                .await
                .unwrap();

            // just make sure it equals our status, and that the handshake succeeded
            assert_eq!(their_status, status_clone);
        });

        let outgoing = TcpStream::connect(local_addr).await.unwrap();
        let sink = PassthroughCodec::default().framed(outgoing);

        // try to connect
        let (_, their_status) = UnauthedEthStream::new(sink)
            .handshake::<EthNetworkPrimitives>(status, fork_filter)
            .await
            .unwrap();

        // their status is a clone of our status, these should be equal
        assert_eq!(their_status, status);

        // await the other handshake
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn fail_handshake_on_high_td_bitlen() {
        let genesis = B256::random();
        let fork_filter = ForkFilter::new(Head::default(), genesis, 0, Vec::new());

        let status = Status {
            version: EthVersion::Eth67,
            chain: NamedChain::Mainnet.into(),
            total_difficulty: U256::from(2).pow(U256::from(100)),
            blockhash: B256::random(),
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
            let stream = PassthroughCodec::default().framed(incoming);
            let handshake_res = UnauthedEthStream::new(stream)
                .handshake::<EthNetworkPrimitives>(status_clone, fork_filter_clone)
                .await;

            // make sure the handshake fails due to td too high
            assert!(matches!(
                handshake_res,
                Err(EthStreamError::EthHandshakeError(
                    EthHandshakeError::TotalDifficultyBitLenTooLarge { got: 101, maximum: 100 }
                ))
            ));
        });

        let outgoing = TcpStream::connect(local_addr).await.unwrap();
        let sink = PassthroughCodec::default().framed(outgoing);

        // try to connect
        let handshake_res = UnauthedEthStream::new(sink)
            .handshake::<EthNetworkPrimitives>(status, fork_filter)
            .await;

        // this handshake should also fail due to td too high
        assert!(matches!(
            handshake_res,
            Err(EthStreamError::EthHandshakeError(
                EthHandshakeError::TotalDifficultyBitLenTooLarge { got: 101, maximum: 100 }
            ))
        ));

        // await the other handshake
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn can_write_and_read_cleartext() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();
        let test_msg: EthMessage = EthMessage::NewBlockHashes(
            vec![
                BlockHashNumber { hash: B256::random(), number: 5 },
                BlockHashNumber { hash: B256::random(), number: 6 },
            ]
            .into(),
        );

        let test_msg_clone = test_msg.clone();
        let handle = tokio::spawn(async move {
            // roughly based off of the design of tokio::net::TcpListener
            let (incoming, _) = listener.accept().await.unwrap();
            let stream = PassthroughCodec::default().framed(incoming);
            let mut stream = EthStream::new(EthVersion::Eth67, stream);

            // use the stream to get the next message
            let message = stream.next().await.unwrap().unwrap();
            assert_eq!(message, test_msg_clone);
        });

        let outgoing = TcpStream::connect(local_addr).await.unwrap();
        let sink = PassthroughCodec::default().framed(outgoing);
        let mut client_stream = EthStream::new(EthVersion::Eth67, sink);

        client_stream.send(test_msg).await.unwrap();

        // make sure the server receives the message and asserts before ending the test
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn can_write_and_read_ecies() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();
        let server_key = SecretKey::new(&mut rand::thread_rng());
        let test_msg: EthMessage = EthMessage::NewBlockHashes(
            vec![
                BlockHashNumber { hash: B256::random(), number: 5 },
                BlockHashNumber { hash: B256::random(), number: 6 },
            ]
            .into(),
        );

        let test_msg_clone = test_msg.clone();
        let handle = tokio::spawn(async move {
            // roughly based off of the design of tokio::net::TcpListener
            let (incoming, _) = listener.accept().await.unwrap();
            let stream = ECIESStream::incoming(incoming, server_key).await.unwrap();
            let mut stream = EthStream::new(EthVersion::Eth67, stream);

            // use the stream to get the next message
            let message = stream.next().await.unwrap().unwrap();
            assert_eq!(message, test_msg_clone);
        });

        // create the server pubkey
        let server_id = pk2id(&server_key.public_key(SECP256K1));

        let client_key = SecretKey::new(&mut rand::thread_rng());

        let outgoing = TcpStream::connect(local_addr).await.unwrap();
        let outgoing = ECIESStream::connect(outgoing, client_key, server_id).await.unwrap();
        let mut client_stream = EthStream::new(EthVersion::Eth67, outgoing);

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
        let test_msg: EthMessage = EthMessage::NewBlockHashes(
            vec![
                BlockHashNumber { hash: B256::random(), number: 5 },
                BlockHashNumber { hash: B256::random(), number: 6 },
            ]
            .into(),
        );

        let genesis = B256::random();
        let fork_filter = ForkFilter::new(Head::default(), genesis, 0, Vec::new());

        let status = Status {
            version: EthVersion::Eth67,
            chain: NamedChain::Mainnet.into(),
            total_difficulty: U256::ZERO,
            blockhash: B256::random(),
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

            let server_hello = HelloMessageWithProtocols {
                protocol_version: ProtocolVersion::V5,
                client_version: "bitcoind/1.0.0".to_string(),
                protocols: vec![EthVersion::Eth67.into()],
                port: DEFAULT_TCP_PORT,
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

        let client_hello = HelloMessageWithProtocols {
            protocol_version: ProtocolVersion::V5,
            client_version: "bitcoind/1.0.0".to_string(),
            protocols: vec![EthVersion::Eth67.into()],
            port: DEFAULT_TCP_PORT,
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

    #[tokio::test]
    async fn handshake_should_timeout() {
        let genesis = B256::random();
        let fork_filter = ForkFilter::new(Head::default(), genesis, 0, Vec::new());

        let status = Status {
            version: EthVersion::Eth67,
            chain: NamedChain::Mainnet.into(),
            total_difficulty: U256::ZERO,
            blockhash: B256::random(),
            genesis,
            // Pass the current fork id.
            forkid: fork_filter.current(),
        };

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let status_clone = status;
        let fork_filter_clone = fork_filter.clone();
        let _handle = tokio::spawn(async move {
            // Delay accepting the connection for longer than the client's timeout period
            tokio::time::sleep(Duration::from_secs(11)).await;
            // roughly based off of the design of tokio::net::TcpListener
            let (incoming, _) = listener.accept().await.unwrap();
            let stream = PassthroughCodec::default().framed(incoming);
            let (_, their_status) = UnauthedEthStream::new(stream)
                .handshake::<EthNetworkPrimitives>(status_clone, fork_filter_clone)
                .await
                .unwrap();

            // just make sure it equals our status (our status is a clone of their status)
            assert_eq!(their_status, status_clone);
        });

        let outgoing = TcpStream::connect(local_addr).await.unwrap();
        let sink = PassthroughCodec::default().framed(outgoing);

        // try to connect
        let handshake_result = UnauthedEthStream::new(sink)
            .handshake_with_timeout::<EthNetworkPrimitives>(
                status,
                fork_filter,
                Duration::from_secs(1),
            )
            .await;

        // Assert that a timeout error occurred
        assert!(
            matches!(handshake_result, Err(e) if e.to_string() == EthStreamError::StreamTimeout.to_string())
        );
    }
}
