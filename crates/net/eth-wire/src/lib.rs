#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
//! Implementation of the `eth` wire protocol.

pub mod builder;
pub mod capability;
mod disconnect;
pub mod error;
mod ethstream;
mod hello;
mod p2pstream;
mod pinger;
pub use builder::*;
pub mod types;
pub use types::*;

#[cfg(test)]
pub use tokio_util::codec::{
    LengthDelimitedCodec as PassthroughCodec, LengthDelimitedCodecError as PassthroughCodecError,
};

pub use crate::{
    disconnect::DisconnectReason,
    ethstream::{EthStream, UnauthedEthStream, MAX_MESSAGE_SIZE},
    hello::HelloMessage,
    p2pstream::{P2PMessage, P2PMessageID, P2PStream, ProtocolVersion, UnauthedP2PStream},
};
