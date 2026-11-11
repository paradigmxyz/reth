//! Ethereum protocol stream implementations.
//!
//! Provides stream types for the Ethereum wire protocol.
//! It separates protocol logic [`EthStreamInner`] from transport concerns [`EthStream`].
//! Handles handshaking, message processing, and RLP serialization.

use crate::{
    errors::{EthHandshakeError, EthStreamError},
    handshake::EthereumEthHandshake,
    message::{EthBroadcastMessage, MessageError, MAX_MESSAGE_SIZE, TX_MEMORY_BUDGET_MULTIPLIER},
    p2pstream::HANDSHAKE_TIMEOUT,
    CanDisconnect, DisconnectReason, EthMessage, EthNetworkPrimitives, EthVersion, ProtocolMessage,
    UnifiedStatus,
};
use alloy_primitives::bytes::{Bytes, BytesMut};
use alloy_rlp::{Decodable, Header};
use futures::{ready, Sink, SinkExt};
use pin_project::pin_project;
use reth_eth_wire_types::{EthMessageID, NetworkPrimitives, RawCapabilityMessage};
use reth_ethereum_forks::ForkFilter;
use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
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
    S: Stream<Item = Result<BytesMut, E>> + CanDisconnect<Bytes> + Send,
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
    /// Responses the remote owes for requests encoded by [`Self::encode_message`], keyed by
    /// request id.
    ///
    /// [`Self::decode_message`] only decodes a response that answers one of these requests, and
    /// removes the entry when it does. The map is therefore bounded by the requests the owner
    /// keeps in flight.
    pending_responses: BTreeMap<u64, PendingResponse>,
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
            pending_responses: BTreeMap::new(),
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
    ///
    /// A response is only decoded if it answers a request sent through [`Self::encode_message`]
    /// and carries at most as many items as that request asked for, see
    /// [`EthStreamError::UnsolicitedResponse`] and [`EthStreamError::ResponseExceedsRequest`].
    pub fn decode_message(&mut self, bytes: BytesMut) -> Result<EthMessage<N>, EthStreamError> {
        if bytes.len() > self.max_message_size {
            return Err(EthStreamError::MessageTooBig(bytes.len()));
        }

        if self.reject_block_announcements &&
            let Some(&id) = bytes.first() &&
            (id == EthMessageID::NewBlock.to_u8() || id == EthMessageID::NewBlockHashes.to_u8())
        {
            return Err(EthStreamError::UnsupportedMessage { message_id: id });
        }

        self.take_pending_response(&bytes)?;

        let msg = match ProtocolMessage::decode_message_with_tx_memory_budget(
            self.version,
            &mut bytes.as_ref(),
            self.max_message_size * TX_MEMORY_BUDGET_MULTIPLIER,
        ) {
            Ok(m) => m,
            Err(err) => {
                let msg = if bytes.len() > 50 {
                    format!("{:02x?}...{:x?}", &bytes[..10], &bytes[bytes.len() - 10..])
                } else {
                    format!("{bytes:02x?}")
                };
                debug!(
                    version=?self.version,
                    %msg,
                    "failed to decode protocol message"
                );
                return Err(EthStreamError::InvalidMessage(err));
            }
        };

        if matches!(msg.message, EthMessage::Status(_)) {
            return Err(EthStreamError::EthHandshakeError(EthHandshakeError::StatusNotInHandshake));
        }

        Ok(msg.message)
    }

    /// Encodes an [`EthMessage`] to bytes.
    ///
    /// Validates that Status messages are not sent after handshake, enforcing protocol rules.
    /// Requests are recorded so that [`Self::decode_message`] accepts their responses.
    pub fn encode_message(&mut self, item: EthMessage<N>) -> Result<Bytes, EthStreamError> {
        if matches!(item, EthMessage::Status(_)) {
            return Err(EthStreamError::EthHandshakeError(EthHandshakeError::StatusNotInHandshake));
        }

        if let Some((request_id, response)) = PendingResponse::for_request(&item) {
            self.pending_responses.insert(request_id, response);
        }

        Ok(Bytes::from(alloy_rlp::encode(ProtocolMessage::from(item))))
    }

    /// Consumes the pending request an encoded response answers, without decoding the response's
    /// items.
    ///
    /// Decoding allocates per item, and an item can be a single RLP byte, so a few hundred KiB of
    /// compressed frame can decode into hundreds of MiB. Responses nobody asked for, and responses
    /// with more items than requested, are rejected before any of that happens. Messages that are
    /// not responses pass through.
    fn take_pending_response(&mut self, bytes: &[u8]) -> Result<(), EthStreamError> {
        let mut buf = bytes;
        let message_id = EthMessageID::decode(&mut buf).map_err(MessageError::from)?;
        if !matches!(
            message_id,
            EthMessageID::BlockHeaders |
                EthMessageID::BlockBodies |
                EthMessageID::PooledTransactions |
                EthMessageID::NodeData |
                EthMessageID::Receipts |
                EthMessageID::BlockAccessLists |
                EthMessageID::Cells
        ) {
            return Ok(())
        }

        // Every response is a list that starts with the request id.
        let mut payload = Header::decode_bytes(&mut buf, true).map_err(MessageError::from)?;
        let request_id = u64::decode(&mut payload).map_err(MessageError::from)?;

        let Some(pending) = self
            .pending_responses
            .remove(&request_id)
            .filter(|pending| pending.message_id == message_id)
        else {
            return Err(EthStreamError::UnsolicitedResponse { message_id, request_id })
        };

        if response_exceeds_items(self.version, message_id, payload, pending.max_items)
            .map_err(MessageError::from)?
        {
            return Err(EthStreamError::ResponseExceedsRequest {
                message_id,
                request_id,
                max_items: pending.max_items,
            })
        }

        Ok(())
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

    /// Sends a raw capability message directly over the stream
    ///
    /// Raw messages are not recorded as requests, so the response to an `eth` request sent this
    /// way is rejected as unsolicited. Send requests as [`EthMessage`]s instead.
    pub fn start_send_raw(&mut self, msg: RawCapabilityMessage) -> Result<(), EthStreamError> {
        self.inner.start_send_unpin(msg.encoded())?;
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

impl<S, N> Sink<EthMessage<N>> for EthStream<S, N>
where
    S: CanDisconnect<Bytes>,
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

        let this = self.project();
        let bytes = this.eth.encode_message(item)?;
        this.inner.start_send(bytes)?;

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

/// The response the remote owes for a request sent on an [`EthStreamInner`].
#[derive(Clone, Copy, Debug)]
struct PendingResponse {
    /// Message id the response must carry.
    message_id: EthMessageID,
    /// Number of items the request asked for, which the response must not exceed.
    max_items: usize,
}

impl PendingResponse {
    /// Returns the request id and the response owed for `msg`, or `None` if it is not a request.
    fn for_request<N: NetworkPrimitives>(msg: &EthMessage<N>) -> Option<(u64, Self)> {
        let (request_id, message_id, max_items) = match msg {
            EthMessage::GetBlockHeaders(req) => (
                req.request_id,
                EthMessageID::BlockHeaders,
                usize::try_from(req.message.limit).unwrap_or(usize::MAX),
            ),
            EthMessage::GetBlockBodies(req) => {
                (req.request_id, EthMessageID::BlockBodies, req.message.0.len())
            }
            EthMessage::GetPooledTransactions(req) => {
                (req.request_id, EthMessageID::PooledTransactions, req.message.0.len())
            }
            EthMessage::GetNodeData(req) => {
                (req.request_id, EthMessageID::NodeData, req.message.0.len())
            }
            EthMessage::GetReceipts(req) => {
                (req.request_id, EthMessageID::Receipts, req.message.0.len())
            }
            EthMessage::GetReceipts70(req) => {
                (req.request_id, EthMessageID::Receipts, req.message.block_hashes.len())
            }
            EthMessage::GetBlockAccessLists(req) => {
                (req.request_id, EthMessageID::BlockAccessLists, req.message.0.len())
            }
            EthMessage::GetCells(req) => {
                (req.request_id, EthMessageID::Cells, req.message.hashes.len())
            }
            _ => return None,
        };
        Some((request_id, Self { message_id, max_items }))
    }
}

/// Returns whether the response payload following the request id holds more than `max_items`
/// items.
fn response_exceeds_items(
    version: EthVersion,
    message_id: EthMessageID,
    mut payload: &[u8],
    max_items: usize,
) -> alloy_rlp::Result<bool> {
    match message_id {
        // eth/70 receipts carry the `lastBlockIncomplete` flag before the receipt lists.
        EthMessageID::Receipts if version >= EthVersion::Eth70 => {
            bool::decode(&mut payload)?;
        }
        // Cells wraps the per-transaction hashes and cell lists in a struct.
        EthMessageID::Cells => {
            let mut cells = Header::decode_bytes(&mut payload, true)?;
            return Ok(list_exceeds_items(&mut cells, max_items)? ||
                list_exceeds_items(&mut cells, max_items)?)
        }
        _ => {}
    }
    list_exceeds_items(&mut payload, max_items)
}

/// Decodes the header of the RLP list at the front of `buf` and returns whether the list holds
/// more than `max_items` items, without decoding them.
///
/// Advances `buf` past the list.
pub(crate) fn list_exceeds_items(buf: &mut &[u8], max_items: usize) -> alloy_rlp::Result<bool> {
    let mut items = Header::decode_bytes(buf, true)?;
    let mut count = 0;
    while !items.is_empty() {
        if count == max_items {
            return Ok(true)
        }
        let header = Header::decode(&mut items)?;
        // `Header::decode` checked that the payload is in bounds.
        items = &items[header.payload_length..];
        count += 1;
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::{EthStreamInner, UnauthedEthStream};
    use crate::{
        broadcast::BlockHashNumber,
        errors::{EthHandshakeError, EthStreamError},
        ethstream::RawCapabilityMessage,
        hello::DEFAULT_TCP_PORT,
        message::RequestPair,
        p2pstream::UnauthedP2PStream,
        BlockAccessLists, BlockBodies, BlockHeaders, Cells, EthMessage, EthMessageID, EthStream,
        EthVersion, GetBlockAccessLists, GetBlockBodies, GetCells, GetReceipts70,
        HelloMessageWithProtocols, PassthroughCodec, ProtocolMessage, ProtocolVersion, Receipts70,
        Status, StatusMessage,
    };
    use alloy_chains::NamedChain;
    use alloy_primitives::{
        bytes::{Bytes, BytesMut},
        B128, B256, U256,
    };
    use alloy_rlp::Decodable;
    use futures::{SinkExt, StreamExt};
    use reth_ecies::stream::ECIESStream;
    use reth_eth_wire_types::{EthNetworkPrimitives, UnifiedStatus};
    use reth_ethereum_forks::{ForkFilter, Head};
    use reth_network_peers::pk2id;
    use secp256k1::SecretKey;
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
            total_difficulty: U256::from(2).pow(U256::from(100)) - U256::ONE,
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
        let server_key = SecretKey::new(&mut rand::rng());
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
        let server_id = pk2id(&server_key.public_key());

        let client_key = SecretKey::new(&mut rand::rng());

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
        let server_key = SecretKey::new(&mut rand::rng());
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
                id: pk2id(&server_key.public_key()),
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
        let server_id = pk2id(&server_key.public_key());

        let client_key = SecretKey::new(&mut rand::rng());

        let outgoing = TcpStream::connect(local_addr).await.unwrap();
        let sink = ECIESStream::connect(outgoing, client_key, server_id).await.unwrap();

        let client_hello = HelloMessageWithProtocols {
            protocol_version: ProtocolVersion::V5,
            client_version: "bitcoind/1.0.0".to_string(),
            protocols: vec![EthVersion::Eth67.into()],
            port: DEFAULT_TCP_PORT,
            id: pk2id(&client_key.public_key()),
        };

        let unauthed_stream = UnauthedP2PStream::new(sink);
        let (p2p_stream, _) = unauthed_stream.handshake(client_hello).await.unwrap();

        let (mut client_stream, _) = UnauthedEthStream::new(p2p_stream)
            .handshake(unified_status, fork_filter)
            .await
            .unwrap();

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

    /// Encodes `msg` with its message id, as [`EthStreamInner::decode_message`] receives it.
    fn encode(msg: EthMessage<EthNetworkPrimitives>) -> BytesMut {
        alloy_rlp::encode(ProtocolMessage::from(msg)).as_slice().into()
    }

    fn get_block_access_lists(request_id: u64, hashes: usize) -> EthMessage<EthNetworkPrimitives> {
        EthMessage::GetBlockAccessLists(RequestPair {
            request_id,
            message: GetBlockAccessLists(vec![B256::ZERO; hashes]),
        })
    }

    fn block_access_lists(request_id: u64, entries: usize) -> EthMessage<EthNetworkPrimitives> {
        EthMessage::BlockAccessLists(RequestPair {
            request_id,
            message: BlockAccessLists(vec![None; entries]),
        })
    }

    #[test]
    fn unsolicited_response_is_rejected() {
        let mut eth = EthStreamInner::<EthNetworkPrimitives>::new(EthVersion::Eth71);

        let err = eth.decode_message(encode(block_access_lists(7, 1))).unwrap_err();
        assert!(matches!(
            err,
            EthStreamError::UnsolicitedResponse {
                message_id: EthMessageID::BlockAccessLists,
                request_id: 7
            }
        ));
        assert!(err.is_protocol_breach());
    }

    #[test]
    fn response_to_sent_request_is_decoded_once() {
        let mut eth = EthStreamInner::<EthNetworkPrimitives>::new(EthVersion::Eth71);
        eth.encode_message(get_block_access_lists(7, 2)).unwrap();

        let response = block_access_lists(7, 2);
        assert_eq!(eth.decode_message(encode(response.clone())).unwrap(), response);
        assert!(matches!(
            eth.decode_message(encode(response)).unwrap_err(),
            EthStreamError::UnsolicitedResponse {
                message_id: EthMessageID::BlockAccessLists,
                request_id: 7
            }
        ));
    }

    #[test]
    fn response_with_more_items_than_requested_is_rejected() {
        let mut eth = EthStreamInner::<EthNetworkPrimitives>::new(EthVersion::Eth71);
        eth.encode_message(get_block_access_lists(7, 2)).unwrap();

        let err = eth.decode_message(encode(block_access_lists(7, 3))).unwrap_err();
        assert!(matches!(
            err,
            EthStreamError::ResponseExceedsRequest {
                message_id: EthMessageID::BlockAccessLists,
                request_id: 7,
                max_items: 2
            }
        ));
        assert!(err.is_protocol_breach());
    }

    #[test]
    fn response_of_other_type_is_rejected() {
        let mut eth = EthStreamInner::<EthNetworkPrimitives>::new(EthVersion::Eth71);
        eth.encode_message(EthMessage::GetBlockBodies(RequestPair {
            request_id: 7,
            message: GetBlockBodies(vec![B256::ZERO]),
        }))
        .unwrap();

        let response = EthMessage::BlockHeaders(RequestPair {
            request_id: 7,
            message: BlockHeaders(Vec::new()),
        });
        assert!(matches!(
            eth.decode_message(encode(response)).unwrap_err(),
            EthStreamError::UnsolicitedResponse {
                message_id: EthMessageID::BlockHeaders,
                request_id: 7
            }
        ));

        // The request was consumed, so its proper response is now unsolicited as well.
        let response = EthMessage::BlockBodies(RequestPair {
            request_id: 7,
            message: BlockBodies(Vec::new()),
        });
        assert!(matches!(
            eth.decode_message(encode(response)).unwrap_err(),
            EthStreamError::UnsolicitedResponse { message_id: EthMessageID::BlockBodies, .. }
        ));
    }

    #[test]
    fn receipts70_response_items_are_receipt_lists() {
        let mut eth = EthStreamInner::<EthNetworkPrimitives>::new(EthVersion::Eth70);
        let request = |request_id| {
            EthMessage::GetReceipts70(RequestPair {
                request_id,
                message: GetReceipts70 {
                    first_block_receipt_index: 0,
                    block_hashes: vec![B256::ZERO],
                },
            })
        };
        let response = |request_id, lists| {
            EthMessage::Receipts70(RequestPair {
                request_id,
                message: Receipts70 {
                    last_block_incomplete: true,
                    receipts: vec![Vec::new(); lists],
                },
            })
        };

        eth.encode_message(request(1)).unwrap();
        assert_eq!(eth.decode_message(encode(response(1, 1))).unwrap(), response(1, 1));

        eth.encode_message(request(2)).unwrap();
        assert!(matches!(
            eth.decode_message(encode(response(2, 2))).unwrap_err(),
            EthStreamError::ResponseExceedsRequest {
                message_id: EthMessageID::Receipts,
                request_id: 2,
                max_items: 1
            }
        ));
    }

    #[test]
    fn cells_response_bounds_hashes_and_cells() {
        let mut eth = EthStreamInner::<EthNetworkPrimitives>::new(EthVersion::Eth72);
        let request = |request_id| {
            EthMessage::GetCells(RequestPair {
                request_id,
                message: GetCells { hashes: vec![B256::ZERO], cell_mask: B128::ZERO },
            })
        };
        let response = |request_id, hashes, cells| {
            EthMessage::Cells(RequestPair {
                request_id,
                message: Cells {
                    hashes: vec![B256::ZERO; hashes],
                    cells: vec![Vec::new(); cells],
                    cell_mask: B128::ZERO,
                },
            })
        };

        eth.encode_message(request(1)).unwrap();
        assert_eq!(eth.decode_message(encode(response(1, 1, 1))).unwrap(), response(1, 1, 1));

        for (request_id, hashes, cells) in [(2, 2, 1), (3, 1, 2)] {
            eth.encode_message(request(request_id)).unwrap();
            assert!(matches!(
                eth.decode_message(encode(response(request_id, hashes, cells))).unwrap_err(),
                EthStreamError::ResponseExceedsRequest {
                    message_id: EthMessageID::Cells,
                    max_items: 1,
                    ..
                }
            ));
        }
    }
}
