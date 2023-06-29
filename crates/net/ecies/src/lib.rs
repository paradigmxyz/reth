#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxzy/reth/issues/"
)]
#![warn(missing_docs, unreachable_pub, unused_crate_dependencies)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! RLPx ECIES framed transport protocol.

pub mod algorithm;
pub mod mac;
pub mod stream;
pub mod util;

mod error;
pub use error::ECIESError;

mod codec;

use reth_primitives::{
    bytes::{Bytes, BytesMut},
    H512 as PeerId,
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
