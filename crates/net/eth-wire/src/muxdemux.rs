//! [`MuxDemuxer`] allows for multiple capability streams to share the same p2p connection. De-/
//! muxing the connection offers two stream types [`MuxDemuxStream`] and [`StreamClone`].
//! [`MuxDemuxStream`] is the main stream that wraps the p2p connection, only this stream can
//! advance transfer across the network. One [`MuxDemuxStream`] can have many [`StreamClone`]s,
//! these are weak clones of the stream and depend on advancing the [`MuxDemuxStream`] to make
//! progress.
//!
//! [`MuxDemuxer`] filters bytes according to message ID offset. The message ID offset is
//! negotiated upon start of the p2p connection. Bytes received by polling the [`MuxDemuxStream`]
//! or a [`StreamClone`] are specific to the capability stream wrapping it. When received the
//! message IDs are unmasked so that all message IDs start at 0x0. [`MuxDemuxStream`] and
//! [`StreamClone`] mask message IDs before sinking bytes to the [`MuxDemuxer`].
//!
//! For example, `EthStream<MuxDemuxStream<P2PStream<S>>>` is the main capability stream.
//! Subsequent capability streams clone the p2p connection via EthStream.
//!
//! When [`MuxDemuxStream`] is polled, [`MuxDemuxer`] receives bytes from the network. If these
//! bytes belong to the capability stream wrapping the [`MuxDemuxStream`] then they are passed up
//! directly. If these bytes however belong to another capability stream, then they are buffered
//! on a channel. When [`StreamClone`] is polled, bytes are read from this buffer. Similarly
//! [`StreamClone`] buffers egress bytes for [`MuxDemuxer`] that are read and sent to the network
//! when [`MuxDemuxStream`] is polled.

use std::{
    collections::HashMap,
    pin::Pin,
    task::{ready, Context, Poll},
};

use derive_more::{Deref, DerefMut};
use futures::{Sink, SinkExt, StreamExt};
use reth_primitives::bytes::{Bytes, BytesMut};
use tokio::sync::mpsc;
use tokio_stream::Stream;

use crate::{
    capability::{Capability, SharedCapabilities, SharedCapability},
    errors::MuxDemuxError,
    CanDisconnect, DisconnectP2P, DisconnectReason,
};

use MuxDemuxError::*;

/// The stream type is be used to identify the channel on which the [`MuxDemuxer`] should send
/// demuxed p2p outputs. Each reth capability stream type is identified by the message type it
/// handles. For example [`EthMessage`] maps to [`crate::EthStream`].
#[derive(Eq, PartialEq, Hash, Debug)]
pub enum CapStreamType {
    /// Key for [`crate::EthStream`] type.
    Eth,
}

/// Stream MUX/DEMUX acts like a regular stream and sink for the owning stream, and handles bytes
/// belonging to other streams over their respective channels.
#[derive(Debug)]
pub struct MuxDemuxer<S> {
    // receive and send muxed p2p outputs
    inner: S,
    // owner of the stream. stores message id offset for this capability.
    owner: SharedCapability,
    // receive muxed p2p inputs from stream clones
    mux: mpsc::UnboundedReceiver<Bytes>,
    // send demuxed p2p outputs to app
    demux: HashMap<SharedCapability, mpsc::UnboundedSender<BytesMut>>,
    // sender to mux stored to make new stream clones
    mux_tx: mpsc::UnboundedSender<Bytes>,
    // capabilities supported by underlying p2p stream (makes testing easier to store here too).
    shared_capabilities: SharedCapabilities,
}

/// The main stream on top of the p2p stream. Wraps [`MuxDemuxer`] and enforces it can't be dropped
/// before all secondary streams are dropped (stream clones).
#[derive(Debug, Deref, DerefMut)]
pub struct MuxDemuxStream<S>(MuxDemuxer<S>);

impl<S> MuxDemuxStream<S> {
    /// Creates a new [`MuxDemuxer`].
    pub fn try_new(
        inner: S,
        cap: Capability,
        shared_capabilities: SharedCapabilities,
    ) -> Result<Self, MuxDemuxError> {
        let owner = Self::shared_cap(&cap, &shared_capabilities)?.clone();

        let demux = HashMap::new();
        let (mux_tx, mux) = mpsc::unbounded_channel();

        Ok(Self(MuxDemuxer { inner, owner, mux, demux, mux_tx, shared_capabilities }))
    }

    /// Clones the stream if the given capability stream type is shared on the underlying p2p
    /// connection.
    pub fn try_clone_stream(&mut self, cap: &Capability) -> Result<StreamClone, MuxDemuxError> {
        let cap = Self::shared_cap(cap, &self.shared_capabilities)?.clone();
        let ingress = self.reg_new_ingress_buffer(&cap)?;
        let mux_tx = self.mux_tx.clone();

        Ok(StreamClone { stream: ingress, sink: mux_tx, cap })
    }

    /// Starts a graceful disconnect. Returns a closure to drop the stream.
    pub fn start_disconnect(&mut self, reason: DisconnectReason) -> Result<(), MuxDemuxError>
    where
        S: DisconnectP2P,
    {
        if !self.can_drop() {
            return Err(StreamInUse)
        }

        self.inner.start_disconnect(reason).map_err(|e| e.into())
    }

    /// Returns `true` if the connection is about to disconnect.
    pub fn is_disconnecting(&self) -> bool
    where
        S: DisconnectP2P,
    {
        self.inner.is_disconnecting()
    }

    /// Shared capabilities of underlying p2p connection as negotiated by peers at connection
    /// open.
    pub fn shared_capabilities(&self) -> &SharedCapabilities {
        &self.shared_capabilities
    }

    fn shared_cap<'a>(
        cap: &Capability,
        shared_capabilities: &'a SharedCapabilities,
    ) -> Result<&'a SharedCapability, MuxDemuxError> {
        for shared_cap in shared_capabilities.iter_caps() {
            match shared_cap {
                SharedCapability::Eth { .. } if cap.is_eth() => return Ok(shared_cap),
                _ => continue,
                // more caps to come ..
            }
        }

        Err(CapabilityNotShared)
    }

    fn reg_new_ingress_buffer(
        &mut self,
        cap: &SharedCapability,
    ) -> Result<mpsc::UnboundedReceiver<BytesMut>, MuxDemuxError> {
        if let Some(tx) = self.demux.get(cap) {
            if !tx.is_closed() {
                return Err(StreamAlreadyExists)
            }
        }
        let (ingress_tx, ingress) = mpsc::unbounded_channel();
        self.demux.insert(cap.clone(), ingress_tx);

        Ok(ingress)
    }

    fn unmask_msg_id(&self, id: &mut u8) -> Result<&SharedCapability, MuxDemuxError> {
        for cap in self.shared_capabilities.iter_caps() {
            let offset = cap.relative_message_id_offset();
            let next_offset = offset + cap.num_messages()?;
            if *id < next_offset {
                *id -= offset;

                return Ok(cap)
            }
        }

        Err(MessageIdOutOfRange(*id))
    }

    /// Masks message id with offset relative to the message id suffix reserved for capability
    /// message ids. The p2p stream further masks the message id (todo: mask whole message id at
    /// once to avoid copying message to mutate id byte or sink BytesMut).
    fn mask_msg_id(&self, msg: Bytes) -> Bytes {
        let mut masked_bytes = BytesMut::zeroed(msg.len());
        masked_bytes[0] = msg[0] + self.owner.relative_message_id_offset();
        masked_bytes[1..].copy_from_slice(&msg[1..]);

        masked_bytes.freeze()
    }

    /// Checks if all clones of this shared stream have been dropped, if true then returns //
    /// function to drop the stream.
    fn can_drop(&mut self) -> bool {
        for tx in self.demux.values() {
            if !tx.is_closed() {
                return false
            }
        }

        true
    }
}

impl<S, E> Stream for MuxDemuxStream<S>
where
    S: Stream<Item = Result<BytesMut, E>> + CanDisconnect<Bytes> + Unpin,
    MuxDemuxError: From<E> + From<<S as Sink<Bytes>>::Error>,
{
    type Item = Result<BytesMut, MuxDemuxError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // sink buffered bytes from `StreamClone`s
        for _ in 0..self.demux.len() {
            if self.inner.poll_ready_unpin(cx).is_pending() {
                break
            }
            let Ok(item) = self.mux.try_recv() else { break };
            self.inner.start_send_unpin(item)?;
        }
        _ = self.inner.poll_flush_unpin(cx);

        // advances the wire and either yields or delegates the message
        loop {
            let res = ready!(self.inner.poll_next_unpin(cx));
            let mut bytes = match res {
                Some(Ok(bytes)) => bytes,
                Some(Err(err)) => return Poll::Ready(Some(Err(err.into()))),
                None => return Poll::Ready(None),
            };

            // normalize message id suffix for capability
            let cap = self.unmask_msg_id(&mut bytes[0])?;

            // yield message for main stream
            if *cap == self.owner {
                return Poll::Ready(Some(Ok(bytes)))
            }

            // delegate message for stream clone
            let tx = self.demux.get(cap).ok_or(CapabilityNotConfigured)?;
            tx.send(bytes).map_err(|_| SendIngressBytesFailed)?;
        }
    }
}

impl<S, E> Sink<Bytes> for MuxDemuxStream<S>
where
    S: Sink<Bytes, Error = E> + CanDisconnect<Bytes> + Unpin,
    MuxDemuxError: From<E>,
{
    type Error = MuxDemuxError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready_unpin(cx).map_err(Into::into)
    }

    fn start_send(mut self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        let item = self.mask_msg_id(item);
        self.inner.start_send_unpin(item).map_err(|e| e.into())
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_flush_unpin(cx).map_err(Into::into)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_close_unpin(cx).map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl<S, E> CanDisconnect<Bytes> for MuxDemuxStream<S>
where
    S: Sink<Bytes, Error = E> + CanDisconnect<Bytes> + Unpin + Send + Sync,
    MuxDemuxError: From<E>,
{
    async fn disconnect(&mut self, reason: DisconnectReason) -> Result<(), MuxDemuxError> {
        if self.can_drop() {
            return self.inner.disconnect(reason).await.map_err(Into::into)
        }
        Err(StreamInUse)
    }
}

/// More or less a weak clone of the stream wrapped in [`MuxDemuxer`] but the bytes belonging to
/// other capabilities have been filtered out.
#[derive(Debug)]
pub struct StreamClone {
    // receive bytes from de-/muxer
    stream: mpsc::UnboundedReceiver<BytesMut>,
    // send bytes to de-/muxer
    sink: mpsc::UnboundedSender<Bytes>,
    // message id offset for capability holding this clone
    cap: SharedCapability,
}

impl StreamClone {
    fn mask_msg_id(&self, msg: Bytes) -> Bytes {
        let mut masked_bytes = BytesMut::zeroed(msg.len());
        masked_bytes[0] = msg[0] + self.cap.relative_message_id_offset();
        masked_bytes[1..].copy_from_slice(&msg[1..]);

        masked_bytes.freeze()
    }
}

impl Stream for StreamClone {
    type Item = BytesMut;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.poll_recv(cx)
    }
}

impl Sink<Bytes> for StreamClone {
    type Error = MuxDemuxError;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        let item = self.mask_msg_id(item);
        self.sink.send(item).map_err(|_| SendEgressBytesFailed)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

#[async_trait::async_trait]
impl CanDisconnect<Bytes> for StreamClone {
    async fn disconnect(&mut self, _reason: DisconnectReason) -> Result<(), MuxDemuxError> {
        Err(CannotDisconnectP2PStream)
    }
}

#[cfg(test)]
mod test {
    use reth_primitives::bytes::{BufMut, BytesMut};

    use crate::{
        capability::{Capability, SharedCapabilities},
        muxdemux::MuxDemuxStream,
        protocol::Protocol,
        EthVersion,
    };

    fn shared_caps_eth68() -> SharedCapabilities {
        let local_capabilities: Vec<Protocol> = vec![Protocol::new(EthVersion::Eth68.into(), 10)];
        let peer_capabilities: Vec<Capability> = vec![EthVersion::Eth68.into()];
        SharedCapabilities::try_new(local_capabilities, peer_capabilities).unwrap()
    }

    #[test]
    fn test_unmask_msg_id() {
        let mut msg = BytesMut::with_capacity(1);
        msg.put_u8(0x07); // eth msg id

        let mxdmx_stream =
            MuxDemuxStream::try_new((), Capability::eth(EthVersion::Eth67), shared_caps_eth68())
                .unwrap();
        _ = mxdmx_stream.unmask_msg_id(&mut msg[0]).unwrap();

        assert_eq!(msg.as_ref(), &[0x07]);
    }

    #[test]
    fn test_mask_msg_id() {
        let mut msg = BytesMut::with_capacity(2);
        msg.put_u8(0x10); // eth msg id
        msg.put_u8(0x20); // some msg data

        let mxdmx_stream =
            MuxDemuxStream::try_new((), Capability::eth(EthVersion::Eth66), shared_caps_eth68())
                .unwrap();
        let egress_bytes = mxdmx_stream.mask_msg_id(msg.freeze());

        assert_eq!(egress_bytes.as_ref(), &[0x10, 0x20]);
    }

    #[test]
    fn test_unmask_msg_id_cap_not_in_shared_range() {
        let mut msg = BytesMut::with_capacity(1);
        msg.put_u8(0x11);

        let mxdmx_stream =
            MuxDemuxStream::try_new((), Capability::eth(EthVersion::Eth68), shared_caps_eth68())
                .unwrap();

        assert!(mxdmx_stream.unmask_msg_id(&mut msg[0]).is_err());
    }

    #[test]
    #[ignore = "more subprotocols than eth have to be implemented"]
    fn test_try_drop_stream_owner() {}
}
