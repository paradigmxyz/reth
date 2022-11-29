#![allow(dead_code)] // TODO: REMOVE once eth-wire is done.
// #![warn(missing_docs, unreachable_pub)]
#![allow(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
//! Implementation of the `eth` wire protocol.

pub use tokio_util::codec::{
    LengthDelimitedCodec as PassthroughCodec, LengthDelimitedCodecError as PassthroughCodecError,
};
pub mod builder;
pub mod capability;
pub mod error;
mod ethstream;
mod p2pstream;
mod pinger;
pub use builder::*;
pub mod types;
pub use types::*;

pub use crate::{
    ethstream::{EthStream, UnauthedEthStream, MAX_MESSAGE_SIZE},
    p2pstream::{DisconnectReason, HelloMessage, P2PStream, ProtocolVersion, UnauthedP2PStream},
};
