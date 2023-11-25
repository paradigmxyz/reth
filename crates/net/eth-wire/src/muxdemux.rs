use std::{
    any::TypeId,
    collections::HashMap,
    mem::ManuallyDrop,
    ops::{Deref, DerefMut},
    pin::Pin,
    task::{ready, Context, Poll},
};

use futures::{Sink, SinkExt, StreamExt};
use reth_primitives::bytes::{Bytes, BytesMut};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_stream::Stream;

use crate::{
    capability::{SharedCapabilities, SharedCapability, SharedCapabilityError},
    CanDisconnect, DisconnectReason, EthMessage,
};

use MuxDemuxError::*;

/// Errors thrown by de-/muxing.
#[derive(Error, Debug)]
pub enum MuxDemuxError {
    /// Stream is in use by secondary stream impeding disconnect.
    #[error("secondary streams are still running")]
    StreamInUse,
    /// Stream has already been set up for this capability stream type.
    #[error("stream already init for stream type")]
    StreamAlreadyExists,
    /// Capability stream type is not shared with peer on underlying p2p connection.
    #[error("stream type is not shared on this p2p connection")]
    CapabilityNotShared,
    /// Capability stream type has not been configured in [`MuxDemuxer`].
    #[error("stream type is not configured")]
    CapabilityNotConfigured,
    /// Capability stream type has not been configured for [`SharedCapabilities`] type.
    #[error("stream type is not recognized")]
    CapabilityNotRecognized,
    /// Message ID is out of range.
    #[error("message id out of range, {0}")]
    MessageIdOutOfRange(u8),
    /// Demux channel failed.
    #[error("sending demuxed bytes to secondary stream failed")]
    SendIngressBytesFailed,
    /// Mux channel failed.
    #[error("sending bytes from secondary stream to mux failed")]
    SendEgressBytesFailed,
    /// Attempt to disconnect the p2p stream via a stream clone.
    #[error("secondary stream cannot disconnect p2p stream")]
    CannotDisconnectP2PStream,
    /// Shared capability error.
    #[error(transparent)]
    SharedCapabilityError(#[from] SharedCapabilityError),
}

/// Each reth capability stream type is identified by the message type it handles. For example
/// [`EthMessage`] maps to [`EthStream`]. The stream type is be used to identify the channel on
/// which the [`MuxDemux`] should send demuxed p2p outputs.
#[derive(Eq, PartialEq, Hash, Debug)]
pub enum CapStreamType {
    /// Key for [`crates::EthStream`] type.
    Eth,
}

impl CapStreamType {
    /// Translates the capability stream message type to a [`CapStreamType`] that is used as key
    /// for the de-/mux channels in [`MuxDemuxer`].
    pub fn new<M: 'static>() -> Self {
        if TypeId::of::<M>() == TypeId::of::<EthMessage>() {
            return CapStreamType::Eth
        }

        panic!("capability stream type not configured")
    }
}

impl From<&SharedCapability> for CapStreamType {
    fn from(value: &SharedCapability) -> Self {
        match value {
            SharedCapability::Eth { .. } => CapStreamType::Eth,
            _ => panic!("capability stream type not configured"),
        }
    }
}

/// Stream MUX/DEMUX acts like a regular stream and sink for the owning stream, and handles bytes
/// belonging to other streams over their respective channels.
#[derive(Debug)]
pub struct MuxDemuxer<S> {
    // receive and send muxed p2p outputs
    inner: S,
    // owner of the stream
    owner: SharedCapability,
    // receive muxed p2p inputs from stream clones
    mux: mpsc::UnboundedReceiver<Bytes>,
    // send demuxed p2p outputs to app
    demux: HashMap<CapStreamType, mpsc::UnboundedSender<BytesMut>>,
    // sender to mux stored to make new stream clones
    mux_tx: mpsc::UnboundedSender<Bytes>,
    // capabilities supported by underlying p2p stream
    shared_capabilities: SharedCapabilities,
}

/// The main stream on top of the p2p stream. Wraps [`MuxDemux`] and enforces it can't be dropped
/// before all secondary streams are dropped (stream clones).
#[derive(Debug)]
pub struct MuxDemuxStream<S>(ManuallyDrop<MuxDemuxer<S>>);

impl<S> Deref for MuxDemuxStream<S> {
    type Target = MuxDemuxer<S>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S> DerefMut for MuxDemuxStream<S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<S> MuxDemuxStream<S> {
    /// Creates a new [`MuxDemux`].
    pub fn try_new<M: 'static>(
        inner: S,
        shared_capabilities: SharedCapabilities,
    ) -> Result<Self, MuxDemuxError> {
        let stream = CapStreamType::new::<M>();
        let owner = shared_capabilities
            .iter_caps()
            .find(|cap| stream == Into::<CapStreamType>::into(*cap))
            .ok_or(CapabilityNotShared)?
            .clone();

        let demux = HashMap::new();
        let (mux_tx, mux) = mpsc::unbounded_channel();

        Ok(Self(ManuallyDrop::new(MuxDemuxer {
            inner,
            owner,
            mux,
            demux,
            mux_tx,
            shared_capabilities,
        })))
    }

    /// Clones the stream if the given capability stream type is shared on the underlying p2p
    /// connection.
    pub fn try_clone_stream<M: 'static>(&mut self) -> Result<StreamClone, MuxDemuxError> {
        let cap = self.shared_cap::<M>()?.clone();
        let ingress = self.reg_new_ingress_buffer(&cap)?;
        let mux_tx = self.mux_tx.clone();

        Ok(StreamClone { stream: ingress, sink: mux_tx, cap })
    }

    fn shared_cap<M: 'static>(&self) -> Result<&SharedCapability, MuxDemuxError> {
        for shared_cap in self.shared_capabilities.iter_caps() {
            if let SharedCapability::Eth { .. } = shared_cap {
                if TypeId::of::<M>() == TypeId::of::<EthMessage>() {
                    return Ok(shared_cap)
                }
            }
            // more caps to come ...
        }

        Err(CapabilityNotShared)
    }

    fn reg_new_ingress_buffer(
        &mut self,
        cap: &SharedCapability,
    ) -> Result<mpsc::UnboundedReceiver<BytesMut>, MuxDemuxError> {
        let cap = cap.into();
        if let Some(tx) = self.demux.get(&cap) {
            if !tx.is_closed() {
                return Err(StreamAlreadyExists)
            }
        }
        let (ingress_tx, ingress) = mpsc::unbounded_channel();
        self.demux.insert(cap, ingress_tx);

        Ok(ingress)
    }

    fn unmask_msg_id(&self, id: &mut u8) -> Result<CapStreamType, MuxDemuxError> {
        use SharedCapability::*;

        for cap in self.shared_capabilities.iter_caps() {
            let offset = cap.relative_message_id_offset();
            let next_offset = offset + cap.num_messages()?;
            if *id < next_offset {
                *id = *id - offset;

                return match cap {
                    Eth { .. } => Ok(CapStreamType::new::<EthMessage>()),
                    UnknownCapability { .. } => Err(CapabilityNotRecognized),
                }
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

    fn can_drop(&mut self) -> bool {
        for tx in self.demux.values() {
            if !tx.is_closed() {
                return false
            }
        }

        true
    }

    /// Checks if all clones of this shared stream have been dropped, if true then returns //
    /// function to drop the stream.
    pub fn try_drop(&mut self) -> Result<impl FnOnce(Self), MuxDemuxError> {
        if !self.can_drop() {
            return Err(StreamInUse)
        }

        Ok(|x: Self| {
            let Self(s) = x;
            _ = ManuallyDrop::into_inner(s)
        })
    }
}

impl<S, E> Stream for MuxDemuxStream<S>
where
    S: Stream<Item = Result<BytesMut, E>> + Unpin,
    MuxDemuxError: From<E>,
{
    type Item = Result<BytesMut, MuxDemuxError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // poll once for each shared stream since shared stream clones cannot poll `MuxDemux`
        for _ in 0..self.demux.len() {
            let res = ready!(self.inner.poll_next_unpin(cx));
            let mut bytes = match res {
                Some(Ok(bytes)) => bytes,
                Some(Err(err)) => return Poll::Ready(Some(Err(err.into()))),
                None => return Poll::Ready(None),
            };

            // normalize message id suffix for capability
            let cap = self.unmask_msg_id(&mut bytes[0])?;
            if cap == (&self.owner).into() {
                return Poll::Ready(Some(Ok(bytes)))
            }
            let tx = self.demux.get(&cap).ok_or(CapabilityNotConfigured)?;
            tx.send(bytes.into()).map_err(|_| SendIngressBytesFailed)?;
        }

        Poll::Ready(None)
    }
}

impl<S> Sink<Bytes> for MuxDemuxStream<S>
where
    S: CanDisconnect<Bytes> + Unpin,
    MuxDemuxError: From<<S as Sink<Bytes>>::Error>,
{
    type Error = MuxDemuxError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready_unpin(cx).map_err(Into::into)
    }

    fn start_send(mut self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        let item = self.mask_msg_id(item);
        self.inner.start_send_unpin(item).map_err(|e| e.into())?;

        // sink buffered bytes from `StreamClone`s
        for _ in 0..self.demux.len() - 1 {
            let Ok(item) = self.mux.try_recv() else { break };
            self.inner.start_send_unpin(item).map_err(|e| e.into())?;
        }

        Ok(())
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_flush_unpin(cx).map_err(Into::into)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_close_unpin(cx).map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl<S> CanDisconnect<Bytes> for MuxDemuxStream<S>
where
    S: CanDisconnect<Bytes> + Send,
    MuxDemuxError: From<<S as Sink<Bytes>>::Error>,
{
    async fn disconnect(&mut self, reason: DisconnectReason) -> Result<(), MuxDemuxError> {
        if self.can_drop() {
            return self.inner.disconnect(reason).await.map_err(Into::into)
        }
        Err(StreamInUse)
    }
}

/// More or less a weak clone of the stream wrapped in [`MuxDemux`] but the bytes belonging to
/// other capabilities have been filtered out.
#[derive(Debug)]
pub struct StreamClone {
    stream: mpsc::UnboundedReceiver<BytesMut>,
    sink: mpsc::UnboundedSender<Bytes>,
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
