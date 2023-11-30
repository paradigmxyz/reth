//! Implementation of the `eth` wire protocol.
//!
//! ## Feature Flags
//!
//! - `serde` (default): Enable serde support
//! - `arbitrary`: Adds `proptest` and `arbitrary` support for wire types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![warn(missing_debug_implementations, missing_docs, unreachable_pub, rustdoc::all)]
#![deny(unused_must_use, rust_2018_idioms)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

pub mod builder;
pub mod capability;
mod disconnect;
pub mod errors;
mod ethstream;
mod hello;
pub mod multiplex;
pub mod muxdemux;
mod p2pstream;
mod pinger;
pub mod protocol;
pub use builder::*;
pub mod types;
pub use types::*;

#[cfg(test)]
pub mod test_utils;

#[cfg(test)]
pub use tokio_util::codec::{
    LengthDelimitedCodec as PassthroughCodec, LengthDelimitedCodecError as PassthroughCodecError,
};

pub use crate::{
    capability::Capability,
    disconnect::{CanDisconnect, DisconnectReason},
    ethstream::{EthStream, UnauthedEthStream, MAX_MESSAGE_SIZE},
    hello::{HelloMessage, HelloMessageBuilder, HelloMessageWithProtocols},
    muxdemux::{MuxDemuxStream, StreamClone},
    p2pstream::{
        DisconnectP2P, P2PMessage, P2PMessageID, P2PStream, ProtocolVersion, UnauthedP2PStream,
        MAX_RESERVED_MESSAGE_ID,
    },
    types::EthVersion,
};
