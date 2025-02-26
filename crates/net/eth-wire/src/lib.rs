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
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

pub mod capability;
mod disconnect;
pub mod errors;
mod ethstream;
mod hello;
pub mod multiplex;
mod p2pstream;
mod pinger;
pub mod protocol;

#[cfg(test)]
pub mod test_utils;

#[cfg(test)]
pub use tokio_util::codec::{
    LengthDelimitedCodec as PassthroughCodec, LengthDelimitedCodecError as PassthroughCodecError,
};

pub use crate::{
    disconnect::CanDisconnect,
    ethstream::{EthStream, UnauthedEthStream, MAX_MESSAGE_SIZE},
    hello::{HelloMessage, HelloMessageBuilder, HelloMessageWithProtocols},
    p2pstream::{
        DisconnectP2P, P2PMessage, P2PMessageID, P2PStream, UnauthedP2PStream,
        MAX_RESERVED_MESSAGE_ID,
    },
    Capability, ProtocolVersion,
};

// Re-export wire types
#[doc(inline)]
pub use reth_eth_wire_types::*;
