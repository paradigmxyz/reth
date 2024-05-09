//! RLPx ECIES framed transport protocol.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

pub mod algorithm;
pub mod mac;
pub mod stream;
pub mod util;

mod error;
pub use error::ECIESError;

mod codec;

use reth_primitives::{
    bytes::{Bytes, BytesMut},
    B512 as PeerId,
};

/// Raw egress values for an ECIES protocol
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EgressECIESValue {
    /// The AUTH message being sent out
    Auth,
    /// The ACK message being sent out
    Ack,
    /// The message being sent out (wrapped bytes)
    Message(Bytes),
}

/// Raw ingress values for an ECIES protocol
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IngressECIESValue {
    /// Receiving a message from a [`PeerId`]
    AuthReceive(PeerId),
    /// Receiving an ACK message
    Ack,
    /// Receiving a message
    Message(BytesMut),
}
