//! [`SharedStream`] sits transparently between P2PStream and EthStream.
//!
//! Under the hood, SharedStream uses a [`CapStream`] for demux/mux of the [`crate::P2PStream`]
//! and capability streams like [`crate::EthStream`]. Demux includes normalizing the message ID so
//! that all capability streams receive messages with IDs starting from 0x0. Mux includes masking
//! the message ID again with the same offset.
//!
//! Only the [`SharedStream`] owning the [`CapStream`] can poll it. [`SharedStream`]s wrapping a
//! [`WeakCapStreamClone`] rely on the first [`SharedStream`].
//
//                                        --BytesMut--> EthStream
// P2PStream --BytesMut--> SharedStream -|
//                                        --BytesMut--> <some other capability>
//
//                                      --Bytes-- EthStream
// P2PStream <--Bytes-- SharedStream <-|
//                                      --Bytes-- <some other capability>
use std::{
    any::TypeId,
    collections::HashMap,
    mem::{self, Discriminant, ManuallyDrop},
    ops::{Deref, DerefMut},
    pin::Pin,
    task::{ready, Context, Poll},
};

use futures::{Sink, SinkExt, StreamExt};
use pin_project::pin_project;
use reth_primitives::bytes::{Bytes, BytesMut};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::Stream;

use crate::{
    capability::{Capability, SharedCapability},
    errors::{P2PStreamError, SharedStreamError},
    p2pstream::SharedCapabilities,
    CanDisconnect, DisconnectReason, EthMessage,
};

use SharedStreamError::*;

type CapKey = Discriminant<IngressCapBytes>;

impl TryFrom<Capability> for CapKey {
    type Error = SharedStreamError;
    fn try_from(value: Capability) -> Result<Self, Self::Error> {
        if value.is_eth_v66() || value.is_eth_v67() || value.is_eth_v68() {
            return Ok(IngressCapBytes::eth_key())
        }

        Err(CapabilityNotRecognized)
    }
}

macro_rules! try_from_shared_cap_impl {
    ($cap:ty) => {
        impl TryFrom<$cap> for CapKey {
            type Error = SharedStreamError;
            fn try_from(value: $cap) -> Result<Self, Self::Error> {
                use SharedCapability::*;

                match value {
                    Eth { .. } => Ok(IngressCapBytes::eth_key()),
                    UnknownCapability { .. } => Err(CapabilityNotRecognized),
                }
            }
        }
    };
}

try_from_shared_cap_impl!(SharedCapability);
try_from_shared_cap_impl!(&SharedCapability);

/// Wrapper around ingress message with unmasked message ID.
#[derive(Debug)]
pub enum IngressCapBytes {
    Eth(BytesMut),
}

impl IngressCapBytes {
    fn eth_key() -> CapKey {
        mem::discriminant(&IngressCapBytes::Eth(BytesMut::default()))
    }
}

impl From<IngressCapBytes> for BytesMut {
    fn from(value: IngressCapBytes) -> Self {
        match value {
            IngressCapBytes::Eth(b) => b,
        }
    }
}

/// Message sent from a [`SharedStream`] to underlying [`CapStream`].
#[derive(Debug)]
pub enum CapStreamMsg {
    /// Control message to clone the shared stream.
    Clone(TypeId, oneshot::Sender<SharedStream<WeakCapStreamClone>>),
    /// Wrapper around egress message with unmasked message ID.
    EgressBytes(Bytes),
}

/// Stream sharing underlying stream. First instant owns the stream and following instants cloned
/// from first own a weak clone of stream. Only the first instant can poll the underlying stream,
/// but doing so triggers poll to immediately repeat once for every weak clone. At most one stream
/// share per message type, e.g. [`EthMessage`], can be open. This is ensured by the underlying
/// [`CapStream`].
#[derive(Debug)]
pub struct SharedStream<S> {
    inner: S,
    ingress: mpsc::UnboundedReceiver<BytesMut>,
    cap: SharedCapability,
}

impl<S> SharedStream<S>
where
    S: Sink<CapStreamMsg> + Unpin,
    SharedStreamError: From<<S as Sink<CapStreamMsg>>::Error>,
{
    #[allow(clippy::type_complexity)]
    fn _clone_for<T: 'static>(
        &mut self,
    ) -> Result<
        Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<SharedStream<WeakCapStreamClone>, SharedStreamError>,
                    > + Send,
            >,
        >,
        SharedStreamError,
    > {
        let cap_type = TypeId::of::<T>();
        let (tx, rx) = oneshot::channel();
        self.inner.start_send_unpin(CapStreamMsg::Clone(cap_type, tx))?;

        Ok(Box::pin(async move { rx.await.map_err(RecvClonedStreamFailed) }))
    }
}

impl<S> SharedStream<S> {
    fn mask_msg_id(&self, msg: Bytes) -> Bytes {
        let mut masked_bytes = BytesMut::zeroed(msg.len());
        masked_bytes[0] = msg[0] + self.cap.offset_rel_caps_suffix();
        masked_bytes[1..].copy_from_slice(&msg[1..]);

        masked_bytes.freeze()
    }
}

impl<S, E> Stream for SharedStream<S>
where
    S: Stream<Item = Result<(), E>> + Unpin,
    SharedStreamError: From<E>,
{
    type Item = Result<BytesMut, SharedStreamError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        _ = self.inner.poll_next_unpin(cx)?;

        let res = ready!(self.ingress.poll_recv(cx));

        match res {
            Some(b) => Poll::Ready(Some(Ok(b))),
            None => Poll::Ready(Some(Err(RecvIngressBytesFailed))),
        }
    }
}

/// Template implementation of [`Sink`], [`CanDisconnect`] and [`CanDisconnectExt`] for crate
/// [`Stream`] + [`Sink`] wrapper types.
#[macro_export]
macro_rules! sink_impl {
    ($stream:ty, $sink_in:ty, $sink_out:ty, $start_send:item, $disconnect: item) => {
        impl<S> Sink<$sink_in> for $stream
        where
            S: CanDisconnect<$sink_out> + Unpin,
            SharedStreamError: From<<S as Sink<$sink_out>>::Error>,
        {
            type Error = SharedStreamError;

            fn poll_ready(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<Result<(), Self::Error>> {
                self.inner.poll_ready_unpin(cx).map_err(Into::into)
            }

            $start_send

            fn poll_flush(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<Result<(), Self::Error>> {
                self.inner.poll_flush_unpin(cx).map_err(Into::into)
            }

            fn poll_close(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<Result<(), Self::Error>> {
                self.inner.poll_close_unpin(cx).map_err(Into::into)
            }
        }

        #[async_trait::async_trait]
        impl<S> CanDisconnect<$sink_in> for $stream
        where
            S: CanDisconnect<$sink_out> + Send,
            SharedStreamError: From<<S as Sink<$sink_out>>::Error>,
        {
            $disconnect
        }
    };
}

sink_impl!(
    SharedStream<S>,
    Bytes,
    CapStreamMsg,
    fn start_send(mut self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        let item = self.mask_msg_id(item);
        self.inner.start_send_unpin(CapStreamMsg::EgressBytes(item))?;
        Ok(())
    },
    async fn disconnect(&mut self, reason: DisconnectReason) -> Result<(), SharedStreamError> {
        self.inner.disconnect(reason).await.map_err(Into::into)
    }
);

impl<S> SharedStream<ManuallyDropCapStream<S>> {
    /// Returns a shared stream if capability is shared.
    pub fn try_new<T: 'static>(caps_stream: CapStream<S>) -> Result<Self, SharedStreamError> {
        caps_stream.try_into_shared::<T>()
    }

    /// Checks if all clones of this shared stream have been dropped, if true then returns function
    /// to drop [`SharedStream`].
    pub fn try_drop(&mut self) -> Result<impl FnOnce(Self), SharedStreamError> {
        if self.inner.can_drop() {
            return Ok(|x: Self| {
                let Self { inner, .. } = x;
                _ = ManuallyDrop::into_inner(inner.inner)
            })
        }

        Err(SharedStreamInUse)
    }
}

/// Stream MUX/DEMUX is wrapped in a [`SharedStream`] that sits transparently between underlying
/// stream and capability streams.
#[derive(Debug)]
pub struct CapStream<S> {
    // receive and send muxed p2p outputs
    inner: S,
    // receive muxed p2p inputs from `WeakCapStreamClone`s
    mux: mpsc::UnboundedReceiver<CapStreamMsg>,
    // sender to mux stored to make new `WeakCapStreamClone`s
    mux_tx: mpsc::UnboundedSender<CapStreamMsg>,
    // send demuxed p2p outputs to app
    demux: HashMap<CapKey, mpsc::UnboundedSender<BytesMut>>,
    // capabilities supported by underlying stream
    shared_capabilities: SharedCapabilities,
}

impl<S> CapStream<S> {
    /// Creates a new [`CapStream`] to demux incoming and mux outgoing capability messages.
    pub fn new(inner: S, shared_capabilities: SharedCapabilities) -> Self {
        let demux = HashMap::new();
        let (mux_tx, mux) = mpsc::unbounded_channel();

        Self { inner, mux, mux_tx, demux, shared_capabilities }
    }

    /// Wraps [`CapStream`] in a [`SharedStream`] that can be used to send and receive capability
    /// specific messages.
    pub fn try_into_shared<T: 'static>(
        mut self,
    ) -> Result<SharedStream<ManuallyDropCapStream<S>>, SharedStreamError> {
        let cap_type = TypeId::of::<T>();
        let cap = self.shared_cap_for(cap_type)?;
        let ingress = self.reg_new_ingress_buffer_for(&cap)?;

        Ok(SharedStream { inner: ManuallyDrop::new(self).into(), ingress, cap })
    }

    fn clone_stream_for(
        &mut self,
        cap: TypeId,
    ) -> Result<SharedStream<WeakCapStreamClone>, SharedStreamError> {
        let cap = self.shared_cap_for(cap)?;
        let ingress = self.reg_new_ingress_buffer_for(&cap)?;
        let mux_tx = self.mux_tx.clone();

        Ok(SharedStream { inner: WeakCapStreamClone(mux_tx), ingress, cap })
    }

    fn shared_cap_for(&self, cap_type: TypeId) -> Result<SharedCapability, SharedStreamError> {
        for shared_cap in self.shared_capabilities.iter_caps() {
            if let SharedCapability::Eth { .. } = shared_cap {
                if cap_type == TypeId::of::<EthMessage>() {
                    return Ok(shared_cap.clone())
                }
            }
        }

        Err(CapabilityNotConfigurable)
    }

    fn reg_new_ingress_buffer_for(
        &mut self,
        cap: &SharedCapability,
    ) -> Result<mpsc::UnboundedReceiver<BytesMut>, SharedStreamError> {
        let cap_key = cap.try_into()?;
        if let Some(tx) = self.demux.get(&cap_key) {
            if !tx.is_closed() {
                return Err(SharedStreamExists)
            }
        }
        let (ingress_tx, ingress) = mpsc::unbounded_channel();
        self.demux.insert(cap_key, ingress_tx);

        Ok(ingress)
    }

    fn interpret_msg(&mut self, msg: CapStreamMsg) -> Result<(), SharedStreamError>
    where
        S: CanDisconnect<Bytes> + Unpin,
        SharedStreamError: From<<S as Sink<Bytes>>::Error>,
    {
        match msg {
            CapStreamMsg::Clone(cap, signal_tx) => {
                let conn_clone = self.clone_stream_for(cap)?;
                signal_tx.send(conn_clone).map_err(|_| SendClonedStreamFailed)
            }
            CapStreamMsg::EgressBytes(bytes) => {
                self.inner.start_send_unpin(bytes).map_err(|e| e.into())
            }
        }
    }

    fn unmask_msg_id(&self, mut msg: BytesMut) -> Result<IngressCapBytes, SharedStreamError> {
        use SharedCapability::*;

        let id = msg[0];

        for cap in self.shared_capabilities.iter_caps() {
            let offset = cap.offset_rel_caps_suffix();
            let next_offset = offset + cap.num_messages()?;
            if id < next_offset {
                msg[0] = id - offset;

                return match cap {
                    Eth { .. } => Ok(IngressCapBytes::Eth(msg)),
                    UnknownCapability { .. } => Err(CapabilityNotRecognized),
                }
            }
        }

        Err(UnknownCapabilityMessageId(id))
    }

    fn can_drop(&mut self) -> bool {
        for tx in self.demux.values() {
            if !tx.is_closed() {
                return false
            }
        }

        true
    }
}

impl<S, E> Stream for CapStream<S>
where
    S: Stream<Item = Result<BytesMut, E>> + Unpin,
    SharedStreamError: From<E>,
{
    type Item = Result<(), SharedStreamError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // poll once for each shared stream since weak shared stream clones cannot poll `CapStream`
        for _ in 0..self.demux.len() {
            let res = ready!(self.inner.poll_next_unpin(cx));
            let bytes = match res {
                Some(Ok(bytes)) => bytes,
                Some(Err(err)) => return Poll::Ready(Some(Err(err.into()))),
                None => return Poll::Ready(None),
            };

            // normalize message id suffix for capability
            let cap_bytes = self.unmask_msg_id(bytes)?;
            let cap_key = mem::discriminant(&cap_bytes);
            let cap_tx = self.demux.get(&cap_key).ok_or(CapabilityNotConfigured)?;
            cap_tx.send(cap_bytes.into()).map_err(SendIngressBytesFailed)?;
        }

        Poll::Ready(Some(Ok(())))
    }
}

sink_impl!(
    CapStream<S>,
    CapStreamMsg,
    Bytes,
    fn start_send(mut self: Pin<&mut Self>, item: CapStreamMsg) -> Result<(), Self::Error> {
        self.interpret_msg(item)?;

        // start send for `WeakCapStreamClone`s too as they cannot poll `CapStream`
        for _ in 0..self.demux.len() - 1 {
            let Ok(item) = self.mux.try_recv() else { break };
            self.interpret_msg(item)?
        }

        Ok(())
    },
    async fn disconnect(&mut self, reason: DisconnectReason) -> Result<(), SharedStreamError> {
        if self.can_drop() {
            return self.inner.disconnect(reason).await.map_err(Into::into)
        }
        Err(SharedStreamInUse)
    }
);

#[derive(Debug)]
pub struct ManuallyDropCapStream<S> {
    inner: ManuallyDrop<CapStream<S>>,
}

impl<S> From<ManuallyDrop<CapStream<S>>> for ManuallyDropCapStream<S> {
    fn from(value: ManuallyDrop<CapStream<S>>) -> Self {
        ManuallyDropCapStream { inner: value }
    }
}

impl<S> Deref for ManuallyDropCapStream<S> {
    type Target = CapStream<S>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<S> DerefMut for ManuallyDropCapStream<S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<S, E> Stream for ManuallyDropCapStream<S>
where
    S: Stream<Item = Result<BytesMut, E>> + Unpin,
    SharedStreamError: From<E>,
{
    type Item = Result<(), SharedStreamError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.poll_next_unpin(cx)
    }
}

sink_impl!(
    ManuallyDropCapStream<S>,
    CapStreamMsg,
    Bytes,
    fn start_send(mut self: Pin<&mut Self>, item: CapStreamMsg) -> Result<(), Self::Error> {
        self.inner.start_send_unpin(item)
    },
    async fn disconnect(&mut self, reason: DisconnectReason) -> Result<(), SharedStreamError> {
        self.inner.disconnect(reason).await
    }
);

/// Weak clone of [`CapStream`] accesses underlying [`CapStream`] by callback.
#[derive(Debug)]
pub struct WeakCapStreamClone(mpsc::UnboundedSender<CapStreamMsg>);

impl Stream for WeakCapStreamClone {
    type Item = Result<(), SharedStreamError>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(Some(Ok(())))
    }
}

impl Sink<CapStreamMsg> for WeakCapStreamClone {
    type Error = SharedStreamError;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: CapStreamMsg) -> Result<(), Self::Error> {
        self.0.send(item).map_err(WeakCloneSendEgressFailed)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

#[async_trait::async_trait]
impl CanDisconnect<CapStreamMsg> for WeakCapStreamClone {
    async fn disconnect(&mut self, _reason: DisconnectReason) -> Result<(), SharedStreamError> {
        Err(NoDisconnectByClone)
    }
}

#[derive(Debug)]
#[pin_project(project = EnumProj)]
pub enum SharedByteStream<S> {
    Owner(#[pin] SharedStream<ManuallyDropCapStream<S>>),
    Clone(#[pin] SharedStream<WeakCapStreamClone>),
}

impl<S, E> Stream for SharedByteStream<S>
where
    S: Stream<Item = Result<BytesMut, E>> + Unpin,
    SharedStream<ManuallyDropCapStream<S>>: Stream<Item = Result<BytesMut, SharedStreamError>>,
    SharedStreamError: From<E>,
{
    type Item = Result<BytesMut, SharedStreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        match this {
            EnumProj::Owner(owner) => owner.poll_next(cx),
            EnumProj::Clone(clone) => clone.poll_next(cx),
        }
    }
}

impl<S> Sink<Bytes> for SharedByteStream<S>
where
    S: CanDisconnect<Bytes> + Unpin,
    SharedStream<ManuallyDropCapStream<S>>: Sink<Bytes>,
    SharedStreamError: From<<S as Sink<Bytes>>::Error>,
    SharedStreamError: From<<SharedStream<ManuallyDropCapStream<S>> as Sink<Bytes>>::Error>,
{
    type Error = SharedStreamError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.project();
        match this {
            EnumProj::Owner(owner) => owner.poll_ready(cx).map_err(Into::into),
            EnumProj::Clone(clone) => clone.poll_ready(cx),
        }
    }

    fn start_send(self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        let this = self.project();
        match this {
            EnumProj::Owner(owner) => owner.start_send(item).map_err(Into::into),
            EnumProj::Clone(clone) => clone.start_send(item.into()).map_err(Into::into),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.project();
        match this {
            EnumProj::Owner(owner) => owner.poll_flush(cx).map_err(Into::into),
            EnumProj::Clone(clone) => clone.poll_flush(cx),
        }
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.project();
        match this {
            EnumProj::Owner(owner) => owner.poll_close(cx).map_err(Into::into),
            EnumProj::Clone(clone) => clone.poll_close(cx),
        }
    }
}

#[async_trait::async_trait]
impl<S> CanDisconnect<Bytes> for SharedByteStream<S>
where
    S: CanDisconnect<Bytes> + Send,
    SharedStream<ManuallyDropCapStream<S>>: CanDisconnect<Bytes>,
    SharedStreamError: From<<S as Sink<Bytes>>::Error>,
    SharedStreamError: From<<SharedStream<ManuallyDropCapStream<S>> as Sink<Bytes>>::Error>,
{
    async fn disconnect(&mut self, reason: DisconnectReason) -> Result<(), SharedStreamError> {
        match self {
            SharedByteStream::Owner(owner) => owner.disconnect(reason).await.map_err(Into::into),
            SharedByteStream::Clone(clone) => clone.disconnect(reason).await,
        }
    }
}

#[cfg(test)]
mod test {
    use reth_primitives::bytes::{BufMut, BytesMut};

    use crate::{capability::Capability, p2pstream::SharedCapabilities, EthMessage, EthVersion};

    use super::CapStream;

    fn shared_caps_eth68() -> SharedCapabilities {
        let local_capabilities: Vec<Capability> = vec![EthVersion::Eth68.into()];
        let peer_capabilities: Vec<Capability> = vec![EthVersion::Eth68.into()];
        SharedCapabilities::try_new(local_capabilities, peer_capabilities).unwrap()
    }

    #[test]
    fn test_unmask_msg_id() {
        let mut msg = BytesMut::with_capacity(1);
        msg.put_u8(0x07); // eth msg id

        let cap_stream = CapStream::new((), shared_caps_eth68());
        let ingress_bytes: BytesMut = cap_stream.unmask_msg_id(msg.clone()).unwrap().into();

        assert_eq!(ingress_bytes.as_ref(), &[0x07]);
    }

    #[test]
    fn test_mask_msg_id() {
        let mut msg = BytesMut::with_capacity(2);
        msg.put_u8(0x10); // eth msg id
        msg.put_u8(0x20); // some msg data

        let cap_stream = CapStream::new((), shared_caps_eth68());
        let shared_stream = cap_stream.try_into_shared::<EthMessage>().unwrap();
        let egress_bytes = shared_stream.mask_msg_id(msg.freeze());

        assert_eq!(egress_bytes.as_ref(), &[0x10, 0x20]);
    }

    #[test]
    fn test_unmask_msg_id_cap_not_in_shared_range() {
        let mut msg = BytesMut::with_capacity(1);
        msg.put_u8(0x11);

        let cap_stream = CapStream::new((), shared_caps_eth68());

        assert!(cap_stream.unmask_msg_id(msg).is_err());
    }

    #[test]
    #[ignore = "more subprotocols than eth have to be implemented"]
    fn test_try_drop_stream_owner() {}
}
