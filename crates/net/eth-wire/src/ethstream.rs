//! Ethereum protocol stream implementations.
//!
//! Provides stream types for the Ethereum wire protocol.
//! It separates protocol logic [`EthStreamInner`] from transport concerns [`EthStream`].
//! Handles handshaking, message processing, and RLP serialization.

use crate::{
    errors::{EthHandshakeError, EthStreamError},
    handshake::EthereumEthHandshake,
    message::{
        EthBroadcastMessage, MessageError, ProtocolBroadcastMessage, MAX_MESSAGE_SIZE,
        TX_MEMORY_BUDGET_MULTIPLIER,
    },
    p2pstream::{compress_eth_only_protocol_message, P2PStream, HANDSHAKE_TIMEOUT},
    CanDisconnect, DisconnectReason, EthMessage, EthNetworkPrimitives, EthVersion, ProtocolMessage,
    SharedTransactions, UnifiedStatus,
};
use alloy_primitives::bytes::{Bytes, BytesMut};
use alloy_rlp::{Encodable, Header};
use futures::{ready, Sink, SinkExt};
use pin_project::pin_project;
use reth_eth_wire_types::{
    EthMessageID, NetworkPrimitives, NewBlockHashes, NewPooledTransactionHashes,
    RawCapabilityMessage,
};
use reth_ethereum_forks::ForkFilter;
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, OnceLock},
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::timeout;
use tokio_stream::Stream;
use tracing::{debug, trace};

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
    S: Stream<Item = Result<BytesMut, E>> + CanDisconnect<Bytes> + Send + Unpin,
    EthStreamError: From<E> + From<<S as Sink<Bytes>>::Error>,
{
    /// Consumes the [`UnauthedEthStream`] and returns an [`EthStream`] after the `Status`
    /// handshake is completed successfully. This also returns the `Status` message sent by the
    /// remote peer.
    ///
    /// Caution: This expects that the [`UnifiedStatus`] has the proper eth version configured, with
    /// ETH69 the initial status message changed.
    pub async fn handshake<N: NetworkPrimitives>(
        self,
        status: UnifiedStatus,
        fork_filter: ForkFilter,
    ) -> Result<(EthStream<S, N>, UnifiedStatus), EthStreamError> {
        self.handshake_with_timeout(status, fork_filter, HANDSHAKE_TIMEOUT).await
    }

    /// Wrapper around handshake which enforces a timeout.
    pub async fn handshake_with_timeout<N: NetworkPrimitives>(
        self,
        status: UnifiedStatus,
        fork_filter: ForkFilter,
        timeout_limit: Duration,
    ) -> Result<(EthStream<S, N>, UnifiedStatus), EthStreamError> {
        timeout(timeout_limit, Self::handshake_without_timeout(self, status, fork_filter))
            .await
            .map_err(|_| EthStreamError::StreamTimeout)?
    }

    /// Handshake with no timeout
    pub async fn handshake_without_timeout<N: NetworkPrimitives>(
        mut self,
        status: UnifiedStatus,
        fork_filter: ForkFilter,
    ) -> Result<(EthStream<S, N>, UnifiedStatus), EthStreamError> {
        trace!(
            status = %status.into_message(),
            "sending eth status to peer"
        );
        let their_status =
            EthereumEthHandshake(&mut self.inner).eth_handshake(status, fork_filter).await?;

        // now we can create the `EthStream` because the peer has successfully completed
        // the handshake
        let stream = EthStream::new(status.version, self.inner);

        Ok((stream, their_status))
    }
}

/// Contains eth protocol specific logic for processing messages
#[derive(Debug)]
pub struct EthStreamInner<N> {
    /// Negotiated eth version
    version: EthVersion,
    /// Maximum allowed ETH message size.
    max_message_size: usize,
    /// When true, `NewBlock` (0x07) and `NewBlockHashes` (0x01) messages are rejected before RLP
    /// decoding to avoid any memory impact for non-PoW networks.
    reject_block_announcements: bool,
    _pd: std::marker::PhantomData<N>,
}

impl<N> EthStreamInner<N>
where
    N: NetworkPrimitives,
{
    /// Creates a new [`EthStreamInner`] with the given eth version
    pub const fn new(version: EthVersion) -> Self {
        Self::with_max_message_size(version, MAX_MESSAGE_SIZE)
    }

    /// Creates a new [`EthStreamInner`] with the given eth version and message size limit.
    pub const fn with_max_message_size(version: EthVersion, max_message_size: usize) -> Self {
        Self {
            version,
            max_message_size,
            reject_block_announcements: false,
            _pd: std::marker::PhantomData,
        }
    }

    /// Returns the eth version
    #[inline]
    pub const fn version(&self) -> EthVersion {
        self.version
    }

    /// Sets whether to reject block announcement messages (`NewBlock`, `NewBlockHashes`) before
    /// RLP decoding.
    pub const fn set_reject_block_announcements(&mut self, reject: bool) {
        self.reject_block_announcements = reject;
    }

    /// Decodes incoming bytes into an [`EthMessage`].
    pub fn decode_message(&self, bytes: BytesMut) -> Result<EthMessage<N>, EthStreamError> {
        self.decode_message_from_slice(&bytes)
    }

    /// Decodes incoming bytes into an [`EthMessage`].
    pub fn decode_message_from_slice(&self, bytes: &[u8]) -> Result<EthMessage<N>, EthStreamError> {
        let Some((&message_id, payload)) = bytes.split_first() else {
            return Err(EthStreamError::InvalidMessage(MessageError::RlpError(
                alloy_rlp::Error::InputTooShort,
            )))
        };

        self.decode_message_payload(message_id, payload)
    }

    /// Decodes an incoming message after the p2p layer has already separated the eth message id
    /// from the snappy-compressed payload.
    pub fn decode_message_payload(
        &self,
        message_id: u8,
        payload: &[u8],
    ) -> Result<EthMessage<N>, EthStreamError> {
        let message_size = payload.len() + 1;
        if message_size > self.max_message_size {
            return Err(EthStreamError::MessageTooBig(message_size));
        }

        if self.reject_block_announcements &&
            (message_id == EthMessageID::NewBlock.to_u8() ||
                message_id == EthMessageID::NewBlockHashes.to_u8())
        {
            return Err(EthStreamError::UnsupportedMessage { message_id });
        }

        let msg = match ProtocolMessage::decode_message_payload_with_tx_memory_budget(
            self.version,
            EthMessageID::from_u8(message_id),
            &mut &payload[..],
            self.max_message_size * TX_MEMORY_BUDGET_MULTIPLIER,
        ) {
            Ok(m) => m,
            Err(err) => {
                let msg = if payload.len() > 50 {
                    format!(
                        "{message_id:02x}:{:02x?}...{:x?}",
                        &payload[..10],
                        &payload[payload.len() - 10..]
                    )
                } else {
                    format!("{message_id:02x}:{payload:02x?}")
                };
                debug!(
                    version=?self.version,
                    %msg,
                    "failed to decode protocol message"
                );
                return Err(EthStreamError::InvalidMessage(err));
            }
        };

        if matches!(msg, EthMessage::Status(_)) {
            return Err(EthStreamError::EthHandshakeError(EthHandshakeError::StatusNotInHandshake));
        }

        Ok(msg)
    }

    /// Encodes an [`EthMessage`] to bytes.
    ///
    /// Validates that Status messages are not sent after handshake, enforcing protocol rules.
    pub fn encode_message(&self, item: EthMessage<N>) -> Result<Bytes, EthStreamError> {
        if matches!(item, EthMessage::Status(_)) {
            return Err(EthStreamError::EthHandshakeError(EthHandshakeError::StatusNotInHandshake));
        }

        Ok(item.encoded())
    }
}

/// An already encoded, id-prefixed eth protocol message.
///
/// The encoded bytes keep the eth-relative message id as the first byte. For eth-only p2p
/// sessions the snappy-compressed wire frame can be cached and reused.
#[derive(Clone, Debug)]
pub struct EncodedEthMessage {
    encoded: Bytes,
    eth_only_compressed: Arc<OnceLock<Bytes>>,
}

impl EncodedEthMessage {
    /// Creates a new encoded eth message.
    pub fn new(encoded: Bytes) -> Self {
        Self { encoded, eth_only_compressed: Arc::new(OnceLock::new()) }
    }

    /// Encodes a `NewBlockHashes` announcement.
    pub fn new_block_hashes(hashes: NewBlockHashes) -> Self {
        let mut out = BytesMut::new();
        out.reserve(EthMessageID::NewBlockHashes.length() + hashes.length());
        EthMessageID::NewBlockHashes.encode(&mut out);
        hashes.encode(&mut out);
        Self::new(out.freeze())
    }

    /// Encodes a `NewPooledTransactionHashes` announcement.
    pub fn new_pooled_transaction_hashes(hashes: NewPooledTransactionHashes) -> Self {
        Self::pooled_transaction_hashes(&hashes)
    }

    /// Encodes a `NewPooledTransactionHashes` announcement by reference.
    pub fn pooled_transaction_hashes(hashes: &NewPooledTransactionHashes) -> Self {
        let mut out = BytesMut::new();
        EthMessageID::NewPooledTransactionHashes.encode(&mut out);
        match hashes {
            NewPooledTransactionHashes::Eth66(hashes) => hashes.encode(&mut out),
            NewPooledTransactionHashes::Eth68(hashes) => hashes.encode(&mut out),
            NewPooledTransactionHashes::Eth72(hashes) => hashes.encode(&mut out),
        }
        Self::new(out.freeze())
    }

    /// Encodes a `Transactions` broadcast with a precomputed RLP list payload length.
    pub fn transactions_broadcast<T: Encodable>(
        transactions: &SharedTransactions<T>,
        payload_length: usize,
    ) -> Self {
        let header = Header { list: true, payload_length };
        let mut out = BytesMut::with_capacity(
            EthMessageID::Transactions.length() + header.length() + payload_length,
        );
        EthMessageID::Transactions.encode(&mut out);
        header.encode(&mut out);
        for tx in transactions.iter() {
            tx.encode(&mut out);
        }
        Self::new(out.freeze())
    }

    /// Returns the encoded eth-relative protocol bytes.
    pub const fn encoded(&self) -> &Bytes {
        &self.encoded
    }

    /// Consumes the wrapper and returns the encoded eth-relative protocol bytes.
    pub fn into_encoded(self) -> Bytes {
        self.encoded
    }

    /// Precomputes the eth-only compressed frame if it is not already cached.
    pub fn precompute_eth_only_compression(&self) -> Result<(), EthStreamError> {
        self.eth_only_compressed()?;
        Ok(())
    }

    fn eth_only_compressed(&self) -> Result<Bytes, EthStreamError> {
        if let Some(compressed) = self.eth_only_compressed.get() {
            return Ok(compressed.clone())
        }

        let compressed = compress_eth_only_protocol_message(&self.encoded)?;
        if self.eth_only_compressed.set(compressed.clone()).is_err() {
            return Ok(self
                .eth_only_compressed
                .get()
                .expect("compressed frame set by another clone")
                .clone())
        }

        Ok(compressed)
    }
}

/// An `EthStream` wraps over any `Stream` that yields bytes and makes it
/// compatible with eth-networking protocol messages, which get RLP encoded/decoded.
#[pin_project]
#[derive(Debug)]
pub struct EthStream<S, N = EthNetworkPrimitives> {
    /// Eth-specific logic
    eth: EthStreamInner<N>,
    #[pin]
    inner: S,
}

impl<S, N: NetworkPrimitives> EthStream<S, N> {
    /// Creates a new unauthed [`EthStream`] from a provided stream. You will need
    /// to manually handshake a peer.
    #[inline]
    pub const fn new(version: EthVersion, inner: S) -> Self {
        Self::with_max_message_size(version, inner, MAX_MESSAGE_SIZE)
    }

    /// Creates a new unauthed [`EthStream`] with a custom max message size.
    #[inline]
    pub const fn with_max_message_size(
        version: EthVersion,
        inner: S,
        max_message_size: usize,
    ) -> Self {
        Self { eth: EthStreamInner::with_max_message_size(version, max_message_size), inner }
    }

    /// Returns the eth version.
    #[inline]
    pub const fn version(&self) -> EthVersion {
        self.eth.version()
    }

    /// Sets whether to reject block announcement messages (`NewBlock`, `NewBlockHashes`) before
    /// RLP decoding.
    pub const fn set_reject_block_announcements(&mut self, reject: bool) {
        self.eth.set_reject_block_announcements(reject);
    }

    /// Returns the underlying stream.
    #[inline]
    pub const fn inner(&self) -> &S {
        &self.inner
    }

    /// Returns mutable access to the underlying stream.
    #[inline]
    pub const fn inner_mut(&mut self) -> &mut S {
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
        self.inner.start_send_unpin(item.encoded())?;
        Ok(())
    }

    /// Sends a Transactions broadcast with a precomputed RLP payload length.
    pub fn start_send_transactions_with_payload_length(
        &mut self,
        transactions: SharedTransactions<N::BroadcastedTransaction>,
        payload_length: usize,
    ) -> Result<(), EthStreamError> {
        self.inner.start_send_unpin(
            EthBroadcastMessage::<N>::encoded_transactions_with_payload_length(
                transactions,
                payload_length,
            ),
        )?;
        Ok(())
    }

    /// Sends a raw capability message directly over the stream
    pub fn start_send_raw(&mut self, msg: RawCapabilityMessage) -> Result<(), EthStreamError> {
        self.inner.start_send_unpin(msg.encoded())?;
        Ok(())
    }

    /// Sends an already encoded eth protocol message.
    pub fn start_send_encoded(&mut self, msg: EncodedEthMessage) -> Result<(), EthStreamError> {
        self.inner.start_send_unpin(msg.into_encoded())?;
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

        match res {
            Some(Ok(bytes)) => Poll::Ready(Some(this.eth.decode_message(bytes))),
            Some(Err(err)) => Poll::Ready(Some(Err(err.into()))),
            None => Poll::Ready(None),
        }
    }
}

impl<S, N> EthStream<P2PStream<S>, N>
where
    S: Sink<Bytes, Error = io::Error> + Unpin + Send + Sync,
    N: NetworkPrimitives,
{
    /// Same as [`Sink::start_send`] but encodes into caller-owned scratch space before compression.
    pub fn start_send_with_encode_buf(
        &mut self,
        item: EthMessage<N>,
        encode_buf: &mut BytesMut,
    ) -> Result<(), EthStreamError> {
        if matches!(item, EthMessage::Status(_)) {
            let _disconnect_future = self.inner.disconnect(DisconnectReason::ProtocolBreach);
            return Err(EthStreamError::EthHandshakeError(EthHandshakeError::StatusNotInHandshake))
        }

        encode_eth_message(item, encode_buf);
        self.inner.start_send_protocol_message(encode_buf)?;
        Ok(())
    }

    /// Same as [`Self::start_send_broadcast`] but encodes into caller-owned scratch space.
    pub fn start_send_broadcast_with_encode_buf(
        &mut self,
        item: EthBroadcastMessage<N>,
        encode_buf: &mut BytesMut,
    ) -> Result<(), EthStreamError> {
        encode_broadcast_message(item, encode_buf);
        self.inner.start_send_protocol_message(encode_buf)?;
        Ok(())
    }

    /// Sends a Transactions broadcast with a precomputed RLP payload length and caller-owned
    /// scratch space.
    pub fn start_send_transactions_with_payload_length_and_encode_buf(
        &mut self,
        transactions: SharedTransactions<N::BroadcastedTransaction>,
        payload_length: usize,
        encode_buf: &mut BytesMut,
    ) -> Result<(), EthStreamError> {
        encode_transactions_broadcast_message(transactions, payload_length, encode_buf);
        self.inner.start_send_protocol_message(encode_buf)?;
        Ok(())
    }

    /// Sends a raw capability message using caller-owned scratch space.
    pub fn start_send_raw_with_encode_buf(
        &mut self,
        msg: RawCapabilityMessage,
        encode_buf: &mut BytesMut,
    ) -> Result<(), EthStreamError> {
        encode_raw_capability_message(msg, encode_buf);
        self.inner.start_send_protocol_message(encode_buf)?;
        Ok(())
    }

    /// Sends an already encoded eth protocol message by reusing its cached eth-only compressed
    /// wire frame.
    pub fn start_send_encoded_eth_only(
        &mut self,
        msg: EncodedEthMessage,
    ) -> Result<(), EthStreamError> {
        let compressed = msg.eth_only_compressed()?;
        self.inner.start_send_precompressed_protocol_message(compressed)?;
        Ok(())
    }
}

fn encode_eth_message<N: NetworkPrimitives>(item: EthMessage<N>, out: &mut BytesMut) {
    out.clear();
    match item {
        EthMessage::NewBlockHashes(hashes) => {
            EthMessageID::NewBlockHashes.encode(out);
            hashes.encode(out);
        }
        EthMessage::NewPooledTransactionHashes66(hashes) => {
            EthMessageID::NewPooledTransactionHashes.encode(out);
            hashes.encode(out);
        }
        EthMessage::NewPooledTransactionHashes68(hashes) => {
            EthMessageID::NewPooledTransactionHashes.encode(out);
            hashes.encode(out);
        }
        EthMessage::NewPooledTransactionHashes72(hashes) => {
            EthMessageID::NewPooledTransactionHashes.encode(out);
            hashes.encode(out);
        }
        this => {
            out.reserve(EthMessageID::Status.length() + this.length());
            ProtocolMessage::from(this).encode(out);
        }
    }
}

fn encode_broadcast_message<N: NetworkPrimitives>(
    item: EthBroadcastMessage<N>,
    out: &mut BytesMut,
) {
    out.clear();
    match item {
        EthBroadcastMessage::Transactions(transactions) => {
            let payload_length = transactions.iter().map(Encodable::length).sum();
            encode_transactions_broadcast_message(transactions, payload_length, out);
        }
        this @ EthBroadcastMessage::NewBlock(_) => {
            out.reserve(EthMessageID::Status.length() + this.length());
            ProtocolBroadcastMessage::from(this).encode(out);
        }
    }
}

fn encode_transactions_broadcast_message<T: Encodable>(
    transactions: SharedTransactions<T>,
    payload_length: usize,
    out: &mut BytesMut,
) {
    let header = Header { list: true, payload_length };
    out.clear();
    out.reserve(EthMessageID::Transactions.length() + header.length() + payload_length);
    EthMessageID::Transactions.encode(out);
    header.encode(out);
    for tx in transactions.iter() {
        tx.encode(out);
    }
}

fn encode_raw_capability_message(msg: RawCapabilityMessage, out: &mut BytesMut) {
    out.clear();
    out.reserve(msg.length());
    msg.encode(out);
}

impl<S, N> EthStream<P2PStream<S>, N>
where
    S: Stream<Item = io::Result<BytesMut>> + Sink<Bytes, Error = io::Error> + Unpin,
    N: NetworkPrimitives,
{
    /// Polls the underlying p2p stream and decodes the next eth message without returning an
    /// intermediate p2p [`BytesMut`] to the caller.
    pub fn poll_next_eth_message(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        decode_buf: &mut BytesMut,
    ) -> Poll<Option<Result<EthMessage<N>, EthStreamError>>> {
        let this = self.project();
        let eth = this.eth;

        let res = ready!(this.inner.poll_next_with(cx, decode_buf, |message_id, payload| {
            eth.decode_message_payload(message_id, payload)
        }));

        match res {
            Some(Ok(Ok(msg))) => Poll::Ready(Some(Ok(msg))),
            Some(Ok(Err(err))) => Poll::Ready(Some(Err(err))),
            Some(Err(err)) => Poll::Ready(Some(Err(err.into()))),
            None => Poll::Ready(None),
        }
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
            // Attempt to disconnect the peer for protocol breach when trying to send Status
            // message after handshake is complete
            let mut this = self.project();
            // We can't await the disconnect future here since this is a synchronous method,
            // but we can start the disconnect process. The actual disconnect will be handled
            // asynchronously by the caller or the stream's poll methods.
            let _disconnect_future = this.inner.disconnect(DisconnectReason::ProtocolBreach);
            return Err(EthStreamError::EthHandshakeError(EthHandshakeError::StatusNotInHandshake))
        }

        self.project().inner.start_send(item.encoded())?;

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
    fn disconnect(
        &mut self,
        reason: DisconnectReason,
    ) -> Pin<Box<dyn Future<Output = Result<(), EthStreamError>> + Send + '_>> {
        Box::pin(async move { self.inner.disconnect(reason).await.map_err(Into::into) })
    }
}

#[cfg(test)]
mod tests {
    use super::UnauthedEthStream;
    use crate::{
        broadcast::BlockHashNumber,
        errors::{EthHandshakeError, EthStreamError},
        ethstream::RawCapabilityMessage,
        hello::DEFAULT_TCP_PORT,
        p2pstream::UnauthedP2PStream,
        EthMessage, EthStream, EthVersion, HelloMessageWithProtocols, PassthroughCodec,
        ProtocolVersion, Status, StatusMessage,
    };
    use alloy_chains::NamedChain;
    use alloy_primitives::{
        bytes::{Bytes, BytesMut},
        B256, U256,
    };
    use alloy_rlp::Decodable;
    use futures::{future::poll_fn, SinkExt, StreamExt};
    use reth_ecies::stream::ECIESStream;
    use reth_eth_wire_types::{EthNetworkPrimitives, UnifiedStatus};
    use reth_ethereum_forks::{ForkFilter, Head};
    use reth_network_peers::pk2id;
    use secp256k1::{SecretKey, SECP256K1};
    use std::{pin::Pin, time::Duration};
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
        let unified_status = UnifiedStatus::from_message(StatusMessage::Legacy(status));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let status_clone = unified_status;
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
            .handshake::<EthNetworkPrimitives>(unified_status, fork_filter)
            .await
            .unwrap();

        // their status is a clone of our status, these should be equal
        assert_eq!(their_status, unified_status);

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
        let unified_status = UnifiedStatus::from_message(StatusMessage::Legacy(status));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let status_clone = unified_status;
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
            .handshake::<EthNetworkPrimitives>(unified_status, fork_filter)
            .await
            .unwrap();

        // their status is a clone of our status, these should be equal
        assert_eq!(their_status, unified_status);

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
            total_difficulty: U256::from(2).pow(U256::from(164)),
            blockhash: B256::random(),
            genesis,
            // Pass the current fork id.
            forkid: fork_filter.current(),
        };
        let unified_status = UnifiedStatus::from_message(StatusMessage::Legacy(status));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let status_clone = unified_status;
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
                    EthHandshakeError::TotalDifficultyBitLenTooLarge { got: 165, maximum: 160 }
                ))
            ));
        });

        let outgoing = TcpStream::connect(local_addr).await.unwrap();
        let sink = PassthroughCodec::default().framed(outgoing);

        // try to connect
        let handshake_res = UnauthedEthStream::new(sink)
            .handshake::<EthNetworkPrimitives>(unified_status, fork_filter)
            .await;

        // this handshake should also fail due to td too high
        assert!(matches!(
            handshake_res,
            Err(EthStreamError::EthHandshakeError(
                EthHandshakeError::TotalDifficultyBitLenTooLarge { got: 165, maximum: 160 }
            ))
        ));

        // await the other handshake
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn can_write_and_read_cleartext() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();
        let test_msg = EthMessage::<EthNetworkPrimitives>::NewBlockHashes(
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
        let server_key = SecretKey::new(&mut rand_08::thread_rng());
        let test_msg = EthMessage::<EthNetworkPrimitives>::NewBlockHashes(
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

        let client_key = SecretKey::new(&mut rand_08::thread_rng());

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
        let server_key = SecretKey::new(&mut rand_08::thread_rng());
        let test_msg = EthMessage::<EthNetworkPrimitives>::NewBlockHashes(
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
        let unified_status = UnifiedStatus::from_message(StatusMessage::Legacy(status));

        let status_copy = unified_status;
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
            let mut decode_buf = BytesMut::new();
            let message =
                poll_fn(|cx| Pin::new(&mut eth_stream).poll_next_eth_message(cx, &mut decode_buf))
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(message, test_msg_clone);
        });

        // create the server pubkey
        let server_id = pk2id(&server_key.public_key(SECP256K1));

        let client_key = SecretKey::new(&mut rand_08::thread_rng());

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

        let (mut client_stream, _) = UnauthedEthStream::new(p2p_stream)
            .handshake(unified_status, fork_filter)
            .await
            .unwrap();

        let mut encode_buf = BytesMut::new();
        client_stream.start_send_with_encode_buf(test_msg, &mut encode_buf).unwrap();
        client_stream.flush().await.unwrap();

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
        let unified_status = UnifiedStatus::from_message(StatusMessage::Legacy(status));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let status_clone = unified_status;
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
                unified_status,
                fork_filter,
                Duration::from_secs(1),
            )
            .await;

        // Assert that a timeout error occurred
        assert!(
            matches!(handshake_result, Err(e) if e.to_string() == EthStreamError::StreamTimeout.to_string())
        );
    }

    #[tokio::test]
    async fn can_write_and_read_raw_capability() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let test_msg = RawCapabilityMessage { id: 0x1234, payload: Bytes::from(vec![1, 2, 3, 4]) };

        let test_msg_clone = test_msg.clone();
        let handle = tokio::spawn(async move {
            let (incoming, _) = listener.accept().await.unwrap();
            let stream = PassthroughCodec::default().framed(incoming);
            let mut stream = EthStream::<_, EthNetworkPrimitives>::new(EthVersion::Eth67, stream);

            let bytes = stream.inner_mut().next().await.unwrap().unwrap();

            // Create a cursor to track position while decoding
            let mut id_bytes = &bytes[..];
            let decoded_id = <usize as Decodable>::decode(&mut id_bytes).unwrap();
            assert_eq!(decoded_id, test_msg_clone.id);

            // Get remaining bytes after ID decoding
            let remaining = id_bytes;
            assert_eq!(remaining, &test_msg_clone.payload[..]);
        });

        let outgoing = TcpStream::connect(local_addr).await.unwrap();
        let sink = PassthroughCodec::default().framed(outgoing);
        let mut client_stream = EthStream::<_, EthNetworkPrimitives>::new(EthVersion::Eth67, sink);

        client_stream.start_send_raw(test_msg).unwrap();
        client_stream.inner_mut().flush().await.unwrap();

        handle.await.unwrap();
    }

    #[tokio::test]
    async fn status_message_after_handshake_triggers_disconnect() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            let (incoming, _) = listener.accept().await.unwrap();
            let stream = PassthroughCodec::default().framed(incoming);
            let mut stream = EthStream::<_, EthNetworkPrimitives>::new(EthVersion::Eth67, stream);

            // Try to send a Status message after handshake - this should trigger disconnect
            let status = Status {
                version: EthVersion::Eth67,
                chain: NamedChain::Mainnet.into(),
                total_difficulty: U256::ZERO,
                blockhash: B256::random(),
                genesis: B256::random(),
                forkid: ForkFilter::new(Head::default(), B256::random(), 0, Vec::new()).current(),
            };
            let status_message =
                EthMessage::<EthNetworkPrimitives>::Status(StatusMessage::Legacy(status));

            // This should return an error and trigger disconnect
            let result = stream.send(status_message).await;
            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                EthStreamError::EthHandshakeError(EthHandshakeError::StatusNotInHandshake)
            ));
        });

        let outgoing = TcpStream::connect(local_addr).await.unwrap();
        let sink = PassthroughCodec::default().framed(outgoing);
        let mut client_stream = EthStream::<_, EthNetworkPrimitives>::new(EthVersion::Eth67, sink);

        // Send a valid message to keep the connection alive
        let test_msg = EthMessage::<EthNetworkPrimitives>::NewBlockHashes(
            vec![BlockHashNumber { hash: B256::random(), number: 5 }].into(),
        );
        client_stream.send(test_msg).await.unwrap();

        handle.await.unwrap();
    }
}
