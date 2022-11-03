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
mod capability;
pub mod error;
mod ethstream;
mod p2pstream;
mod pinger;
pub mod types;
pub use types::*;

pub use ethstream::EthStream;
