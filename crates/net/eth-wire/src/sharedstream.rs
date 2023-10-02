//! SharedStream sits transparently between P2PStream and EthStream
//!
//!                                        ,--BytesMut--> EthStream
//! P2PStream --BytesMut--> SharedStream -|
//!                                        `--BytesMut--> <some other capability>
//!
//!                                      ,--Bytes-- EthStream
//! P2PStream <--Bytes-- SharedStream <-|
//!                                      `--Bytes-- <some other capability>
//!
//! Under the hood, SharedStream uses a [`CapStream`] for demux/mux of the [`crate::P2PStream`]
//! and capability streams. Demux includes normalizing the message ID so that all capability
//! streams receive messages with IDs starting from 0x0.
//!
//! Only the [`SharedStream`] owning the [`CapStream`] can poll it. [`SharedStream`]s wrapping a
//! [`WeakCapStreamClone`] rely on the first [`SharedStream`].

use std::{
    collections::HashMap,
    mem,
    mem::Discriminant,
    pin::Pin,
    task::{ready, Context, Poll},
};

use futures::{Sink, SinkExt, StreamExt};
use reth_primitives::bytes::{BufMut, Bytes, BytesMut};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::Stream;

use crate::{
    capability::{Capability, SharedCapability, SharedCapabilityError},
    errors::P2PStreamError,
    p2pstream::SharedCapabilities,
    CanDisconnect,
};

use CapStreamError::*;

type CapKey = Discriminant<IngressCapBytes>;

#[derive(Debug, Error)]
pub enum CapStreamError {
    #[error(transparent)]
    P2P(#[from] P2PStreamError),
    #[error("capability not recognized")]
    CapabilityNotRecognized,
    #[error("capability not configured in app")]
    CapabilityNotConfigured,
    #[error("unknown capability message id: {0}")]
    UnknownCapabilityMessageId(u8),
    #[error(transparent)]
    ParseSharedCapabilityFailed(#[from] SharedCapabilityError),
    #[error("failed to clone shared stream, {0}")]
    RecvClonedStreamFailed(#[from] oneshot::error::RecvError),
    #[error("failed to return shared stream clone")]
    SendClonedStreamFailed,
    #[error("failed to receive ingress bytes, channel closed")]
    RecvIngressBytesFailed,
    #[error("failed to stream ingress bytes, {0}")]
    SendIngressBytesFailed(mpsc::error::SendError<BytesMut>),
    #[error("weak cap stream clone failed to sink message to cap stream, {0}")]
    WeakCloneSendEgressFailed(mpsc::error::SendError<CapStreamMsg>),
    #[error("shared stream already cloned for capability")]
    SharedStreamExists,
}

/// Wrapper around ingress message with unmasked message ID.
#[derive(Debug)]
pub enum IngressCapBytes {
    EthMessage(BytesMut),
}

impl From<IngressCapBytes> for BytesMut {
    fn from(value: IngressCapBytes) -> Self {
        match value {
            IngressCapBytes::EthMessage(b) => b,
        }
    }
}

/// Message sent from a [`SharedStream`] to underlying [`CapStream`].
#[derive(Debug)]
pub enum CapStreamMsg {
    /// Control message to clone the shared stream.
    Clone(Capability, oneshot::Sender<SharedStream<WeakCapStreamClone>>),
    /// Wrapper around egress message with unmasked message ID.
    EgressBytes(Bytes),
}

impl TryFrom<Capability> for CapKey {
    type Error = CapStreamError;
    fn try_from(value: Capability) -> Result<Self, Self::Error> {
        let cap_msg_type = if value.is_eth_v66() || value.is_eth_v67() || value.is_eth_v68() {
            IngressCapBytes::EthMessage(BytesMut::default())
        } else {
            return Err(CapabilityNotRecognized)
        };

        Ok(mem::discriminant(&cap_msg_type))
    }
}

macro_rules! try_from_shared_cap_impl {
    ($cap:ty) => {
        impl TryFrom<$cap> for CapKey {
            type Error = CapStreamError;
            fn try_from(value: $cap) -> Result<Self, Self::Error> {
                use SharedCapability::*;

                let cap_msg_type = match value {
                    Eth { .. } => IngressCapBytes::EthMessage(BytesMut::default()),
                    UnknownCapability { .. } => return Err(CapabilityNotRecognized),
                };
                Ok(mem::discriminant(&cap_msg_type))
            }
        }
    };
}

try_from_shared_cap_impl!(SharedCapability);
try_from_shared_cap_impl!(&SharedCapability);

/// Template implementation of [`Sink`] for crate types wrapping a type that is [`Sink`] and
/// [`Stream`].
#[macro_export]
macro_rules! sink_impl {
    ($stream:ty, $sink_in:ty, $sink_out:ty, $start_send:item) => {
        impl<S> Sink<$sink_in> for $stream
        where
            S: CanDisconnect<$sink_out> + Unpin,
            CapStreamError: From<<S as Sink<$sink_out>>::Error>,
        {
            type Error = CapStreamError;

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
    };
}

/// Template implementation of [`CanDisconnect`] for crate types wrapping a type that is [`Sink`]
/// and [`Stream`].
macro_rules! can_disconnect_impl {
    ($stream:ty, $sink_in:ty, $sink_out:ty) => {
        #[async_trait::async_trait]
        impl<S> CanDisconnect<$sink_in> for $stream
        where
            S: CanDisconnect<$sink_out> + Send,
            CapStreamError: From<<S as Sink<$sink_out>>::Error>,
        {
            async fn disconnect(
                &mut self,
                reason: $crate::DisconnectReason,
            ) -> Result<(), CapStreamError> {
                self.inner.disconnect(reason).await.map_err(Into::into)
            }
        }
    };
}

/// Stream sharing underlying stream. First instant owns the stream and following instants cloned
/// from first own a weak clone of stream. Only the first instant can poll the underlying stream,
/// but doing so triggers poll to immediately repeat once for every weak clone. [`CapStream`]
/// ensures at most one shared stream is opened per shared capability.
#[derive(Debug)]
pub struct SharedStream<S> {
    inner: S,
    ingress: mpsc::UnboundedReceiver<BytesMut>,
    cap: SharedCapability,
}

impl<S> SharedStream<S>
where
    S: Sink<CapStreamMsg> + Unpin,
    CapStreamError: From<<S as Sink<CapStreamMsg>>::Error>,
{
    #[allow(clippy::type_complexity)]
    fn _clone_for(
        &mut self,
        cap: Capability,
    ) -> Result<
        Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<SharedStream<WeakCapStreamClone>, CapStreamError>,
                    > + Send,
            >,
        >,
        CapStreamError,
    > {
        let (tx, rx) = oneshot::channel();
        self.inner.start_send_unpin(CapStreamMsg::Clone(cap, tx))?;

        Ok(Box::pin(async move { rx.await.map_err(RecvClonedStreamFailed) }))
    }

    fn mask_msg_id(&self, msg: Bytes) -> Bytes {
        let mut masked_bytes = BytesMut::zeroed(msg.len());
        masked_bytes[0] = msg[0] + self.cap.offset();
        masked_bytes.put_slice(&msg[1..]);

        masked_bytes.freeze()
    }
}

impl<S, E> Stream for SharedStream<S>
where
    S: Stream<Item = Result<BytesMut, E>> + Unpin,
    CapStreamError: From<E>,
{
    type Item = Result<BytesMut, CapStreamError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        _ = self.inner.poll_next_unpin(cx)?;

        let res = ready!(self.ingress.poll_recv(cx));

        match res {
            Some(b) => Poll::Ready(Some(Ok(b))),
            None => Poll::Ready(Some(Err(RecvIngressBytesFailed))),
        }
    }
}

can_disconnect_impl!(SharedStream<S>, Bytes, CapStreamMsg);
sink_impl!(
    SharedStream<S>,
    Bytes,
    CapStreamMsg,
    fn start_send(mut self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        let item = self.mask_msg_id(item);
        self.inner.start_send_unpin(CapStreamMsg::EgressBytes(item))?;
        Ok(())
    }
);

impl<S> SharedStream<CapStream<S>> {
    /// Returns a shared stream if capability is shared
    pub fn try_new(caps_stream: CapStream<S>, cap: Capability) -> Result<Self, CapStreamError> {
        caps_stream.into_conn_for(cap)
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
    pub fn new(inner: S, shared_capabilities: SharedCapabilities) -> Self {
        let demux = HashMap::new();
        let (mux_tx, mux) = mpsc::unbounded_channel();

        Self { inner, mux, mux_tx, demux, shared_capabilities }
    }

    pub fn into_conn_for(mut self, cap: Capability) -> Result<SharedStream<Self>, CapStreamError> {
        let cap = self.shared_capabilities.try_shared_from(cap)?;
        let ingress = self.reg_new_ingress_buffer_for(&cap)?;

        Ok(SharedStream { inner: self, ingress, cap })
    }

    fn clone_conn_for(
        &mut self,
        cap: Capability,
    ) -> Result<SharedStream<WeakCapStreamClone>, CapStreamError> {
        let cap = self.shared_capabilities.try_shared_from(cap)?;
        let ingress = self.reg_new_ingress_buffer_for(&cap)?;
        let mux_tx = self.mux_tx.clone();

        Ok(SharedStream { inner: WeakCapStreamClone(mux_tx), ingress, cap })
    }

    fn reg_new_ingress_buffer_for(
        &mut self,
        cap: &SharedCapability,
    ) -> Result<mpsc::UnboundedReceiver<BytesMut>, CapStreamError> {
        let cap_key = cap.try_into()?;
        if self.demux.contains_key(&cap_key) {
            return Err(SharedStreamExists)
        }
        let (ingress_tx, ingress) = mpsc::unbounded_channel();
        self.demux.insert(cap_key, ingress_tx);

        Ok(ingress)
    }

    fn interpret_msg(&mut self, msg: CapStreamMsg) -> Result<(), CapStreamError>
    where
        S: CanDisconnect<Bytes> + Unpin,
        CapStreamError: From<<S as Sink<Bytes>>::Error>,
    {
        match msg {
            CapStreamMsg::Clone(cap, signal_tx) => {
                let conn_clone = self.clone_conn_for(cap)?;
                signal_tx.send(conn_clone).map_err(|_| SendClonedStreamFailed)
            }
            CapStreamMsg::EgressBytes(bytes) => {
                self.inner.start_send_unpin(bytes).map_err(|e| e.into())
            }
        }
    }

    fn unmask_msg_id(&self, mut msg: BytesMut) -> Result<IngressCapBytes, CapStreamError> {
        use SharedCapability::*;

        let id = msg[0];
        let mut offset = 0;
        for cap in self.shared_capabilities.iter_caps() {
            offset += cap.num_messages()?;
            if id < offset {
                msg[0] = id - offset;

                return match cap {
                    Eth { .. } => Ok(IngressCapBytes::EthMessage(msg)),
                    UnknownCapability { .. } => Err(CapabilityNotRecognized),
                }
            }
        }
        Err(UnknownCapabilityMessageId(id))
    }
}

impl<S, E> Stream for CapStream<S>
where
    S: Stream<Item = Result<BytesMut, E>> + Unpin,
    CapStreamError: From<E>,
{
    type Item = Result<(), CapStreamError>;

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

can_disconnect_impl!(CapStream<S>, CapStreamMsg, Bytes);
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
    }
);

/// Weak clone of [`CapStream`] accesses underlying [`CapStream`] by callback.
#[derive(Debug)]
pub struct WeakCapStreamClone(mpsc::UnboundedSender<CapStreamMsg>);

impl Sink<CapStreamMsg> for WeakCapStreamClone {
    type Error = CapStreamError;

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
