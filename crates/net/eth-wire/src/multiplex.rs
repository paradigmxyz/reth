//! Rlpx protocol multiplexer and satellite stream
//!
//! A Satellite is a Stream that primarily drives a single RLPx subprotocol but can also handle additional subprotocols.
//!
//! Most of other subprotocols are "dependent satellite" protocols of "eth" and not a fully standalone protocol, for example "snap", See also [snap protocol](https://github.com/ethereum/devp2p/blob/298d7a77c3bf833641579ecbbb5b13f0311eeeea/caps/snap.md?plain=1#L71)
//! Hence it is expected that the primary protocol is "eth" and the additional protocols are "dependent satellite" protocols.

use std::pin::Pin;
use std::task::{Context, Poll};
use bytes::BytesMut;
use futures::{Stream, StreamExt};
use tokio::sync::mpsc;
use tokio::sync::mpsc::UnboundedSender;
use tokio_stream::wrappers::UnboundedReceiverStream;
use crate::capability::{Capability, SharedCapabilities, SharedCapability};
use crate::P2PStream;


/// A Stream and Sink type that wraps a raw rlpx stream [P2PStream] and handles message ID multiplexing.
#[derive(Debug)]
pub struct RlpxProtocolMultiplexer<St> {
    /// The raw p2p stream
    conn: P2PStream<St>,
    /// All the subprotocols that are multiplexed on top of the raw p2p stream
    protocols: Vec<ProtocolStream>
}

impl<St> RlpxProtocolMultiplexer<St> {

    /// Wraps the raw p2p stream
    pub fn new(conn: P2PStream<St>) -> Self {
        Self {conn, protocols: Default::default()}
    }

    /// Installs a new protocol on top of the raw p2p stream
    pub  fn install_protocol<S>(&mut self, cap: Capability, st: S) -> Result<(),()> {
        todo!()
    }

    /// Converts this multiplexer into a [RlpxSatelliteStream] with the given primary protocol.
    pub async fn into_satellite_stream<F, Primary>(self, cap: &str, prim: F) -> Result<RlpxSatelliteStream<St, Primary>, Self> {
        todo!()
    }

}

#[derive(Debug)]
pub struct ProtocolProxy {
    shared_cap: SharedCapability,
    from_wire: UnboundedReceiverStream<BytesMut>,
    to_wire: UnboundedSender<BytesMut>,
}

impl Stream for ProtocolProxy {
    type Item = BytesMut;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.from_wire.poll_next_unpin(cx)
    }
}

/// A connection channel to receive messages for the negotiated protocol.
///
/// This is a [Stream] that returns raw bytes of the received messages for this protocol.
#[derive(Debug)]
pub struct ProtocolConnection {
    from_wire: UnboundedReceiverStream<BytesMut>,
}

impl Stream for ProtocolConnection {
    type Item = BytesMut;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.from_wire.poll_next_unpin(cx)
    }
}

/// A Stream and Sink type that acts as a wrapper around a primary RLPx subprotocol (e.g. "eth") [EthStream](crate::EthStream) and can also handle additional subprotocols.
#[derive(Debug)]
pub struct RlpxSatelliteStream<St, Primary> {
   /// The raw p2p stream
   p2p_stream: P2PStream<St>,
   primary: Primary,
   primary_capability: SharedCapability,
   shared_capabilities: SharedCapabilities,
   satellites: Vec<ProtocolStream>
}

impl<St, Primary> RlpxSatelliteStream<St, Primary> {

    pub fn new(primary: St, primary_capability: Capability, capabilities: SharedCapabilities, ) -> Result<Self,()> {
        todo!()

        // Self {
        //     primary,
        //     primary_capability,
        //     shared_capabilities,
        //     satellites: Vec::new()
        // }
    }

}

impl<St, Primary> Stream for RlpxSatelliteStream<St, Primary> {
    type Item = ();

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        todo!()
    }
}

/// Wraps a RLPx subprotocol and handles message ID multiplexing.
struct ProtocolStream {
    cap: SharedCapability,
    to_satellite: UnboundedSender<BytesMut>,
    satellite_st: Pin<Box< dyn Stream<Item = BytesMut>>>
}